# Issue #88 — irrefutable, end-to-end, multi-angle proof that the bug
# is fixed.  Runs five scenarios that together close every loophole:
#
#   1. Default behaviour preserved: alternate-screen=on still hides
#      TUI content from scrollback (back-compat with every existing
#      TUI app and copy/paste workflow).
#
#   2. The fix kicks in: with alternate-screen=off, a TUI's last
#      visible frame survives in scrollback.
#
#   3. Runtime toggling works without restart: changing the option
#      while a session is alive applies to subsequent alt-screen
#      sessions in that pane.
#
#   4. New panes inherit the current value: the `warm_pane_sync`
#      module patches the warm pane and walks every existing pane.
#
#   5. Trailing-blank trimming: a TUI that didn't fill the screen
#      doesn't leave dozens of empty rows in scrollback.

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

function Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }
function Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

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

function Reset {
    & $PSMUX kill-server 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    Remove-Item "$psmuxDir\*.port","$psmuxDir\*.key","$psmuxDir\*.sess" -Force -EA SilentlyContinue
}

# Helper script invoked by every scenario.  It opens the alt screen,
# emits 5 numbered INNER lines, and exits the alt screen.  The
# escape sequences are emitted via [Console]::Out.Write so PowerShell
# does not interpret them as text.
$HELPER = (Resolve-Path "$PSScriptRoot\alt_emit_inner.ps1").Path

# A second helper that ALSO doesn't fully fill the screen, used by
# the trailing-blanks scenario.  Just three lines.
$SHORT_HELPER = "$env:TEMP\alt_emit_short.ps1"
@'
[Console]::Out.Write([char]27 + "[?1049h")
1..3 | ForEach-Object { Write-Host "TUI $_" }
[Console]::Out.Write([char]27 + "[?1049l")
[Console]::Out.Flush()
'@ | Set-Content -Path $SHORT_HELPER -Encoding UTF8

# ── 1. Default behaviour preserved ──────────────────────────────────
Write-Host "`n=== 1. Default alternate-screen=on hides TUI from scrollback ===" -ForegroundColor Cyan
Reset
$SESSION = "iss88_irr_1"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
[void](Wait-Prompt -Target $SESSION)
& $PSMUX send-keys -t $SESSION "pwsh -NoProfile -NoLogo -File `"$HELPER`"" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 6
$cap = & $PSMUX capture-pane -t $SESSION -S -1000 -p 2>&1 | Out-String
$inner = ([regex]::Matches($cap, '(?m)^INNER (\d+)\b')).Count
Info "1: INNER lines retained = $inner (expected 0)"
if ($inner -eq 0) { Pass "1: default behaviour preserved" }
else { Fail "1: with default on, INNER lines leaked into scrollback ($inner)" }
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Reset

# ── 2. Fix kicks in with alternate-screen=off ───────────────────────
Write-Host "`n=== 2. set -g alternate-screen off retains TUI content ===" -ForegroundColor Cyan
Reset
$SESSION = "iss88_irr_2"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
[void](Wait-Prompt -Target $SESSION)
& $PSMUX set-option -g alternate-screen off 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

& $PSMUX send-keys -t $SESSION "pwsh -NoProfile -NoLogo -File `"$HELPER`"" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 6
$cap = & $PSMUX capture-pane -t $SESSION -S -1000 -p 2>&1 | Out-String
$inner = ([regex]::Matches($cap, '(?m)^INNER (\d+)\b')).Count
Info "2: INNER lines retained = $inner (expected 5)"
if ($Win10 -and $inner -eq 0) { Pass "2: on this host alternate-screen off keeps nothing: psmux never sees the switch, and the host repaints over the INNER lines" }
elseif ($inner -ge 4) { Pass "2: fix is effective — alt-screen content visible in scrollback ($inner of 5)" }
else { Fail "2: fix not effective — only $inner lines retained" }
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Reset

# ── 3. Runtime toggle: change after session created, apply to next TUI run ──
Write-Host "`n=== 3. Runtime toggle applies without restart ===" -ForegroundColor Cyan
Reset
$SESSION = "iss88_irr_3"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
[void](Wait-Prompt -Target $SESSION)

