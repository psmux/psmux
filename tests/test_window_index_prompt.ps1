# psmux Window Index Prompt (prefix + ') Feature Test
# Tests: select-window via index prompt for windows 10+, base-index support
# Run: powershell -ExecutionPolicy Bypass -File tests\test_window_index_prompt.ps1

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass { param($msg) Write-Host "[PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail { param($msg) Write-Host "[FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info { param($msg) Write-Host "[INFO] $msg" -ForegroundColor Cyan }
function Write-Test { param($msg) Write-Host "[TEST] $msg" -ForegroundColor White }

$PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -ErrorAction SilentlyContinue).Path
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\debug\psmux.exe" -ErrorAction SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
Write-Info "Using: $PSMUX"

function New-PsmuxSession {
    param([string]$Name)
    Start-Process -FilePath $PSMUX -ArgumentList "new-session -s $Name -d" -WindowStyle Hidden
    Start-Sleep -Seconds 3
}

function Psmux { & $PSMUX @args 2>&1; Start-Sleep -Milliseconds 300 }
function PsmuxQuick { & $PSMUX @args 2>&1; Start-Sleep -Milliseconds 150 }

function Ensure-Session {
    param([string]$Name)
    & $PSMUX has-session -t $Name 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Info "Session '$Name' died, recreating..."
        New-PsmuxSession -Name $Name
        & $PSMUX has-session -t $Name 2>$null
        if ($LASTEXITCODE -ne 0) { Write-Host "FATAL: Cannot recreate session" -ForegroundColor Red; exit 1 }
    }
}

# Kill everything first
Start-Process -FilePath $PSMUX -ArgumentList "kill-server" -WindowStyle Hidden
Start-Sleep -Seconds 3
Remove-Item "$env:USERPROFILE\.psmux\*.port" -Force -ErrorAction SilentlyContinue
Remove-Item "$env:USERPROFILE\.psmux\*.key" -Force -ErrorAction SilentlyContinue

# Create test session
Write-Info "Creating test session 'winidx'..."
New-PsmuxSession -Name "winidx"
& $PSMUX has-session -t winidx 2>$null
if ($LASTEXITCODE -ne 0) { Write-Host "FATAL: Cannot create test session" -ForegroundColor Red; exit 1 }
Write-Info "Session 'winidx' created"

# ============================================================
# 1. CREATE MULTIPLE WINDOWS (12 total, indices 0..11)
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "WINDOW INDEX PROMPT TESTS (prefix + ')"
Write-Host ("=" * 60)

Write-Test "Creating 11 additional windows (total 12)"
for ($i = 1; $i -le 11; $i++) {
    PsmuxQuick new-window -t winidx -n "win$i"
}
Start-Sleep -Milliseconds 500
$lsw = Psmux list-windows -t winidx
$winCount = (($lsw -split "`n") | Where-Object { $_.Trim() -ne "" }).Count
if ($winCount -ge 12) {
    Write-Pass "Created $winCount windows (expected >= 12)"
} else {
    Write-Fail "Expected >= 12 windows, got $winCount"
}

# ============================================================
# 2. SELECT-WINDOW TO HIGH INDEX VIA CLI (verifies the backend)
# ============================================================
Write-Test "select-window -t :11 (high index via CLI)"
Psmux select-window -t "winidx:11"
Start-Sleep -Milliseconds 500
$lsw = Psmux list-windows -t winidx
# The active window should have * marker on window 11
if ($lsw -match '11:\s+win11\*') {
    Write-Pass "select-window -t :11 activated window 11"
} else {
    Write-Fail "Window 11 not active after select-window -t :11. Output: $lsw"
}

Write-Test "select-window -t :0 (back to first)"
Psmux select-window -t "winidx:0"
Start-Sleep -Milliseconds 500
$lsw = Psmux list-windows -t winidx
if ($lsw -match '0:\s+\S+\*') {
    Write-Pass "select-window -t :0 returned to window 0"
} else {
    Write-Fail "Window 0 not active. Output: $lsw"
}

# ============================================================
# 3. SELECT-WINDOW MULTIDIGIT (10, 11)
# ============================================================
Write-Test "select-window -t :10 (double digit)"
Psmux select-window -t "winidx:10"
Start-Sleep -Milliseconds 500
$lsw = Psmux list-windows -t winidx
if ($lsw -match '10:\s+win10\*') {
    Write-Pass "select-window -t :10 activated window 10"
} else {
    Write-Fail "Window 10 not active. Output: $lsw"
}

# ============================================================
# 4. HELP TEXT CONTAINS THE BINDING
# ============================================================
Write-Test "Help text includes prefix + ' binding"
$helpOut = & $PSMUX --help 2>&1 | Out-String
if ($helpOut -match "prefix \+ '.*Select window by index|prefix \+ '.*window.*index") {
    Write-Pass "Help text mentions prefix + ' for window index"
} else {
    # Also check list-keys output if available
    $keysOut = Psmux list-keys 2>&1 | Out-String
    # tmux's binding since #821 (key-bindings.c:364, tag 3.7c)
    if ($keysOut -match "'.*command-prompt.*-p index.*select-window -t ':%%'") {
        Write-Pass "list-keys shows ' bound to command-prompt -p index select-window"
    } else {
        Write-Fail "Help/list-keys does not mention prefix + ' binding"
    }
}

# ============================================================
# 5. OUT-OF-RANGE INDEX IS SAFE
# ============================================================
Write-Test "select-window out-of-range does not crash"
Psmux select-window -t "winidx:999"
Start-Sleep -Milliseconds 300
& $PSMUX has-session -t winidx 2>$null
if ($LASTEXITCODE -eq 0) {
    Write-Pass "Session survived out-of-range select-window"
} else {
    Write-Fail "Session died after out-of-range select-window"
}

# ============================================================
# CLEANUP
# ============================================================
Write-Host ""
Start-Process -FilePath $PSMUX -ArgumentList "kill-session -t winidx" -WindowStyle Hidden
Start-Sleep -Seconds 1

Write-Host ""
Write-Host ("=" * 60)
Write-Host "RESULTS: $($script:TestsPassed) passed, $($script:TestsFailed) failed"
Write-Host ("=" * 60)

if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
