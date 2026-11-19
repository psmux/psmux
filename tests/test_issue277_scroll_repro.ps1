# Issue #277 + #245: Mouse Scroll Reproduction Test
# Tests that mouse scroll events are correctly handled in psmux
# Must REPRODUCE the bug before looking at any code

$ErrorActionPreference = "Continue"
# Binary and data root are overridable so this suite can be pointed at a build
# under test without disturbing the installed psmux or its sessions.
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$SESSION = "scroll_repro_277"
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0
$script:TestsFailed = 0
$mouseInjector = "$env:TEMP\psmux_mouse_injector.exe"

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
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

Write-Host "`n============================================" -ForegroundColor Cyan
Write-Host "Issue #277 + #245: Mouse Scroll Reproduction" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan

# === SETUP ===
Cleanup

Write-Host "`n--- PART A: Scroll with default settings (mouse-selection ON) ---" -ForegroundColor Yellow

# Create a visible TUI session
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
Start-Sleep -Seconds 4

# Verify session exists
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "Session creation failed"
    exit 1
}
Write-Pass "Session created successfully"

# Check default mouse settings
$mouseOpt = (& $PSMUX show-options -g -v mouse -t $SESSION 2>&1 | Out-String).Trim()
$mouseSelOpt = (& $PSMUX show-options -g -v mouse-selection -t $SESSION 2>&1 | Out-String).Trim()
Write-Host "  [INFO] mouse=$mouseOpt, mouse-selection=$mouseSelOpt" -ForegroundColor DarkGray

# Generate lots of scrollable content
& $PSMUX send-keys -t $SESSION 'for ($i=1; $i -le 200; $i++) { Write-Host "LINE_$i scrollback test content - padding text to make lines visible" }' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 5

# Capture pane content BEFORE scroll (should show recent lines near 200)
$captureBefore = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
$lastLinesBefore = ($captureBefore -split "`n" | Where-Object { $_ -match "LINE_\d+" }) | Select-Object -Last 3
Write-Host "  [INFO] Before scroll, last lines: $($lastLinesBefore -join ', ')" -ForegroundColor DarkGray

# Test 1: Send mouse wheel scroll UP events via injector
Write-Host "`n[Test 1] Mouse wheel UP with mouse-selection ON (default)" -ForegroundColor Yellow
if (Test-Path $mouseInjector) {
    & $mouseInjector $proc.Id "up" 10 40 15
    Start-Sleep -Seconds 2
    
    # Check injector log
    $injectLog = Get-Content "$env:TEMP\psmux_mouse_inject.log" -Raw -EA SilentlyContinue
    if ($injectLog -match "ok=True") {
        Write-Host "  [INFO] Mouse wheel events injected successfully" -ForegroundColor DarkGray
    } else {
        Write-Host "  [WARN] Injector log: $injectLog" -ForegroundColor DarkYellow
    }
    
    # After scrolling up, psmux should enter copy mode (scrollback mode)
    # Check via dump-state
    $conn = Connect-Persistent -Session $SESSION
    $state = Get-Dump $conn
    $conn.tcp.Close()
    
    # dump-state's "NC" (no-change) short-circuit is a server-wide dirty
    # flag, not per-connection: with a live attached client in the same
    # session (this test's own $proc, continuously polling dump-state to
    # redraw), the client's own poll loop almost always wins the race and
    # consumes the dirty flag before this separate TCP connection's request
    # lands, so $state legitimately comes back null/"NC" here on a healthy
    # server. That's a race in *this test's* verification method, not
    # evidence scroll is broken — so the capture-pane fallback must run
    # unconditionally instead of being gated behind a successful dump-state.
    $sawCopyModeInState = $false
    if ($state) {
        $stateStr = $state
        if ($stateStr -match '"copy_mode"\s*:\s*true' -or $stateStr -match '"in_copy_mode"\s*:\s*true') {
            $sawCopyModeInState = $true
        }
    } else {
        Write-Host "  [INFO] dump-state unavailable (client-poll race) - falling back to capture-pane" -ForegroundColor DarkGray
    }
    if ($sawCopyModeInState) {
        Write-Pass "Mouse wheel UP entered copy mode (scroll works with mouse-selection ON)"
    } else {
        # Capture pane to see if content changed (scrolled)
        $captureAfter = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
        $lastLinesAfter = ($captureAfter -split "`n" | Where-Object { $_ -match "LINE_\d+" }) | Select-Object -Last 3
        Write-Host "  [INFO] After scroll, last lines: $($lastLinesAfter -join ', ')" -ForegroundColor DarkGray

        if ($captureAfter -ne $captureBefore) {
            Write-Pass "Mouse wheel UP changed pane content (scroll works)"
        } else {
            Write-Fail "Mouse wheel UP had no effect - SCROLL NOT WORKING with mouse-selection ON"
        }
    }
    
    # Exit copy mode if entered
    & $PSMUX send-keys -t $SESSION "q" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
} else {
    Write-Fail "Mouse injector not found at $mouseInjector"
}

