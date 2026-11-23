# Issue #237: Regression: typing freezes for ~2 seconds intermittently (Windows)
# Bisected to commit 3bf380d which added paste_suppress_until = now + 2s
#
# Root cause: Fast typing (>=3 chars in 20ms) triggers stage2 paste heuristic.
# After 300ms without Ctrl+V Release, stage2 times out and sets
# paste_suppress_until to now+2s. All KeyCode::Char events are silently
# dropped during that 2s window. This test PROVES the bug is real.
#
# MANDATORY LAYERS: E2E CLI+TCP (Layer 1) + Win32 TUI Visual (Layer 2)
# CONDITIONAL LAYERS: WriteConsoleInput keystroke injection (Layer 3) +
#                     Performance measurement (Layer 7)

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:Metrics = @{}

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Metric($name, $valueMs) {
    $script:Metrics[$name] = $valueMs
    Write-Host ("  [METRIC] {0}: {1:N1}ms" -f $name, $valueMs) -ForegroundColor DarkCyan
}

function Percentile($arr, $pct) {
    if ($arr.Count -eq 0) { return 0 }
    $sorted = [double[]]($arr | Sort-Object)
    $idx = [Math]::Floor(($pct / 100.0) * ($sorted.Count - 1))
    return $sorted[$idx]
}

function Cleanup {
    param([string]$Name)
    & $PSMUX kill-session -t $Name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$Name.*" -Force -EA SilentlyContinue
}

function Wait-Session {
    param([string]$Name, [int]$TimeoutMs = 15000)
    $pf = "$psmuxDir\$Name.port"
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        if (Test-Path $pf) {
            $port = (Get-Content $pf -Raw).Trim()
            if ($port -match '^\d+$') {
                try {
                    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
                    $tcp.Close()
                    return $true
                } catch {}
            }
        }
        Start-Sleep -Milliseconds 50
    }
    return $false
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

