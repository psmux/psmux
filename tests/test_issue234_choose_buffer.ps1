# Issue #234: choose-buffer interactive chooser
# Tests that choose-buffer works as an interactive chooser (not static popup)
# and that buffers can be selected, pasted, and deleted

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "test_i234"
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

function Send-TcpCommand {
    param([string]$Session, [string]$Command)
    $port = (Get-Content "$psmuxDir\$Session.port" -Raw).Trim()
    $key = (Get-Content "$psmuxDir\$Session.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush()
    $authResp = $reader.ReadLine()
    if ($authResp -ne "OK") { $tcp.Close(); return "AUTH_FAILED" }
    $writer.Write("$Command`n"); $writer.Flush()
    $stream.ReadTimeout = 10000
    try { $resp = $reader.ReadLine() } catch { $resp = "TIMEOUT" }
    $tcp.Close()
    return $resp
}

# === SETUP ===
Cleanup
& $PSMUX new-session -d -s $SESSION
Start-Sleep -Seconds 3

# Verify session exists
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "Session creation failed"
    exit 1
}

Write-Host "`n=== Issue #234 Tests: choose-buffer ===" -ForegroundColor Cyan

# === Part A: CLI path tests ===
Write-Host "`n--- Part A: CLI Path ---" -ForegroundColor Yellow

# Test 1: set-buffer adds buffers
Write-Host "[Test 1] set-buffer adds paste buffers" -ForegroundColor Yellow
& $PSMUX set-buffer -t $SESSION "First buffer content"
& $PSMUX set-buffer -t $SESSION "Second buffer content"
& $PSMUX set-buffer -t $SESSION "Third buffer for testing"
Start-Sleep -Milliseconds 500
$buffers = & $PSMUX list-buffers -t $SESSION 2>&1
$bufCount = ($buffers | Where-Object { $_ -match '^buffer\d+:' }).Count
if ($bufCount -eq 3) {
    Write-Pass "Three buffers created"
} else {
    Write-Fail "Expected 3 buffers, got $bufCount : $($buffers -join ' | ')"
}

# Test 2: choose-buffer CLI returns buffer list (non-interactive, just text)
Write-Host "[Test 2] choose-buffer CLI returns buffer list" -ForegroundColor Yellow
$output = & $PSMUX choose-buffer -t $SESSION 2>&1 | Out-String
if ($output -match "buffer0:" -and $output -match "bytes:") {
    Write-Pass "choose-buffer CLI returns formatted buffer list"
} else {
    Write-Fail "choose-buffer CLI output unexpected: $output"
}

# Test 3: list-buffers shows all buffers
Write-Host "[Test 3] list-buffers shows all buffers" -ForegroundColor Yellow
$list = & $PSMUX list-buffers -t $SESSION 2>&1 | Out-String
if ($list -match "buffer0:" -and $list -match "buffer1:" -and $list -match "buffer2:") {
    Write-Pass "list-buffers shows all 3 buffers"
} else {
    Write-Fail "list-buffers missing entries: $list"
}

# === Part B: TCP Server Path ===
Write-Host "`n--- Part B: TCP Server Path ---" -ForegroundColor Yellow

# Test 4: choose-buffer via TCP returns buffer list
Write-Host "[Test 4] choose-buffer via TCP returns buffer data" -ForegroundColor Yellow
$resp = Send-TcpCommand -Session $SESSION -Command "choose-buffer"
if ($resp -match "buffer0:" -and $resp -match "bytes:") {
    Write-Pass "TCP choose-buffer returns buffer data"
} else {
    Write-Fail "TCP choose-buffer unexpected: $resp"
}

# Test 5: delete-buffer-at via TCP removes specific buffer
Write-Host "[Test 5] delete-buffer-at removes specific buffer" -ForegroundColor Yellow
$beforeBufs = (& $PSMUX list-buffers -t $SESSION 2>&1 | Where-Object { $_ -match '^buffer\d+:' }).Count
$null = Send-TcpCommand -Session $SESSION -Command "delete-buffer-at 0"
Start-Sleep -Milliseconds 1000
$afterBufs = (& $PSMUX list-buffers -t $SESSION 2>&1 | Where-Object { $_ -match '^buffer\d+:' }).Count
if ($afterBufs -eq ($beforeBufs - 1)) {
    Write-Pass "delete-buffer-at removed one buffer ($beforeBufs -> $afterBufs)"
} else {
    Write-Fail "Expected $($beforeBufs - 1) buffers, got $afterBufs"
}

# Test 6: delete-buffer -b removes the buffer of a name list-buffers shows.
# -b is a name, as in tmux (issue #818): there is no index form.
Write-Host "[Test 6] delete-buffer -b <name> removes that buffer" -ForegroundColor Yellow
# Add more buffers first
& $PSMUX set-buffer -t $SESSION "Buffer A"
& $PSMUX set-buffer -t $SESSION "Buffer B"
& $PSMUX set-buffer -t $SESSION "Buffer C"
Start-Sleep -Milliseconds 500
$lines = @(& $PSMUX list-buffers -t $SESSION 2>&1 | Where-Object { $_ -match '^buffer\d+:' })
$before = $lines.Count
$victim = ($lines | Where-Object { $_ -match '"Buffer B"' } | Select-Object -First 1) -replace ':.*$', ''
$null = Send-TcpCommand -Session $SESSION -Command "delete-buffer -b $victim"
Start-Sleep -Milliseconds 1000
$afterLines = @(& $PSMUX list-buffers -t $SESSION 2>&1 | Where-Object { $_ -match '^buffer\d+:' })
if ($afterLines.Count -eq ($before - 1) -and -not ($afterLines -match '"Buffer B"')) {
    Write-Pass "delete-buffer -b $victim removed Buffer B"
} else {
    Write-Fail "Expected Buffer B ($victim) gone, got: $($afterLines -join ' / ')"
}