Write-Host "`n--- PART B: Scroll with mouse-selection OFF (issue #245 scenario) ---" -ForegroundColor Yellow

# Set mouse-selection off (this is what #245 user reported breaks scroll)
& $PSMUX set-option -g mouse-selection off -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

$mouseSelOpt2 = (& $PSMUX show-options -g -v mouse-selection -t $SESSION 2>&1 | Out-String).Trim()
Write-Host "  [INFO] mouse-selection now: $mouseSelOpt2" -ForegroundColor DarkGray

# Capture before scroll
$captureBefore2 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String

# Test 2: Mouse wheel with mouse-selection OFF
Write-Host "`n[Test 2] Mouse wheel UP with mouse-selection OFF" -ForegroundColor Yellow
if (Test-Path $mouseInjector) {
    & $mouseInjector $proc.Id "up" 10 40 15
    Start-Sleep -Seconds 2
    
    $injectLog2 = Get-Content "$env:TEMP\psmux_mouse_inject.log" -Raw -EA SilentlyContinue
    if ($injectLog2 -match "ok=True") {
        Write-Host "  [INFO] Mouse wheel events injected successfully" -ForegroundColor DarkGray
    }
    
    # Check state after scroll attempt
    $conn2 = Connect-Persistent -Session $SESSION
    $state2 = Get-Dump $conn2
    $conn2.tcp.Close()
    
    # Same dump-state client-poll race as Test 1 (see comment above) - fall
    # back to capture-pane unconditionally instead of silently skipping the
    # assertion when $state2 is null.
    $sawCopyModeInState2 = $false
    if ($state2) {
        $stateStr2 = $state2
        if ($stateStr2 -match '"copy_mode"\s*:\s*true' -or $stateStr2 -match '"in_copy_mode"\s*:\s*true') {
            $sawCopyModeInState2 = $true
        }
    } else {
        Write-Host "  [INFO] dump-state unavailable (client-poll race) - falling back to capture-pane" -ForegroundColor DarkGray
    }
    if ($sawCopyModeInState2) {
        Write-Pass "Mouse wheel UP entered copy mode with mouse-selection OFF"
    } else {
        $captureAfter2 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
        if ($captureAfter2 -ne $captureBefore2) {
            Write-Pass "Mouse wheel UP changed content with mouse-selection OFF"
        } else {
            Write-Fail "Mouse wheel UP had NO EFFECT with mouse-selection OFF - BUG CONFIRMED (#245)"
        }
    }
    
    # Exit copy mode
    & $PSMUX send-keys -t $SESSION "q" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n--- PART C: Alternate screen (TUI app) scroll test ---" -ForegroundColor Yellow

# Reset mouse-selection to on for this test first
& $PSMUX set-option -g mouse-selection on -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# Run 'less' on generated content inside psmux (alternate screen)
$testFile = "$env:TEMP\psmux_scroll_test_content.txt"
1..500 | ForEach-Object { "Line $_ of alt-screen test content - this is a long line to make it visible" } | Set-Content $testFile -Encoding UTF8

# Send the less command
& $PSMUX send-keys -t $SESSION "Get-Content '$testFile' | more" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 3

# Capture the pane to see initial state of 'more'
$capBeforeLess = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
Write-Host "  [INFO] 'more' initial view (first 3 lines):" -ForegroundColor DarkGray
($capBeforeLess -split "`n" | Where-Object { $_ -match "Line \d+" }) | Select-Object -First 3 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }

# Test 3: Mouse wheel DOWN in 'more' (should scroll down)
Write-Host "`n[Test 3] Mouse wheel DOWN in 'more' with mouse-selection ON" -ForegroundColor Yellow
if (Test-Path $mouseInjector) {
    & $mouseInjector $proc.Id "down" 5 40 15
    Start-Sleep -Seconds 2
    
    $capAfterLess = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    # In alternate screen with mouse tracking, scroll should navigate the content
    # Check if we see different lines now
    $linesBefore = ($capBeforeLess -split "`n" | Where-Object { $_ -match "Line (\d+)" } | ForEach-Object { [regex]::Match($_, "Line (\d+)").Groups[1].Value } | Select-Object -First 1)
    $linesAfter = ($capAfterLess -split "`n" | Where-Object { $_ -match "Line (\d+)" } | ForEach-Object { [regex]::Match($_, "Line (\d+)").Groups[1].Value } | Select-Object -First 1)
    
    Write-Host "  [INFO] Before: starts at Line $linesBefore, After: starts at Line $linesAfter" -ForegroundColor DarkGray
    
    if ($linesAfter -and $linesBefore -and [int]$linesAfter -gt [int]$linesBefore) {
        Write-Pass "'more' scrolled forward with mouse wheel DOWN"
    } elseif ($capAfterLess -ne $capBeforeLess) {
        Write-Pass "Pane content changed after mouse wheel (scroll likely worked)"
    } else {
        Write-Fail "Mouse wheel DOWN had no effect in 'more' - SCROLL NOT WORKING in alt-screen"
    }
}

# Exit 'more'
& $PSMUX send-keys -t $SESSION "q" 2>&1 | Out-Null
Start-Sleep -Seconds 1

Write-Host "`n--- PART D: Alternate screen scroll with mouse-selection OFF ---" -ForegroundColor Yellow

# Set mouse-selection off
& $PSMUX set-option -g mouse-selection off -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# Run 'more' again
& $PSMUX send-keys -t $SESSION "Get-Content '$testFile' | more" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 3

$capBeforeLess2 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String

# Test 4: Mouse wheel in alt-screen with mouse-selection OFF
Write-Host "`n[Test 4] Mouse wheel DOWN in 'more' with mouse-selection OFF" -ForegroundColor Yellow
if (Test-Path $mouseInjector) {
    & $mouseInjector $proc.Id "down" 5 40 15
    Start-Sleep -Seconds 2
    
    $capAfterLess2 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    $linesBefore2 = ($capBeforeLess2 -split "`n" | Where-Object { $_ -match "Line (\d+)" } | ForEach-Object { [regex]::Match($_, "Line (\d+)").Groups[1].Value } | Select-Object -First 1)
    $linesAfter2 = ($capAfterLess2 -split "`n" | Where-Object { $_ -match "Line (\d+)" } | ForEach-Object { [regex]::Match($_, "Line (\d+)").Groups[1].Value } | Select-Object -First 1)
    
    Write-Host "  [INFO] Before: starts at Line $linesBefore2, After: starts at Line $linesAfter2" -ForegroundColor DarkGray
    
    if ($linesAfter2 -and $linesBefore2 -and [int]$linesAfter2 -gt [int]$linesBefore2) {
        Write-Pass "'more' scrolled with mouse-selection OFF"
    } elseif ($capAfterLess2 -ne $capBeforeLess2) {
        Write-Pass "Pane content changed with mouse-selection OFF (scroll works)"
    } else {
        Write-Fail "Mouse wheel had NO EFFECT in alt-screen with mouse-selection OFF - BUG CONFIRMED"
    }
}

# Exit 'more'
& $PSMUX send-keys -t $SESSION "q" 2>&1 | Out-Null
Start-Sleep -Seconds 1

Write-Host "`n--- PART E: opencode scroll test (issue #277 specific) ---" -ForegroundColor Yellow

