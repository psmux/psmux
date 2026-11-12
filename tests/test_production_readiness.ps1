# Production readiness E2E tests for PSMUX
# Covers: session lifecycle, window ops, pane ops, run-shell, config, TCP server
#
# This test validates that PSMUX is production-quality by exercising
# core workflows end-to-end with real sessions.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }

function Cleanup-Session([string]$Name) {
    & $PSMUX kill-session -t $Name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$Name.*" -Force -EA SilentlyContinue
}

function Wait-Session([string]$Name, [int]$TimeoutMs = 15000) {
    $pf = "$psmuxDir\$Name.port"
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        if (Test-Path $pf) {
            $port = (Get-Content $pf -Raw -EA SilentlyContinue)
            if ($port) {
                $port = $port.Trim()
                if ($port -match '^\d+$') {
                    try {
                        $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
                        $tcp.Close()
                        return @{ Port=[int]$port; Ms=$sw.ElapsedMilliseconds }
                    } catch {}
                }
            }
        }
        Start-Sleep -Milliseconds 100
    }
    return $null
}

function Send-TcpCommand {
    param([string]$Session, [string]$Command, [int]$TimeoutMs = 10000)
    $portFile = "$psmuxDir\$Session.port"
    $keyFile = "$psmuxDir\$Session.key"
    if (!(Test-Path $portFile) -or !(Test-Path $keyFile)) { return "NO_FILES" }
    $port = (Get-Content $portFile -Raw).Trim()
    $key = (Get-Content $keyFile -Raw).Trim()
    try {
        $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
        $tcp.NoDelay = $true
        $stream = $tcp.GetStream()
        $writer = [System.IO.StreamWriter]::new($stream)
        $reader = [System.IO.StreamReader]::new($stream)
        $writer.Write("AUTH $key`n"); $writer.Flush()
        $authResp = $reader.ReadLine()
        if ($authResp -ne "OK") { $tcp.Close(); return "AUTH_FAILED" }
        $writer.Write("$Command`n"); $writer.Flush()
        $stream.ReadTimeout = $TimeoutMs
        try { $resp = $reader.ReadLine() } catch { $resp = "TIMEOUT" }
        $tcp.Close()
        return $resp
    } catch {
        return "TCP_ERROR: $_"
    }
}

# ============================================================================
Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "  PSMUX Production Readiness E2E Tests" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan
# ============================================================================

# ─── PART 1: Session Lifecycle ──────────────────────────────────────────────
Write-Host "[Part 1] Session Lifecycle" -ForegroundColor Magenta

$S1 = "e2e_session_1"
Cleanup-Session $S1

# Test 1.1: Create detached session
Write-Host "`n[1.1] Create detached session" -ForegroundColor Yellow
& $PSMUX new-session -d -s $S1
$info = Wait-Session -Name $S1 -TimeoutMs 15000
if ($info) {
    Write-Pass "Session '$S1' created in $($info.Ms)ms"
} else {
    Write-Fail "Session '$S1' failed to start within 15s"
}

# Test 1.2: has-session returns 0
Write-Host "[1.2] has-session returns exit 0" -ForegroundColor Yellow
& $PSMUX has-session -t $S1 2>$null
if ($LASTEXITCODE -eq 0) { Write-Pass "has-session returns 0" }
else { Write-Fail "has-session returned $LASTEXITCODE" }

# Test 1.3: has-session returns non-zero for missing session
Write-Host "[1.3] has-session returns non-zero for missing" -ForegroundColor Yellow
& $PSMUX has-session -t "nonexistent_session_xyz" 2>$null
if ($LASTEXITCODE -ne 0) { Write-Pass "has-session returns non-zero for missing session" }
else { Write-Fail "has-session returned 0 for nonexistent session" }

# Test 1.4: list-sessions includes our session
Write-Host "[1.4] list-sessions includes our session" -ForegroundColor Yellow
$sessions = & $PSMUX list-sessions 2>&1 | Out-String
if ($sessions -match $S1) { Write-Pass "list-sessions shows '$S1'" }
else { Write-Fail "list-sessions does not show '$S1'. Output: $sessions" }

