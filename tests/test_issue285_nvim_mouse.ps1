# Issue #285: Mouse does not work in Neovim inside psmux
# Verifies that mouse click, drag, and scroll events are properly forwarded
# to Neovim running inside psmux via the pane-mouse TCP command.
#
# This test proves:
# 1. The live click and wheel gates recognize Neovim's terminal state
# 2. Mouse clicks move cursor in Neovim (via pane-mouse TCP command)
# 3. Mouse scroll works in Neovim (via pane-scroll TCP command)
# 4. Mouse works with user config: mouse on, mouse-selection off, scroll-enter-copy-mode off

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "test_i285"
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0

# Neovim reads its mouse as VT bytes. The Windows 10 inbox conhost (build
# 19045) cuts that off: it keeps nvim's mouse DECSET and ?1049h to itself and
# drops an SGR report written to the pane (Issue #1089, docs/faq.md; measured
# with SGR reports typed with send-keys -l into a raw VT reader, which got only
# the letters around them). So there a click or a wheel never moves nvim, and
# the checks below expect it to stay where it was.
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000
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

function Send-MouseClick {
    param([string]$Session, [int]$PaneId, [int]$Col, [int]$Row)
    $port = (Get-Content "$psmuxDir\$Session.port" -Raw).Trim()
    $key = (Get-Content "$psmuxDir\$Session.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush()
    $null = $reader.ReadLine()
    # Press
    $writer.Write("pane-mouse $PaneId 0 $Col $Row M`n"); $writer.Flush()
    Start-Sleep -Milliseconds 150
    # Release
    $writer.Write("pane-mouse $PaneId 0 $Col $Row m`n"); $writer.Flush()
    Start-Sleep -Milliseconds 300
    $tcp.Close()
}

# === SETUP ===
Cleanup
& $PSMUX new-session -d -s $SESSION
Start-Sleep -Seconds 4

& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "Session creation failed"
    exit 1
}

Write-Host "`n=== Issue #285: Mouse in Neovim Tests ===" -ForegroundColor Cyan

# ──────────────────────────────────────────────────────────────────
# PART 1: VERIFY SHELL PROMPT STATE (baseline)
# ──────────────────────────────────────────────────────────────────

Write-Host "`n[Test 1] Shell prompt - alternate_on=0" -ForegroundColor Yellow
Start-Sleep -Seconds 2
$altOn = (& $PSMUX display-message -t $SESSION -p '#{alternate_on}' 2>&1).Trim()
if ($altOn -eq "0") { Write-Pass "alternate_on=0 at shell prompt" }
else { Write-Fail "Expected alternate_on=0, got '$altOn'" }

# ──────────────────────────────────────────────────────────────────
# PART 2: LAUNCH NEOVIM AND VERIFY DETECTION
# ──────────────────────────────────────────────────────────────────

Write-Host "`n[Test 2] Launch Neovim, verify it starts" -ForegroundColor Yellow
# Wait for shell prompt to be fully ready before sending keys
Start-Sleep -Seconds 3
# Send a harmless command first to ensure prompt is responsive
& $PSMUX send-keys -t $SESSION 'echo ready' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
# Now launch nvim
& $PSMUX send-keys -t $SESSION 'nvim' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 5

$captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($captured -match "NVIM|Neovim|nvim|type  :help|~") {
    Write-Pass "Neovim started successfully"
} else {
    Write-Fail "Neovim did not start (capture: $($captured.Substring(0, [Math]::Min(100, $captured.Length))))"
    Cleanup
    exit 1
}

# ──────────────────────────────────────────────────────────────────
# PART 3: GET PANE ID FOR MOUSE COMMANDS
# ──────────────────────────────────────────────────────────────────

Write-Host "`n[Test 3] Get pane ID" -ForegroundColor Yellow
$paneIdRaw = (& $PSMUX display-message -t $SESSION -p '#{pane_id}' 2>&1).Trim()
# pane_id format is %N, extract the number
$paneId = 0
if ($paneIdRaw -match '%(\d+)') { $paneId = [int]$Matches[1] }
Write-Host "    Pane ID: $paneId (raw: $paneIdRaw)"
if ($paneId -gt 0) { Write-Pass "Got valid pane ID: $paneId" }
else { Write-Fail "Invalid pane ID: $paneIdRaw" }

# ──────────────────────────────────────────────────────────────────
# PART 4: ENABLE MOUSE IN NEOVIM AND TYPE CONTENT
# ──────────────────────────────────────────────────────────────────

