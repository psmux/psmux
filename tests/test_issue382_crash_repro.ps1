# Issue #382: "psmux + claude + neovim = window crashes"
#
# CONCLUSION (proven by this harness): this is NOT a psmux bug. It is a Windows
# conhost.exe use-after-free provoked by Claude Code spawning nvim as its editor.
# It reproduces IDENTICALLY with no psmux involved.
#
# Evidence chain:
#   - WER dumps (%LOCALAPPDATA%\CrashDumps\conhost.exe.*.dmp) are all identical:
#     0xC0000005 read access violation at conhost.exe TextBuffer::GetSize, called
#     from ConsoleWaitBlock::Notify (a pending console read completed against a
#     TextBuffer that nvim's alternate-screen swap had already freed).
#   - Inside psmux, the only byte psmux writes to the pane before the crash is the
#     forwarded 0x07 (Ctrl+G) keystroke: psmux injects nothing unsolicited.
#   - PART B below runs the SAME scenario with NO psmux (claude in its own
#     console) and gets the SAME conhost crash.
#
# This harness is DIAGNOSTIC, not a pass/fail regression: there is no psmux code
# change that fixes a conhost bug. It documents and re-demonstrates the finding.
#
# SAFETY: launches a second `claude`. It NEVER kills anything by name: not
# claude (that would kill the Claude Code session driving this), not psmux, not
# nvim, and above all NOT WerFault.exe. Everything it ends is a pid it recorded
# as a descendant of a process it launched itself.
#
# WHY WerFault IS NEVER KILLED (sweep 2026-11-23_21-49-55)
# The crashing conhost is suspended inside its unhandled exception filter while
# Windows Error Reporting (WerFault.exe -u -p <conhost pid>) collects the report.
# When WerFault dies before it finishes, the filter falls back to a hard error
# popup: "conhost.exe - Application Error: The instruction at 0x... referenced
# memory at 0x... Click on OK to terminate the program", a modal csrss dialog
# that stays until a human clicks OK, with the crashed conhost (and, for PART B,
# its "Claude Code (Not Responding)" window) frozen behind it. This suite used to
# end every WerFault on the machine by name 15 s after Ctrl+G; on a loaded sweep
# machine WER was still running at that moment, and the two popups the user
# found were logged 15.6 s and 15.1 s after the two conhost faults. Reproduced on
# demand by holding WerFault past the 15 s mark: two popups and a hung window
# with the old cleanup, none with this one.
# So the suite now waits (bounded) for the WER runs that belong to ITS crashed
# processes to finish, then ends its own leftover processes by pid.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN -and (Test-Path $env:PSMUX_TEST_BIN)) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA SilentlyContinue).Source }
$SESSION = "crash382"
$NS = "t382_" + [guid]::NewGuid().ToString('N').Substring(0, 8)
$injectorExe = "$env:TEMP\psmux_injector.exe"
$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path

$claudeCmd = Get-Command claude -EA SilentlyContinue
$nvimCmd   = Get-Command nvim   -EA SilentlyContinue
if (-not $PSMUX -or -not $claudeCmd -or -not $nvimCmd) {
    Write-Host "[SKIP] needs psmux + claude + nvim on PATH (psmux=$([bool]$PSMUX) claude=$([bool]$claudeCmd) nvim=$([bool]$nvimCmd))" -ForegroundColor Yellow
    exit 0
}

# Own namespace and data dir: nothing here touches the default namespace or
# another session's servers.
$savedDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = Join-Path $env:TEMP ("psmux382_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force -Path $env:PSMUX_DATA_DIR | Out-Null

if (-not (Test-Path $injectorExe)) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    & $csc /nologo /optimize /out:$injectorExe (Join-Path $repoTests 'injector.cs') 2>&1 | Out-Null
}

