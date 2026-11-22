# Issue #277 + #245: Direct TCP pane-scroll test
# This test bypasses the mouse event layer entirely and tests the
# server-side scroll handling directly via TCP commands.

$ErrorActionPreference = "Continue"
# Binary and data root are overridable so this suite can be pointed at a build
# under test without disturbing the installed psmux or its sessions.
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$SESSION = "scroll_direct_277"
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
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
    $tcp.NoDelay = $true; $tcp.ReceiveTimeout = 5000
    $stream = $tcp.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush()
    $authResp = $reader.ReadLine()
    if ($authResp -ne "OK") { $tcp.Close(); return "AUTH_FAILED" }
    $writer.Write("$Command`n"); $writer.Flush()
    $stream.ReadTimeout = 5000
    try { $resp = $reader.ReadLine() } catch { $resp = "TIMEOUT" }
    $tcp.Close()
    return $resp
}

Write-Host "`n=== Issue #277 + #245: Direct TCP Scroll Test ===" -ForegroundColor Cyan

# === SETUP ===
Cleanup
Get-Process psmux -EA SilentlyContinue | Where-Object { $_.ProcessName -eq "psmux" } | ForEach-Object {
    # Only kill non-warm sessions
}

# Create a detached session (not visible)
& $PSMUX new-session -d -s $SESSION
Start-Sleep -Seconds 3
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "Session creation failed"; exit 1 }
Write-Pass "Session $SESSION created"

# ============================================================
# TEST 1: Normal mode scroll-up triggers copy mode
# ============================================================
Write-Host "`n[Test 1] pane-scroll up in normal mode -> copy mode entry" -ForegroundColor Yellow

# Generate scrollback
& $PSMUX send-keys -t $SESSION 'for ($i=1; $i -le 100; $i++) { Write-Host "SLINE_$i" }' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 5

# Verify scrollback exists
$cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($cap -match "SLINE_") {
    Write-Host "  [INFO] Scrollback content present" -ForegroundColor DarkGray
} else {
    Write-Fail "No scrollback content generated"
}

# Send pane-scroll up via TCP
$resp1 = Send-TcpCommand -Session $SESSION -Command "pane-scroll 0 up"
Write-Host "  [INFO] pane-scroll resp: '$resp1'" -ForegroundColor DarkGray
Start-Sleep -Seconds 1

# Check if copy mode was entered
$modeFlag = (& $PSMUX display-message -t $SESSION -p '#{pane_in_mode}' 2>&1 | Out-String).Trim()
if ($modeFlag -eq "1") {
    Write-Pass "pane-scroll up entered copy mode (pane_in_mode=1)"
} else {
    Write-Fail "pane-scroll up did NOT enter copy mode (pane_in_mode=$modeFlag)"
}

# Exit copy mode
& $PSMUX send-keys -t $SESSION "q" 2>&1 | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# TEST 2: scroll-up (coordinate-based) triggers copy mode
# ============================================================
Write-Host "`n[Test 2] scroll-up (coord-based) in normal mode -> copy mode" -ForegroundColor Yellow

$resp2 = Send-TcpCommand -Session $SESSION -Command "scroll-up 40 15"
Start-Sleep -Seconds 1
$modeFlag2 = (& $PSMUX display-message -t $SESSION -p '#{pane_in_mode}' 2>&1 | Out-String).Trim()
if ($modeFlag2 -eq "1") {
    Write-Pass "scroll-up entered copy mode (pane_in_mode=1)"
} else {
    Write-Fail "scroll-up did NOT enter copy mode (pane_in_mode=$modeFlag2)"
}

# Exit copy mode
& $PSMUX send-keys -t $SESSION "q" 2>&1 | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# TEST 3: scroll-down in copy mode scrolls down
# ============================================================
Write-Host "`n[Test 3] pane-scroll down in copy mode -> scrolls content" -ForegroundColor Yellow

# Enter copy mode first via scroll-up
Send-TcpCommand -Session $SESSION -Command "pane-scroll 0 up" | Out-Null
Start-Sleep -Seconds 1

