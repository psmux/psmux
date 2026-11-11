# Issue #4: Over SSH `ls` prints nothing and can't attach
#
# Root issue: commands run in a psmux pane over SSH produced no visible output,
# and attaching to the session failed.
#
# SSH probe: sshd not available on this machine (Get-Service sshd returns nothing,
# `ssh localhost` → connection refused). The SSH-specific path is UNVERIFIED.
#
# PROXY: Assert that commands produce visible output in a pane via capture-pane -p.
# The rendering/output path is the same regardless of whether the client connected
# over SSH or a local terminal. If output is visible locally, the pane output
# pipeline works. The SSH-specific part (PTY allocation over SSH transport) cannot
# be verified without a running sshd.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue4_output_over_ssh.ps1

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA SilentlyContinue).Source
if (-not $PSMUX) { Write-Host "[FAIL] psmux not found in PATH" -ForegroundColor Red; exit 1 }

$SESSION  = "gap4"
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
Write-Host "`n=== Issue #4: pane output visible (SSH proxy test) ===" -ForegroundColor Cyan

$sshAvail = $false
$sshTest = & ssh -o BatchMode=yes -o StrictHostKeyChecking=no -o ConnectTimeout=3 localhost "echo SSHPROBE_OK" 2>&1
if ($sshTest -match "SSHPROBE_OK") { $sshAvail = $true }

if ($sshAvail) {
    Write-Info "SSH available — NOTE: real SSH test not yet implemented; running proxy only"
} else {
    Write-Info "SSH server not available (no sshd). Running PROXY assertions only."
}

# ── Setup ───────────────────────────────────────────────────────────────────
Cleanup
& $PSMUX new-session -d -s $SESSION -x 200 -y 50
if (-not (Wait-PortFile $SESSION)) {
    Write-Fail "Session '$SESSION' port file never appeared"
    exit 1
}
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "Session not alive after startup"; exit 1 }

Start-Sleep -Milliseconds 800

# ── [Test 1] PROXY: echo command output appears in capture-pane ──────────────
Write-Host "`n[Test 1] PROXY: echo output is visible in capture-pane -p" -ForegroundColor Yellow
$marker = "ISSUE4_OUTPUT_MARKER_$(Get-Random -Maximum 99999)"
& $PSMUX send-keys -t $SESSION "echo $marker" Enter
Start-Sleep -Milliseconds 1200
$cap1 = (& $PSMUX capture-pane -t $SESSION -p 2>&1) | Out-String
if ($cap1 -match [regex]::Escape($marker)) {
    Write-Pass "PROXY_PASS: echo output '$marker' visible in capture-pane"
} else {
    Write-Fail "echo output '$marker' NOT found in capture-pane. Got: $($cap1.Substring(0,[Math]::Min(300,$cap1.Length)))"
}

# ── [Test 2] PROXY: multi-command output in same pane ───────────────────────
Write-Host "`n[Test 2] PROXY: second command output also visible" -ForegroundColor Yellow
$marker2 = "ISSUE4_SECOND_$(Get-Random -Maximum 99999)"
& $PSMUX send-keys -t $SESSION "echo $marker2" Enter
Start-Sleep -Milliseconds 1200
$cap2 = (& $PSMUX capture-pane -t $SESSION -p 2>&1) | Out-String
if ($cap2 -match [regex]::Escape($marker2)) {
    Write-Pass "PROXY_PASS: second echo output '$marker2' visible in capture-pane"
} else {
    Write-Fail "Second echo output NOT found. Got: $($cap2.Substring(0,[Math]::Min(300,$cap2.Length)))"
}

# ── [Test 3] PROXY: has-session works (attach precondition) ─────────────────
Write-Host "`n[Test 3] PROXY: has-session exits 0 (attach precondition)" -ForegroundColor Yellow
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -eq 0) {
    Write-Pass "PROXY_PASS: has-session exits 0 — session is attachable"
} else {
    Write-Fail "has-session returned $LASTEXITCODE — session not attachable"
}

# ── [Test 4] PROXY: new-session -d then list-sessions shows it ──────────────
Write-Host "`n[Test 4] PROXY: list-sessions includes session name" -ForegroundColor Yellow
$ls = (& $PSMUX list-sessions 2>&1) | Out-String
if ($ls -match [regex]::Escape($SESSION)) {
    Write-Pass "PROXY_PASS: list-sessions shows '$SESSION'"
} else {
    Write-Fail "list-sessions does not show '$SESSION'. Output: $($ls.Substring(0,[Math]::Min(200,$ls.Length)))"
}

# ── SSH skip notice ─────────────────────────────────────────────────────────
Write-Skip "REAL SSH PATH: 'ssh localhost psmux attach -t $SESSION' requires sshd with key auth — not available on this host"

# ── Teardown ─────────────────────────────────────────────────────────────────
Cleanup

Write-Host "`n$('=' * 60)" -ForegroundColor Cyan
Write-Host "RESULTS  Passed: $($script:TestsPassed)  Failed: $($script:TestsFailed)  Skipped: $($script:TestsSkipped)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host ""
exit $script:TestsFailed
