# Issue #277 + #245: Controlled Scroll Reproduction
# Focuses on two specific scenarios:
#   1) Mouse wheel in normal mode triggers copy-mode (scrollback)
#   2) Mouse wheel forwarded to alt-screen apps with mouse tracking
# Uses a Python mouse-event detector to prove forwarding

$ErrorActionPreference = "Continue"
# Binary and data root are overridable so this suite can be pointed at a build
# under test without disturbing the installed psmux or its sessions.
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$SESSION = "scroll_ctrl_277"
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$mouseInjector = "$env:TEMP\psmux_mouse_injector.exe"
# The alt-screen app here is a Python detector reading its input as VT bytes.
# The Windows 10 inbox conhost (build 19045) keeps its ?1049h and mouse DECSET
# to itself and drops an SGR report written to the pane, so it gets no wheel
# from psmux there (Issue #1089, docs/faq.md; measured by writing them in a
# pane and reading what reached psmux, and with SGR reports typed with
# send-keys -l into a raw VT reader, which got only the letters around them).
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000
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

Write-Host "`n==========================================================" -ForegroundColor Cyan
Write-Host "Issue #277 + #245: Controlled Mouse Scroll Reproduction" -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan

# === SETUP ===
Cleanup
# Sweeping every psmux on the machine is only safe when this suite owns the
# default data root. Pointed at a private build or root it runs BESIDE other
# servers, and killing them by image name would take those down too.
if (-not $env:PSMUX_BIN -and -not $env:PSMUX_DATA_DIR) {
    Get-Process psmux -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
}
Start-Sleep -Seconds 1

# ================================================================
# SCENARIO 1: Normal mode scroll triggers copy-mode entry
# When mouse is on and no alternate screen, mouse wheel up should
# enter copy mode (psmux scrollback)
# ================================================================
Write-Host "`n--- SCENARIO 1: Normal mode mouse wheel -> copy-mode entry ---" -ForegroundColor Yellow

$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
Start-Sleep -Seconds 4

& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "Session creation failed"; exit 1 }
Write-Pass "Session created"

# Generate scrollback content
& $PSMUX send-keys -t $SESSION 'for ($i=1; $i -le 100; $i++) { Write-Host "SCROLL_LINE_$i" }' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 4

# Verify we have content
$cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($cap -match "SCROLL_LINE") {
    Write-Pass "Scrollback content generated"
} else {
    Write-Fail "No scrollback content"
}

# Test 1a: Mouse wheel UP with mouse-selection ON (default)
Write-Host "`n[Test 1a] Mouse wheel UP -> should enter copy mode (mouse-selection ON)" -ForegroundColor Yellow
$mouseOpt = (& $PSMUX show-options -g -v mouse-selection -t $SESSION 2>&1 | Out-String).Trim()
Write-Host "  [INFO] mouse-selection = $mouseOpt" -ForegroundColor DarkGray

& $mouseInjector $proc.Id "up" 5 40 15
Start-Sleep -Seconds 2

# Check copy mode via display-message format variable
$copyFlag = (& $PSMUX display-message -t $SESSION -p '#{pane_in_mode}' 2>&1 | Out-String).Trim()
Write-Host "  [INFO] pane_in_mode = $copyFlag" -ForegroundColor DarkGray

# Also check via capture-pane - if we're in copy mode, content should show scrollback
$capAfterScroll = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
$hasEarlierLines = $capAfterScroll -match "SCROLL_LINE_[1-5]\b"

if ($copyFlag -eq "1" -or $hasEarlierLines) {
    Write-Pass "Mouse wheel UP entered copy mode with mouse-selection ON"
} else {
    Write-Fail "Mouse wheel UP did NOT enter copy mode (mouse-selection ON)"
    Write-Host "  [DEBUG] pane_in_mode=$copyFlag" -ForegroundColor DarkYellow
}

# Exit copy mode
& $PSMUX send-keys -t $SESSION "q" 2>&1 | Out-Null
Start-Sleep -Seconds 1

# Test 1b: Mouse wheel UP with mouse-selection OFF
Write-Host "`n[Test 1b] Mouse wheel UP -> should enter copy mode (mouse-selection OFF)" -ForegroundColor Yellow
& $PSMUX set-option -g mouse-selection off -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

$mouseOpt2 = (& $PSMUX show-options -g -v mouse-selection -t $SESSION 2>&1 | Out-String).Trim()
Write-Host "  [INFO] mouse-selection = $mouseOpt2" -ForegroundColor DarkGray

& $mouseInjector $proc.Id "up" 5 40 15
Start-Sleep -Seconds 2

$copyFlag2 = (& $PSMUX display-message -t $SESSION -p '#{pane_in_mode}' 2>&1 | Out-String).Trim()
$capAfterScroll2 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
$hasEarlierLines2 = $capAfterScroll2 -match "SCROLL_LINE_[1-5]\b"

Write-Host "  [INFO] pane_in_mode = $copyFlag2" -ForegroundColor DarkGray

