# Issue #77: Mouse events leak as raw text into SSH panes
#
# Root issue: when mouse is disabled (set -g mouse off), mouse SGR sequences
# sent to psmux were forwarded raw into the pane as printable garbage text
# (e.g. "<ESC>[<0;10;5M" appearing in the terminal output).
#
# SSH probe: sshd not available on this machine. PROXY path used.
#
# PROXY: With `set -g mouse off`, send a mouse SGR sequence via send-keys and
# assert it does NOT appear as raw text in capture-pane. This exercises the
# same code path that was broken over SSH.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue77_mouse_leak.ps1

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA SilentlyContinue).Source
if (-not $PSMUX) { Write-Host "[FAIL] psmux not found in PATH" -ForegroundColor Red; exit 1 }

$SESSION  = "gap77"
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

function Send-TcpCommand {
    param([string]$Sess, [string]$Cmd)
    $portFile = "$psmuxDir\$Sess.port"
    $keyFile  = "$psmuxDir\$Sess.key"
    if (-not (Test-Path $portFile) -or -not (Test-Path $keyFile)) { return "NO_PORT_FILE" }
    $port = (Get-Content $portFile -Raw).Trim()
    $key  = (Get-Content $keyFile  -Raw).Trim()
    try {
        $tcp    = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
        $tcp.NoDelay = $true
        $stream = $tcp.GetStream()
        $writer = [System.IO.StreamWriter]::new($stream)
        $reader = [System.IO.StreamReader]::new($stream)
        $writer.Write("AUTH $key`n"); $writer.Flush()
        $authResp = $reader.ReadLine()
        if ($authResp -ne "OK") { $tcp.Close(); return "AUTH_FAILED" }
        $writer.Write("$Cmd`n"); $writer.Flush()
        $stream.ReadTimeout = 6000
        try   { $resp = $reader.ReadLine() } catch { $resp = "TIMEOUT" }
        $tcp.Close()
        return $resp
    } catch {
        return "TCP_ERROR: $_"
    }
}

# ── SSH availability check ───────────────────────────────────────────────────
Write-Host "`n=== Issue #77: mouse event leak (SSH proxy test) ===" -ForegroundColor Cyan
$sshAvail = $false
$sshTest = & ssh -o BatchMode=yes -o StrictHostKeyChecking=no -o ConnectTimeout=3 localhost "echo SSHPROBE_OK" 2>&1
if ($sshTest -match "SSHPROBE_OK") { $sshAvail = $true }
if (-not $sshAvail) {
    Write-Info "SSH server not available. Running PROXY assertions (mouse-off + SGR injection)."
}

# ── Setup ───────────────────────────────────────────────────────────────────
Cleanup
& $PSMUX new-session -d -s $SESSION -x 220 -y 50
if (-not (Wait-PortFile $SESSION)) {
    Write-Fail "Session '$SESSION' port file never appeared"
    exit 1
}
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "Session not alive after startup"; exit 1 }
Start-Sleep -Milliseconds 800

# ── [Test 1] Ensure mouse is off ─────────────────────────────────────────────
Write-Host "`n[Test 1] set -g mouse off takes effect" -ForegroundColor Yellow
& $PSMUX set -g mouse off -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
$mouseVal = (& $PSMUX show-options -g -v mouse -t $SESSION 2>&1).Trim()
Write-Info "mouse option = '$mouseVal'"
if ($mouseVal -match "^off$") {
    Write-Pass "mouse is off"
} else {
    # Some builds may show empty or 0 for off
    if ($mouseVal -eq "" -or $mouseVal -eq "0") {
        Write-Pass "mouse is effectively off (value='$mouseVal')"
    } else {
        Write-Fail "mouse option is '$mouseVal' (expected 'off')"
    }
}