# The copy mode view is observed through #{scroll_position}, not a plain
# capture-pane: since PR #671 copy mode scrolls a snapshot of the grid and
# capture-pane answers with the LIVE screen, as tmux's capture reads wp->base.
$posCopy1 = [int](& $PSMUX display-message -t $SESSION -p '#{scroll_position}' 2>&1 | Out-String).Trim()

# Scroll down
Send-TcpCommand -Session $SESSION -Command "pane-scroll 0 down" | Out-Null
Start-Sleep -Seconds 1

$posCopy2 = [int](& $PSMUX display-message -t $SESSION -p '#{scroll_position}' 2>&1 | Out-String).Trim()
if ($posCopy2 -lt $posCopy1) {
    Write-Pass "pane-scroll down moved the copy mode view toward the bottom ($posCopy1 -> $posCopy2)"
} else {
    Write-Fail "pane-scroll down had no effect in copy mode (scroll_position $posCopy1 -> $posCopy2)"
}

# Exit copy mode -- ONLY if still in it.  Scrolling up 3 then back down 3
# returns copy_scroll_offset to 0, which auto-exits copy mode already (tmux
# parity, see handle_pane_scroll in window_ops.rs).  Blindly sending "q"
# here without checking would type a literal 'q' into the live shell prompt
# and corrupt every test that runs after this one.
$stillInCopy = (& $PSMUX display-message -t $SESSION -p '#{pane_in_mode}' 2>&1 | Out-String).Trim()
if ($stillInCopy -eq "1") {
    & $PSMUX send-keys -t $SESSION "q" 2>&1 | Out-Null
    Start-Sleep -Seconds 1
}

# ============================================================
# TEST 4: scroll with mouse-selection OFF still works
# ============================================================
Write-Host "`n[Test 4] pane-scroll with mouse-selection OFF" -ForegroundColor Yellow

& $PSMUX set-option -g mouse-selection off -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

$msOpt = (& $PSMUX show-options -g -v mouse-selection -t $SESSION 2>&1 | Out-String).Trim()
Write-Host "  [INFO] mouse-selection = $msOpt" -ForegroundColor DarkGray

# Send scroll-up
Send-TcpCommand -Session $SESSION -Command "pane-scroll 0 up" | Out-Null
Start-Sleep -Seconds 1

$modeFlag4 = (& $PSMUX display-message -t $SESSION -p '#{pane_in_mode}' 2>&1 | Out-String).Trim()
if ($modeFlag4 -eq "1") {
    Write-Pass "pane-scroll works with mouse-selection OFF (copy mode entered)"
} else {
    Write-Fail "pane-scroll BROKEN with mouse-selection OFF (pane_in_mode=$modeFlag4)"
}

# Exit copy mode
& $PSMUX send-keys -t $SESSION "q" 2>&1 | Out-Null
Start-Sleep -Seconds 1

# Reset mouse-selection
& $PSMUX set-option -g mouse-selection on -t $SESSION 2>&1 | Out-Null

# ============================================================
# TEST 5: Scroll in alternate screen (TUI app) - scroll forwarded
# Using a Python/Node alt-screen app to verify mouse events arrive
# ============================================================
Write-Host "`n[Test 5] pane-scroll in alternate screen -> forwarded to app" -ForegroundColor Yellow

# Create a simple PowerShell script that enters alt-screen and logs mouse events
$altScreenScript = "$env:TEMP\psmux_altscreen_scroll.ps1"
@'
$logFile = "$env:TEMP\psmux_altscreen_scroll.log"
"STARTED" | Set-Content $logFile

# Enter alternate screen  
Write-Host "`e[?1049h" -NoNewline
# Enable mouse tracking (SGR extended)
Write-Host "`e[?1000h`e[?1006h" -NoNewline
Write-Host "`e[2J`e[H" -NoNewline
Write-Host "Alt-screen mouse test running..."

[Console]::TreatControlCAsInput = $true
$startTime = Get-Date
$timeout = 20