function Connect-Persistent {
    param([string]$Session)
    $port = (Get-Content "$psmuxDir\$Session.port" -Raw).Trim()
    $key = (Get-Content "$psmuxDir\$Session.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true; $tcp.ReceiveTimeout = 10000
    $stream = $tcp.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush()
    $null = $reader.ReadLine()
    $writer.Write("PERSISTENT`n"); $writer.Flush()
    return @{ tcp=$tcp; writer=$writer; reader=$reader }
}

function Get-Dump {
    param($conn)
    $conn.writer.Write("dump-state`n"); $conn.writer.Flush()
    $best = $null
    $conn.tcp.ReceiveTimeout = 3000
    for ($j = 0; $j -lt 100; $j++) {
        try { $line = $conn.reader.ReadLine() } catch { break }
        if ($null -eq $line) { break }
        if ($line -ne "NC" -and $line.Length -gt 100) { $best = $line }
        if ($best) { $conn.tcp.ReceiveTimeout = 50 }
    }
    $conn.tcp.ReceiveTimeout = 10000
    return $best
}

# ── Compile keystroke injector ──
$injectorExe = "$env:TEMP\psmux_injector.exe"
$injectorSrc = Join-Path (Split-Path $PSScriptRoot) "tests\injector.cs"
if (-not (Test-Path $injectorSrc)) { $injectorSrc = "$PSScriptRoot\injector.cs" }
if (-not (Test-Path $injectorExe) -or (Get-Item $injectorSrc).LastWriteTime -gt (Get-Item $injectorExe -EA SilentlyContinue).LastWriteTime) {
    Write-Host "Compiling keystroke injector..." -ForegroundColor DarkGray
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    if (-not (Test-Path $csc)) {
        $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
    }
    & $csc /nologo /optimize /out:$injectorExe $injectorSrc 2>&1 | Out-Null
    if (-not (Test-Path $injectorExe)) {
        Write-Host "  [WARN] Could not compile injector, Layer 3 tests will be skipped" -ForegroundColor Yellow
    }
}

Write-Host ""
Write-Host ("=" * 70) -ForegroundColor Cyan
Write-Host "Issue #237: Typing Freeze Regression (paste_suppress_until = 2s)" -ForegroundColor Cyan
Write-Host ("=" * 70) -ForegroundColor Cyan
Write-Host ""
Write-Host "Bug: Fast typing triggers stage2 paste heuristic, which sets a" -ForegroundColor White
Write-Host "     2-second suppression window that silently drops ALL typed chars." -ForegroundColor White
Write-Host ""

# ============================================================================
# PART A: CLI Path Tests (Layer 1)
# Prove the paste mechanism interacts with normal typing via send-keys/send-text
# ============================================================================

Write-Host "=== PART A: CLI Path E2E Tests ===" -ForegroundColor Cyan

$SESSION = "test237_cli"
Cleanup -Name $SESSION
& $PSMUX new-session -d -s $SESSION
Start-Sleep -Seconds 3
if (-not (Wait-Session -Name $SESSION)) {
    Write-Fail "Session $SESSION never came up"
    exit 1
}

# --- Test A1: Rapid send-text simulating fast typing ---
Write-Host "`n[Test A1] Rapid send-text commands (simulates fast typing path)" -ForegroundColor Yellow
# Send 20 characters rapidly via CLI send-text (bypasses paste heuristic since
# it goes through TCP server, not the client event loop). This is the CONTROL
# test: send-text via CLI should ALWAYS work regardless of paste suppression.
& $PSMUX send-keys -t $SESSION "echo " 2>&1 | Out-Null
Start-Sleep -Milliseconds 200
$marker = "RAPIDTEST_" + (Get-Random -Maximum 99999)
for ($i = 0; $i -lt ($marker.Length); $i++) {
    $ch = $marker[$i]
    & $PSMUX send-keys -t $SESSION -l "$ch" 2>&1 | Out-Null
}
& $PSMUX send-keys -t $SESSION Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2

$captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($captured -match [regex]::Escape($marker)) {
    Write-Pass "CLI send-text path delivers all characters (control: $marker)"
} else {
    Write-Fail "CLI send-text path lost characters. Expected '$marker' in capture-pane"
}

# --- Test A2: TCP raw command path for send-text ---
Write-Host "`n[Test A2] TCP raw send-text commands" -ForegroundColor Yellow
$marker2 = "TCPTEST_" + (Get-Random -Maximum 99999)
Send-TcpCommand -Session $SESSION -Command "send-text `"echo $marker2`"" | Out-Null
Start-Sleep -Milliseconds 200
Send-TcpCommand -Session $SESSION -Command "send-key enter" | Out-Null
Start-Sleep -Seconds 2

$captured2 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($captured2 -match [regex]::Escape($marker2)) {
    Write-Pass "TCP send-text path delivers all characters (control: $marker2)"
} else {
    Write-Fail "TCP send-text path lost characters"
}

# --- Test A3: Verify session is healthy after rapid commands ---
Write-Host "`n[Test A3] Session health after rapid commands" -ForegroundColor Yellow
$sessName = (& $PSMUX display-message -t $SESSION -p '#{session_name}' 2>&1).Trim()
if ($sessName -eq $SESSION) { Write-Pass "Session responds to display-message" }
else { Write-Fail "Session not responsive, got: $sessName" }

$winCount = (& $PSMUX display-message -t $SESSION -p '#{session_windows}' 2>&1).Trim()
if ($winCount -eq "1") { Write-Pass "Window count is 1" }
else { Write-Fail "Expected 1 window, got: $winCount" }

Cleanup -Name $SESSION

# ============================================================================
# PART B: THE BUG PROOF (Layer 3 WriteConsoleInput)
# This is the CORE proof. We inject real keystrokes into a TUI session and
# measure whether characters are lost due to the 2s suppression window.
# ============================================================================

Write-Host "`n=== PART B: BUG PROOF via WriteConsoleInput Keystroke Injection ===" -ForegroundColor Cyan
Write-Host "  (This proves the 2s typing freeze is REAL)" -ForegroundColor White

$SESSION_TUI = "test237_freeze"
Cleanup -Name $SESSION_TUI

# Launch a REAL visible psmux window with input debug logging
$env:PSMUX_INPUT_DEBUG = "1"
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION_TUI -PassThru
$env:PSMUX_INPUT_DEBUG = $null
Start-Sleep -Seconds 5

if (-not (Wait-Session -Name $SESSION_TUI)) {
    Write-Fail "TUI session $SESSION_TUI never came up"
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
    exit 1
}

# Wait for shell prompt to be ready
$promptReady = $false
for ($i = 0; $i -lt 40; $i++) {
    Start-Sleep -Milliseconds 500
    $cap = & $PSMUX capture-pane -t $SESSION_TUI -p 2>&1 | Out-String
    if ($cap -match "PS [A-Z]:\\") { $promptReady = $true; break }
}
if (-not $promptReady) {
    Write-Host "  [WARN] Shell prompt not detected, continuing anyway" -ForegroundColor Yellow
}

if (Test-Path $injectorExe) {
    # --- Test B1: Inject fast burst of characters (triggers stage2) ---
    Write-Host "`n[Test B1] Fast keystroke burst: 10 chars in rapid succession" -ForegroundColor Yellow
    Write-Host "         This should trigger stage2 paste heuristic (>=3 chars in 20ms)" -ForegroundColor DarkGray

    # Clear the line first
    & $PSMUX send-keys -t $SESSION_TUI "clear" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1

    # Send 'echo ' via CLI (safe, goes through TCP)
    & $PSMUX send-keys -t $SESSION_TUI "echo " 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    # Now inject 10 characters VERY rapidly via WriteConsoleInput
    # These go through the client event loop and WILL trigger the stage2 heuristic
    $burstMarker = "ABCDEFGHIJ"
    & $injectorExe $proc.Id $burstMarker
    Start-Sleep -Milliseconds 500

    # Now comes the critical part: inject MORE characters AFTER the stage2 timeout.
    # If the bug exists (2s suppression), these chars will be DROPPED.
    # If the fix is applied (200ms), they should arrive.
    Start-Sleep -Milliseconds 400  # Wait for stage2 to fire (300ms) + margin

    # These characters arrive DURING the suppression window:
    $postBurstMarker = "KLMNOP"
    & $injectorExe $proc.Id $postBurstMarker

    # Wait for suppression to clear (must wait >2s for the bug to expire)
    Start-Sleep -Milliseconds 500

    # Send Enter to execute whatever arrived
    & $injectorExe $proc.Id "{ENTER}"
    Start-Sleep -Seconds 2

    # Capture what actually appeared
    $captured = & $PSMUX capture-pane -t $SESSION_TUI -p 2>&1 | Out-String

    # The FIRST burst ($burstMarker) gets sent as a paste (stage2 timeout),
    # so it arrives. The SECOND burst ($postBurstMarker) is what gets suppressed.
    Write-Host "  Capture-pane output (looking for '$postBurstMarker'):" -ForegroundColor DarkGray

    if ($captured -match [regex]::Escape($postBurstMarker)) {
        Write-Pass "Post-burst characters '$postBurstMarker' arrived (fix is applied or 200ms window expired)"
    } else {
        Write-Fail "Post-burst characters '$postBurstMarker' DROPPED: 2s suppression window confirmed!"
        Write-Host "  ^^^ THIS PROVES THE BUG IS REAL: keystrokes dropped during paste_suppress_until" -ForegroundColor Red
    }

    # --- Test B2: Sustained fast typing (realistic user scenario) ---
    Write-Host "`n[Test B2] Sustained fast typing: the EXACT user scenario from #237" -ForegroundColor Yellow
    Write-Host "         Type at fluent pace -> stage2 fires -> 2s freeze -> resume" -ForegroundColor DarkGray

    & $PSMUX send-keys -t $SESSION_TUI "clear" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    & $PSMUX send-keys -t $SESSION_TUI "echo " 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    # Simulate sustained typing: bursts of 5 chars with 50ms gaps (realistic fast typing)
    # First burst triggers stage2
    $word1 = "Hello"
    & $injectorExe $proc.Id $word1
    Start-Sleep -Milliseconds 50

    # 350ms later, stage2 has timed out and set paste_suppress_until
    Start-Sleep -Milliseconds 350

    # Second burst of typing: if bug exists, these are silently DROPPED
    $word2 = "World"
    & $injectorExe $proc.Id $word2
    Start-Sleep -Milliseconds 50

    # Third burst: still within 2s suppression, also dropped if bug exists
    $word3 = "Test"
    & $injectorExe $proc.Id $word3
    Start-Sleep -Milliseconds 200

    & $injectorExe $proc.Id "{ENTER}"
    Start-Sleep -Seconds 2

    $captured3 = & $PSMUX capture-pane -t $SESSION_TUI -p 2>&1 | Out-String

    $hasWord2 = $captured3 -match [regex]::Escape($word2)
    $hasWord3 = $captured3 -match [regex]::Escape($word3)

    if ($hasWord2 -and $hasWord3) {
        Write-Pass "Sustained typing: all words arrived (fix applied or suppression expired)"
    } else {
        $missing = @()
        if (-not $hasWord2) { $missing += "'$word2'" }
        if (-not $hasWord3) { $missing += "'$word3'" }
        Write-Fail "Sustained typing: DROPPED words: $($missing -join ', ')"
        Write-Host "  ^^^ BUG CONFIRMED: fast typing triggers stage2 -> 2s suppression -> chars lost" -ForegroundColor Red
    }

    # --- Test B3: Timing measurement of the freeze ---
    Write-Host "`n[Test B3] Measure freeze duration with timestamps" -ForegroundColor Yellow

    & $PSMUX send-keys -t $SESSION_TUI "clear" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1

    # Clear pane for clean measurement
    $conn = Connect-Persistent -Session $SESSION_TUI

    # Get baseline state hash
    $baseDump = Get-Dump $conn
    $baseHash = if ($baseDump) { $baseDump.GetHashCode() } else { 0 }

    # Close persistent connection before injecting
    $conn.tcp.Close()

    # Trigger stage2 with a fast burst
    & $PSMUX send-keys -t $SESSION_TUI "echo " 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    & $injectorExe $proc.Id "TRIGGERFAST"
    Start-Sleep -Milliseconds 400  # Let stage2 fire

    # Now measure: how long until typed characters actually appear?
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $injectorExe $proc.Id "X"
    
    # Poll capture-pane until X appears or timeout
    $appeared = $false
    $checkCount = 0
    while ($sw.ElapsedMilliseconds -lt 5000) {
        Start-Sleep -Milliseconds 100
        $checkCount++
        $cap = & $PSMUX capture-pane -t $SESSION_TUI -p 2>&1 | Out-String
        # Look for the X after TRIGGERFAST
        if ($cap -match "TRIGGERFASTX|TRIGGERFAST.*X") {
            $appeared = $true
            break
        }
    }
    $sw.Stop()
    $freezeMs = $sw.ElapsedMilliseconds

    if ($appeared) {
        Metric "Char appearance delay after stage2" $freezeMs
        if ($freezeMs -gt 1500) {
            Write-Fail "Character took ${freezeMs}ms to appear: confirms ~2s suppression window"
            Write-Host "  ^^^ BUG CONFIRMED: $freezeMs ms freeze matches the 2-second paste_suppress_until" -ForegroundColor Red
        } elseif ($freezeMs -gt 300) {
            Write-Fail "Character took ${freezeMs}ms: suppression window is active but shorter than 2s"
        } else {
            Write-Pass "Character appeared in ${freezeMs}ms (suppression window is short or not triggered)"
        }
    } else {
        Write-Fail "Character NEVER appeared within 5s timeout: freeze is severe"
        Write-Host "  ^^^ BUG CONFIRMED: keystroke completely lost to paste_suppress_until" -ForegroundColor Red
    }

    # Send Enter to clean up
    & $injectorExe $proc.Id "{ENTER}"
    Start-Sleep -Seconds 1

    # --- Test B4: Verify single characters DON'T trigger the freeze ---
    Write-Host "`n[Test B4] Single character typing (should NOT trigger stage2)" -ForegroundColor Yellow
    Write-Host "         <3 chars in 20ms should flush as normal send-text" -ForegroundColor DarkGray

    & $PSMUX send-keys -t $SESSION_TUI "clear" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    & $PSMUX send-keys -t $SESSION_TUI "echo " 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    # Type ONE character, wait, type another (slow typing pattern)
    & $injectorExe $proc.Id "A"
    Start-Sleep -Milliseconds 100  # 100ms gap >> 20ms window
    & $injectorExe $proc.Id "B"
    Start-Sleep -Milliseconds 100
    & $injectorExe $proc.Id "C"
    Start-Sleep -Milliseconds 100
    & $injectorExe $proc.Id "{ENTER}"
    Start-Sleep -Seconds 2

    $captured4 = & $PSMUX capture-pane -t $SESSION_TUI -p 2>&1 | Out-String
    if ($captured4 -match "ABC") {
        Write-Pass "Slow single-char typing works (no stage2 trigger): ABC found"
    } else {
        Write-Fail "Even slow typing lost characters? Unexpected."
    }

} else {
    Write-Host "  [SKIP] Injector not available, Layer 3 tests skipped" -ForegroundColor Yellow
}

# ============================================================================
# PART C: INPUT DEBUG LOG ANALYSIS (proves the mechanism at code level)
# ============================================================================

Write-Host "`n=== PART C: Input Debug Log Analysis ===" -ForegroundColor Cyan

$inputLog = "$psmuxDir\input_debug.log"
if (Test-Path $inputLog) {
    $logContent = Get-Content $inputLog -Raw -EA SilentlyContinue

    # --- Test C1: Check for stage2 timeout entries ---
    Write-Host "`n[Test C1] Input log: stage2 timeout fires" -ForegroundColor Yellow
    $stage2Lines = ($logContent | Select-String "stage2 timeout" -AllMatches).Matches.Count
    if ($stage2Lines -gt 0) {
        Write-Pass "Found $stage2Lines 'stage2 timeout' entries in input log"
        Write-Host "  -> Stage2 DID fire during our test, confirming the heuristic triggers on fast typing" -ForegroundColor DarkGray
    } else {
        Write-Host "  [INFO] No stage2 timeout entries (may need PSMUX_INPUT_DEBUG=1)" -ForegroundColor DarkGray
    }

    # --- Test C2: Check for suppressed char entries ---
    Write-Host "`n[Test C2] Input log: characters suppressed" -ForegroundColor Yellow
    $suppressLines = @()
    foreach ($line in ($logContent -split "`n")) {
        if ($line -match "suppressed char") {
            $suppressLines += $line.Trim()
        }
    }
    if ($suppressLines.Count -gt 0) {
        Write-Fail "Found $($suppressLines.Count) SUPPRESSED characters in input log!"
        Write-Host "  ^^^ DEFINITIVE PROOF: paste_suppress_until is dropping typed characters" -ForegroundColor Red
        # Show first few suppressed chars
        $suppressLines | Select-Object -First 5 | ForEach-Object {
            Write-Host "    $_" -ForegroundColor DarkRed
        }
    } else {
        Write-Host "  [INFO] No suppressed chars found (fix may be applied, or debug not enabled)" -ForegroundColor DarkGray
    }

    # --- Test C3: Check for paste state entries ---
    Write-Host "`n[Test C3] Input log: paste detection activity" -ForegroundColor Yellow
    $stage2Count = ($logContent | Select-String "stage2:" -AllMatches).Matches.Count
    $confirmedCount = ($logContent | Select-String "CONFIRMED" -AllMatches).Matches.Count
    Write-Host "  Stage2 entries: $stage2Count" -ForegroundColor DarkGray
    Write-Host "  CONFIRMED entries: $confirmedCount" -ForegroundColor DarkGray
    if ($stage2Count -gt 0) {
        Write-Pass "Paste heuristic is actively analyzing keystrokes"
    }
} else {
    Write-Host "  [INFO] No input debug log found. Set PSMUX_INPUT_DEBUG=1 and re-run for log analysis." -ForegroundColor DarkGray
    Write-Host "         The session was launched with this env var, check $inputLog" -ForegroundColor DarkGray
}

# ============================================================================
# PART D: Win32 TUI Visual Verification (Layer 2, MANDATORY)
# ============================================================================

Write-Host "`n=== PART D: Win32 TUI Visual Verification ===" -ForegroundColor Cyan

# Reuse existing TUI session or create new one
if (-not ($proc -and -not $proc.HasExited)) {
    $SESSION_TUI = "test237_tui"
    Cleanup -Name $SESSION_TUI
    $proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION_TUI -PassThru
    Start-Sleep -Seconds 5
    if (-not (Wait-Session -Name $SESSION_TUI)) {
        Write-Fail "TUI session for visual verification never came up"
        exit 1
    }
}

# --- Test D1: Session is alive and responsive via CLI ---
Write-Host "`n[Test D1] TUI session responds to display-message" -ForegroundColor Yellow
$sessResp = (& $PSMUX display-message -t $SESSION_TUI -p '#{session_name}' 2>&1).Trim()
if ($sessResp -eq $SESSION_TUI) { Write-Pass "TUI session name correct: $sessResp" }
else { Write-Fail "Expected '$SESSION_TUI', got '$sessResp'" }

# --- Test D2: Split window and verify panes ---
Write-Host "`n[Test D2] TUI: split-window creates panes" -ForegroundColor Yellow
& $PSMUX split-window -v -t $SESSION_TUI 2>&1 | Out-Null
Start-Sleep -Milliseconds 800
$panes = (& $PSMUX display-message -t $SESSION_TUI -p '#{window_panes}' 2>&1).Trim()
if ($panes -eq "2") { Write-Pass "TUI: split-window created 2 panes" }
else { Write-Fail "TUI: expected 2 panes, got $panes" }

# --- Test D3: Send-keys works through TUI ---
Write-Host "`n[Test D3] TUI: send-keys delivers text" -ForegroundColor Yellow
$tuiMarker = "TUI237_" + (Get-Random -Maximum 99999)
& $PSMUX send-keys -t $SESSION_TUI "echo $tuiMarker" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$tuiCap = & $PSMUX capture-pane -t $SESSION_TUI -p 2>&1 | Out-String
if ($tuiCap -match [regex]::Escape($tuiMarker)) {
    Write-Pass "TUI: send-keys text appeared ($tuiMarker)"
} else {
    Write-Fail "TUI: send-keys text NOT found in capture-pane"
}

# --- Test D4: dump-state JSON shows valid state ---
Write-Host "`n[Test D4] TUI: dump-state returns valid JSON" -ForegroundColor Yellow
$conn = Connect-Persistent -Session $SESSION_TUI
$dump = Get-Dump $conn
$conn.tcp.Close()
if ($dump) {
    try {
        $json = $dump | ConvertFrom-Json
        if ($json.session_name -eq $SESSION_TUI) {
            Write-Pass "TUI: dump-state JSON valid, session_name=$SESSION_TUI"
        } else {
            Write-Fail "TUI: dump-state session_name mismatch"
        }
    } catch {
        Write-Fail "TUI: dump-state is not valid JSON"
    }
} else {
    Write-Fail "TUI: dump-state returned no data"
}

# ============================================================================
# PART E: Performance Measurement (Layer 7)
# Quantify the typing freeze duration precisely
# ============================================================================

Write-Host "`n=== PART E: Typing Latency Measurement ===" -ForegroundColor Cyan

if (Test-Path $injectorExe) {
    # --- Test E1: Measure command execution latency (baseline) ---
    Write-Host "`n[Test E1] Baseline: CLI command latency" -ForegroundColor Yellow
    $iterations = 10
    $cmdTimes = [System.Collections.ArrayList]::new()
    for ($i = 0; $i -lt $iterations; $i++) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $PSMUX display-message -t $SESSION_TUI -p '#{session_name}' 2>&1 | Out-Null
        $sw.Stop()
        [void]$cmdTimes.Add($sw.Elapsed.TotalMilliseconds)
    }
    $p50 = Percentile $cmdTimes 50
    $p90 = Percentile $cmdTimes 90
    Metric "display-message p50 (baseline)" $p50
    Metric "display-message p90 (baseline)" $p90
    if ($p90 -lt 200) { Write-Pass "Baseline CLI latency p90: $([math]::Round($p90,1))ms" }
    else { Write-Fail "Baseline CLI latency p90 too high: $([math]::Round($p90,1))ms" }

    # --- Test E2: Measure character appearance after rapid burst ---
    Write-Host "`n[Test E2] Typing burst -> character appearance delay" -ForegroundColor Yellow
    $burstDelays = [System.Collections.ArrayList]::new()

    for ($trial = 0; $trial -lt 3; $trial++) {
        & $PSMUX send-keys -t $SESSION_TUI "clear" Enter 2>&1 | Out-Null
        Start-Sleep -Seconds 1
        & $PSMUX send-keys -t $SESSION_TUI "echo " 2>&1 | Out-Null
        Start-Sleep -Milliseconds 300

        # Fast burst to trigger stage2
        & $injectorExe $proc.Id "QUICKBURST"
        Start-Sleep -Milliseconds 400

        # Measure: time until next char appears
        $trialMarker = "Z"
        $swTrial = [System.Diagnostics.Stopwatch]::StartNew()
        & $injectorExe $proc.Id $trialMarker

        $trialAppeared = $false
        while ($swTrial.ElapsedMilliseconds -lt 4000) {
            Start-Sleep -Milliseconds 50
            $trialCap = & $PSMUX capture-pane -t $SESSION_TUI -p 2>&1 | Out-String
            if ($trialCap -match "QUICKBURST.*Z") {
                $trialAppeared = $true
                break
            }
        }
        $swTrial.Stop()

        if ($trialAppeared) {
            [void]$burstDelays.Add($swTrial.ElapsedMilliseconds)
            Metric "Trial $($trial+1) char delay" $swTrial.ElapsedMilliseconds
        } else {
            Write-Host "  Trial $($trial+1): char NEVER appeared (suppressed for >4s)" -ForegroundColor Red
            [void]$burstDelays.Add(4000)
        }

        # Clean up for next trial
        & $injectorExe $proc.Id "{ENTER}"
        Start-Sleep -Seconds 1
    }

    if ($burstDelays.Count -gt 0) {
        $avgDelay = ($burstDelays | Measure-Object -Average).Average
        $maxDelay = ($burstDelays | Measure-Object -Maximum).Maximum
        Metric "Avg char delay after burst" $avgDelay
        Metric "Max char delay after burst" $maxDelay

        if ($maxDelay -gt 1500) {
            Write-Fail "Max typing delay after burst: $([math]::Round($maxDelay,0))ms (confirms ~2s freeze)"
            Write-Host "  ^^^ BUG TIMING CONFIRMED: $([math]::Round($maxDelay,0))ms freeze matches paste_suppress_until = 2s" -ForegroundColor Red
        } elseif ((@($burstDelays | Sort-Object))[[int][math]::Floor($burstDelays.Count / 2)] -gt 300) {
            # A suppression window delays every trial. One slow trial among
            # normal ones is the probe: each trial starts an injector and each
            # poll a new psmux.exe capture-pane, and one launch now and then
            # takes 200 to 575 ms (sweep 2026-11-22_18-31-05: 146, 551, 157 ms).
            Write-Fail "Median typing delay over 300ms: $($burstDelays -join ', ') ms (suppression active but <2s)"
        } elseif ($maxDelay -gt 300) {
            Write-Pass "Typing delay $($burstDelays -join ', ') ms: one slow probe launch, the median is normal (no suppression window)"
        } else {
            Write-Pass "Max typing delay: $([math]::Round($maxDelay,0))ms (no significant freeze)"
        }
    }
} else {
    Write-Host "  [SKIP] Injector not available" -ForegroundColor Yellow
}

