# Issue #817: PSMUX_CONPTY_DIR set at new-session time is ignored when the
# session is made by claiming the namespace's __warm__ standby, because that
# standby already picked its console host (inbox conhost or OpenConsole) from
# its OWN environment.  The reverse holds too: a standby started WITH the
# variable hands OpenConsole to a client that no longer asks for it.
#
# The console host is a direct child of the server process that owns the
# pane, so the check is simply "what is the name of the server's console
# host child": conhost.exe or OpenConsole.exe.
#
# Needs a conpty.dll + OpenConsole.exe pair (PSMUX_TEST_CONPTY_DIR, or the
# one tests/test_issue597_da_replies_openconsole.ps1 unpacks); skips without.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "i817_$PID"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }
function Summary { Write-Host "`n=== Issue #817 warm standby and PSMUX_CONPTY_DIR: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:TestsSkipped) skipped ===" -ForegroundColor Cyan }

foreach ($v in 'PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX','PSMUX_CONPTY_DIR','PSMUX_NO_WARM') {
    Remove-Item "env:$v" -EA SilentlyContinue
}

$hostDir = $env:PSMUX_TEST_CONPTY_DIR
if (-not $hostDir) { $hostDir = "$env:TEMP\occ-conpty\nugetstable" }
if (-not ((Test-Path (Join-Path $hostDir "conpty.dll")) -and (Test-Path (Join-Path $hostDir "OpenConsole.exe")))) {
    Write-Skip "no conpty.dll + OpenConsole.exe pair at $hostDir"
    Summary; exit 0
}

$psmuxDir = "$env:USERPROFILE\.psmux"
$warmPort = Join-Path $psmuxDir "${NS}____warm__.port"

function Wait-Warm {
    # The standby registers its port file once it is ready to be claimed.
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path $warmPort) { Start-Sleep -Milliseconds 800; return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

function Get-HostName([string]$session) {
    # The pane's console host is a child of the server that owns the pane.
    # The server's own console is a plain `conhost.exe 0x4` child; pane hosts
    # run `--headless` (inbox) or are OpenConsole.exe.
    $spid = (& $PSMUX -L $NS display-message -t $session -p '#{pid}' 2>$null | Out-String).Trim()
    if (-not $spid) { return "no-server" }
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $deadline) {
        $kids = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$spid" -EA SilentlyContinue |
            Where-Object { $_.Name -eq 'OpenConsole.exe' -or ($_.Name -eq 'conhost.exe' -and $_.CommandLine -match '--headless') } |
            ForEach-Object Name | Sort-Object -Unique)
        if ($kids.Count -gt 0) { return ($kids -join ',') }
        Start-Sleep -Milliseconds 200
    }
    return "none"
}

function Is-Claimed([string]$session) {
    $spid = (& $PSMUX -L $NS display-message -t $session -p '#{pid}' 2>$null | Out-String).Trim()
    if (-not $spid) { return $false }
    $cl = (Get-CimInstance Win32_Process -Filter "ProcessId=$spid" -EA SilentlyContinue).CommandLine
    return ($cl -match '__warm__')
}

$before = (& $PSMUX ls 2>&1 | Out-String)

try {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null

    Write-Host "`n[Part 1] standby made WITHOUT the variable, client asks for OpenConsole" -ForegroundColor Yellow
    & $PSMUX -L $NS new-session -d -s base 2>&1 | Out-Null
    $h = Get-HostName base
    if ($h -eq 'conhost.exe') { Write-Pass "baseline session without the variable runs on conhost.exe" }
    else { Write-Fail "baseline session host: $h" }
    if (-not (Wait-Warm)) { Write-Fail "no __warm__ standby appeared for $NS" }

    $warmPid = [int](((Get-Content (Join-Path $psmuxDir "${NS}____warm__.pid") -EA SilentlyContinue) -split ':')[0])
    $env:PSMUX_CONPTY_DIR = $hostDir
    & $PSMUX -L $NS new-session -d -s want_occ 2>&1 | Out-Null
    $h = Get-HostName want_occ
    Write-Info "want_occ claimed a standby: $(Is-Claimed want_occ)"
    if ($h -eq 'OpenConsole.exe') { Write-Pass "PSMUX_CONPTY_DIR set at new-session time gives OpenConsole.exe" }
    else { Write-Fail "PSMUX_CONPTY_DIR set at new-session time ignored, pane host is $h" }

    # The standby that could not serve the client must go away, not linger.
    Start-Sleep -Milliseconds 1500
    if ($warmPid -gt 0 -and -not (Get-Process -Id $warmPid -EA SilentlyContinue)) {
        Write-Pass "the mismatched standby (pid $warmPid) exited"
    } elseif ($warmPid -gt 0 -and (& $PSMUX -L $NS display-message -t want_occ -p '#{pid}' 2>$null) -eq "$warmPid") {
        Write-Fail "the mismatched standby (pid $warmPid) was claimed"
    } else {
        Write-Fail "the mismatched standby (pid $warmPid) is still running unclaimed"
    }

    # A new window in that session must use the same host.
    & $PSMUX -L $NS new-window -t want_occ 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    $h = Get-HostName want_occ
    if ($h -eq 'OpenConsole.exe') { Write-Pass "new-window in that session stays on OpenConsole.exe" }
    else { Write-Fail "new-window in that session host set: $h" }

    Write-Host "`n[Part 2] standby made WITH the variable, client no longer asks for it" -ForegroundColor Yellow
    if (-not (Wait-Warm)) { Write-Fail "no __warm__ standby appeared for $NS" }
    Remove-Item env:PSMUX_CONPTY_DIR -EA SilentlyContinue
    & $PSMUX -L $NS new-session -d -s want_inbox 2>&1 | Out-Null
    $h = Get-HostName want_inbox
    Write-Info "want_inbox claimed a standby: $(Is-Claimed want_inbox)"
    if ($h -eq 'conhost.exe') { Write-Pass "without the variable the new session runs on conhost.exe" }
    else { Write-Fail "without the variable the new session host is $h" }

    Write-Host "`n[Part 3] matching environment still claims the standby (fast path kept)" -ForegroundColor Yellow
    if (-not (Wait-Warm)) { Write-Fail "no __warm__ standby appeared for $NS" }
    & $PSMUX -L $NS new-session -d -s same_env 2>&1 | Out-Null
    $h = Get-HostName same_env
    $claimed = Is-Claimed same_env
    if ($claimed -and $h -eq 'conhost.exe') { Write-Pass "same environment claimed the standby, conhost.exe" }
    else { Write-Fail "same environment: claimed=$claimed host=$h" }
}
finally {
    Remove-Item env:PSMUX_CONPTY_DIR -EA SilentlyContinue
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
}

$after = (& $PSMUX ls 2>&1 | Out-String)
if ($before -ne $after) { Write-Fail "default namespace session list changed!`n$before`n---`n$after" }
Summary
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