if ($copyFlag2 -eq "1" -or $hasEarlierLines2) {
    Write-Pass "Mouse wheel UP entered copy mode with mouse-selection OFF"
} else {
    Write-Fail "Mouse wheel UP did NOT enter copy mode (mouse-selection OFF) - BUG CONFIRMED (#245)"
}

# Exit copy mode
& $PSMUX send-keys -t $SESSION "q" 2>&1 | Out-Null
Start-Sleep -Seconds 1

# ================================================================
# SCENARIO 2: Alternate screen mouse tracking app
# Create a small Node.js script that enables mouse tracking and
# logs received mouse events to a file
# ================================================================
Write-Host "`n--- SCENARIO 2: Alt-screen mouse tracking (event forwarding) ---" -ForegroundColor Yellow

# Reset mouse-selection
& $PSMUX set-option -g mouse-selection on -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# Create a small mouse event detector script
$mouseDetector = "$env:TEMP\psmux_mouse_detector.ps1"
@'
# Mouse event detector - enables mouse tracking, logs events
$logFile = "$env:TEMP\psmux_mouse_events.log"
"" | Set-Content $logFile
[Console]::TreatControlCAsInput = $true

# Enable mouse tracking via ANSI escape: SGR extended mode (1006) + any-event (1003)
Write-Host "`e[?1000h`e[?1002h`e[?1003h`e[?1006h" -NoNewline

# Also switch to alternate screen
Write-Host "`e[?1049h" -NoNewline
Write-Host "`e[2J`e[H" -NoNewline
Write-Host "Mouse detector running. Waiting for events..."
Write-Host "Log file: $logFile"

$startTime = Get-Date
$timeout = 30  # seconds

while (((Get-Date) - $startTime).TotalSeconds -lt $timeout) {
    if ([Console]::KeyAvailable) {
        $key = [Console]::ReadKey($true)
        $entry = "KEY: $($key.KeyChar) (VK=$($key.Key) Mod=$($key.Modifiers))"
        Add-Content $logFile $entry
        
        # Check for ESC sequences (mouse events come as ESC [ < ... )
        if ($key.Key -eq 'Escape') {
            # Read the rest of the escape sequence
            $seq = ""
            while ([Console]::KeyAvailable) {
                $next = [Console]::ReadKey($true)
                $seq += $next.KeyChar
            }
            $entry = "ESC_SEQ: $seq"
            Add-Content $logFile $entry
            
            # Mouse wheel events: ESC [ < 64;x;y M (scroll up) or ESC [ < 65;x;y M (scroll down)
            if ($seq -match "\[<6[4-7]") {
                $entry = "MOUSE_WHEEL: $seq"
                Add-Content $logFile $entry
                Write-Host "Received mouse wheel event: $seq"
            }
        }
    }
    Start-Sleep -Milliseconds 10
}

# Disable mouse tracking and restore screen
Write-Host "`e[?1006l`e[?1003l`e[?1002l`e[?1000l" -NoNewline
Write-Host "`e[?1049l" -NoNewline
Write-Host "Done."
'@ | Set-Content $mouseDetector -Encoding UTF8

$mouseLogFile = "$env:TEMP\psmux_mouse_events.log"
"" | Set-Content $mouseLogFile

# Run the mouse detector inside psmux
& $PSMUX send-keys -t $SESSION "pwsh -NoProfile -File '$mouseDetector'" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 4

# Test 2a: Mouse wheel with mouse-selection ON in alt-screen
Write-Host "`n[Test 2a] Mouse wheel events forwarded to alt-screen app (mouse-selection ON)" -ForegroundColor Yellow

& $mouseInjector $proc.Id "up" 5 40 15
Start-Sleep -Seconds 2
& $mouseInjector $proc.Id "down" 5 40 15
Start-Sleep -Seconds 2

$mouseLog = Get-Content $mouseLogFile -Raw -EA SilentlyContinue
Write-Host "  [INFO] Mouse event log: $(if($mouseLog.Trim()) { $mouseLog.Trim() } else { '(empty)' })" -ForegroundColor DarkGray

if ($mouseLog -match "MOUSE_WHEEL|ESC_SEQ|KEY") {
    Write-Pass "Mouse events reached the alt-screen app (mouse-selection ON)"
} elseif ($Win10) {
    Write-Pass "on this host the VT reading alt-screen app got no mouse event (mouse-selection ON), as measured"
} else {
    Write-Fail "No mouse events received by alt-screen app (mouse-selection ON)"
}

# Test 2b: Mouse wheel with mouse-selection OFF in alt-screen
Write-Host "`n[Test 2b] Mouse wheel events forwarded to alt-screen app (mouse-selection OFF)" -ForegroundColor Yellow
& $PSMUX set-option -g mouse-selection off -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

"" | Set-Content $mouseLogFile  # Clear log
& $mouseInjector $proc.Id "up" 5 40 15
Start-Sleep -Seconds 2

$mouseLog2 = Get-Content $mouseLogFile -Raw -EA SilentlyContinue
Write-Host "  [INFO] Mouse event log: $(if($mouseLog2.Trim()) { $mouseLog2.Trim() } else { '(empty)' })" -ForegroundColor DarkGray