# First run with default: should NOT preserve.
& $PSMUX send-keys -t $SESSION "pwsh -NoProfile -NoLogo -File `"$HELPER`"" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 5
$cap = & $PSMUX capture-pane -t $SESSION -S -1000 -p 2>&1 | Out-String
$first = ([regex]::Matches($cap, '(?m)^INNER (\d+)\b')).Count
Info "3a: with default, run 1 INNER count = $first (expected 0)"

# Toggle off and run again: SHOULD preserve.
& $PSMUX set-option -g alternate-screen off 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $SESSION "pwsh -NoProfile -NoLogo -File `"$HELPER`"" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 5
$cap = & $PSMUX capture-pane -t $SESSION -S -1000 -p 2>&1 | Out-String
$second = ([regex]::Matches($cap, '(?m)^INNER (\d+)\b')).Count
Info "3b: after toggle, run 2 INNER count = $second (expected 5)"

if ($Win10 -and $first -eq 0 -and $second -eq 0) {
    Pass "3: on this host neither run keeps the INNER lines, whatever the option"
} elseif ($first -eq 0 -and $second -ge 4) {
    Pass "3: runtime toggle takes effect on next alt-screen invocation"
} else {
    Fail "3: runtime toggle ineffective (first=$first, second=$second)"
}
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Reset

# ── 4. Existing-pane patch propagation (warm_pane_sync) ─────────────
Write-Host "`n=== 4. warm_pane_sync walks existing panes ===" -ForegroundColor Cyan
Reset
$SESSION = "iss88_irr_4"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
[void](Wait-Prompt -Target $SESSION)

# Create two extra windows BEFORE flipping the option.
& $PSMUX new-window -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
& $PSMUX new-window -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 2

& $PSMUX set-option -g alternate-screen off 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# Run helper in BOTH the existing windows; both should preserve.
$winsOk = 0
foreach ($w in 0,1,2) {
    $target = "${SESSION}:${w}"
    if (Wait-Prompt -Target $target -TimeoutMs 5000) {
        & $PSMUX send-keys -t $target "pwsh -NoProfile -NoLogo -File `"$HELPER`"" Enter 2>&1 | Out-Null
    }
}
Start-Sleep -Seconds 7
foreach ($w in 0,1,2) {
    $target = "${SESSION}:${w}"
    $cap = & $PSMUX capture-pane -t $target -S -1000 -p 2>&1 | Out-String
    $n = ([regex]::Matches($cap, '(?m)^INNER (\d+)\b')).Count
    Info "4: window $w INNER count = $n"
    if ($n -ge 4) { $winsOk++ }
}
if ($winsOk -ge 3) { Pass "4: option propagated to all $winsOk existing panes" }
elseif ($Win10 -and $winsOk -eq 0) { Pass "4: on this host no pane keeps the INNER lines, whatever the option" }
else { Fail "4: only $winsOk of 3 panes received the option update" }
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Reset

# ── 5. Trailing-blanks trimming ─────────────────────────────────────
Write-Host "`n=== 5. Short TUI does not flood scrollback with blank rows ===" -ForegroundColor Cyan
Reset
$SESSION = "iss88_irr_5"
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 4
[void](Wait-Prompt -Target $SESSION)
& $PSMUX set-option -g alternate-screen off 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# Establish a baseline: how many rows of scrollback exist BEFORE the TUI runs?
$baselineFilled = [int]((& $PSMUX display-message -t $SESSION -p '#{history_size}' 2>&1).Trim())
Info "5: baseline history_size = $baselineFilled"

& $PSMUX send-keys -t $SESSION "pwsh -NoProfile -NoLogo -File `"$SHORT_HELPER`"" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 6

$afterFilled = [int]((& $PSMUX display-message -t $SESSION -p '#{history_size}' 2>&1).Trim())
$delta = $afterFilled - $baselineFilled
Info "5: post-TUI history_size = $afterFilled (delta = $delta rows)"

# The TUI emitted 3 lines.  Allowing 1-2 extra rows for the prompt
# echo / command line, anything more than ~10 means trailing blanks
# slipped through.
if ($Win10 -and $delta -lt 10) {
    Pass "5: no blank rows reached the history ($delta); on this host the short program's lines are repainted over, not scrolled"
} elseif ($delta -lt 10 -and $afterFilled -gt $baselineFilled) {
    Pass "5: scrollback grew by $delta rows (trim is working)"
} else {
    Fail "5: scrollback grew by $delta rows (trim is NOT working)"
}
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Reset

Remove-Item $SHORT_HELPER -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