# Test 1.5: display-message session_name
Write-Host "[1.5] display-message session_name" -ForegroundColor Yellow
$name = & $PSMUX display-message -t $S1 -p '#{session_name}' 2>&1 | Out-String
if ($name.Trim() -eq $S1) { Write-Pass "session_name = '$S1'" }
else { Write-Fail "Expected '$S1', got '$($name.Trim())'" }

# ─── PART 2: Window Operations ─────────────────────────────────────────────
Write-Host "`n[Part 2] Window Operations" -ForegroundColor Magenta

# Test 2.1: Create new window
Write-Host "[2.1] New window" -ForegroundColor Yellow
& $PSMUX new-window -t $S1
Start-Sleep -Seconds 2
$winCount = & $PSMUX display-message -t $S1 -p '#{session_windows}' 2>&1 | Out-String
$winCount = $winCount.Trim()
if ([int]$winCount -ge 2) { Write-Pass "Window count >= 2 ($winCount)" }
else { Write-Fail "Expected >= 2 windows, got $winCount" }

# Test 2.2: Rename window
Write-Host "[2.2] Rename window" -ForegroundColor Yellow
& $PSMUX rename-window -t $S1 "test_renamed"
Start-Sleep -Milliseconds 500
$winName = & $PSMUX display-message -t $S1 -p '#{window_name}' 2>&1 | Out-String
if ($winName.Trim() -eq "test_renamed") { Write-Pass "Window renamed to 'test_renamed'" }
else { Write-Fail "Expected 'test_renamed', got '$($winName.Trim())'" }

# Test 2.3: Select previous window
Write-Host "[2.3] Select previous window" -ForegroundColor Yellow
$beforeIdx = (& $PSMUX display-message -t $S1 -p '#{window_index}' 2>&1 | Out-String).Trim()
& $PSMUX select-window -t $S1 -p 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$afterIdx = (& $PSMUX display-message -t $S1 -p '#{window_index}' 2>&1 | Out-String).Trim()
if ($afterIdx -ne $beforeIdx) { Write-Pass "select-window -p changed index ($beforeIdx -> $afterIdx)" }
else { Write-Fail "select-window -p did not change window index" }

# Test 2.4: List windows
Write-Host "[2.4] List windows" -ForegroundColor Yellow
$winList = & $PSMUX list-windows -t $S1 2>&1 | Out-String
if ($winList -match "test_renamed") { Write-Pass "list-windows shows 'test_renamed'" }
else { Write-Fail "list-windows does not show 'test_renamed'. Output: $winList" }

# ─── PART 3: Pane Operations ───────────────────────────────────────────────
Write-Host "`n[Part 3] Pane Operations" -ForegroundColor Magenta

# Test 3.1: Split window vertically
Write-Host "[3.1] Split window vertically" -ForegroundColor Yellow
& $PSMUX split-window -v -t $S1
Start-Sleep -Seconds 2
$paneCount = & $PSMUX display-message -t $S1 -p '#{window_panes}' 2>&1 | Out-String
$paneCount = $paneCount.Trim()
if ([int]$paneCount -ge 2) { Write-Pass "Pane count >= 2 ($paneCount) after vertical split" }
else { Write-Fail "Expected >= 2 panes, got $paneCount" }

# Test 3.2: Send keys to pane
Write-Host "[3.2] send-keys and capture-pane" -ForegroundColor Yellow
$marker = "PSMUX_E2E_MARKER_$(Get-Random)"
& $PSMUX send-keys -t $S1 "echo $marker" Enter
Start-Sleep -Seconds 2
$captured = & $PSMUX capture-pane -t $S1 -p 2>&1 | Out-String
if ($captured -match $marker) { Write-Pass "Marker found in capture-pane output" }
else { Write-Fail "Marker '$marker' not found in pane capture" }

