# Ctrl+C with a VT bridge on a PRIVATE console in the pane's process tree.
#
# A pane shell that started an ssh.exe on its own hidden console (an `ssh -N`
# tunnel a tool or TUI launched) made the Ctrl+C router choose "raw 0x03
# only" by ancestry and strip ENABLE_PROCESSED_INPUT from the pane console.
# The bridge is out of the broadcast's reach, so a shell builtin in the
# foreground lost its interrupt: pwsh `Start-Sleep`, a pwsh `while` loop and a
# cmd `for /l` loop ignored C-c (0 of 6 each on d1b1f627, fixed in d0ee618e).
# Console membership, not ancestry, now decides.
#
# tmux parity: tmux writes 0x03 to the pane pty and the line discipline
# signals that pty's foreground group only; a process on another pty is never
# involved, which is what scoping by console membership reproduces.
#
#   [1] pwsh + private helper, Start-Sleep 100        -> interrupted
#   [2] pwsh + private helper, while ($true) loop     -> interrupted
#   [3] cmd  + private helper, for /l endless loop    -> interrupted
#   [4] guard: helper on the pane console, ping -t    -> interrupted
#   [5] guard: wsl.exe pane, sleep 100 gets SIGINT from the raw 0x03 and the
#       WSL shell survives (skipped without a WSL distribution)
#
# Each case runs 3 times. The ssh.exe helper is tests\mouse_bridge_console_child.cs
# in its `sleep` mode, copied under that name into a private temp directory;
# it never touches the network. PSMUX_EXE (else psmux on PATH) picks the
# binary. Own PSMUX_DATA_DIR and a -L namespace per run; cleanup is
# `-L <ns> kill-server` plus helper PIDs found by their unique exe path.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
Remove-Item Env:SHELL -EA SilentlyContinue
$PSMUX = $env:PSMUX_EXE
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -EA SilentlyContinue).Source }
if (-not $PSMUX) { Write-Error "psmux binary not found (set PSMUX_EXE)"; exit 1 }

$RUNS = 3
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:Namespaces = @()
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$Temp = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_ctrlc_pb_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $Temp | Out-Null
$env:PSMUX_DATA_DIR = $Temp
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_RUN_NOPAUSE = "1"

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
$HelperExe = Join-Path $Temp "ssh.exe"
if ($csc) { & $csc /nologo /optimize "/out:$HelperExe" (Join-Path $PSScriptRoot "mouse_bridge_console_child.cs") 2>&1 | Out-Null }
if (-not (Test-Path $HelperExe)) { Write-Error "could not build tests\mouse_bridge_console_child.cs"; exit 1 }

function Cap($ns) { (& $PSMUX -L $ns capture-pane -p -t d -S -200 2>$null) -join "`n" }
function WaitFor($ns, $re, $ms) {
    $dl = [DateTime]::UtcNow.AddMilliseconds($ms)
    do { if ((Cap $ns) -match $re) { return $true }; Start-Sleep -Milliseconds 150 } while ([DateTime]::UtcNow -lt $dl)
    return $false
}
function Helpers { @(Get-CimInstance Win32_Process -Filter "Name='ssh.exe'" | Where-Object { $_.ExecutablePath -eq $HelperExe }) }
function Under([int]$procId, [int]$root) {
    for ($k = 0; $k -lt 4; $k++) {
        $p = Get-CimInstance Win32_Process -Filter "ProcessId=$procId" -EA SilentlyContinue
        if (-not $p) { return $false }
        if ($p.ParentProcessId -eq $root) { return $true }
        $procId = $p.ParentProcessId
    }
    return $false
}
function Stop-Helpers { foreach ($h in Helpers) { Stop-Process -Id $h.ProcessId -Force -EA SilentlyContinue } }