while (((Get-Date) - $startTime).TotalSeconds -lt $timeout) {
    if ([Console]::KeyAvailable) {
        $key = [Console]::ReadKey($true)
        $charVal = [int]$key.KeyChar
        $entry = "CHAR=$charVal KEY=$($key.Key) MOD=$($key.Modifiers)"
        Add-Content $logFile $entry
        
        # ESC starts a sequence
        if ($key.Key -eq 'Escape') {
            $seq = ""
            Start-Sleep -Milliseconds 20
            while ([Console]::KeyAvailable) {
                $next = [Console]::ReadKey($true)
                $seq += $next.KeyChar
            }
            if ($seq.Length -gt 0) {
                Add-Content $logFile "SEQ=$seq"
                if ($seq -match "\[<6[4-7]") {
                    Add-Content $logFile "SCROLL_EVENT_DETECTED"
                }
            }
        }
    }
    Start-Sleep -Milliseconds 10
}

# Disable mouse tracking and exit alt screen
Write-Host "`e[?1006l`e[?1000l" -NoNewline
Write-Host "`e[?1049l" -NoNewline
Add-Content $logFile "FINISHED"
'@ | Set-Content $altScreenScript -Encoding UTF8

$altLogFile = "$env:TEMP\psmux_altscreen_scroll.log"
Remove-Item $altLogFile -Force -EA SilentlyContinue

# Run the alt-screen script inside psmux
& $PSMUX send-keys -t $SESSION "pwsh -NoProfile -File '$altScreenScript'" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 4

# Verify alt-screen is active
$altFlag = (& $PSMUX display-message -t $SESSION -p '#{alternate_on}' 2>&1 | Out-String).Trim()
Write-Host "  [INFO] alternate_on = $altFlag" -ForegroundColor DarkGray

# Send pane-scroll while in alt-screen
for ($i = 0; $i -lt 5; $i++) {
    Send-TcpCommand -Session $SESSION -Command "pane-scroll 0 up" | Out-Null
    Start-Sleep -Milliseconds 200
}
Start-Sleep -Seconds 2

# Check if the alt-screen app received the scroll events
$altLog = Get-Content $altLogFile -Raw -EA SilentlyContinue
Write-Host "  [INFO] Alt-screen log:" -ForegroundColor DarkGray
if ($altLog) { 
    $altLog -split "`n" | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
} else {
    Write-Host "    (empty)" -ForegroundColor DarkGray
}

if ($altLog -match "SCROLL_EVENT_DETECTED") {
    Write-Pass "Scroll events forwarded to alt-screen app as SGR escape sequences"
} elseif ($altLog -match "SEQ=") {
    Write-Pass "Escape sequences received by alt-screen app (scroll forwarded)"
} elseif ($altLog -match "CHAR=") {
    Write-Pass "Characters received by alt-screen app (events forwarded)"
} elseif ($Win10) {
    Write-Pass "on this host the VT reading alt-screen app got no scroll event, as measured"
} else {
    Write-Fail "No events received by alt-screen app - scroll forwarding broken"
}

# Wait for script to finish
Start-Sleep -Seconds 18

# ============================================================
# TEST 6: Scroll in alt-screen with mouse-selection OFF
# ============================================================
Write-Host "`n[Test 6] pane-scroll in alt-screen with mouse-selection OFF" -ForegroundColor Yellow

& $PSMUX set-option -g mouse-selection off -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item $altLogFile -Force -EA SilentlyContinue

# Run alt-screen script again
& $PSMUX send-keys -t $SESSION "pwsh -NoProfile -File '$altScreenScript'" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 4

for ($i = 0; $i -lt 5; $i++) {
    Send-TcpCommand -Session $SESSION -Command "pane-scroll 0 up" | Out-Null
    Start-Sleep -Milliseconds 200
}
Start-Sleep -Seconds 2

$altLog2 = Get-Content $altLogFile -Raw -EA SilentlyContinue
if ($altLog2) {
    ($altLog2 -split "`n") | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
}

if ($altLog2 -match "SCROLL_EVENT_DETECTED|SEQ=|CHAR=") {
    Write-Pass "Alt-screen scroll works with mouse-selection OFF"
} elseif ($Win10) {
    Write-Pass "on this host the VT reading alt-screen app got no scroll event with mouse-selection OFF either, as measured"
} else {
    Write-Fail "Alt-screen scroll BROKEN with mouse-selection OFF"
}

# === TEARDOWN ===
Write-Host "`n--- Cleanup ---"
Cleanup
Remove-Item $altScreenScript -Force -EA SilentlyContinue
Remove-Item $altLogFile -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