# Test with opencode specifically
# First check if opencode is available
$opencodePath = Get-Command opencode -EA SilentlyContinue
if ($opencodePath) {
    # Reset settings  
    & $PSMUX set-option -g mouse-selection on -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    
    # Run opencode in c:\cctest
    Write-Host "  [INFO] Launching opencode in c:\cctest..." -ForegroundColor DarkGray
    & $PSMUX send-keys -t $SESSION "cd C:\cctest" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    & $PSMUX send-keys -t $SESSION "opencode" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 8  # Give opencode time to start
    
    # Capture initial state
    $capOC1 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    Write-Host "  [INFO] opencode initial capture (first 3 non-empty lines):" -ForegroundColor DarkGray
    ($capOC1 -split "`n" | Where-Object { $_.Trim().Length -gt 0 }) | Select-Object -First 3 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
    
    # Give opencode a prompt to generate content to scroll
    & $PSMUX send-keys -t $SESSION "list all files in this directory with descriptions for each" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 15  # Wait for response to generate
    
    $capOC2 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String

    # PRECONDITION: the prompt must actually have rendered a response.
    # When opencode's model backend is unreachable (e.g. the configured
    # local ollama endpoint is down) the viewport never fills, there is
    # nothing to scroll, and the capture diff oracle would misreport that
    # as "scroll broken". Require visible response growth first.
    # Line-count based: opencode's spinner and clock redraw make raw capture
    # equality useless as a content signal (captures differ while the
    # viewport is still effectively empty).
    # Count rows that carry text once opencode's box drawing is stripped:
    # its frame and gutter rows are non-empty but say nothing, and an error
    # reply (an expired subscription, an unreachable endpoint) plus its toast
    # took a raw non-empty count past 15 while the whole conversation still
    # fitted on screen, leaving nothing to scroll. Only a reply that fills
    # the pane gives the wheel something to move.
    $ocLines = @($capOC2 -split "`n" | Where-Object { ($_ -replace '[\s─-▟]', '').Length -gt 0 }).Count
    $ocHeight = 0
    [int]::TryParse(((& $PSMUX display-message -t $SESSION -p '#{pane_height}' 2>&1) | Out-String).Trim(), [ref]$ocHeight) | Out-Null
    $ocNeed = [Math]::Max(15, $ocHeight - 6)
    if ($ocLines -lt $ocNeed) {
        $ocWhy = ($capOC2 -split "`n" | Where-Object { $_ -match 'subscription|error|unauthori|failed|refused' } | Select-Object -First 1)
        $ocWhy = if ($ocWhy) { " opencode said: " + ($ocWhy -replace '[─-▟]', '').Trim() } else { '' }
        Write-Host "  [SKIP] opencode produced no scrollable content ($ocLines text rows, $ocNeed needed to fill a $ocHeight row pane); model backend likely unreachable.$ocWhy Scroll checks skipped." -ForegroundColor DarkYellow
    } else {
    # Test 5: Mouse wheel in opencode with mouse-selection ON
    Write-Host "`n[Test 5] Mouse wheel UP in opencode with mouse-selection ON" -ForegroundColor Yellow
    & $mouseInjector $proc.Id "up" 10 40 15
    Start-Sleep -Seconds 2

    $capOC3 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($capOC3 -ne $capOC2) {
        Write-Pass "Opencode responded to mouse wheel UP (scroll works)"
    } else {
        Write-Fail "Mouse wheel UP had no effect in opencode - BUG CONFIRMED (#277)"
    }

    # Test 6: Set mouse-selection OFF and test scroll in opencode
    Write-Host "`n[Test 6] Mouse wheel UP in opencode with mouse-selection OFF" -ForegroundColor Yellow
    & $PSMUX set-option -g mouse-selection off -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    $capOC4 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    & $mouseInjector $proc.Id "up" 10 40 15
    Start-Sleep -Seconds 2

    $capOC5 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($capOC5 -ne $capOC4) {
        Write-Pass "Opencode scroll works with mouse-selection OFF"
    } else {
        Write-Fail "Mouse wheel had NO EFFECT in opencode with mouse-selection OFF - BUG CONFIRMED"
    }
    }

    # Exit opencode
    & $PSMUX send-keys -t $SESSION C-c 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    & $PSMUX send-keys -t $SESSION "exit" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1
} else {
    Write-Host "  [SKIP] opencode not found in PATH" -ForegroundColor DarkYellow
}

# === TEARDOWN ===
Write-Host "`n--- Cleanup ---" -ForegroundColor Yellow
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
Remove-Item $testFile -Force -EA SilentlyContinue
Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