# === Part C: Edge Cases ===
Write-Host "`n--- Part C: Edge Cases ---" -ForegroundColor Yellow

# Test 7: choose-buffer with no buffers returns empty
Write-Host "[Test 7] choose-buffer with empty buffer list" -ForegroundColor Yellow
# Delete all buffers
$count = (& $PSMUX list-buffers -t $SESSION 2>&1).Count
for ($i = 0; $i -lt $count + 5; $i++) {
    $null = Send-TcpCommand -Session $SESSION -Command "delete-buffer"
}
Start-Sleep -Milliseconds 500
$resp = Send-TcpCommand -Session $SESSION -Command "choose-buffer"
# Empty response is expected (no buffers)
if ($null -eq $resp -or $resp -eq "" -or $resp -eq "TIMEOUT") {
    Write-Pass "choose-buffer with no buffers returns empty/timeout (correct)"
} elseif (-not ($resp -match "buffer0:")) {
    Write-Pass "choose-buffer with no buffers has no buffer entries"
} else {
    Write-Fail "Expected empty response, got: $resp"
}

# Test 8: delete-buffer-at with invalid index does not crash
Write-Host "[Test 8] delete-buffer-at with invalid index" -ForegroundColor Yellow
$null = Send-TcpCommand -Session $SESSION -Command "delete-buffer-at 999"
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -eq 0) {
    Write-Pass "Session still alive after invalid delete-buffer-at index"
} else {
    Write-Fail "Session died after invalid delete-buffer-at"
}

# Test 9: paste-buffer-at with buffers works
Write-Host "[Test 9] paste-buffer-at pastes specific buffer content" -ForegroundColor Yellow
& $PSMUX set-buffer -t $SESSION "PASTE_MARKER_234"
Start-Sleep -Milliseconds 500
$null = Send-TcpCommand -Session $SESSION -Command "paste-buffer-at 0"
Start-Sleep -Seconds 2
$captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($captured -match "PASTE_MARKER_234") {
    Write-Pass "paste-buffer-at 0 pasted buffer content into pane"
} else {
    Write-Fail "paste-buffer-at did not paste content. Captured: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
}

# === Part D: Win32 TUI Visual Verification ===
Write-Host "`n--- Part D: TUI Visual Verification ---" -ForegroundColor Yellow

$SESSION_TUI = "i234_tui"
& $PSMUX kill-session -t $SESSION_TUI 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$SESSION_TUI.*" -Force -EA SilentlyContinue

$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION_TUI -PassThru
Start-Sleep -Seconds 4

# Verify TUI session is alive
& $PSMUX has-session -t $SESSION_TUI 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "TUI session creation failed"
} else {
    # Test 10: Add buffers to TUI session and verify list-buffers
    Write-Host "[Test 10] TUI: Adding buffers and listing" -ForegroundColor Yellow
    & $PSMUX set-buffer -t $SESSION_TUI "TUI Buffer One"
    & $PSMUX set-buffer -t $SESSION_TUI "TUI Buffer Two"
    Start-Sleep -Milliseconds 500
    $tui_list = & $PSMUX list-buffers -t $SESSION_TUI 2>&1 | Out-String
    if ($tui_list -match "buffer0:" -and $tui_list -match "buffer1:") {
        Write-Pass "TUI: Two buffers visible in list-buffers"
    } else {
        Write-Fail "TUI: Expected 2 buffers, got: $tui_list"
    }

    # Test 11: delete-buffer-at via TUI session
    Write-Host "[Test 11] TUI: delete-buffer-at removes buffer" -ForegroundColor Yellow
    $null = Send-TcpCommand -Session $SESSION_TUI -Command "delete-buffer-at 0"
    Start-Sleep -Milliseconds 1000
    $afterList = & $PSMUX list-buffers -t $SESSION_TUI 2>&1 | Out-String
    $afterBufCount = (& $PSMUX list-buffers -t $SESSION_TUI 2>&1 | Where-Object { $_ -match '^buffer\d+:' }).Count
    if ($afterBufCount -eq 1) {
        Write-Pass "TUI: Buffer deleted, 1 remaining"
    } else {
        Write-Fail "TUI: Expected 1 buffer after delete, got $afterBufCount"
    }

    # Test 12: paste-buffer-at in TUI session
    Write-Host "[Test 12] TUI: paste-buffer-at pastes into pane" -ForegroundColor Yellow
    & $PSMUX send-keys -t $SESSION_TUI "clear" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    $null = Send-TcpCommand -Session $SESSION_TUI -Command "paste-buffer-at 0"
    Start-Sleep -Seconds 1
    $captured = & $PSMUX capture-pane -t $SESSION_TUI -p 2>&1 | Out-String
    if ($captured -match "TUI Buffer") {
        Write-Pass "TUI: paste-buffer-at pasted content into pane"
    } else {
        Write-Fail "TUI: paste-buffer-at content not found. Got: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }

    # Cleanup TUI
    & $PSMUX kill-session -t $SESSION_TUI 2>&1 | Out-Null
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
}

# === TEARDOWN ===
Cleanup

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