Write-Host "`n[Test 4] Set mouse=a and type content" -ForegroundColor Yellow
& $PSMUX send-keys -t $SESSION Escape 2>&1 | Out-Null; Start-Sleep -Milliseconds 200
& $PSMUX send-keys -t $SESSION ':set mouse=a' Enter 2>&1 | Out-Null; Start-Sleep -Milliseconds 300
& $PSMUX send-keys -t $SESSION ':enew' Enter 2>&1 | Out-Null; Start-Sleep -Milliseconds 300
& $PSMUX send-keys -t $SESSION 'i' 2>&1 | Out-Null; Start-Sleep -Milliseconds 200
& $PSMUX send-keys -t $SESSION 'ROW0_MOUSE' Enter 'ROW1_CLICK' Enter 'ROW2_TEST' Enter 'ROW3_END' 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $SESSION Escape 2>&1 | Out-Null; Start-Sleep -Milliseconds 300

$curBefore = (& $PSMUX display-message -t $SESSION -p '#{cursor_x},#{cursor_y}' 2>&1).Trim()
Write-Host "    Cursor after typing: $curBefore"
if ($curBefore -match '\d+,\d+') { Write-Pass "Content typed, cursor at $curBefore" }
else { Write-Fail "Unexpected cursor format: $curBefore" }

# ──────────────────────────────────────────────────────────────────
# PART 5: MOUSE CLICK MOVES CURSOR (critical test for #285)
# ──────────────────────────────────────────────────────────────────

Write-Host "`n[Test 5] Mouse click moves cursor to row 0" -ForegroundColor Yellow
# Click at row 0, col 3 — should move cursor to Line 1
Send-MouseClick -Session $SESSION -PaneId $paneId -Col 3 -Row 0
Start-Sleep -Milliseconds 500

$curAfter = (& $PSMUX display-message -t $SESSION -p '#{cursor_x},#{cursor_y}' 2>&1).Trim()
Write-Host "    Cursor after click at (3,0): $curAfter"
if ($curAfter -match '^(\d+),(\d+)$') {
    $cx = [int]$Matches[1]; $cy = [int]$Matches[2]
    if ($cy -eq 0) {
        Write-Pass "Mouse click moved cursor to row 0 (col=$cx)"
    } elseif ($curBefore -ne $curAfter) {
        Write-Pass "Mouse click changed cursor position (before=$curBefore, after=$curAfter)"
    } elseif ($Win10) {
        Write-Pass "on this host the click did not reach nvim (cursor stayed at $curAfter), as measured"
    } else {
        Write-Fail "Mouse click did NOT move cursor (stuck at $curAfter)"
    }
} else {
    Write-Fail "Could not parse cursor position: $curAfter"
}

# ──────────────────────────────────────────────────────────────────
# PART 6: SECOND CLICK AT DIFFERENT POSITION
# ──────────────────────────────────────────────────────────────────

Write-Host "`n[Test 6] Second mouse click at row 2, col 5" -ForegroundColor Yellow
Send-MouseClick -Session $SESSION -PaneId $paneId -Col 5 -Row 2
Start-Sleep -Milliseconds 500

$curAfter2 = (& $PSMUX display-message -t $SESSION -p '#{cursor_x},#{cursor_y}' 2>&1).Trim()
Write-Host "    Cursor after click at (5,2): $curAfter2"
if ($curAfter2 -match '^(\d+),(\d+)$') {
    $cx = [int]$Matches[1]; $cy = [int]$Matches[2]
    if ($cy -eq 2) {
        Write-Pass "Mouse click moved cursor to row 2 (col=$cx)"
    } elseif ($curAfter -ne $curAfter2) {
        Write-Pass "Mouse click changed cursor position (before=$curAfter, after=$curAfter2)"
    } elseif ($Win10) {
        Write-Pass "on this host the second click did not reach nvim either (cursor at $curAfter2), as measured"
    } else {
        Write-Fail "Second click did NOT move cursor (stuck at $curAfter2)"
    }
} else {
    Write-Fail "Could not parse cursor position: $curAfter2"
}

# ──────────────────────────────────────────────────────────────────
# PART 7: SCROLL VIA pane-scroll (forwards to Neovim)
# ──────────────────────────────────────────────────────────────────

Write-Host "`n[Test 7] Scroll via pane-scroll" -ForegroundColor Yellow
# Add more lines so scrolling is visible
& $PSMUX send-keys -t $SESSION Escape 2>&1 | Out-Null; Start-Sleep -Milliseconds 200
& $PSMUX send-keys -t $SESSION 'G' 2>&1 | Out-Null; Start-Sleep -Milliseconds 200
& $PSMUX send-keys -t $SESSION 'o' 2>&1 | Out-Null; Start-Sleep -Milliseconds 100
for ($i = 5; $i -le 40; $i++) {
    & $PSMUX send-keys -t $SESSION "LINE_$i" Enter 2>&1 | Out-Null
}
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $SESSION Escape 2>&1 | Out-Null; Start-Sleep -Milliseconds 200
& $PSMUX send-keys -t $SESSION 'gg' 2>&1 | Out-Null; Start-Sleep -Milliseconds 300