# ── [Test 2] PROXY: establish a clean pane baseline ──────────────────────────
Write-Host "`n[Test 2] PROXY: establish baseline text in pane" -ForegroundColor Yellow
$baseline = "MOUSE_BASELINE_$(Get-Random -Maximum 99999)"
& $PSMUX send-keys -t $SESSION "echo $baseline" Enter
Start-Sleep -Milliseconds 1200
$capBefore = (& $PSMUX capture-pane -t $SESSION -p 2>&1) | Out-String
if ($capBefore -match [regex]::Escape($baseline)) {
    Write-Pass "Baseline text '$baseline' visible in pane"
} else {
    Write-Fail "Baseline text '$baseline' NOT visible. Cannot run leak test reliably."
}

# ── [Test 3] PROXY: send mouse SGR via send-keys, assert no raw leak ─────────
Write-Host "`n[Test 3] PROXY: mouse SGR sequence does not leak as raw text with mouse off" -ForegroundColor Yellow
# A mouse SGR press event: ESC[<0;10;5M  (button 0, col 10, row 5, press)
# When the old bug was present, psmux forwarded this raw into the pane, and
# capture-pane -p would show the literal bytes "<0;10;5M" or similar.
# We inject it via send-keys (as literal escape characters).
$mouseSGR = "`e[<0;10;5M"
& $PSMUX send-keys -t $SESSION $mouseSGR ""
Start-Sleep -Milliseconds 800
$capAfter = (& $PSMUX capture-pane -t $SESSION -p 2>&1) | Out-String

# Check that the raw SGR text did not appear as printable garbage
# Patterns that would indicate a leak:
$leakPatterns = @('<0;10;5M', '<0;10;5', ';10;5M', '\[<0')
$leaked = $false
foreach ($pat in $leakPatterns) {
    if ($capAfter -match [regex]::Escape($pat)) {
        $leaked = $true
        Write-Info "Leak pattern found: '$pat'"
    }
}
if (-not $leaked) {
    Write-Pass "PROXY_PASS: No raw mouse SGR text leaked into pane output (mouse off respected)"
} else {
    Write-Fail "Raw mouse SGR text leaked into pane — issue #77 regression. Sample: $($capAfter.Substring(0,[Math]::Min(400,$capAfter.Length)))"
}

# ── [Test 4] PROXY: TCP show-options confirms mouse remains off after events ──
Write-Host "`n[Test 4] PROXY: mouse option still off after SGR injection (not auto-enabled)" -ForegroundColor Yellow
$mouseAfter = (& $PSMUX show-options -g -v mouse -t $SESSION 2>&1).Trim()
Write-Info "mouse after injection = '$mouseAfter'"
if ($mouseAfter -match "^off$" -or $mouseAfter -eq "" -or $mouseAfter -eq "0") {
    Write-Pass "PROXY_PASS: mouse remains off after SGR injection"
} else {
    Write-Fail "mouse option changed to '$mouseAfter' after SGR injection — unexpected auto-enable"
}

# ── [Test 5] PROXY: pane content integrity (baseline still visible, no garble)
Write-Host "`n[Test 5] PROXY: pane content intact after mouse SGR injection" -ForegroundColor Yellow
if ($capAfter -match [regex]::Escape($baseline)) {
    Write-Pass "PROXY_PASS: Baseline text '$baseline' still visible after SGR injection (no garble)"
} else {
    Write-Fail "Baseline text lost after SGR injection — pane may be garbled"
}

# ── SSH skip notice ──────────────────────────────────────────────────────────
Write-Skip "REAL SSH PATH: mouse-event-leak-over-SSH requires sshd with key auth — not available on this host"

# ── Teardown ─────────────────────────────────────────────────────────────────
Cleanup

Write-Host "`n$('=' * 60)" -ForegroundColor Cyan
Write-Host "RESULTS  Passed: $($script:TestsPassed)  Failed: $($script:TestsFailed)  Skipped: $($script:TestsSkipped)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host ""
exit $script:TestsFailed