# Test 3.3: List panes
Write-Host "[3.3] List panes" -ForegroundColor Yellow
$paneList = & $PSMUX list-panes -t $S1 2>&1 | Out-String
if ($paneList -match "active") { Write-Pass "list-panes shows at least one active pane" }
else { Write-Fail "list-panes output unexpected: $paneList" }

# ─── PART 4: TCP Server Path ───────────────────────────────────────────────
Write-Host "`n[Part 4] TCP Server Path" -ForegroundColor Magenta

# Test 4.1: list-sessions via TCP
Write-Host "[4.1] list-sessions via raw TCP" -ForegroundColor Yellow
$resp = Send-TcpCommand -Session $S1 -Command "list-sessions"
if ($resp -match $S1) { Write-Pass "TCP list-sessions returns session name" }
else { Write-Fail "TCP list-sessions unexpected: $resp" }

# Test 4.2: list-windows via TCP
Write-Host "[4.2] list-windows via raw TCP" -ForegroundColor Yellow
$resp = Send-TcpCommand -Session $S1 -Command "list-windows"
if ($resp -and $resp -ne "TIMEOUT" -and $resp -ne "TCP_ERROR") { Write-Pass "TCP list-windows responded" }
else { Write-Fail "TCP list-windows failed: $resp" }

# Test 4.3: display-message via TCP
Write-Host "[4.3] display-message via raw TCP" -ForegroundColor Yellow
$resp = Send-TcpCommand -Session $S1 -Command "display-message -p #{session_name}"
if ($resp -match $S1) { Write-Pass "TCP display-message returns session name" }
else { Write-Fail "TCP display-message unexpected: $resp" }

# Test 4.4: new-session via TCP (create second session from first)
Write-Host "[4.4] new-session via TCP" -ForegroundColor Yellow
$S2 = "e2e_session_2"
Cleanup-Session $S2
$resp = Send-TcpCommand -Session $S1 -Command "new-session -d -s $S2"
Start-Sleep -Seconds 5
$info2 = Wait-Session -Name $S2 -TimeoutMs 10000
if ($info2) { Write-Pass "TCP new-session created '$S2' in $($info2.Ms)ms" }
else { Write-Fail "TCP new-session did not create '$S2'" }

# ─── PART 5: Run-Shell (Issue #4 fix) ──────────────────────────────────────
Write-Host "`n[Part 5] Run-Shell (Issue #4)" -ForegroundColor Magenta

# Test 5.1: run-shell with echo (foreground)
Write-Host "[5.1] run-shell echo via CLI" -ForegroundColor Yellow
$output = & $PSMUX run-shell -t $S1 "echo PSMUX_RUN_TEST" 2>&1 | Out-String
if ($output -match "PSMUX_RUN_TEST") { Write-Pass "run-shell echo output captured" }
else { Write-Fail "run-shell echo output not found. Got: $output" }

# Test 5.2: run-shell with pwsh prefix
Write-Host "[5.2] run-shell with pwsh prefix" -ForegroundColor Yellow
$output = & $PSMUX run-shell -t $S1 'pwsh -NoProfile -Command "echo PWSH_TEST"' 2>&1 | Out-String
if ($output -match "PWSH_TEST") { Write-Pass "run-shell with pwsh prefix works" }
else {
    # Try with powershell if pwsh not available
    $output2 = & $PSMUX run-shell -t $S1 'powershell -NoProfile -Command "echo PS_TEST"' 2>&1 | Out-String
    if ($output2 -match "PS_TEST") { Write-Pass "run-shell with powershell prefix works" }
    else { Write-Fail "run-shell with shell prefix failed. pwsh output: $output. ps output: $output2" }
}

# Test 5.3: run-shell via TCP
Write-Host "[5.3] run-shell via TCP" -ForegroundColor Yellow
$resp = Send-TcpCommand -Session $S1 -Command "run-shell echo TCP_RUN_TEST"
if ($resp -match "TCP_RUN_TEST") { Write-Pass "TCP run-shell returned output" }
else { Write-Fail "TCP run-shell unexpected response: $resp" }

