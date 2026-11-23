# Issue #88 — clean E2E that bypasses PSReadLine prompt-redraw noise.
#
# Strategy: send-keys runs `pwsh -NoProfile` with a single Command
# that emits exactly the escape sequences we want, then exits.  The
# parent PSReadLine'd shell never gets a chance to redraw between
# 1049h and 1049l, so we observe the raw effect of the parser's
# alt-screen handling.

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
        $cap = & $PSMUX capture-pane -t $Target -p 2>&1 | Out-String
        if ($cap -match $Pattern) { return $true }
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

function Reset {
    & $PSMUX kill-server 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    Remove-Item "$psmuxDir\*.port","$psmuxDir\*.key","$psmuxDir\*.sess" -Force -EA SilentlyContinue
}

# Path to the helper script that the inner pwsh runs.  It emits
# 1049h, 5 INNER lines, 1049l, then exits.  When alternate-screen=on
# the 5 INNER lines go to the alt grid (lost on exit).  When
# alternate-screen=off, they stay on the main grid and survive.
$HELPER = (Resolve-Path "$PSScriptRoot\alt_emit_inner.ps1").Path

# ── A: default (alternate-screen=on) — INNER content disappears ───
Write-Host "`n=== A: default on, INNER content should NOT survive ===" -ForegroundColor Cyan
Reset
$SESSION = "iss88_clean_a"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
[void](Wait-Prompt -Target $SESSION)

& $PSMUX send-keys -t $SESSION "pwsh -NoProfile -NoLogo -File `"$HELPER`"" Enter 2>&1 | Out-Null
# Wait for the inner pwsh to finish — when it exits, the parent's prompt returns.
Start-Sleep -Seconds 6
$capA = & $PSMUX capture-pane -t $SESSION -S -2000 -p 2>&1 | Out-String
$innerA = ([regex]::Matches($capA, '(?m)^INNER (\d+)\b')).Count
Write-Info "A (default on): INNER lines retained = $innerA"
if ($innerA -eq 0) {
    Write-Pass "A: default behaviour preserved — INNER lines vanished"
} else {
    Write-Fail "A: with default on, INNER lines unexpectedly retained ($innerA)"
}
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null

# ── B: alternate-screen=off — INNER content survives ──────────────
Write-Host "`n=== B: alternate-screen=off, INNER content SHOULD survive ===" -ForegroundColor Cyan
Reset
$SESSION = "iss88_clean_b"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
[void](Wait-Prompt -Target $SESSION)

& $PSMUX set-option -g alternate-screen off 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$opt = (& $PSMUX show-options -g -v alternate-screen 2>&1).Trim()
Write-Info "B: option = '$opt' (expected 'off')"

& $PSMUX send-keys -t $SESSION "pwsh -NoProfile -NoLogo -File `"$HELPER`"" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 6
$capB = & $PSMUX capture-pane -t $SESSION -S -2000 -p 2>&1 | Out-String
$innerB = ([regex]::Matches($capB, '(?m)^INNER (\d+)\b')).Count
Write-Info "B (off): INNER lines retained = $innerB"
if ($Win10 -and $innerB -eq 0) {
    Write-Pass "B: on this host alternate-screen off keeps nothing: psmux never sees the switch, and the host repaints its main buffer over the INNER lines"
} elseif ($innerB -ge 4) {
    Write-Pass "B: BUG FIX PROVEN — alt-screen disabled keeps content in scrollback ($innerB of 5)"
} else {
    Write-Fail "B: fix not effective — only $innerB of 5 INNER lines retained"
    # Dump tail for diagnosis
    $tail = if ($capB.Length -gt 600) { $capB.Substring($capB.Length - 600) } else { $capB }
    Write-Host "    Capture tail:`n$tail" -ForegroundColor DarkGray
}
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Reset

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
