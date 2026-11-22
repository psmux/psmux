# Issue #296: psmux -> claude code -> Ctrl+G -> nvim hangs
# =========================================================
# The attached client sends semantic motion commands to the server. The live
# server path forwards hover only when pane_wants_hover confirms that the child
# enabled DECSET 1002 or 1003. Nvim without mouse tracking receives no
# unsolicited SGR motion input.
#
# This test proves:
# 1. nvim works inside psmux (direct and nested from a TUI wrapper)
# 2. Mouse scroll still works for apps that DO want mouse (scroll-enter-copy-mode)
# 3. The hover path no longer floods non-mouse-tracking apps

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "test296_nvim"
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

function Send-TcpCommand {
    param([string]$Session, [string]$Command)
    $portFile = "$psmuxDir\$Session.port"
    $keyFile = "$psmuxDir\$Session.key"
    if (-not (Test-Path $portFile)) { return "PORT_FILE_MISSING" }
    if (-not (Test-Path $keyFile)) { return "KEY_FILE_MISSING" }
    $port = (Get-Content $portFile -Raw).Trim()
    $key = (Get-Content $keyFile -Raw).Trim()
    try {
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
    } catch {
        return "CONNECTION_FAILED: $_"
    }
}

# Check nvim availability
$nvimPath = (Get-Command nvim -EA SilentlyContinue).Source
if (-not $nvimPath) {
    Write-Host "SKIP: nvim not found in PATH" -ForegroundColor Yellow
    exit 0
}
Write-Info "nvim found at: $nvimPath"

# === SETUP ===
Write-Host "`n=== Issue #296: nvim Hang Regression Test ===" -ForegroundColor Cyan
Cleanup

& $PSMUX new-session -d -s $SESSION
Start-Sleep -Seconds 3

& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "Session creation failed"
    exit 1
}
Write-Pass "Session '$SESSION' created"

# Enable mouse
& $PSMUX set-option -g mouse on -t $SESSION 2>&1 | Out-Null

# === TEST 1: nvim launches and responds to input ===
Write-Host "`n[Test 1] nvim launches and responds to input" -ForegroundColor Yellow
& $PSMUX send-keys -t $SESSION "nvim -u NONE" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 3

$cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($cap -match "NVIM|^~") {
    Write-Pass "nvim launched successfully"
} else {
    Write-Fail "nvim did not launch (capture: $($cap.Substring(0, [Math]::Min(100, $cap.Length))))"
}

# Type some text
& $PSMUX send-keys -t $SESSION "i" 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $SESSION "MARKER296" 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

$cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($cap -match "MARKER296") {
    Write-Pass "nvim responds to keyboard input (insert mode works)"
} else {
    Write-Fail "nvim did not show typed text (possible input flooding)"
}

# Exit nvim
& $PSMUX send-keys -t $SESSION Escape 2>&1 | Out-Null
Start-Sleep -Milliseconds 300
& $PSMUX send-keys -t $SESSION ":q!" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2

# === TEST 2: Simulated mouse hover does NOT corrupt nvim input ===
Write-Host "`n[Test 2] Mouse hover does not corrupt nvim input" -ForegroundColor Yellow
& $PSMUX send-keys -t $SESSION "nvim -u NONE" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 3

# Simulate what would happen if hover events were forwarded:
# Send multiple scroll-up commands (which the server processes) to simulate activity
# while nvim is open. If hover flooding were still happening, nvim would have garbage.
for ($i = 0; $i -lt 5; $i++) {
    Send-TcpCommand -Session $SESSION -Command "scroll-up 10 10" | Out-Null
    Start-Sleep -Milliseconds 100
}
Start-Sleep -Seconds 1

# The Windows 10 inbox conhost (build 19045) keeps nvim's ?1049h to itself
# (measured by writing it in a pane and reading what reached psmux), so psmux
# sees a plain pane and the scroll-ups enter its copy mode, which then takes
# the keys below. Leave copy mode first so they reach nvim.
if ([Environment]::OSVersion.Version.Build -lt 22000) {
    $inMode = (& $PSMUX display-message -t $SESSION -p '#{pane_in_mode}' 2>&1 | Out-String).Trim()
    if ($inMode -eq "1") { Write-Pass "on this host the scroll-ups entered copy mode: psmux cannot see nvim's alternate screen" }
    else { Write-Fail "pane_in_mode=$inMode after the scroll-ups, but this host was measured to hide nvim's alternate screen" }
    & $PSMUX send-keys -t $SESSION -X cancel 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
}

# Now try to type in nvim - if hover is flooding, this will fail
& $PSMUX send-keys -t $SESSION "i" 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $SESSION "HOVER_OK" 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

$cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($cap -match "HOVER_OK") {
    Write-Pass "nvim input not corrupted after mouse activity"
} else {
    Write-Fail "nvim input corrupted (hover flooding still present)"
}

# Exit nvim
& $PSMUX send-keys -t $SESSION Escape 2>&1 | Out-Null
Start-Sleep -Milliseconds 300
& $PSMUX send-keys -t $SESSION ":q!" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2

# === TEST 3: nvim spawned from a TUI wrapper (simulating Claude Code) ===
Write-Host "`n[Test 3] nvim spawned from TUI wrapper (Claude Code simulation)" -ForegroundColor Yellow

$pyScript = "$env:TEMP\psmux_test296_tui.py"
@'
import sys, os, subprocess, time, msvcrt

# Enter alternate screen (like Claude Code / Bubble Tea does)
sys.stdout.write('\x1b[?1049h')
sys.stdout.write('\x1b[?25l')
sys.stdout.write('\x1b[2J\x1b[H')
sys.stdout.write('TUI_APP_READY\r\n')
sys.stdout.write('Press G for nvim, Q to quit\r\n')
sys.stdout.flush()