# One run: start $shell, optionally a helper, run $fg, press C-c, then prove
# the shell took the next line. Returns "ok", "stuck" or a setup failure text.
function Run-Case([string]$shell, [string]$helper, [string]$fg, [int]$i) {
    $ns = "cpb" + [guid]::NewGuid().ToString('N').Substring(0, 8)
    $script:Namespaces += $ns
    try {
        switch ($shell) {
            'pwsh' { & $PSMUX -L $ns -f NUL new-session -d -s d -x 120 -y 30 pwsh -NoProfile -NoLogo }
            'cmd'  { & $PSMUX -L $ns -f NUL new-session -d -s d -x 120 -y 30 cmd.exe }
            'wsl'  { & $PSMUX -L $ns -f NUL new-session -d -s d -x 120 -y 30 wsl.exe }
        }
        if (-not (WaitFor $ns '(?m)(>|\$)\s*$' 30000)) { return "no prompt" }
        Start-Sleep -Milliseconds 800
        if ($helper -ne 'none') {
            $panePid = [int]((& $PSMUX -L $ns display -p -t d '#{pane_pid}') | Out-String).Trim()
            $line = switch ("$shell/$helper") {
                'pwsh/private' { "Start-Process -WindowStyle Hidden '$HelperExe' sleep" }
                'pwsh/same'    { "Start-Process -NoNewWindow '$HelperExe' sleep" }
                'cmd/private'  { "start /min `"`" `"$HelperExe`" sleep" }
            }
            & $PSMUX -L $ns send-keys -t d $line Enter
            $dl = [DateTime]::UtcNow.AddSeconds(10)
            # A command pane can run its program under the default shell, so
            # the helper may sit a level or two below pane_pid.
            do { Start-Sleep -Milliseconds 200; $mine = @(Helpers | Where-Object { Under $_.ProcessId $panePid }) } until ($mine.Count -gt 0 -or [DateTime]::UtcNow -gt $dl)
            if ($mine.Count -eq 0) {
                $seen = (Helpers | ForEach-Object { "$($_.ProcessId)<-$($_.ParentProcessId)" }) -join ','
                return "helper did not start under the pane shell (pane_pid=$panePid helpers=[$seen])"
            }
            Start-Sleep -Milliseconds 800
        }
        & $PSMUX -L $ns send-keys -t d $fg Enter
        Start-Sleep -Seconds 3
        & $PSMUX -L $ns send-keys -t d C-c
        Start-Sleep -Seconds 2
        $probe = switch ($shell) {
            'pwsh' { "Write-Output ('CPB'+'OK$i')" }
            'cmd'  { "echo CPB^OK$i" }
            'wsl'  { "[ `$(uname -s) = Linux ] && echo CPB''OK$i" }
        }
        & $PSMUX -L $ns send-keys -t d $probe Enter
        if (WaitFor $ns "(?m)^CPBOK$i\s*$" 5000) { return "ok" }
        & $PSMUX -L $ns has-session -t d 2>$null
        if ($LASTEXITCODE -ne 0) { return "server gone" }
        return "stuck"
    } finally {
        Stop-Helpers
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    }
}

function Case([string]$label, [string]$shell, [string]$helper, [string]$fg) {
    Write-Host "`n$label" -ForegroundColor Yellow
    # Only "ok" and "stuck" are verdicts. A run whose setup failed (no prompt,
    # helper missing, server gone) says nothing about Ctrl+C, so it is redone,
    # at most 3 times per case, and then counts as a failure.
    $ok = 0; $bad = @(); $redo = @(); $i = 0
    while (($ok + $bad.Count) -lt $RUNS) {
        $i++
        $r = Run-Case $shell $helper $fg $i
        if ($r -eq 'ok') { $ok++ }
        elseif ($r -eq 'stuck') { $bad += "run ${i}: C-c did not interrupt" }
        elseif ($redo.Count -lt 3) { $redo += "run ${i}: $r"; Write-Host "    redo, setup failed: $r" -ForegroundColor DarkYellow }
        else { $bad += "run ${i}: $r" }
    }
    if ($bad.Count -eq 0) { Write-Pass "$label ($RUNS of $RUNS interrupted)" }
    else { Write-Fail "$label ($ok of $RUNS interrupted; $($bad -join '; '))" }
}

Write-Host "=== Ctrl+C with a VT bridge on a private console ($PSMUX) ===" -ForegroundColor Cyan
try {
    Case "[1] pwsh, private ssh helper, Start-Sleep 100" 'pwsh' 'private' 'Start-Sleep 100'
    Case "[2] pwsh, private ssh helper, while loop" 'pwsh' 'private' 'while ($true) { [Threading.Thread]::Sleep(50) }'
    Case "[3] cmd, private ssh helper, for /l loop" 'cmd' 'private' 'for /l %i in (0,0,1) do @rem'
    Case "[4] guard: pwsh, ssh helper on the pane console, ping -t" 'pwsh' 'same' 'ping -t 127.0.0.1'
    & wsl.exe -e true 2>$null
    if ($LASTEXITCODE -eq 0) {
        Case "[5] guard: wsl pane, sleep 100 interrupted, WSL shell survives" 'wsl' 'none' 'sleep 100; echo NOT""INTERRUPTED'
    } else {
        Write-Host "`n[5] SKIP: no WSL distribution" -ForegroundColor Yellow
    }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    Stop-Helpers
    foreach ($n in $script:Namespaces) { & $PSMUX -L $n kill-server 2>&1 | Out-Null }
    Remove-Item -Recurse -Force $Temp -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
