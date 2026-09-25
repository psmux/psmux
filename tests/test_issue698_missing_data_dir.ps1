# Issue #698: a missing data directory was read as lock contention.
#
# `AppState::new` allocates a session id, which takes `CounterLock` on
# `<data dir>/next_session_id.lock` (session.rs). Creating that file fails with
# NotFound when the directory is not there yet, which is what a first run on a
# machine looks like, and the acquire loop treated every failure as "someone
# else holds it": 2000 sleeps of 1ms before giving up and proceeding.
#
# Measured on Windows 11 Pro 26200, release build, before the fix:
#
#     data dir missing   3.348 s
#     data dir present   0.119 s
#
# Nothing reported it: the session still came up, only slowly. The counter was
# wrong too, because `next_session_id` cannot be written into a directory that
# is not there, so the id never advanced.
#
# The assertion below is the DIFFERENCE between the two, not a fixed budget: a
# loaded or slow machine moves both numbers together, and only the bug moves
# them apart.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = if ($env:PSMUX_TEST_NS) { $env:PSMUX_TEST_NS } else { "i698dir" }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor Cyan }

foreach ($v in 'PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') {
    Remove-Item "env:$v" -EA SilentlyContinue
}
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm  = $env:PSMUX_NO_WARM
$env:PSMUX_NO_WARM = "1"
$root = Join-Path $env:TEMP "psmux_i698_missing_dir"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null

Write-Host ""
Write-Host "=== Issue #698: a missing data directory must not be waited on ===" -ForegroundColor Magenta
Write-Info "Binary: $PSMUX"

# One `new-session -d` against $dataDir, returning how long it took in seconds.
function Measure-NewSession([string]$dataDir, [string]$session) {
    $env:PSMUX_DATA_DIR = $dataDir
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & $PSMUX -L $NS new-session -d -s $session -x 80 -y 24 2>&1 | Out-Null
    $sw.Stop()
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 350
    return $sw.Elapsed.TotalSeconds
}

$missing = Join-Path $root "absent"     # deliberately never created
$present = Join-Path $root "present"
New-Item -ItemType Directory -Force $present | Out-Null

$tMissing = Measure-NewSession $missing "i698_a"
$tPresent = Measure-NewSession $present "i698_b"
Write-Info ("data dir missing: {0:N3} s" -f $tMissing)
Write-Info ("data dir present: {0:N3} s" -f $tPresent)

# The old loop could not finish in under 2000ms by construction, so a second of
# headroom over the control still fails the moment it comes back.
$delta = $tMissing - $tPresent
if ($delta -lt 1.0) {
    Write-Pass ("a missing data directory costs {0:N3} s more than a present one" -f $delta)
} else {
    Write-Fail ("a missing data directory costs {0:N3} s more than a present one; the acquire budget is being spent on a NotFound again" -f $delta)
}

if (Test-Path (Join-Path $missing "next_session_id")) {
    Write-Pass "the run created the data directory and wrote the session counter into it"
} else {
    Write-Fail "no next_session_id under the missing data directory: the counter cannot persist"
}

& $PSMUX -L $NS kill-server 2>&1 | Out-Null
Remove-Item -Recurse -Force $root -EA SilentlyContinue
if ($null -ne $savedDataDir) { $env:PSMUX_DATA_DIR = $savedDataDir } else { Remove-Item env:PSMUX_DATA_DIR -EA SilentlyContinue }
if ($null -ne $savedNoWarm)  { $env:PSMUX_NO_WARM  = $savedNoWarm }  else { Remove-Item env:PSMUX_NO_WARM  -EA SilentlyContinue }

Write-Host ""
Write-Host "=== Results ===" -ForegroundColor Magenta
Write-Host "  Passed:  $script:TestsPassed" -ForegroundColor Green
Write-Host "  Failed:  $script:TestsFailed" -ForegroundColor $(if ($script:TestsFailed -gt 0) { 'Red' } else { 'Green' })
exit $(if ($script:TestsFailed -gt 0) { 1 } else { 0 })