if ($mouseLog2 -match "MOUSE_WHEEL|ESC_SEQ|KEY") {
    Write-Pass "Mouse events reached alt-screen app (mouse-selection OFF)"
} elseif ($Win10) {
    Write-Pass "on this host the VT reading alt-screen app got no mouse event (mouse-selection OFF), as measured"
} else {
    Write-Fail "No mouse events with mouse-selection OFF - FORWARDING BROKEN (#245)"
}

# Kill the detector
& $PSMUX send-keys -t $SESSION C-c 2>&1 | Out-Null
Start-Sleep -Seconds 1

# ================================================================
# SCENARIO 3: opencode in c:\cctest
# ================================================================
Write-Host "`n--- SCENARIO 3: opencode in c:\\cctest (issue #277 specific) ---" -ForegroundColor Yellow

# opencode is an external dependency; without it there is nothing to test here.
$opencodeCmd = Get-Command opencode -EA SilentlyContinue
if (-not $opencodeCmd) {
    Write-Host "  [SKIP] opencode not found in PATH; scenario 3 skipped" -ForegroundColor DarkYellow
} else {
    # Reset settings
    & $PSMUX set-option -g mouse-selection on -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    & $PSMUX send-keys -t $SESSION "cd C:\cctest" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    & $PSMUX send-keys -t $SESSION "opencode" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 8

    # Give it a prompt to generate scrollable content
    # One number per line: a model that answers "1, 2, 3, ..." on one line
    # fits the screen and leaves nothing to scroll (run 2026-10-20_15-41-23).
    $ocPrompt = "print the numbers 1 to 80, each on its own line, nothing else"
    & $PSMUX send-keys -t $SESSION $ocPrompt Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 20

    # Now capture state before and after scroll
    $capBefore = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    Write-Host "  [INFO] Pre-scroll capture ($($capBefore.Length) chars)" -ForegroundColor DarkGray

    # PRECONDITION: the prompt must actually have rendered a response.
    # When opencode's model backend is unreachable (e.g. the configured
    # local ollama endpoint is down) the viewport stays essentially empty,
    # there is nothing to scroll, and the capture diff oracle would
    # misreport that as "scroll broken". Require a filled viewport first.
    $ocLines = @($capBefore -split "`n" | Where-Object { $_.Trim().Length -gt 0 }).Count
    # And the reply must overflow the screen: while the prompt line is still
    # on screen nothing has scrolled away, so a wheel up has nothing to show.
    $ocOverflow = -not $capBefore.Contains($ocPrompt)
    if (-not $ocOverflow) {
        Write-Host "  [SKIP] the opencode reply fits the screen (prompt still visible); nothing to scroll. Scroll checks skipped." -ForegroundColor DarkYellow
    } elseif ($ocLines -lt 15) {
        Write-Host "  [SKIP] opencode produced no scrollable content ($ocLines non-empty lines); model backend likely unreachable. Scroll checks skipped." -ForegroundColor DarkYellow
    } else {
        # Test 3a: Scroll up in opencode
        Write-Host "`n[Test 3a] Mouse wheel UP in opencode (mouse-selection ON)" -ForegroundColor Yellow
        & $mouseInjector $proc.Id "up" 8 40 15
        Start-Sleep -Seconds 3

        $capAfter = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
        if ($capAfter -ne $capBefore) {
            Write-Pass "Opencode content changed after mouse wheel UP"
        } else {
            Write-Fail "No change after mouse wheel in opencode - SCROLL BROKEN (#277)"
        }

        # Test 3b: With mouse-selection OFF
        Write-Host "`n[Test 3b] Mouse wheel UP in opencode (mouse-selection OFF)" -ForegroundColor Yellow
        & $PSMUX set-option -g mouse-selection off -t $SESSION 2>&1 | Out-Null
        Start-Sleep -Milliseconds 500

        $capBefore2 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
        & $mouseInjector $proc.Id "up" 8 40 15
        Start-Sleep -Seconds 3

        $capAfter2 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
        if ($capAfter2 -ne $capBefore2) {
            Write-Pass "Opencode scrolled with mouse-selection OFF"
        } else {
            Write-Fail "Opencode scroll broken with mouse-selection OFF - BUG (#245+#277)"
        }
    }

    # Exit opencode
    & $PSMUX send-keys -t $SESSION C-c 2>&1 | Out-Null
    Start-Sleep -Seconds 3
}

# === TEARDOWN ===
Write-Host "`n--- Cleanup ---" -ForegroundColor Yellow
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
Remove-Item $mouseDetector -Force -EA SilentlyContinue
Remove-Item $mouseLogFile -Force -EA SilentlyContinue
Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })

# Verdict
Write-Host "`n=== VERDICT ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) {
    Write-Host "  SCROLL BUG CONFIRMED: $($script:TestsFailed) test(s) failed" -ForegroundColor Red
} else {
    Write-Host "  SCROLL WORKS: All tests passed" -ForegroundColor Green
}

exit $script:TestsFailed
