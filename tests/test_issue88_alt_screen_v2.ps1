# Issue #88 — definitive test of alt-screen scrollback behaviour.
# Uses [Console]::Out.Write to inject DEC private mode 1049 escapes
# reliably through PowerShell.

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

function Wait-AltState {
    param([string]$Target, [string]$Want, [int]$TimeoutMs = 5000)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        $v = (& $PSMUX display-message -t $Target -p '#{alternate_on}' 2>&1).Trim()
        if ($v -eq $Want) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

function Reset-Server {
    & $PSMUX kill-server 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    Remove-Item "$psmuxDir\*.port","$psmuxDir\*.key","$psmuxDir\*.sess" -Force -EA SilentlyContinue
}

Reset-Server
$SESSION = "iss88_v2"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
if (-not (Wait-Prompt -Target $SESSION)) {
    Write-Fail "shell not ready"
    exit 1
}
Write-Pass "shell ready"

# ── Step 1: 50 lines on MAIN screen ────────────────────────────────
Write-Host "`n=== Step 1: 50 main-screen lines ===" -ForegroundColor Cyan
& $PSMUX send-keys -t $SESSION '1..50 | ForEach-Object { "main $_" }' Enter 2>&1 | Out-Null
if (-not (Wait-Output -Target $SESSION -Marker "main 49")) {
    Write-Fail "main lines never appeared"
    exit 1
}
Start-Sleep -Seconds 1
$cap = & $PSMUX capture-pane -t $SESSION -S -1000 -p 2>&1 | Out-String
$mainBefore = ([regex]::Matches($cap, '(?m)^main (\d+)\b')).Count
Write-Info "Step 1: $mainBefore of 50 main lines in scrollback"

# ── Step 2: enter alt screen via [Console]::Out.Write ──────────────
Write-Host "`n=== Step 2: enter alt screen ===" -ForegroundColor Cyan
# This PowerShell expression writes the literal escape bytes to
# stdout, which the ConPTY then forwards to psmux's vt100 parser.
& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049h")' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1

if ($Win10) {
    $av = (& $PSMUX display-message -t $SESSION -p '#{alternate_on}' 2>&1).Trim()
    if ($av -eq "0") { Write-Pass "Step 2: this host kept ?1049h to itself, #{alternate_on} stays 0" }
    else { Write-Fail "Step 2: on Windows 10, #{alternate_on}=$av" }
} elseif (Wait-AltState -Target $SESSION -Want "1") {
    Write-Pass "Step 2: alt screen activated (#{alternate_on}=1)"
} else {
    $av = (& $PSMUX display-message -t $SESSION -p '#{alternate_on}' 2>&1).Trim()
    Write-Fail "Step 2: alt screen not activated, #{alternate_on}=$av"
    # Bail out
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    exit 1
}

# ── Step 3: 30 lines on ALT screen ────────────────────────────────
Write-Host "`n=== Step 3: 30 alt-screen lines ===" -ForegroundColor Cyan
& $PSMUX send-keys -t $SESSION '1..30 | ForEach-Object { "alt $_" }' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2

# Capture WHILE STILL IN ALT SCREEN.  Default capture reads visible
# (alt) grid; -S goes into scrollback (which for the alt grid is 0).
$capInAlt = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
$altInAlt = ([regex]::Matches($capInAlt, '(?m)^alt (\d+)\b')).Count
$mainInAlt = ([regex]::Matches($capInAlt, '(?m)^main (\d+)\b')).Count
Write-Info "Step 3: while in alt — visible capture sees alt=$altInAlt, main=$mainInAlt"

$capInAltDeep = & $PSMUX capture-pane -t $SESSION -S -1000 -p 2>&1 | Out-String
$altDeepInAlt = ([regex]::Matches($capInAltDeep, '(?m)^alt (\d+)\b')).Count
$mainDeepInAlt = ([regex]::Matches($capInAltDeep, '(?m)^main (\d+)\b')).Count
Write-Info "Step 3: while in alt — -S -1000 sees alt=$altDeepInAlt, main=$mainDeepInAlt"

# ── Step 4: exit alt screen, capture ────────────────────────────────
Write-Host "`n=== Step 4: exit alt screen, see what survived ===" -ForegroundColor Cyan
& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049l")' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
if (Wait-AltState -Target $SESSION -Want "0") {
    Write-Pass "Step 4: alt screen exited"
} else {
    Write-Fail "Step 4: still in alt screen"
}

$capPost = & $PSMUX capture-pane -t $SESSION -S -2000 -p 2>&1 | Out-String
$mainAfter = ([regex]::Matches($capPost, '(?m)^main (\d+)\b')).Count
$altAfter = ([regex]::Matches($capPost, '(?m)^alt (\d+)\b')).Count
Write-Info "Step 4: post-exit capture-pane -S -2000: main=$mainAfter, alt=$altAfter"

Write-Host "`n=== ANALYSIS ===" -ForegroundColor Yellow
if ($Win10) {
    # The host drew its alternate buffer as ordinary output: the alt lines
    # that scrolled off meanwhile stay, the rest were repainted over.
    if ($mainAfter -ge 48 -and $altAfter -lt 30) { Write-Pass "Windows 10: the main lines remain ($mainAfter); of the alt lines only the $altAfter that scrolled off while this host drew its alternate buffer stay" }
    else { Write-Fail "Windows 10: main=$mainAfter, alt=$altAfter" }
} elseif ($altAfter -eq 0 -and $mainAfter -ge 48) {
    Write-Host "  ROOT CAUSE: alt-screen content is not preserved in scrollback after exit." -ForegroundColor Yellow
    Write-Host "  This is correct vt100 semantics, but matches the user's #88 symptom:" -ForegroundColor Yellow
    Write-Host "  codex's TUI emits text into the alt screen; capture-pane and copy mode" -ForegroundColor Yellow
    Write-Host "  read from the MAIN scrollback so they cannot see codex output." -ForegroundColor Yellow
    Write-Host "  This is NOT the same root cause as #271 (history-limit cap)." -ForegroundColor Yellow
    Write-Pass "Hypothesis confirmed: alt-screen behaviour drives #88's symptoms"
} elseif ($altAfter -gt 0) {
    Write-Host "  alt-screen content WAS preserved after exit ($altAfter lines)." -ForegroundColor Yellow
    Write-Host "  Root cause is something else." -ForegroundColor Yellow
    Write-Fail "Alt-screen content unexpectedly preserved"
} else {
    Write-Host "  Unexpected state: main=$mainAfter, alt=$altAfter" -ForegroundColor Yellow
    Write-Fail "Inconclusive"
}

& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Reset-Server

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