# ── process tree bookkeeping, by pid and creation time ──────────────────────
# pid -> creation time of every process seen below a root this suite launched.
# Collected several times while the tree is alive, because once a parent exits
# its children can no longer be reached by walking down from the root.
$script:Owned = @{}
function Add-OwnedTree([int[]]$Roots) {
    $all = @{}
    $kids = @{}
    foreach ($p in @(Get-CimInstance Win32_Process -Property ProcessId,ParentProcessId,CreationDate -EA SilentlyContinue)) {
        $all[[int]$p.ProcessId] = $p
        $pp = [int]$p.ParentProcessId
        if (-not $kids.ContainsKey($pp)) { $kids[$pp] = New-Object System.Collections.Generic.List[object] }
        $kids[$pp].Add($p)
    }
    $queue = New-Object System.Collections.Generic.Queue[int]
    foreach ($r in $Roots) {
        if ($r -gt 0 -and $all.ContainsKey($r)) {
            if (-not $script:Owned.ContainsKey($r)) { $script:Owned[$r] = $all[$r].CreationDate }
            $queue.Enqueue($r)
        }
    }
    # Roots recorded earlier are walked again too: their children may be new.
    foreach ($k in @($script:Owned.Keys)) {
        if ($all.ContainsKey($k) -and $all[$k].CreationDate -eq $script:Owned[$k]) { $queue.Enqueue($k) }
    }
    $seen = @{}
    while ($queue.Count -gt 0) {
        $cur = $queue.Dequeue()
        if ($seen.ContainsKey($cur)) { continue }
        $seen[$cur] = $true
        if (-not $kids.ContainsKey($cur)) { continue }
        foreach ($c in $kids[$cur]) {
            # A child created before its parent is a reused parent pid, not ours.
            if ($c.CreationDate -lt $all[$cur].CreationDate) { continue }
            $cid = [int]$c.ProcessId
            if (-not $script:Owned.ContainsKey($cid)) { $script:Owned[$cid] = $c.CreationDate }
            $queue.Enqueue($cid)
        }
    }
}
function Get-LiveOwned {
    $live = @()
    foreach ($k in @($script:Owned.Keys)) {
        $p = Get-CimInstance -Query "SELECT ProcessId,Name,CreationDate FROM Win32_Process WHERE ProcessId=$k" -EA SilentlyContinue
        if (-not $p -or $p.CreationDate -ne $script:Owned[$k]) { continue }
        # WER's process snapshot of a crashed conhost (PssCaptureSnapshot clone)
        # is listed by WMI as a child conhost with no command line, but it is not
        # a process anyone can open or end. Only count what Get-Process can see.
        if (-not (Get-Process -Id $k -EA SilentlyContinue)) { continue }
        $live += $p
    }
    return $live
}
# Let WER finish with OUR crashed processes (bounded), never ending WerFault.
function Wait-OwnWer([int]$MaxSeconds = 90) {
    $deadline = (Get-Date).AddSeconds($MaxSeconds)
    $first = $true
    while ((Get-Date) -lt $deadline) {
        $mine = @(Get-CimInstance Win32_Process -Filter "Name='WerFault.exe'" -EA SilentlyContinue | Where-Object {
            $_.CommandLine -match '-p (\d+)' -and $script:Owned.ContainsKey([int]$Matches[1])
        })
        if ($mine.Count -eq 0) { return }
        if ($first) { Write-Host ("  waiting for Windows Error Reporting to finish with {0} crashed process(es) of ours" -f $mine.Count) -ForegroundColor DarkGray; $first = $false }
        Start-Sleep -Milliseconds 500
    }
    Write-Host "  WER still busy after ${MaxSeconds}s; ending our crashed processes anyway (WerFault is left alone)" -ForegroundColor DarkYellow
}
function Stop-Owned([string]$What) {
    # WerFault -u is started BY the crashing process, so it is in our tree; it
    # is still never ended (that is what raises the popup), and it exits on its
    # own once the process it reports on is gone. amesh is the mesh client the
    # second claude starts as an MCP server; by rule it is never touched, and it
    # exits when its claude does.
    $left = @(Get-LiveOwned | Where-Object { $_.Name -ne 'WerFault.exe' -and $_.Name -notlike 'amesh*' })
    foreach ($p in $left) {
        try { Stop-Process -Id ([int]$p.ProcessId) -Force -EA Stop } catch {}
    }
    if ($left.Count) {
        Write-Host ("  {0}: ended {1} leftover process(es) of ours by pid: {2}" -f $What, $left.Count, (($left | ForEach-Object { "$($_.Name)#$($_.ProcessId)" }) -join ' ')) -ForegroundColor DarkGray
    }
}
function Conhost-FaultSince($since) {
    $fault = $false
    Get-WinEvent -FilterHashtable @{LogName='Application'; StartTime=$since; Id=1000} -EA SilentlyContinue |
      Select-Object -First 8 | ForEach-Object { if ($_.Message -match "conhost") { $fault = $true } }
    return $fault
}

