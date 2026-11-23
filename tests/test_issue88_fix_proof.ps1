# Issue #88 — irrefutable proof that the alternate-screen toggle fixes
# the symptom.
#
# Before the fix: psmux always honoured DEC 47/1049, so any TUI app
# (codex, vim, less) wrote to the alt grid which has zero scrollback.
# `capture-pane -S` retrieved main scrollback and could not see TUI
# output.  Confirmed in tests/test_issue88_alt_screen_v2.ps1.
#
# After the fix: `set -g alternate-screen off` makes the parser drop
# DEC 47/1049 mode switches; TUI apps' output lands in main scrollback
# and is reachable by capture-pane and copy mode.
#
# This test proves three things:
#
#   1. With `alternate-screen off`, ESC[?1049h does NOT activate alt
#      mode (#{alternate_on}=0).
#   2. Output written between 1049h and 1049l survives in scrollback
#      after the bracket is exited.
#   3. With the default (`alternate-screen on`), behaviour is
#      unchanged — alt content is still ephemeral, full backwards
#      compatibility for TUI apps that rely on the standard semantics.

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

# ── SCENARIO A: alternate-screen ON (default) — old behaviour ────────
Write-Host "`n=== SCENARIO A: default alternate-screen=on (alt content stays ephemeral) ===" -ForegroundColor Cyan
Reset-Server
$SESSION = "iss88_fix_on"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
if (-not (Wait-Prompt -Target $SESSION)) { Write-Fail "A: shell not ready"; exit 1 }
Write-Pass "A: shell ready"

$opt = (& $PSMUX show-options -g -v alternate-screen 2>&1).Trim()
Write-Info "A: alternate-screen option = '$opt' (expected 'on' default)"
if ($opt -eq "on") { Write-Pass "A: option default is on" } else { Write-Fail "A: default was '$opt'" }

# Emit 30 main, enter alt, emit 20 alt, exit alt, capture
& $PSMUX send-keys -t $SESSION '1..30 | ForEach-Object { "main $_" }' Enter 2>&1 | Out-Null
[void](Wait-Output -Target $SESSION -Marker "main 29")
Start-Sleep -Seconds 1
& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049h")' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
$altOn = (& $PSMUX display-message -t $SESSION -p '#{alternate_on}' 2>&1).Trim()
Write-Info "A: after 1049h, #{alternate_on}=$altOn"
if ($altOn -eq "1") { Write-Pass "A: alt mode honoured (default)" }
elseif ($Win10 -and $altOn -eq "0") { Write-Pass "A: this host kept ?1049h to itself, #{alternate_on} stays 0" }
else { Write-Fail "A: alt mode NOT honoured (default), got $altOn" }

& $PSMUX send-keys -t $SESSION '1..20 | ForEach-Object { "alt $_" }' Enter 2>&1 | Out-Null
[void](Wait-Output -Target $SESSION -Marker "alt 19")
Start-Sleep -Seconds 1
& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049l")' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1

$capA = & $PSMUX capture-pane -t $SESSION -S -2000 -p 2>&1 | Out-String
$mainA = ([regex]::Matches($capA, '(?m)^main (\d+)\b')).Count
$altA = ([regex]::Matches($capA, '(?m)^alt (\d+)\b')).Count
Write-Info "A: scrollback after exit: main=$mainA, alt=$altA"
if ($mainA -ge 28 -and $altA -eq 0) {
    Write-Pass "A: default behaviour preserved — alt content NOT in scrollback"
} else {
    Write-Fail "A: unexpected default state main=$mainA alt=$altA"
}

& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Reset-Server

# ── SCENARIO B: alternate-screen OFF — the fix ───────────────────────
Write-Host "`n=== SCENARIO B: alternate-screen=off (alt content lands in scrollback) ===" -ForegroundColor Cyan
Reset-Server
$SESSION = "iss88_fix_off"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
if (-not (Wait-Prompt -Target $SESSION)) { Write-Fail "B: shell not ready"; exit 1 }
Write-Pass "B: shell ready"

# Disable alt-screen
& $PSMUX set-option -g alternate-screen off 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$opt = (& $PSMUX show-options -g -v alternate-screen 2>&1).Trim()
Write-Info "B: alternate-screen option after set = '$opt'"
if ($opt -eq "off") { Write-Pass "B: option recorded as off" } else { Write-Fail "B: option not off, got '$opt'" }