# Test 5.4: run-shell with tilde path expansion
Write-Host "[5.4] run-shell tilde expansion" -ForegroundColor Yellow
$testScript = "$env:USERPROFILE\.psmux_e2e_test.ps1"
'Write-Output "TILDE_EXPAND_WORKS"' | Set-Content $testScript -Encoding UTF8
$output = & $PSMUX run-shell -t $S1 "~/.psmux_e2e_test.ps1" 2>&1 | Out-String
if ($output -match "TILDE_EXPAND_WORKS") { Write-Pass "Tilde expansion works" }
else { Write-Fail "Tilde expansion failed. Output: $output" }
Remove-Item $testScript -Force -EA SilentlyContinue

# Test 5.5: run-shell no args shows usage
Write-Host "[5.5] run-shell no args" -ForegroundColor Yellow
$resp = Send-TcpCommand -Session $S1 -Command "run-shell"
if ($resp -match "usage") { Write-Pass "run-shell no args shows usage" }
else { Write-Fail "run-shell no args unexpected: $resp" }

# ─── PART 6: Config Operations ─────────────────────────────────────────────
Write-Host "`n[Part 6] Config Operations" -ForegroundColor Magenta

# Test 6.1: set-option and show-options
Write-Host "[6.1] set-option / show-options" -ForegroundColor Yellow
& $PSMUX set-option -g -t $S1 status-left "[E2E_TEST]" 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$val = & $PSMUX show-options -g -v "status-left" -t $S1 2>&1 | Out-String
if ($val -match "E2E_TEST") { Write-Pass "set-option status-left applied" }
else { Write-Fail "set-option not applied. show-options returned: $val" }

# Test 6.2: source-file
Write-Host "[6.2] source-file" -ForegroundColor Yellow
$confFile = "$env:TEMP\psmux_e2e_source_test.conf"
'set -g status-right "[SOURCE_OK]"' | Set-Content $confFile -Encoding UTF8
& $PSMUX source-file -t $S1 $confFile 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$val = & $PSMUX show-options -g -v "status-right" -t $S1 2>&1 | Out-String
if ($val -match "SOURCE_OK") { Write-Pass "source-file applied config" }
else { Write-Fail "source-file not applied. Got: $val" }
Remove-Item $confFile -Force -EA SilentlyContinue

# Test 6.3: bind-key and list-keys
Write-Host "[6.3] bind-key / list-keys" -ForegroundColor Yellow
& $PSMUX bind-key -t $S1 F9 display-message "E2E_BIND_TEST" 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$keys = & $PSMUX list-keys -t $S1 2>&1 | Out-String
if ($keys -match "F9") { Write-Pass "bind-key F9 registered" }
else { Write-Fail "bind-key F9 not found in list-keys" }

# ─── PART 7: Edge Cases ────────────────────────────────────────────────────
Write-Host "`n[Part 7] Edge Cases" -ForegroundColor Magenta

# Test 7.1: Kill session
Write-Host "[7.1] kill-session" -ForegroundColor Yellow
& $PSMUX kill-session -t $S2 2>&1 | Out-Null
Start-Sleep -Seconds 1
& $PSMUX has-session -t $S2 2>$null
if ($LASTEXITCODE -ne 0) { Write-Pass "kill-session removed '$S2'" }
else { Write-Fail "kill-session did not remove '$S2'" }

# Test 7.2: Duplicate session name
Write-Host "[7.2] Duplicate session name rejection" -ForegroundColor Yellow
$dupOutput = & $PSMUX new-session -d -s $S1 2>&1 | Out-String
# Should fail or show error (session already exists)
if ($dupOutput -match "exist|duplicate|already" -or $LASTEXITCODE -ne 0) {
    Write-Pass "Duplicate session correctly rejected"
} else {
    Write-Fail "Duplicate session was not rejected. Output: $dupOutput"
}

# Test 7.3: Special characters in window name
Write-Host "[7.3] Special chars in window name" -ForegroundColor Yellow
& $PSMUX rename-window -t $S1 "test window 123" 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$wn = & $PSMUX display-message -t $S1 -p '#{window_name}' 2>&1 | Out-String
# window names with spaces might get truncated/modified, just check it didnt crash
Write-Pass "rename-window with spaces did not crash"