try {
# ── PART A: inside psmux ────────────────────────────────────────────────────
Write-Host "`n=== PART A: claude + nvim + Ctrl+G INSIDE psmux ===" -ForegroundColor Cyan

$tA = Get-Date
$proc = Start-Process -FilePath $PSMUX -ArgumentList "-L",$NS,"new-session","-s",$SESSION -PassThru
Start-Sleep -Seconds 4
Add-OwnedTree @($proc.Id)
& $PSMUX -L $NS send-keys -t $SESSION '$env:EDITOR="nvim --clean"' Enter
Start-Sleep -Milliseconds 800
& $PSMUX -L $NS send-keys -t $SESSION 'claude' Enter
Start-Sleep -Seconds 12
Add-OwnedTree @($proc.Id)
& $injectorExe $proc.Id "{ENTER}" | Out-Null
Start-Sleep -Seconds 2
& $injectorExe $proc.Id "hello" | Out-Null
Start-Sleep -Milliseconds 800
Add-OwnedTree @($proc.Id)
Write-Host "[ACTION] Ctrl+G" -ForegroundColor Yellow
& $injectorExe $proc.Id "^g" | Out-Null
$aCrashed = $false
for ($i=0; $i -lt 15; $i++) {
    Start-Sleep -Seconds 1
    Add-OwnedTree @($proc.Id)
    & $PSMUX -L $NS has-session -t $SESSION 2>$null
    if (($LASTEXITCODE -ne 0) -or ($null -eq (Get-Process -Id $proc.Id -EA SilentlyContinue))) { $aCrashed = $true; break }
}
$aFault = Conhost-FaultSince $tA
Write-Host ("PART A: sessionDied={0}  conhostFault={1}" -f $aCrashed, $aFault) -ForegroundColor $(if($aCrashed -or $aFault){"Red"}else{"Green"})
Add-OwnedTree @($proc.Id)
Wait-OwnWer
& $PSMUX -L $NS kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 800
Stop-Owned "PART A"
Start-Sleep -Seconds 1

# ── PART B: control, NO psmux ───────────────────────────────────────────────
Write-Host "`n=== PART B: SAME scenario with NO psmux (claude in its own console) ===" -ForegroundColor Cyan
$tB = Get-Date
$env:EDITOR = "nvim --clean"
$bproc = Start-Process -FilePath $claudeCmd.Source -WorkingDirectory (Split-Path $repoTests -Parent) -PassThru
Start-Sleep -Seconds 13
Add-OwnedTree @($bproc.Id)
& $injectorExe $bproc.Id "{ENTER}" | Out-Null
Start-Sleep -Seconds 2
& $injectorExe $bproc.Id "hello" | Out-Null
Start-Sleep -Milliseconds 800
Add-OwnedTree @($bproc.Id)
Write-Host "[ACTION] Ctrl+G (no psmux)" -ForegroundColor Yellow
& $injectorExe $bproc.Id "^g" | Out-Null
$bCrashed = $false
for ($i=0; $i -lt 15; $i++) {
    Start-Sleep -Seconds 1
    Add-OwnedTree @($bproc.Id)
    if ($null -eq (Get-Process -Id $bproc.Id -EA SilentlyContinue)) { $bCrashed = $true; break }
}
$bFault = Conhost-FaultSince $tB
Write-Host ("PART B: claudeDied={0}  conhostFault={1}" -f $bCrashed, $bFault) -ForegroundColor $(if($bCrashed -or $bFault){"Red"}else{"Green"})
Add-OwnedTree @($bproc.Id)
Wait-OwnWer
Stop-Owned "PART B"
$env:EDITOR = $null

# ── Verdict ─────────────────────────────────────────────────────────────────
Write-Host "`n=== VERDICT ===" -ForegroundColor Cyan
if (($aCrashed -or $aFault) -and ($bCrashed -or $bFault)) {
    Write-Host "conhost.exe crashes BOTH inside psmux AND without psmux -> confirmed Windows/Claude-Code/nvim bug, NOT a psmux regression." -ForegroundColor Yellow
} elseif (($aCrashed -or $aFault) -and -not ($bCrashed -or $bFault)) {
    Write-Host "Crashed only inside psmux -> revisit: psmux may be involved after all." -ForegroundColor Red
} else {
    Write-Host "No crash observed this run (the conhost UAF is timing-racy; re-run a few times)." -ForegroundColor DarkYellow
}
} finally {
    # Whatever happened above, nothing of ours outlives the suite.
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Stop-Owned "final"
    $env:EDITOR = $null
    $dd = $env:PSMUX_DATA_DIR
    $env:PSMUX_DATA_DIR = $savedDataDir
    Remove-Item $dd -Recurse -Force -EA SilentlyContinue
}