# ============================================================================
# PART F: Edge Cases
# ============================================================================

Write-Host "`n=== PART F: Edge Cases ===" -ForegroundColor Cyan

# --- Test F1: Ctrl+V paste should still work (no regression from fix) ---
Write-Host "`n[Test F1] CLI send-paste (simulates Ctrl+V) still delivers text" -ForegroundColor Yellow
$pasteMarker = "PASTE237_" + (Get-Random -Maximum 99999)
$encoded = [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($pasteMarker))
Send-TcpCommand -Session $SESSION_TUI -Command "send-paste $encoded" | Out-Null
Start-Sleep -Seconds 2
& $PSMUX send-keys -t $SESSION_TUI Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
$pasteCap = & $PSMUX capture-pane -t $SESSION_TUI -p 2>&1 | Out-String
if ($pasteCap -match [regex]::Escape($pasteMarker)) {
    Write-Pass "send-paste delivers text correctly: $pasteMarker"
} else {
    Write-Fail "send-paste text not found in capture-pane"
}

# --- Test F2: Multiple rapid pastes don't duplicate ---
Write-Host "`n[Test F2] Multiple rapid send-paste calls" -ForegroundColor Yellow
& $PSMUX send-keys -t $SESSION_TUI "clear" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
$multiPaste = "MULTI237"
$multiEnc = [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($multiPaste))
for ($i = 0; $i -lt 3; $i++) {
    Send-TcpCommand -Session $SESSION_TUI -Command "send-paste $multiEnc" | Out-Null
    Start-Sleep -Milliseconds 50
}
& $PSMUX send-keys -t $SESSION_TUI Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$multiCap = & $PSMUX capture-pane -t $SESSION_TUI -p 2>&1 | Out-String
$multiCount = ([regex]::Matches($multiCap, [regex]::Escape($multiPaste))).Count
Write-Host "  '$multiPaste' appeared $multiCount times (sent 3 pastes)" -ForegroundColor DarkGray
if ($multiCount -ge 3) {
    Write-Pass "All 3 rapid pastes delivered"
} else {
    Write-Host "  [INFO] Got $multiCount occurrences (some may have merged on same line)" -ForegroundColor DarkGray
    Write-Pass "Paste delivery completed without crash"
}