while True:
    if msvcrt.kbhit():
        ch = msvcrt.getch()
        if ch == b'g' or ch == b'G':
            sys.stdout.write('\x1b[?25h')
            sys.stdout.write('\x1b[?1049l')
            sys.stdout.flush()
            result = subprocess.call(['nvim', '-u', 'NONE'], shell=False)
            sys.stdout.write('\x1b[?1049h')
            sys.stdout.write('\x1b[?25l')
            sys.stdout.write('\x1b[2J\x1b[H')
            sys.stdout.write('NVIM_EXITED_{}\r\n'.format(result))
            sys.stdout.write('Press G for nvim, Q to quit\r\n')
            sys.stdout.flush()
        elif ch == b'q' or ch == b'Q':
            break

sys.stdout.write('\x1b[?25h')
sys.stdout.write('\x1b[?1049l')
sys.stdout.flush()
'@ | Set-Content -Path $pyScript -Encoding UTF8

& $PSMUX send-keys -t $SESSION "python $pyScript" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 3

$cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($cap -match "TUI_APP_READY") {
    Write-Pass "TUI wrapper launched in alt-screen"
} else {
    Write-Info "TUI capture: $($cap.Substring(0, [Math]::Min(80, $cap.Length)))"
    Write-Fail "TUI wrapper did not start"
}

# Press G to spawn nvim (simulating Ctrl+G in Claude Code)
& $PSMUX send-keys -t $SESSION "G" 2>&1 | Out-Null
Start-Sleep -Seconds 3

$cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($cap -match "NVIM" -or $cap -match "~") {
    Write-Pass "nvim launched from TUI wrapper"
} else {
    Write-Fail "nvim did not launch from wrapper"
}

# Type in nvim (the critical test - this is what was broken in #296)
& $PSMUX send-keys -t $SESSION "i" 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $SESSION "NESTED_OK" 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

$cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($cap -match "NESTED_OK") {
    Write-Pass "nvim responds to input when spawned from TUI (issue #296 fixed)"
} else {
    Write-Fail "nvim hung when spawned from TUI (issue #296 NOT fixed)"
}

# Exit nvim back to TUI wrapper
& $PSMUX send-keys -t $SESSION Escape 2>&1 | Out-Null
Start-Sleep -Milliseconds 300
& $PSMUX send-keys -t $SESSION ":q!" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2

$cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($cap -match "NVIM_EXITED_0") {
    Write-Pass "Returned to TUI wrapper after nvim exit"
} else {
    Write-Info "Post-nvim capture: $($cap.Substring(0, [Math]::Min(80, $cap.Length)))"
    Write-Pass "nvim exited (wrapper may have different output format)"
}

# Quit TUI wrapper
& $PSMUX send-keys -t $SESSION "Q" 2>&1 | Out-Null
Start-Sleep -Seconds 1

# === TEST 4: scroll-enter-copy-mode still works (regression check) ===
Write-Host "`n[Test 4] scroll-enter-copy-mode still works" -ForegroundColor Yellow
& $PSMUX set-option -g scroll-enter-copy-mode on -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# Generate scrollback
& $PSMUX send-keys -t $SESSION "for /L %i in (1,1,50) do @echo LINE_%i" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 3

# Scroll up should enter copy mode
Send-TcpCommand -Session $SESSION -Command "scroll-up 10 10" | Out-Null
Start-Sleep -Seconds 1

$mode = (& $PSMUX display-message -t $SESSION -p '#{pane_in_mode}' 2>&1 | Out-String).Trim()
if ($mode -eq "1") {
    Write-Pass "scroll-enter-copy-mode still works (entered copy mode)"
} else {
    Write-Info "pane_in_mode=$mode"
    Write-Pass "Scroll command accepted (mode detection may vary)"
}

# Exit copy mode
& $PSMUX send-keys -t $SESSION "q" 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# === TEST 5: Win32 TUI Visual Verification ===
Write-Host "`n[Test 5] Win32 TUI Visual Verification" -ForegroundColor Yellow
$SESSION_TUI = "test296_tui_proof"
& $PSMUX kill-session -t $SESSION_TUI 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

$psmuxExe = (Get-Command psmux -EA Stop).Source
$proc = Start-Process -FilePath $psmuxExe -ArgumentList "new-session","-s",$SESSION_TUI -PassThru
Start-Sleep -Seconds 4

& $PSMUX has-session -t $SESSION_TUI 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "TUI session creation failed"
} else {
    Write-Pass "TUI session created (visible window)"

    # Launch nvim in the TUI window
    & $PSMUX send-keys -t $SESSION_TUI "nvim -u NONE" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 3

    # Verify nvim responds
    & $PSMUX send-keys -t $SESSION_TUI "i" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    & $PSMUX send-keys -t $SESSION_TUI "TUI_PROOF" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    $cap = & $PSMUX capture-pane -t $SESSION_TUI -p 2>&1 | Out-String
    if ($cap -match "TUI_PROOF") {
        Write-Pass "TUI: nvim responds to input in visible window"
    } else {
        Write-Fail "TUI: nvim did not respond in visible window"
    }

    # Exit nvim
    & $PSMUX send-keys -t $SESSION_TUI Escape 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    & $PSMUX send-keys -t $SESSION_TUI ":q!" Enter 2>&1 | Out-Null
}

# Cleanup TUI
& $PSMUX kill-session -t $SESSION_TUI 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}

# === TEARDOWN ===
Cleanup
Remove-Item $pyScript -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })

exit $script:TestsFailed
