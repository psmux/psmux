# Issue #668: send-keys C-c silently fails to interrupt a pane child.
#
# WHAT WENT WRONG, AND WHY NOTHING SHOWED IT
#
# ConsoleFlags bit 0 in RTL_USER_PROCESS_PARAMETERS is a process's "Ctrl+C is
# disabled" state.  CreateProcess with CREATE_NEW_PROCESS_GROUP sets it, so does
# SetConsoleCtrlHandler(NULL, TRUE), it is INHERITED by every child -- measured:
# inheritance survives CREATE_NEW_CONSOLE, which is how psmux starts its server
# -- and Windows offers no API to read it back.
#
# So a launcher that holds the bit (backgrounding a shell with Node's
# spawn({detached:true}) is exactly this) hands it to the psmux CLI, the CLI
# hands it to the server, the server hands it to every ConPTY pane shell and
# thereby to every program the user runs in a pane.  From then on
# GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) still returns success, the pane's
# child is still a member of the console it was broadcast to, the console input
# mode still has ENABLE_PROCESSED_INPUT -- and conhost skips the child anyway.
# `send-keys C-c` exits 0 and does nothing, for that session's whole life, with
# no signal anywhere that the interrupt was dropped.
#
# THE TEST
#
# Two arms that differ in exactly one variable: whether the process that runs
# the psmux CLI was created with CREATE_NEW_PROCESS_GROUP.  Both must interrupt
# the pane child.  Measured against the unfixed build the poisoned arm never
# interrupts it (3 of 3 runs, child still counting 7 s later) while the control
# arm interrupts in ~220 ms, so the arms really do separate the defect.
#
# The launcher itself is asserted first: if newprocgroup.exe stopped setting the
# bit, both arms would pass while proving nothing, so a control child that does
# NOT read ignoreCtrlC=True fails the suite rather than skipping it.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$launcher = "$env:TEMP\psmux_668_newprocgroup.exe"
$flagsExe = "$env:TEMP\psmux_668_ctrlflags.exe"
$floodJs  = "$env:TEMP\psmux_668_flood.js"

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "Issue #668: inherited ignore-Ctrl+C in a pane" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

if (-not (Test-Path $csc)) {
    Write-Info "csc.exe not found; cannot build the probes. Skipping."
    exit 0
}
$node = (Get-Command node -EA SilentlyContinue)
if (-not $node) {
    Write-Info "node not found; the flood child is unavailable. Skipping."
    exit 0
}

foreach ($pair in @(@("$PSScriptRoot\newprocgroup.cs", $launcher), @("$PSScriptRoot\ctrlflags.cs", $flagsExe))) {
    $src = $pair[0]; $out = $pair[1]
    if (-not (Test-Path $src)) { Write-Fail "missing probe source $src"; exit $script:TestsFailed }
    $stale = (-not (Test-Path $out)) -or ((Get-Item $src).LastWriteTime -gt (Get-Item $out -EA SilentlyContinue).LastWriteTime)
    if ($stale) { & $csc /nologo /optimize /out:$out $src 2>&1 | Out-Null }
    if (-not (Test-Path $out)) { Write-Fail "could not build $out"; exit $script:TestsFailed }
}

@'
let n = 0;
setInterval(() => { process.stdout.write(`i668 tick ${n}\n`); n++; }, 50);
'@ | Set-Content $floodJs -Encoding UTF8

function Get-IgnoreCtrlC([int]$ProcId) {
    if ($ProcId -le 0) { return $null }
    $line = (& $flagsExe $ProcId 2>&1 | Out-String).Trim()
    if ($line -match 'ignoreCtrlC=(True|False)') { return ($Matches[1] -eq 'True') }
    return $null
}

