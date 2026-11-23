# Issue #88 — irrefutable proof of WHAT the bug is.
#
# Hypothesis: codex (and any TUI app) uses the alternate screen
# (DEC private mode 1049).  Alt-screen output does NOT land in the
# main grid's scrollback, so `capture-pane -S` cannot retrieve it.
# This is correct vt100/tmux semantics — but it explains every
# symptom in #88: mouse scroll inside codex doesn't show earlier
# conversation, copy-mode page-up shows nothing useful, etc.
#
# This script proves the hypothesis with raw escape sequences (no
# external TUI app needed).

# Windows 10 (a build before 22000, the inbox conhost): that host keeps a
# program's CSI ?1049h and ?1049l to itself. It draws its alternate buffer to
# psmux as ordinary output, a full repaint from the top, and on leaving
# repaints its main buffer over it (measured with PSMUX_PANE_RAW: no ?1049 at
# all). So psmux never enters the alternate screen there, #{alternate_on}
# stays 0, and alternate-screen off has nothing to drop. With PSMUX_CONPTY_DIR
# (OpenConsole) the switch reaches psmux as on Windows 11.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000 -and -not $env:PSMUX_CONPTY_DIR
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }

function Wait-Prompt {
    param([string]$Target, [int]$TimeoutMs = 15000, [string]$Pattern = "PS [A-Z]:\\")
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        try {
            $cap = & $PSMUX capture-pane -t $Target -p 2>&1 | Out-String
            if ($cap -match $Pattern) { return $true }
        } catch {}
        Start-Sleep -Milliseconds 200
    }
    return $false
}

