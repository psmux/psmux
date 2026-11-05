#requires -Version 5
<#
Issue #989, the detach: the server must survive repeated Ctrl+C in a Git bash
pane, and protecting it must not hold the ignore bit long enough to leak into
the children it starts.

Bug: FreeConsole and AttachConsole each empty a process's list of console control
handlers.  With a plain native app (git.exe from the stock prompt, ping.exe, ...)
on the pane console under an MSYS shell, send_ctrl_c_event leaves
ENABLE_PROCESSED_INPUT on, so a raw 0x03 becomes a real CTRL_C_EVENT inside
conhost, dispatched asynchronously.  One that reached the server with neither
handler nor bit ended it with 0xC000013A (STATUS_CONTROL_C_EXIT): no panic, no
crash.log, every session gone.

Fix: free_and_attach and detach_console (src/platform.rs) hold "ignore Ctrl+C"
across each console change, restore the handler, then clear the bit.

The default scenarios force each hazard with a helper and take about 10 s:
  1. attach: an event in flight WHILE the server is attached, at every press.
  2. late: an event that reaches the server at or just AFTER the detach.  The
     helper raises events as fast as it can, so some straddle the detach.  A
     server without detach_console dies within about 10 presses.
  3. inherit: holding the bit for a whole injection instead of for each console
     change.  Every child inherits the bit (issue #668) and only ConPTY spawns
     wait for the console lock, so run-shell children started during injections
     would be born ignoring Ctrl+C (measured: 13 of 40).  Each console change
     still leaves a short window (measured: under 1 in 100), so this fails only
     above 20%.

-Stock adds the bug as users meet it, with no helper: an interactive login Git
bash in a git repository, so the stock prompt runs git.exe, and send-keys C-c
with a random 0 to 20 ms pause.  The pause matters: at fixed gaps the bug did not
appear.  Without detach_console 5 of 30 runs of 300 presses died, about one death
per 1,600 presses, so this takes about 15 minutes when it passes and can miss the
bug.

Why a LIVE native program in the default scenarios: psmux takes the risky "keep
PROCESSED_INPUT" branch only while a plain native app is on the pane console at
the moment of the press.  So the pane runs Git's /usr/bin/bash directly, ignoring
SIGINT, and that runs tests\ctrlc_native_survivor.cs, which stays alive.

A pass must prove the risky path ran, so it cannot come from doing nothing: the
server's debug log (PSMUX_MOUSE_DEBUG=1, set for this test's servers only) must
show the branch at the presses, and the helper must report what it raised.

This test runs in its OWN namespaces (-L) and only ever kills those.  It needs
Git for Windows and csc.exe, and skips (exit 0) without them.

Usage:  pwsh tests\test_issue989_detach_ctrlc.ps1 [-Psmux <path>]
             [-Bursts 2] [-PerBurst 10] [-LateBursts 6] [-Children 20]
             [-Stock] [-StockRuns 30] [-StockPresses 300]
        Point -Psmux at a fresh build.  Raise -LateBursts to soak.
#>
param(
    [string]$Psmux = $(if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } elseif ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }),
    [int]$Bursts = 2,
    [int]$PerBurst = 10,
    [int]$LateBursts = 6,
    [int]$Children = 20,
    [switch]$Stock,
    [int]$StockRuns = 30,
    [int]$StockPresses = 300
)
$ErrorActionPreference = 'Continue'
Remove-Item Env:SHELL -EA SilentlyContinue
$script:Pass = 0; $script:Fail = 0
function P($m){ Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function F($m){ Write-Host "  [FAIL] $m" -ForegroundColor Red;   $script:Fail++ }
$sw = [Diagnostics.Stopwatch]::StartNew()
if (-not (Get-Command $Psmux -EA SilentlyContinue)) { F "psmux not found: $Psmux"; exit 1 }

# Git for Windows' MSYS bash (NOT whatever bash.exe is first on PATH: that is
# often the WSL launcher in System32).
$gitRoot = $null
foreach ($c in @("$env:ProgramFiles\Git", "${env:ProgramFiles(x86)}\Git")) {
    if ($c -and (Test-Path "$c\usr\bin\bash.exe")) { $gitRoot = $c; break }
}
if (-not $gitRoot) {
    $git = (Get-Command git.exe -EA SilentlyContinue).Source
    if ($git) {
        $c = Split-Path (Split-Path $git)
        if (Test-Path "$c\usr\bin\bash.exe") { $gitRoot = $c }
    }
}
if (-not $gitRoot) { Write-Host "SKIP: Git for Windows usr\bin\bash.exe not found"; exit 0 }
$bash = "$gitRoot\usr\bin\bash.exe"

# Compile the helper (cached in TEMP, rebuilt when the source is newer).
$src = Join-Path $PSScriptRoot 'ctrlc_native_survivor.cs'
$helper = "$env:TEMP\psmux_ctrlc_native_survivor.exe"
if (-not (Test-Path $helper) -or (Get-Item $helper).LastWriteTime -lt (Get-Item $src).LastWriteTime) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) 'csc.exe'
    if (-not (Test-Path $csc)) { $csc = "$env:SystemRoot\Microsoft.NET\Framework64\v4.0.30319\csc.exe" }
    if (-not (Test-Path $csc)) { Write-Host "SKIP: csc.exe not found"; exit 0 }
    & $csc /nologo /optimize "/out:$helper" $src 2>&1 | Out-Null
    if (-not (Test-Path $helper)) { F "could not compile $src"; exit 1 }
}