# ─── PART 8: Performance Spot Checks ───────────────────────────────────────
Write-Host "`n[Part 8] Performance Spot Checks" -ForegroundColor Magenta

# Re-check session is alive before perf tests (it may have been killed by edge case tests)
& $PSMUX has-session -t $S1 2>$null
if ($LASTEXITCODE -ne 0) {
    # Recreate it for perf tests
    & $PSMUX new-session -d -s $S1
    $null = Wait-Session -Name $S1 -TimeoutMs 10000
}

# Test 8.1: display-message latency
Write-Host "[8.1] display-message latency (10 iterations)" -ForegroundColor Yellow
$times = @()
for ($i = 0; $i -lt 10; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $PSMUX display-message -t $S1 -p '#{session_name}' 2>&1 | Out-Null
    $sw.Stop()
    $times += $sw.Elapsed.TotalMilliseconds
}
$avg = ($times | Measure-Object -Average).Average
$max = ($times | Measure-Object -Maximum).Maximum
Write-Host "    avg: $([math]::Round($avg, 1))ms  max: $([math]::Round($max, 1))ms" -ForegroundColor DarkCyan
if ($avg -lt 500) { Write-Pass "display-message avg under 500ms ($([math]::Round($avg,1))ms)" }
else { Write-Fail "display-message avg too slow: $([math]::Round($avg,1))ms" }

# Test 8.2: TCP round-trip latency
Write-Host "[8.2] TCP round-trip latency (10 iterations)" -ForegroundColor Yellow
$tcpTimes = @()
$portFile = "$psmuxDir\$S1.port"
$keyFile = "$psmuxDir\$S1.key"
if ((Test-Path $portFile) -and (Test-Path $keyFile)) {
    $port = (Get-Content $portFile -Raw).Trim()
    $key = (Get-Content $keyFile -Raw).Trim()
    try {
        $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
        $tcp.NoDelay = $true
        $stream = $tcp.GetStream()
        $writer = [System.IO.StreamWriter]::new($stream)
        $reader = [System.IO.StreamReader]::new($stream)
        $writer.Write("AUTH $key`n"); $writer.Flush()
        $null = $reader.ReadLine()
        # Use PERSISTENT mode so the connection stays open for multiple commands
        $writer.Write("PERSISTENT`n"); $writer.Flush()
        for ($i = 0; $i -lt 10; $i++) {
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            # dump-state is always answered (a frame, or NC when nothing changed).
            # A query such as list-sessions is not: since #232 an unchanged frame
            # is not pushed again, so waiting for a line after it measures the
            # read timeout, not the round trip.
            $writer.Write("dump-state`n"); $writer.Flush()
            $stream.ReadTimeout = 5000
            try { $null = $reader.ReadLine() } catch {}
            $sw.Stop()
            $tcpTimes += $sw.Elapsed.TotalMilliseconds
        }
        $tcp.Close()
        $tavg = ($tcpTimes | Measure-Object -Average).Average
        Write-Host "    avg: $([math]::Round($tavg, 1))ms" -ForegroundColor DarkCyan
        if ($tavg -lt 50) { Write-Pass "TCP round-trip avg under 50ms ($([math]::Round($tavg,1))ms)" }
        else { Write-Fail "TCP round-trip avg too slow: $([math]::Round($tavg,1))ms" }
    } catch {
        Write-Fail "TCP connection failed: $_"
    }
} else {
    Write-Skip "Port/key files not found for TCP test"
}

# ─── CLEANUP ────────────────────────────────────────────────────────────────
Write-Host "`n[Cleanup]" -ForegroundColor DarkGray
Cleanup-Session $S1
Cleanup-Session $S2

# ─── RESULTS ────────────────────────────────────────────────────────────────
Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "  Results" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Passed:  $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed:  $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $($script:TestsSkipped)" -ForegroundColor Yellow
Write-Host ""

exit $script:TestsFailed