function Wait-Output {
    param([string]$Target, [string]$Marker, [int]$TimeoutMs = 30000)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        $cap = & $PSMUX capture-pane -t $Target -p 2>&1 | Out-String
        if ($cap -match $Marker) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

function Reset-Server {
    & $PSMUX kill-server 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    Remove-Item "$psmuxDir\*.port","$psmuxDir\*.key","$psmuxDir\*.sess" -Force -EA SilentlyContinue
}

# Send raw escape sequences via the TCP `send-text` command.  send-text
# accepts a quoted UTF-8 string and writes it directly to the pane's
# PTY master, bypassing send-keys' VT-key translation.  This lets us
# emit the literal `ESC[?1049h` sequences without shell quoting hell.
function Send-Raw {
    param([string]$Session, [string]$RawText)
    $port = (Get-Content "$psmuxDir\$Session.port" -Raw).Trim()
    $key = (Get-Content "$psmuxDir\$Session.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush()
    $null = $reader.ReadLine()
    # Encode the raw bytes as a base64 hex-escape compatible string;
    # send-keys with -H takes hex pairs.  We will use send-keys -H
    # which is the canonical way to send arbitrary bytes.
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($RawText)
    $hex = ($bytes | ForEach-Object { "{0:x2}" -f $_ }) -join ' '
    $writer.Write("send-keys -t $Session -H $hex`n"); $writer.Flush()
    $stream.ReadTimeout = 5000
    try { $null = $reader.ReadLine() } catch {}
    $tcp.Close()
}

Reset-Server
$SESSION = "iss88_altproof"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
if (-not (Wait-Prompt -Target $SESSION)) {
    Write-Fail "shell not ready"
    exit 1
}
Write-Pass "shell ready"

# ── PART A: Establish a baseline of MAIN-screen content ───────────
Write-Host "`n=== PART A: 50 lines on MAIN screen, capture sees them ===" -ForegroundColor Cyan
& $PSMUX send-keys -t $SESSION '1..50 | ForEach-Object { "main $_" }' Enter 2>&1 | Out-Null
if (-not (Wait-Output -Target $SESSION -Marker "main 49" -TimeoutMs 30000)) {
    Write-Fail "main lines never appeared"
    exit 1
}
Start-Sleep -Seconds 1
$cap = & $PSMUX capture-pane -t $SESSION -S -1000 -p 2>&1 | Out-String
$mainCount = ([regex]::Matches($cap, '(?m)^main (\d+)\b')).Count
Write-Info "PART A: captured $mainCount of 50 main-screen lines"
if ($mainCount -ge 48) { Write-Pass "PART A: main scrollback works" }
else { Write-Fail "PART A: main scrollback broken (got $mainCount)" }

# ── PART B: Enter alt screen, write content there ────────────────
Write-Host "`n=== PART B: enter alt screen, write 30 lines ===" -ForegroundColor Cyan
# ESC[?1049h = enter alt screen.
#
# Root-cause note (2026-07-18 debugging pass): this used to embed a
# literal ESC byte in the send-keys STRING argument (typed character by
# character into the target pwsh's PSReadLine input line). PSReadLine
# treats a raw ESC arriving mid-keystroke-stream as an actual Escape
# key / ANSI input sequence, not as literal text to insert - it silently
# swallows the ESC and the "[?1049h" that followed it, so the typed
# command became "Write-Host ("")" and 1049h was NEVER actually sent to
# the terminal. That fully explained both this test's failures (flag
# reads 0, content "preserved" because alt mode was never truly
# entered). Fixed by building the escape sequence via a PowerShell
# expression evaluated at RUNTIME inside the target shell (matching the
# working technique in the sibling test_issue88_fix_proof.ps1), so
# PSReadLine only ever sees ordinary typed ASCII while the command is
# being entered; the real ESC byte is emitted by Console.Out.Write()
# after Enter has already been accepted.
& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049h")' Enter 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# Verify pane is in alt screen via dump-state's `alternate_on`
$altOn = (& $PSMUX display-message -t $SESSION -p '#{alternate_on}' 2>&1).Trim()
Write-Info "PART B: #{alternate_on} after enter = $altOn"
if ($altOn -eq "1") { Write-Pass "PART B: alt screen is active" }
elseif ($Win10 -and $altOn -eq "0") { Write-Pass "PART B: this host kept ?1049h to itself, #{alternate_on} stays 0" }
else { Write-Fail "PART B: alt screen not detected (#{alternate_on}=$altOn)" }

# Write 30 lines while in alt screen
& $PSMUX send-keys -t $SESSION '1..30 | ForEach-Object { "alt $_" }' Enter 2>&1 | Out-Null
if (Wait-Output -Target $SESSION -Marker "alt 29" -TimeoutMs 30000) {
    Start-Sleep -Seconds 1
    Write-Pass "PART B: alt-screen output rendered"
} else {
    Write-Fail "PART B: alt-screen output never rendered"
}

# Capture WHILE STILL IN ALT SCREEN.  Default capture-pane reads the
# currently-visible grid (alt) — we expect to see 'alt N' lines.
$capAlt = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
$altCount = ([regex]::Matches($capAlt, '(?m)^alt (\d+)\b')).Count
Write-Info "PART B: while in alt screen, default capture sees $altCount 'alt N' lines"

# Capture with -S -1000 WHILE IN ALT SCREEN.  Alt grid has no
# scrollback so this should not reveal more alt rows; what extra it
# returns (if anything) comes from the MAIN grid behind the alt.
$capAltDeep = & $PSMUX capture-pane -t $SESSION -S -1000 -p 2>&1 | Out-String
$altDeepCount = ([regex]::Matches($capAltDeep, '(?m)^alt (\d+)\b')).Count
$mainStillThere = ([regex]::Matches($capAltDeep, '(?m)^main (\d+)\b')).Count
Write-Info "PART B: -S -1000 while in alt: alt N=$altDeepCount, main N=$mainStillThere"

# ── PART C: Exit alt screen, see what survives ────────────────────
Write-Host "`n=== PART C: exit alt screen, scrollback content ===" -ForegroundColor Cyan
& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049l")' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
$altOff = (& $PSMUX display-message -t $SESSION -p '#{alternate_on}' 2>&1).Trim()
if ($altOff -eq "0") { Write-Pass "PART C: alt screen exited" }
else { Write-Fail "PART C: alt screen still active after exit ($altOff)" }

# Now capture deep scrollback.  Two outcomes:
#   - 'alt N' lines visible: alt-screen content was preserved into
#     main scrollback when alt mode exited (tmux-like option).
#   - 'alt N' lines absent, 'main N' lines still there: this is the
#     STANDARD vt100 behaviour and the actual cause of #88's symptom.
$capPost = & $PSMUX capture-pane -t $SESSION -S -2000 -p 2>&1 | Out-String
$postAltCount = ([regex]::Matches($capPost, '(?m)^alt (\d+)\b')).Count
$postMainCount = ([regex]::Matches($capPost, '(?m)^main (\d+)\b')).Count
Write-Info "PART C: post-exit -S -2000: alt N retained=$postAltCount, main N retained=$postMainCount"

if ($Win10) {
    # The host drew its alternate buffer as ordinary output, so the 'alt N'
    # lines that scrolled off meanwhile stay in the history; the rest were
    # repainted over by the main buffer.
    if ($postMainCount -ge 48 -and $postAltCount -lt 30) { Write-Pass "PART C: the main lines remain ($postMainCount of 50); of the alt lines only the $postAltCount that scrolled off while this host drew its alternate buffer stay" }
    else { Write-Fail "PART C: on Windows 10, alt=$postAltCount main=$postMainCount" }
} elseif ($postMainCount -ge 48 -and $postAltCount -eq 0) {
    Write-Pass "PART C: BUG ROOT CAUSE CONFIRMED — alt screen content (30 'alt N' lines) is NOT preserved in scrollback after exit. Only the 50 main-screen lines remain ($postMainCount of 50). This is correct vt100 behaviour but is exactly what users see as 'capture-pane misses my codex output' (#88)."
} elseif ($postAltCount -gt 0 -and $postMainCount -ge 48) {
    Write-Fail "PART C: alt screen content WAS preserved ($postAltCount 'alt N' lines) — not the root cause then"
} else {
    Write-Fail "PART C: unexpected state: alt=$postAltCount main=$postMainCount"
}

# ── PART D: same scenario with mouse mode (relevant to original
#            #88 'mouse scroll' complaint) ─────────────────────────
Write-Host "`n=== PART D: alt screen + mouse — does scroll work? ===" -ForegroundColor Cyan
& $PSMUX set-option -g mouse on -t $SESSION 2>&1 | Out-Null
$mouse = (& $PSMUX show-options -g -v mouse 2>&1).Trim()
Write-Info "PART D: mouse=$mouse"
# In alt-screen mode, scroll wheel events are FORWARDED to the
# child app (tmux convention).  Codex is supposed to handle them
# itself.  If codex does not implement scroll handling, the user
# sees 'nothing happens'.  This is not a psmux bug — it's a codex
# integration gap.  Proving this requires sending a mouse event
# to the pane and showing the app receives it; outside the scope
# of a CLI test (would need WriteConsoleInput injection).  We
# document the architecture instead.
Write-Info "PART D: psmux forwards scroll events to child apps when alternate-screen=on (default). If codex does not handle them, user sees no scroll. Use 'set-option -g alternate-screen off' to make psmux capture scroll into copy mode instead — but then codex's TUI breaks."

& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Reset-Server

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