# A private data dir, so pid files, the debug log and every server of this
# test stay out of the user's own data dir.
$oldDataDir = $env:PSMUX_DATA_DIR
$psmuxDir = Join-Path $env:TEMP ("psmux_t989d_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$null = New-Item -ItemType Directory -Force $psmuxDir
$env:PSMUX_DATA_DIR = $psmuxDir
$debugLog = "$psmuxDir\mouse_debug.log"
function To-Msys([string]$p) {
    $p = $p.Replace('\', '/')
    if ($p -match '^([A-Za-z]):(.*)$') { return '/' + $Matches[1].ToLower() + $Matches[2] }
    return $p
}
function Wait-Until([scriptblock]$cond, [int]$timeoutMs) {
    $t = [Diagnostics.Stopwatch]::StartNew()
    while ($t.ElapsedMilliseconds -lt $timeoutMs) {
        $v = & $cond
        if ($v) { return $v }
        Start-Sleep -Milliseconds 25
    }
    return $null
}
function Read-LogFrom([string]$path, [long]$offset) {
    if (-not (Test-Path $path)) { return '' }
    $fs = [IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
    try {
        $null = $fs.Seek([Math]::Min($offset, $fs.Length), 'Begin')
        return (New-Object IO.StreamReader($fs)).ReadToEnd()
    } finally { $fs.Dispose() }
}
function New-Namespace { 't989d_' + ([guid]::NewGuid().ToString('N').Substring(0, 8)) }

# Start a server in namespace $ns whose pane runs the helper with $helperArgs, and
# wait for it to print READY.  bash ignores SIGINT and restarts the helper should
# it die.  Returns the server's Process, with its handle held so ExitCode stays
# readable, or $null.
function Start-Pane([string]$ns, [string]$helperArgs) {
    $s = 't'
    $loop = "trap '' INT; while :; do '$(To-Msys $helper)' $helperArgs; done"
    $oldDbg = $env:PSMUX_MOUSE_DEBUG
    $env:PSMUX_MOUSE_DEBUG = '1'        # inherited by this test's server only
    # The helper finds the server by process name; tell it this build's name.
    $env:PSMUX_T989_SERVER_NAME = [IO.Path]::GetFileNameWithoutExtension($Psmux)
    & $Psmux -L $ns new-session -d -s $s -x 120 -y 30 -- $bash --norc --noprofile -c $loop | Out-Null
    $env:PSMUX_MOUSE_DEBUG = $oldDbg
    Remove-Item Env:PSMUX_T989_SERVER_NAME -EA SilentlyContinue

    $srv = Wait-Until {
        $f = "$psmuxDir\${ns}__${s}.pid"
        if (Test-Path $f) {
            $p = [int]((((Get-Content $f -Raw -EA SilentlyContinue) + '').Trim() -split ':')[0])
            if ($p -gt 0 -and (Get-Process -Id $p -EA SilentlyContinue)) { $p }
        }
    } 10000
    if (-not $srv) { F "could not find the server pid"; return $null }
    $proc = Get-Process -Id $srv
    $null = $proc.Handle
    $ready = Wait-Until { (& $Psmux -L $ns capture-pane -t $s -p 2>$null | Out-String) -match 'READY' } 15000
    if (-not $ready) { F "helper never printed READY in the pane"; return $null }
    return $proc
}

function Stop-Namespace([string]$ns, $proc) {
    & $Psmux -L $ns kill-server 2>&1 | Out-Null
    if ($proc -and -not $proc.HasExited) { $null = $proc.WaitForExit(3000) }
    # The helper exits with its console; reap anything of THIS run (its command
    # line carries the unique namespace) that has not, by PID.
    $mine = { @(Get-CimInstance Win32_Process -Filter "Name='bash.exe' OR Name='psmux_ctrlc_native_survivor.exe'" -EA SilentlyContinue |
        Where-Object { $_.CommandLine -like "*$ns*" }) }
    $null = Wait-Until { (& $mine).Count -eq 0 } 2000
    & $mine | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
    Remove-Item "$psmuxDir\${ns}__*" -Force -EA SilentlyContinue
}

# Send $bursts bursts of $PerBurst presses, stopping early if the server dies.
# Returns the number of presses sent.
function Send-Presses([string]$ns, $proc, [int]$bursts) {
    $keys = @('C-c') * $PerBurst
    $sent = 0
    for ($b = 0; $b -lt $bursts -and -not $proc.HasExited; $b++) {
        & $Psmux -L $ns send-keys -t t @keys 2>$null | Out-Null
        $sent += $PerBurst
        if ($proc.WaitForExit(50)) { break }
    }
    $null = $proc.WaitForExit(500)
    return $sent
}

function Count-Matches([string]$text, [string]$pattern) { ([regex]::Matches($text, $pattern)).Count }
$KeepBranch = 'plain native app on pane console: keep PROCESSED_INPUT'

# 1. An event in flight while the server is attached.
function Test-Attach {
    Write-Host "`n--- attach: an event in flight during the attach, at every press"
    $ns = New-Namespace
    $countFile = "$env:TEMP\$ns.ctrlc"
    $logStart = if (Test-Path $debugLog) { (Get-Item $debugLog).Length } else { 0 }
    $proc = $null
    try {
        $proc = Start-Pane $ns "attach '$(To-Msys $countFile)' 60"
        if (-not $proc) { return }
        $sent = Send-Presses $ns $proc $Bursts

        $hl = if (Test-Path $countFile) { @(Get-Content $countFile) } else { @() }
        $fired = @($hl | Where-Object { $_ -eq 'fire' }).Count
        $recv  = @($hl | Where-Object { $_ -eq 'recv' }).Count
        $log = Read-LogFrom $debugLog $logStart
        $called = Count-Matches $log 'ctrl_c: called: pid='
        $keep = Count-Matches $log $KeepBranch
        Write-Host ("presses sent {0}, reached the router {1}, keep-PROCESSED_INPUT {2}; helper raised {3} events during attaches, received {4}" -f $sent, $called, $keep, $fired, $recv)

        if ($proc.HasExited) {
            F ("server died at press {0} of {1}, exit 0x{2:X8} (0xC000013A = STATUS_CONTROL_C_EXIT)" -f $called, $sent, $proc.ExitCode)
            return
        }
        P "server alive after $sent presses"
        $want = [int][Math]::Ceiling($sent * 0.9)
        if ($keep -ge $want) { P "router took the keep-PROCESSED_INPUT branch at $keep presses" }
        else { F "router took the keep-PROCESSED_INPUT branch at only $keep presses (want >= $want): risky path not exercised" }
        # The helper polls, so a loaded machine misses some attaches.
        $wantFired = [int][Math]::Ceiling($sent / 2)
        if ($fired -ge $wantFired) { P "a CTRL_C_EVENT was in flight during the server's attach at $fired presses" }
        else { F "a CTRL_C_EVENT was raised during only $fired attaches (want >= $wantFired): hazard not exercised" }
        # recv = the helper's own events + conhost's conversions of psmux's raw 0x03.
        $converted = $recv - $fired
        $wantConv = [int][Math]::Ceiling($sent / 2)
        if ($converted -ge $wantConv) { P "conhost turned about $converted of psmux's raw 0x03 into real CTRL_C_EVENTs" }
        else { F "only about $converted raw 0x03 became real CTRL_C_EVENTs (want >= $wantConv): PROCESSED_INPUT was not on" }
    } finally {
        Stop-Namespace $ns $proc
        Remove-Item $countFile -Force -EA SilentlyContinue
    }
}

# 2. An event that arrives just after the server has detached.
function Test-Late {
    Write-Host "`n--- late: events raised nonstop, so some straddle the detach"
    $ns = New-Namespace
    $countFile = "$env:TEMP\$ns.ctrlc"
    $logStart = if (Test-Path $debugLog) { (Get-Item $debugLog).Length } else { 0 }
    $proc = $null
    try {
        $proc = Start-Pane $ns "storm 0 '$(To-Msys $countFile)' 60"
        if (-not $proc) { return }
        $sent = Send-Presses $ns $proc $LateBursts

        $raised = 100 * @(if (Test-Path $countFile) { Get-Content $countFile }).Count
        # Under the storm each injection is slow, so the server is still working
        # through presses when send-keys returns: count once the log settles.
        $log = Read-LogFrom $debugLog $logStart
        for ($k = 0; $k -lt 20 -and -not $proc.HasExited; $k++) {
            Start-Sleep -Milliseconds 250
            $next = Read-LogFrom $debugLog $logStart
            if ($next.Length -eq $log.Length) { break }
            $log = $next
        }
        $called = Count-Matches $log 'ctrl_c: called: pid='
        $keep = Count-Matches $log $KeepBranch
        Write-Host ("presses sent {0}, reached the router {1}, keep-PROCESSED_INPUT {2}; helper raised at least {3} events" -f $sent, $called, $keep, $raised)

        if ($proc.HasExited) {
            F ("server died at press {0} of {1}, exit 0x{2:X8} (0xC000013A = STATUS_CONTROL_C_EXIT)" -f $called, $sent, $proc.ExitCode)
            return
        }
        P "server alive after $sent presses"
        # Not 90% as in the attach scenario: under the storm some send-keys
        # presses never reach the server (measured: 29 to 37 of 60).  A third
        # is still many times the press at which master dies (the first).
        $want = [int][Math]::Ceiling($sent / 3)
        if ($keep -ge $want) { P "router took the keep-PROCESSED_INPUT branch at $keep presses" }
        else { F "router took the keep-PROCESSED_INPUT branch at only $keep presses (want >= $want): risky path not exercised" }
        $wantEvents = $sent * 5
        if ($raised -ge $wantEvents) { P "at least $raised CTRL_C_EVENTs were raised around those presses" }
        else { F "only $raised CTRL_C_EVENTs were raised (want >= $wantEvents): hazard not exercised" }
    } finally {
        Stop-Namespace $ns $proc
        Remove-Item $countFile -Force -EA SilentlyContinue
    }
}

# 3. Children started during injections must not be born ignoring Ctrl+C.
function Test-Inherit {
    Write-Host "`n--- inherit: run-shell children started during injections"
    $ns = New-Namespace
    $idle = "$env:TEMP\$ns.idle"
    $report = "$env:TEMP\$ns.bit"
    $logStart = if (Test-Path $debugLog) { (Get-Item $debugLog).Length } else { 0 }
    $proc = $null; $ps = $null
    try {
        # A storm that never fires: only a live native program on the console.
        $proc = Start-Pane $ns "storm 100000 '$(To-Msys $idle)' 60"
        if (-not $proc) { return }

        # Presses on another thread, so injections overlap the spawns below.
        $ps = [powershell]::Create()
        $null = $ps.AddScript({
            param($exe, $target)
            $keys = @('C-c') * 10
            while ($true) { & $exe -L $target send-keys -t t @keys 2>$null | Out-Null }
        }).AddArgument($Psmux).AddArgument($ns)
        $null = $ps.BeginInvoke()
        Start-Sleep -Milliseconds 300

        $cmd = "$($helper.Replace('\', '/')) bit $($report.Replace('\', '/'))"
        for ($i = 0; $i -lt $Children; $i++) {
            & $Psmux -L $ns run-shell -b $cmd 2>$null | Out-Null
            Start-Sleep -Milliseconds 40
        }
        $null = Wait-Until { (Test-Path $report) -and @(Get-Content $report).Count -ge $Children } 8000
        $null = $ps.Stop()

        $r = if (Test-Path $report) { @(Get-Content $report) } else { @() }
        $clear  = @($r | Where-Object { $_ -eq 'clear' }).Count
        $ignore = @($r | Where-Object { $_ -eq 'IGNORE' }).Count
        $called = Count-Matches (Read-LogFrom $debugLog $logStart) 'ctrl_c: called: pid='
        Write-Host ("{0} children reported: {1} clear, {2} ignoring Ctrl+C; {3} presses reached the router meanwhile" -f $r.Count, $clear, $ignore, $called)

        if ($proc.HasExited) { F ("server died, exit 0x{0:X8}" -f $proc.ExitCode); return }
        # Not zero: see scenario 3 in the header.
        if ($ignore * 5 -le $r.Count) { P "$ignore of $($r.Count) children inherited the ignore bit" }
        else { F "$ignore of $($r.Count) run-shell children were born ignoring Ctrl+C (issue #668): the bit is held too long" }
        if ($r.Count -ge [int][Math]::Ceiling($Children * 0.9)) { P "$($r.Count) children reported, so the check above saw them" }
        else { F "only $($r.Count) of $Children children reported: check not exercised" }
        # About 27 presses per 20 children on an idle machine; the bar only has to
        # show that they overlapped, not race the machine's load.
        $wantPresses = [int][Math]::Ceiling($Children / 2)
        if ($called -ge $wantPresses) { P "$called presses ran alongside the spawns" }
        else { F "only $called presses ran alongside the spawns (want >= $wantPresses): no overlap" }
    } finally {
        if ($ps) { $null = $ps.Stop(); $ps.Dispose() }
        Stop-Namespace $ns $proc
        Remove-Item $report, $idle -Force -EA SilentlyContinue
    }
}

# The bug as users meet it, no helper.  See -Stock in the header.
function Test-Stock {
    Write-Host "`n--- stock: Git prompt, $StockRuns runs of $StockPresses presses, random 0 to 20 ms pauses"
    $home0 = $env:HOME
    $repo = Split-Path $PSScriptRoot
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $rng = [Random]::new()
    $total = 0
    for ($r = 1; $r -le $StockRuns; $r++) {
        $ns = New-Namespace; $proc = $null
        $empty = "$env:TEMP\$ns.home"
        try {
            # An empty HOME, so no one's own profile changes the prompt.
            $null = New-Item -ItemType Directory -Force $empty
            $env:HOME = $empty
            & $Psmux -L $ns new-session -d -s t -c $repo -x 100 -y 25 -- "$gitRoot\bin\bash.exe" --login -i | Out-Null
            $env:HOME = $home0
            $srv = Wait-Until {
                $f = "$psmuxDir\${ns}__t.pid"
                if (Test-Path $f) {
                    $p = [int]((((Get-Content $f -Raw -EA SilentlyContinue) + '').Trim() -split ':')[0])
                    if ($p -gt 0 -and (Get-Process -Id $p -EA SilentlyContinue)) { $p }
                }
            } 10000
            if (-not $srv) { F "could not find the server pid"; return }
            $proc = Get-Process -Id $srv
            $null = $proc.Handle
            Start-Sleep -Seconds 2   # bash at its prompt

            $psi = [Diagnostics.ProcessStartInfo]::new($Psmux)
            $psi.Arguments = "-L $ns send-keys -t t C-c"
            $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
            $n = 0
            while ($n -lt $StockPresses -and -not $proc.HasExited) {
                $sender = [Diagnostics.Process]::Start($psi); $n++; $total++
                $sender.WaitForExit()
                # A spin: Start-Sleep cannot do 1 ms.
                $until = $clock.Elapsed.TotalMilliseconds + $rng.NextDouble() * 20
                while ($clock.Elapsed.TotalMilliseconds -lt $until) { }
            }
            if ($proc.WaitForExit(800)) {
                F ("run {0}: server died at press {1}, exit 0x{2:X8} (0xC000013A = STATUS_CONTROL_C_EXIT)" -f $r, $n, $proc.ExitCode)
                return
            }
        } finally {
            $env:HOME = $home0
            Stop-Namespace $ns $proc
            Remove-Item $empty -Recurse -Force -EA SilentlyContinue
        }
    }
    P "server alive through $StockRuns runs, $total presses"
}

Write-Host "`n=== Issue #989: the server survives Ctrl+C in a Git bash pane ===" -ForegroundColor Cyan
Write-Host "psmux: $Psmux"
Test-Attach
Test-Late
Test-Inherit
if ($Stock) { Test-Stock }
$env:PSMUX_DATA_DIR = $oldDataDir
Remove-Item $psmuxDir -Recurse -Force -EA SilentlyContinue
Write-Host ("`nPassed: {0}  Failed: {1}  ({2:N1} s)" -f $script:Pass, $script:Fail, ($sw.ElapsedMilliseconds / 1000))
if ($script:Fail -gt 0 -or $script:Pass -eq 0) { exit 1 } else { exit 0 }