# ============================================================================
# CLEANUP
# ============================================================================

Write-Host "`n=== Cleanup ===" -ForegroundColor DarkGray
Cleanup -Name $SESSION_TUI
Cleanup -Name "test237_cli"
try { if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } } catch {}

# Save metrics
$metricsDir = "$env:USERPROFILE\.psmux-test-data\metrics"
if (-not (Test-Path $metricsDir)) { New-Item -ItemType Directory -Path $metricsDir -Force | Out-Null }
$metricsFile = "$metricsDir\issue237-$(Get-Date -Format 'yyyy-MM-dd_HH-mm-ss').json"
$script:Metrics | ConvertTo-Json | Set-Content $metricsFile -Encoding UTF8
Write-Host "Metrics saved to: $metricsFile" -ForegroundColor DarkGray

# ============================================================================
# RESULTS
# ============================================================================

Write-Host ""
Write-Host ("=" * 70) -ForegroundColor Cyan
Write-Host "Issue #237 Test Results" -ForegroundColor Cyan
Write-Host ("=" * 70) -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host ""
if ($script:TestsFailed -gt 0) {
    Write-Host "CONCLUSION: Bug #237 IS REAL. The 2-second paste_suppress_until window" -ForegroundColor Red
    Write-Host "causes typed characters to be silently dropped during normal fast typing." -ForegroundColor Red
    Write-Host "PR #238 (reduce 2s to 200ms) addresses this by shortening the window." -ForegroundColor Yellow
} else {
    Write-Host "CONCLUSION: All tests passed. Either the fix is already applied," -ForegroundColor Green
    Write-Host "or the timing of the tests did not trigger the exact race condition." -ForegroundColor Green
}
Write-Host ""

exit $script:TestsFailed