# ---------------------------------------------------------------------------
# The launcher must really set the bit, or neither arm means anything.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 1] the harness itself sets the inheritable ignore-Ctrl+C bit" -ForegroundColor Yellow
$probe = Start-Process -FilePath $launcher -ArgumentList "cmd.exe","/c","ping -n 12 127.0.0.1 > nul" -PassThru -WindowStyle Hidden
Start-Sleep -Milliseconds 900
$victim = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($probe.Id)" -EA SilentlyContinue) | Select-Object -First 1
$victimIgnores = if ($victim) { Get-IgnoreCtrlC ([int]$victim.ProcessId) } else { $null }
if ($victimIgnores -eq $true) {
    Write-Pass "newprocgroup.exe child reads ignoreCtrlC=True (the defect's input is present)"
} else {
    Write-Fail "newprocgroup.exe child does NOT read ignoreCtrlC=True (got '$victimIgnores'): the poisoned arm below would prove nothing"
}
if ($victim) { Stop-Process -Id ([int]$victim.ProcessId) -Force -EA SilentlyContinue }
if (-not $probe.HasExited) { Stop-Process -Id $probe.Id -Force -EA SilentlyContinue }

# ---------------------------------------------------------------------------
# One arm: create a session through $Launch, flood a pane, interrupt it.
# ---------------------------------------------------------------------------
function Invoke-Arm {
    param([string]$Label, [switch]$Poisoned)

    $ns = "i668$Label"
    $sess = "s668$Label"
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300

    Write-Host "`n[$Label arm] psmux CLI launched with CREATE_NEW_PROCESS_GROUP = $([bool]$Poisoned)" -ForegroundColor Yellow
    $args = @()
    if (-not $Poisoned) { $args += "--plain" }
    $args += @($PSMUX, "-L", $ns, "new-session", "-d", "-s", $sess)
    & $launcher @args 2>&1 | Out-Null
    Start-Sleep -Seconds 3

    $panePid = 0
    try { $panePid = [int](& $PSMUX -L $ns display-message -t "${sess}:0" -p '#{pane_pid}' 2>&1 | Out-String).Trim() } catch { }
    if ($panePid -le 0) {
        Write-Fail "$Label arm: no session (pane_pid=$panePid)"
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
        return
    }

    $shellIgnores = Get-IgnoreCtrlC $panePid
    if ($shellIgnores -eq $false) {
        Write-Pass "$Label arm: pane shell pid=$panePid reads ignoreCtrlC=False"
    } else {
        Write-Fail "$Label arm: pane shell pid=$panePid reads ignoreCtrlC=$shellIgnores -- the pane was born deaf to Ctrl+C"
    }

    & $PSMUX -L $ns send-keys -t "${sess}:0" "node `"$floodJs`"" Enter
    Start-Sleep -Seconds 4
    $childPid = 0
    foreach ($c in @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$panePid" -EA SilentlyContinue)) {
        if ($c.Name -eq "node.exe") { $childPid = [int]$c.ProcessId }
    }
    if ($childPid -le 0) {
        Write-Fail "$Label arm: the flood child never started"
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
        return
    }
    $childIgnores = Get-IgnoreCtrlC $childPid
    if ($childIgnores -eq $false) {
        Write-Pass "$Label arm: pane child pid=$childPid reads ignoreCtrlC=False"
    } else {
        Write-Fail "$Label arm: pane child pid=$childPid reads ignoreCtrlC=$childIgnores -- no console signal can reach it"
    }

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $PSMUX -L $ns send-keys -t "${sess}:0" C-c
    $rc = $LASTEXITCODE
    $died = $false
    while ($sw.ElapsedMilliseconds -lt 8000) {
        if (-not (Get-Process -Id $childPid -EA SilentlyContinue)) { $died = $true; break }
        Start-Sleep -Milliseconds 100
    }
    if ($died) {
        Write-Pass "$Label arm: send-keys C-c interrupted the pane child in $($sw.ElapsedMilliseconds)ms (rc=$rc)"
    } else {
        $cap = & $PSMUX -L $ns capture-pane -t "${sess}:0" -p 2>&1 | Out-String
        $tick = -1
        foreach ($m in [regex]::Matches($cap, 'i668 tick (\d+)')) { $v = [int]$m.Groups[1].Value; if ($v -gt $tick) { $tick = $v } }
        Write-Fail "$Label arm: send-keys C-c returned $rc and the child is still alive after $($sw.ElapsedMilliseconds)ms (screen counter=$tick)"
    }

    if ($childPid -gt 0) { Stop-Process -Id $childPid -Force -EA SilentlyContinue }
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
}

Invoke-Arm -Label "clean"
Invoke-Arm -Label "deaf" -Poisoned

Remove-Item $floodJs -Force -EA SilentlyContinue

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "========================================" -ForegroundColor Cyan

exit $script:TestsFailed
