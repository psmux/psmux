# Discussion #349 (follow-up, comment 17754744): after the motion-leak fix
# (57813d1), mouse CLICKS still leaked SGR reports into an interactive
# container terminal once command output filled the screen. The reporter saw
# "0;37;26M0;37;26m" (left) and "0;48;26M0;48;26m" (right).
#
# The bug needs a pane whose foreground is a NON-shell, NON-(wsl/ssh) process
# that has filled the screen and echoes bytes psmux writes to its pty. A local
# podman machine reaches its VM over ssh.exe, which trips psmux's VT-bridge
# guard and hides the trap — so this test uses a tiny synthetic "filler.exe"
# that reproduces the reporter's mechanism deterministically:
#   1. it is not a recognized shell and not a VT bridge;
#   2. it fills the screen (cursor ends near the bottom);
#   3. it echoes every byte read from stdin — so a forwarded click SGR
#      (ESC[<0;x;yM) is echoed as visible text.
#
# Real mouse clicks are injected into the psmux client console via
# WriteConsoleInput and the pane is inspected with capture-pane.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "test_disc349_click"
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }

# SGR click leak signature echoed as text (e.g. "0;21;13M", "0;26;15m").
$LeakPattern = '\d+;\d+;\d+[Mm]'

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$FILLER = "$env:TEMP\mouse_click_leak_filler.exe"
& $csc /nologo /out:$FILLER (Join-Path $PSScriptRoot "mouse_click_leak_filler.cs") 2>&1 | Out-Null
$INJ = "$env:TEMP\psmux_mouse_move_injector.exe"
& $csc /nologo /optimize /out:$INJ (Join-Path $PSScriptRoot "mouse_move_injector.cs") 2>&1 | Out-Null
if (-not (Test-Path $FILLER)) { Write-Fail "filler.exe failed to compile"; exit 1 }
if (-not (Test-Path $INJ)) { Write-Fail "mouse injector failed to compile"; exit 1 }

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

Write-Host "`n=== Discussion #349 follow-up: mouse CLICK leak into a filled non-shell pane ===" -ForegroundColor Cyan
Cleanup

$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
$ok = $false
for ($i=0;$i -lt 40;$i++){ Start-Sleep -Milliseconds 500; if (Test-Path "$psmuxDir\$SESSION.port"){$ok=$true;break} }
if (-not $ok){ Write-Fail "session did not start"; exit 1 }
Start-Sleep 4

# ── [Test 1] the non-shell filler fills the screen and becomes the foreground ──
Write-Host "`n[Test 1] non-shell filler is the pane foreground" -ForegroundColor Yellow
& $PSMUX send-keys -t $SESSION "& '$FILLER'" Enter
Start-Sleep 3
$cmd = (& $PSMUX display-message -t $SESSION -p '#{pane_current_command}' 2>&1 | Out-String).Trim()
if ($cmd -match "filler") { Write-Pass "foreground is the non-shell filler ($cmd)" }
else { Write-Fail "expected filler foreground, got '$cmd'" }

# ── [Test 2] baseline: no stray SGR before any click ──
Write-Host "`n[Test 2] no SGR before any click" -ForegroundColor Yellow
$cap0 = (& $PSMUX capture-pane -t $SESSION -p 2>&1) | Out-String
if ($cap0 -notmatch $LeakPattern) { Write-Pass "clean before clicking" }
else { Write-Fail "unexpected SGR before clicking" }

# ── [Test 3] THE REPORTED BUG: left clicks on the filled screen ──
Write-Host "`n[Test 3] left clicks do not leak SGR into the pane" -ForegroundColor Yellow
& $INJ $proc.Id click 20 12
Start-Sleep -Milliseconds 600
& $INJ $proc.Id click 25 14
Start-Sleep 2
$cap1 = (& $PSMUX capture-pane -t $SESSION -p 2>&1) | Out-String
if ($cap1 -notmatch $LeakPattern) {
    Write-Pass "no SGR click leak on the filled non-shell screen (discussion #349 comment 17754744 fixed)"
} else {
    $sample = ($cap1 -split "`n" | Where-Object { $_ -match $LeakPattern } | Select-Object -First 2) -join ' / '
    Write-Fail "SGR click leaked into the pane: $sample"
}

# ── [Test 4] pane still alive/interactive after the clicks ──
Write-Host "`n[Test 4] pane still functional" -ForegroundColor Yellow
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -eq 0) { Write-Pass "session alive after clicks" } else { Write-Fail "session died" }

Cleanup
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