# Emit 30 main, attempt alt, emit 20 'alt' lines, exit attempt, capture
& $PSMUX send-keys -t $SESSION '1..30 | ForEach-Object { "main $_" }' Enter 2>&1 | Out-Null
[void](Wait-Output -Target $SESSION -Marker "main 29")
Start-Sleep -Seconds 1

& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049h")' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
$altOn = (& $PSMUX display-message -t $SESSION -p '#{alternate_on}' 2>&1).Trim()
Write-Info "B: after 1049h with toggle off, #{alternate_on}=$altOn"
if ($altOn -eq "0") {
    Write-Pass "B: 1049h was DROPPED (parser stayed on main grid)"
} else {
    Write-Fail "B: 1049h still activated alt mode despite alternate-screen off"
}

& $PSMUX send-keys -t $SESSION '1..20 | ForEach-Object { "alt $_" }' Enter 2>&1 | Out-Null
[void](Wait-Output -Target $SESSION -Marker "alt 19")
Start-Sleep -Seconds 1
& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049l")' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1

$capB = & $PSMUX capture-pane -t $SESSION -S -2000 -p 2>&1 | Out-String
$mainB = ([regex]::Matches($capB, '(?m)^main (\d+)\b')).Count
$altB = ([regex]::Matches($capB, '(?m)^alt (\d+)\b')).Count
Write-Info "B: scrollback: main=$mainB, alt=$altB"
if ($Win10 -and $mainB -ge 28 -and $altB -lt 18) {
    Write-Pass "B: on this host alternate-screen off keeps no alt lines ($altB): psmux never sees the switch, and the host repaints its main buffer over them"
} elseif ($mainB -ge 28 -and $altB -ge 18) {
    Write-Pass "B: BUG FIX PROVEN — both main ($mainB) and alt ($altB) lines retained in scrollback"
} else {
    Write-Fail "B: fix did not work as expected (main=$mainB, alt=$altB)"
}

& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Reset-Server

# ── SCENARIO C: runtime toggle on existing pane ──────────────────────
# Verify warm_pane_sync's apply_patch_to_existing_panes really does
# walk live panes — set the option AFTER content has already been
# written, then ensure subsequent alt sequences in the SAME session
# are dropped.
Write-Host "`n=== SCENARIO C: runtime toggle takes effect on existing pane ===" -ForegroundColor Cyan
Reset-Server
$SESSION = "iss88_runtime"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
if (-not (Wait-Prompt -Target $SESSION)) { Write-Fail "C: shell not ready"; exit 1 }
Write-Pass "C: shell ready"

# Verify default (on) — alt activates
& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049h")' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
$beforeToggle = (& $PSMUX display-message -t $SESSION -p '#{alternate_on}' 2>&1).Trim()
& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049l")' Enter 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Write-Info "C: with default on, 1049h activated alt? -> $beforeToggle"
if ($beforeToggle -eq "1") { Write-Pass "C: baseline confirmed (default on)" }

# Now toggle off at runtime
& $PSMUX set-option -g alternate-screen off 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# Try 1049h again — should NO LONGER activate alt mode
& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049h")' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
$afterToggle = (& $PSMUX display-message -t $SESSION -p '#{alternate_on}' 2>&1).Trim()
Write-Info "C: after runtime set-option alternate-screen off, 1049h activates? -> $afterToggle"
if ($afterToggle -eq "0") {
    Write-Pass "C: runtime toggle reached the existing pane's parser"
} else {
    Write-Fail "C: existing pane did NOT pick up the toggle (still $afterToggle)"
}

# Toggle back on, verify symmetry
& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049l")' Enter 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
& $PSMUX set-option -g alternate-screen on 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049h")' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
$afterReset = (& $PSMUX display-message -t $SESSION -p '#{alternate_on}' 2>&1).Trim()
Write-Info "C: after toggle back on, 1049h activates? -> $afterReset"
if ($afterReset -eq "1") {
    Write-Pass "C: toggle is symmetric (off/on works in both directions)"
} elseif ($Win10 -and $afterReset -eq "0") {
    Write-Pass "C: this host kept ?1049h to itself, #{alternate_on} stays 0 with the option back on"
} else {
    Write-Fail "C: re-enabling alternate-screen did not restore behaviour"
}

& $PSMUX send-keys -t $SESSION '[Console]::Out.Write([char]27 + "[?1049l")' Enter 2>&1 | Out-Null
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Reset-Server

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
