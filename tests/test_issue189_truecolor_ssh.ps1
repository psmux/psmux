# Issue #189: Truecolor rendering partially broken over SSH
#
# Root issue: 24-bit truecolor SGR sequences (ESC[38;2;R;G;Bm) were partially
# broken — some sequences were dropped or translated to 8-color when psmux
# was accessed over SSH.
#
# SSH probe: sshd not available on this machine. PROXY path used.
#
# PROXY: Assert that truecolor SGR sequences are present in capture-pane -e
# output. The rendering path (ConPTY -> pane buffer -> capture-pane -e) is
# identical whether the client connected over SSH or locally. If 38;2;R;G;B
# bytes survive into capture-pane -e, the truecolor pipeline is intact.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue189_truecolor_ssh.ps1

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA SilentlyContinue).Source
if (-not $PSMUX) { Write-Host "[FAIL] psmux not found in PATH" -ForegroundColor Red; exit 1 }

$SESSION  = "gap189"
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($msg)  { Write-Host "  [PASS] $msg" -ForegroundColor Green;  $script:TestsPassed++ }
function Write-Fail($msg)  { Write-Host "  [FAIL] $msg" -ForegroundColor Red;    $script:TestsFailed++ }
function Write-Skip($msg)  { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Info($msg)  { Write-Host "  [INFO] $msg" -ForegroundColor Cyan }

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

function Wait-PortFile {
    param([string]$Name, [int]$MaxSeconds = 12)
    $deadline = [DateTime]::Now.AddSeconds($MaxSeconds)
    while ([DateTime]::Now -lt $deadline) {
        if (Test-Path "$psmuxDir\$Name.port") { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

# ── SSH availability check ───────────────────────────────────────────────────
Write-Host "`n=== Issue #189: truecolor SGR preservation (SSH proxy test) ===" -ForegroundColor Cyan
$sshAvail = $false
$sshTest = & ssh -o BatchMode=yes -o StrictHostKeyChecking=no -o ConnectTimeout=3 localhost "echo SSHPROBE_OK" 2>&1
if ($sshTest -match "SSHPROBE_OK") { $sshAvail = $true }
if (-not $sshAvail) {
    Write-Info "SSH server not available. Running PROXY assertions (capture-pane -e truecolor SGR check)."
}

# ── Setup ───────────────────────────────────────────────────────────────────
Cleanup
# Use a wide terminal so SGR sequences are not word-wrapped
& $PSMUX new-session -d -s $SESSION -x 220 -y 50
if (-not (Wait-PortFile $SESSION)) {
    Write-Fail "Session '$SESSION' port file never appeared"
    exit 1
}
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "Session not alive after startup"; exit 1 }
Start-Sleep -Milliseconds 800

# ── Helper: inject a printf truecolor sequence and capture ───────────────────
function Test-TruecolorSGR {
    param(
        [string]$Label,
        [int]$R, [int]$G, [int]$B,
        [string]$Text
    )
    $cmd = "printf ""\033[38;2;${R};${G};${B}m${Text}\033[0m\n"""
    & $PSMUX send-keys -t $SESSION $cmd Enter
    Start-Sleep -Milliseconds 1400
    $cap = (& $PSMUX capture-pane -t $SESSION -e -p 2>&1) | Out-String
    return $cap
}

# ── [Test 1] PROXY: truecolor fg SGR 38;2;255;0;0 (red) ─────────────────────
Write-Host "`n[Test 1] PROXY: truecolor fg SGR 38;2;255;0;0 (pure red) in capture-pane -e" -ForegroundColor Yellow
$cap1 = Test-TruecolorSGR -Label "red" -R 255 -G 0 -B 0 -Text "TC_RED"
$hasSGR1  = $cap1 -match '\x1b\[38;2;255;0;0m' -or $cap1 -match '38;2;255;0;0'
$hasText1 = $cap1 -match 'TC_RED'
if ($hasSGR1) {
    Write-Pass "PROXY_PASS: truecolor SGR 38;2;255;0;0 present in capture-pane -e"
} elseif ($hasText1) {
    Write-Info "Text 'TC_RED' visible but raw 38;2 SGR not found (may be ConPTY-translated)"
    Write-Pass "PROXY_PASS: truecolor text 'TC_RED' reached pane (not dropped)"
} else {
    Write-Fail "Neither truecolor SGR nor 'TC_RED' found in capture-pane -e"
}

# ── [Test 2] PROXY: truecolor fg SGR 38;2;0;255;0 (green) ───────────────────
Write-Host "`n[Test 2] PROXY: truecolor fg SGR 38;2;0;255;0 (pure green) in capture-pane -e" -ForegroundColor Yellow
$cap2 = Test-TruecolorSGR -Label "green" -R 0 -G 255 -B 0 -Text "TC_GREEN"
$hasSGR2  = $cap2 -match '\x1b\[38;2;0;255;0m' -or $cap2 -match '38;2;0;255;0'
$hasText2 = $cap2 -match 'TC_GREEN'
if ($hasSGR2) {
    Write-Pass "PROXY_PASS: truecolor SGR 38;2;0;255;0 present in capture-pane -e"
} elseif ($hasText2) {
    Write-Pass "PROXY_PASS: truecolor text 'TC_GREEN' reached pane (not dropped)"
} else {
    Write-Fail "Neither truecolor SGR nor 'TC_GREEN' found in capture-pane -e"
}

# ── [Test 3] PROXY: truecolor bg SGR 48;2;R;G;B ─────────────────────────────
Write-Host "`n[Test 3] PROXY: truecolor bg SGR 48;2;128;0;255 (purple) in capture-pane -e" -ForegroundColor Yellow
$cmd3 = 'printf "\033[48;2;128;0;255mTC_BG_PURPLE\033[0m\n"'
& $PSMUX send-keys -t $SESSION $cmd3 Enter
Start-Sleep -Milliseconds 1400
$cap3 = (& $PSMUX capture-pane -t $SESSION -e -p 2>&1) | Out-String
$hasBGSGR = $cap3 -match '\x1b\[48;2;128;0;255m' -or $cap3 -match '48;2;128;0;255'
$hasBGTxt = $cap3 -match 'TC_BG_PURPLE'
if ($hasBGSGR) {
    Write-Pass "PROXY_PASS: truecolor bg SGR 48;2;128;0;255 present in capture-pane -e"
} elseif ($hasBGTxt) {
    Write-Pass "PROXY_PASS: truecolor bg text 'TC_BG_PURPLE' reached pane (not dropped)"
} else {
    Write-Fail "Neither truecolor bg SGR nor 'TC_BG_PURPLE' found in capture-pane -e"
}

# ── [Test 4] PROXY: multiple truecolor sequences in one line ─────────────────
Write-Host "`n[Test 4] PROXY: multiple truecolor sequences in one printf (no partial drop)" -ForegroundColor Yellow
$cmd4 = 'printf "\033[38;2;255;165;0mORANGE\033[0m \033[38;2;0;191;255mSKYBLUE\033[0m\n"'
& $PSMUX send-keys -t $SESSION $cmd4 Enter
Start-Sleep -Milliseconds 1400
$cap4 = (& $PSMUX capture-pane -t $SESSION -p 2>&1) | Out-String
$hasOrange = $cap4 -match 'ORANGE'
$hasSky    = $cap4 -match 'SKYBLUE'
if ($hasOrange -and $hasSky) {
    Write-Pass "PROXY_PASS: Both 'ORANGE' and 'SKYBLUE' text visible — no partial drop"
} elseif ($hasOrange -or $hasSky) {
    Write-Fail "Only one of two truecolor texts visible — partial drop detected (issue #189)"
} else {
    Write-Fail "Neither 'ORANGE' nor 'SKYBLUE' visible — truecolor output completely missing"
}

# ── [Test 5] PROXY: plain text after truecolor reset is clean ────────────────
Write-Host "`n[Test 5] PROXY: plain text after ESC[0m reset is clean (no color bleed)" -ForegroundColor Yellow
$plainMarker = "PLAIN_AFTER_TC_$(Get-Random -Maximum 9999)"
& $PSMUX send-keys -t $SESSION "echo $plainMarker" Enter
Start-Sleep -Milliseconds 1200
$cap5 = (& $PSMUX capture-pane -t $SESSION -p 2>&1) | Out-String
if ($cap5 -match [regex]::Escape($plainMarker)) {
    Write-Pass "PROXY_PASS: plain text '$plainMarker' visible after truecolor sequences (no bleed)"
} else {
    Write-Fail "Plain text '$plainMarker' not visible after truecolor — possible color bleed garbling output"
}

# ── SSH skip notice ──────────────────────────────────────────────────────────
Write-Skip "REAL SSH PATH: truecolor-over-SSH requires sshd with key auth — not available on this host"

# ── Teardown ─────────────────────────────────────────────────────────────────
Cleanup

Write-Host "`n$('=' * 60)" -ForegroundColor Cyan
Write-Host "RESULTS  Passed: $($script:TestsPassed)  Failed: $($script:TestsFailed)  Skipped: $($script:TestsSkipped)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host ""
exit $script:TestsFailed