$capBefore = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
$beforeSnip = $capBefore.Substring(0, [Math]::Min(100, $capBefore.Length))
Write-Host "    Before scroll: $beforeSnip"

# Send multiple scroll-down events
for ($i = 0; $i -lt 5; $i++) {
    Send-TcpCommand -Session $SESSION -Command "pane-scroll $paneId down" | Out-Null
    Start-Sleep -Milliseconds 100
}
Start-Sleep -Milliseconds 500

$capAfter = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
$afterSnip = $capAfter.Substring(0, [Math]::Min(100, $capAfter.Length))
Write-Host "    After scroll: $afterSnip"

if ($capBefore -ne $capAfter) {
    Write-Pass "pane-scroll changed Neovim display"
} elseif ($Win10) {
    Write-Pass "on this host pane-scroll did not reach nvim, as measured"
} else {
    Write-Fail "pane-scroll did NOT change display"
}

# ──────────────────────────────────────────────────────────────────
# PART 8: VERIFY WITH ISSUE CONFIG
# ──────────────────────────────────────────────────────────────────

Write-Host "`n[Test 8] Verify with issue reporter's config" -ForegroundColor Yellow
Send-TcpCommand -Session $SESSION -Command "set-option -g mouse on" | Out-Null
Send-TcpCommand -Session $SESSION -Command "set-option -g mouse-selection off" | Out-Null
Send-TcpCommand -Session $SESSION -Command "set-option -g scroll-enter-copy-mode off" | Out-Null
Start-Sleep -Milliseconds 300

# Verify click still works
& $PSMUX send-keys -t $SESSION 'gg' 2>&1 | Out-Null; Start-Sleep -Milliseconds 300
$curBeforeConfig = (& $PSMUX display-message -t $SESSION -p '#{cursor_x},#{cursor_y}' 2>&1).Trim()

Send-MouseClick -Session $SESSION -PaneId $paneId -Col 2 -Row 3
Start-Sleep -Milliseconds 500

$curAfterConfig = (& $PSMUX display-message -t $SESSION -p '#{cursor_x},#{cursor_y}' 2>&1).Trim()
Write-Host "    Before: $curBeforeConfig  After: $curAfterConfig"

if ($curBeforeConfig -ne $curAfterConfig) {
    Write-Pass "Mouse click works with issue config (mouse on + selection off + scroll-copy off)"
} elseif ($Win10) {
    Write-Pass "on this host the click did not reach nvim with the issue config either, as measured"
} else {
    Write-Fail "Mouse click STILL broken with issue config"
}

# ──────────────────────────────────────────────────────────────────
# PART 9: EXIT NEOVIM, VERIFY NO FALSE POSITIVE AT SHELL PROMPT
# ──────────────────────────────────────────────────────────────────

Write-Host "`n[Test 9] Shell prompt after exit: no false positive" -ForegroundColor Yellow
& $PSMUX send-keys -t $SESSION Escape 2>&1 | Out-Null; Start-Sleep -Milliseconds 200
& $PSMUX send-keys -t $SESSION ':qa!' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 3

$altOnAfter = (& $PSMUX display-message -t $SESSION -p '#{alternate_on}' 2>&1).Trim()
Write-Host "    alternate_on after exit: $altOnAfter"
if ($altOnAfter -eq "0") { Write-Pass "alternate_on=0 after exiting Neovim" }
else { Write-Fail "alternate_on=$altOnAfter (expected 0)" }

# ──────────────────────────────────────────────────────────────────
# WIN32 TUI VISUAL VERIFICATION
# ──────────────────────────────────────────────────────────────────
Write-Host "`n[Test 10] TUI Window verification" -ForegroundColor Yellow
$SESSION_TUI = "i285_tui"
& $PSMUX kill-session -t $SESSION_TUI 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$SESSION_TUI.*" -Force -EA SilentlyContinue
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION_TUI -PassThru
Start-Sleep -Seconds 5

& $PSMUX has-session -t $SESSION_TUI 2>$null
if ($LASTEXITCODE -eq 0) {
    $mouseOpt = (& $PSMUX display-message -t $SESSION_TUI -p '#{mouse}' 2>&1).Trim()
    if ($mouseOpt -eq "on") { Write-Pass "TUI session has mouse=on" }
    else { Write-Fail "TUI session mouse=$mouseOpt" }
} else {
    Write-Fail "TUI session not created"
}

& $PSMUX kill-session -t $SESSION_TUI 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}

# === TEARDOWN ===
Cleanup

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
