# Issue #457: SSH attach must NOT enable mouse reporting on ConPTY builds < 22523.
#
# On Win10 / early Win11 (< 22523), psmux used to log "does NOT support mouse"
# and then force-enable mouse anyway via a raw WriteFile that bypassed ConPTY.
# The first click then sent an SGR mouse report back into ConPTY input, where
# the old conhost VT parser fast-failed (0xc0000409) and killed the session.
#
# The fix gates send_mouse_enable() on build >= 22523. This test drives the
# real SSH-input module (activated by SSH_TTY) and pins the reported build via
# PSMUX_FAKE_WIN_BUILD, then inspects ~/.psmux/ssh_input.log to prove:
#   - build 19045  -> SUPPRESSED, zero mouse-enable writes  (the fix)
#   - build 26200  -> mouse-enable still sent (written=32)   (no regression)

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "test_issue457"
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$logFile  = "$psmuxDir\ssh_input.log"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red;   $script:TestsFailed++ }

function Cleanup {
    & $PSMUX -L $SESSION kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

# Launch a real attach window with the SSH-input module active (SSH_TTY set) and
# a pinned build, let it initialise + resize, then tear it down and return the log.
function Attach-AndCaptureLog {
    param([string]$FakeBuild)

    Remove-Item $logFile -Force -EA SilentlyContinue

    $env:SSH_TTY            = "/dev/pts/0"
    $env:SSH_CONNECTION     = "127.0.0.1 5555 127.0.0.1 22"
    $env:PSMUX_SSH_DEBUG    = "1"
    $env:PSMUX_MOUSE_DEBUG  = "1"
    if ($FakeBuild) { $env:PSMUX_FAKE_WIN_BUILD = $FakeBuild }

    $proc = Start-Process -FilePath $PSMUX -ArgumentList "-L",$SESSION,"attach","-t",$SESSION -PassThru
    Start-Sleep -Seconds 3
    # Force a resize so the resize-path re-send is exercised too.
    & $PSMUX -L $SESSION resize-window -t $SESSION -x 100 -y 28 2>&1 | Out-Null
    Start-Sleep -Seconds 2

    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
    Start-Sleep -Milliseconds 500

    $env:SSH_TTY = $null; $env:SSH_CONNECTION = $null
    $env:PSMUX_SSH_DEBUG = $null; $env:PSMUX_MOUSE_DEBUG = $null; $env:PSMUX_FAKE_WIN_BUILD = $null

    if (Test-Path $logFile) { return (Get-Content $logFile -Raw) }
    return ""
}

Write-Host "`n=== Issue #457: SSH mouse-enable build gate ===" -ForegroundColor Cyan

Cleanup
& $PSMUX -L $SESSION new-session -s $SESSION -d -- cmd
Start-Sleep -Seconds 2
& $PSMUX -L $SESSION set-option -g mouse on 2>&1 | Out-Null
& $PSMUX -L $SESSION has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "session setup failed"; Cleanup; exit 1 }
Write-Pass "session created with mouse on"

# --- Scenario A: reporter's build 19045 must SUPPRESS mouse-enable ---
Write-Host "`n[Scenario A] build 19045 (reporter): mouse-enable suppressed" -ForegroundColor Yellow
$logA = Attach-AndCaptureLog -FakeBuild "19045"
if ($logA -match "Windows build 19045") { Write-Pass "log reports faked build 19045" }
else { Write-Fail "log did not report build 19045" }

if ($logA -match "SUPPRESSED.*19045.*issue #457") { Write-Pass "send_mouse_enable SUPPRESSED on 19045" }
else { Write-Fail "expected SUPPRESSED line for 19045" }

if ($logA -notmatch "writing mouse-enable VT sequences") { Write-Pass "no mouse-enable write attempted (root cause removed)" }
else { Write-Fail "BUG #457 STILL PRESENT: mouse-enable written on 19045" }

if ($logA -notmatch "WriteFile ok=1 written=32") { Write-Pass "zero 32-byte mouse-enable WriteFile on 19045" }
else { Write-Fail "BUG #457 STILL PRESENT: 32-byte mouse-enable WriteFile on 19045" }

# --- Scenario B: modern build 26200 must STILL enable mouse (no regression) ---
Write-Host "`n[Scenario B] build 26200 (modern): mouse-enable still sent" -ForegroundColor Yellow
$logB = Attach-AndCaptureLog -FakeBuild "26200"
if ($logB -match "mouse over SSH should be supported") { Write-Pass "modern build reported supported" }
else { Write-Fail "modern build not reported supported" }

if ($logB -match "WriteFile ok=1 written=32") { Write-Pass "mouse-enable still sent on 26200 (no regression)" }
else { Write-Fail "REGRESSION: mouse-enable not sent on modern build 26200" }

Cleanup

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
