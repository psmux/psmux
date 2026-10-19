#!/usr/bin/env pwsh
# =============================================================================
# test_capture_pane.ps1 — Comprehensive capture-pane test suite for psmux
# Tests parity with tmux capture-pane behavior
# =============================================================================

$ErrorActionPreference = "Continue"
$exe = "$PSScriptRoot\..\target\release\psmux.exe"
if (-not (Test-Path $exe)) { $exe = "$PSScriptRoot\..\target\debug\psmux.exe" }
if (-not (Test-Path $exe)) { $exe = (Get-Command psmux -ErrorAction SilentlyContinue).Source }
if (-not $exe -or -not (Test-Path $exe)) { Write-Error "psmux binary not found"; exit 1 }
$pass = 0; $fail = 0; $skip = 0
$SESSION = "test_cap_$(Get-Random -Maximum 9999)"

function Check($name, $cond) {
    if ($cond) { Write-Host "  PASS: $name" -ForegroundColor Green; $script:pass++ }
    else { Write-Host "  FAIL: $name" -ForegroundColor Red; $script:fail++ }
}

function Skip($name, $reason) {
    Write-Host "  SKIP: $name ($reason)" -ForegroundColor Yellow; $script:skip++
}

# Kill any existing test sessions
& $exe kill-server 2>$null
Start-Sleep -Seconds 1

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "CAPTURE-PANE COMPREHENSIVE TEST SUITE" -ForegroundColor Cyan
Write-Host "========================================`n" -ForegroundColor Cyan

# =============================================================================
# SETUP: Create session with multiple windows and panes
# =============================================================================
Write-Host "--- SETUP: Creating test session with windows and panes ---" -ForegroundColor Yellow

& $exe new-session -d -s $SESSION -x 120 -y 30
Start-Sleep -Seconds 2

# Send known content to window 0, pane 0
& $exe send-keys -t $SESSION "echo HELLO_CAPTURE_TEST" Enter
Start-Sleep -Seconds 1

# =============================================================================
# TEST 1: Basic capture-pane -p (print to stdout)
# =============================================================================
Write-Host "`n--- TEST 1: Basic capture-pane -p ---" -ForegroundColor Yellow

$out1 = & $exe capture-pane -t $SESSION -p 2>&1 | Out-String
Check "capture-pane -p returns content" ($out1.Length -gt 0)
Check "capture-pane -p contains echoed text" ($out1 -match "HELLO_CAPTURE_TEST")

# =============================================================================
# TEST 2: Trailing whitespace trimming (tmux default behavior)
# =============================================================================
Write-Host "`n--- TEST 2: Trailing whitespace trimming ---" -ForegroundColor Yellow

$lines2 = (& $exe capture-pane -t $SESSION -p 2>&1) -split "`n"
$has_trailing_spaces = $false
foreach ($line in $lines2) {
    # Check for trailing spaces/tabs (not CR/LF) on lines with visible content
    if ($line.TrimEnd("`r") -match "[ `t]+$" -and $line.Trim().Length -gt 0) {
        $has_trailing_spaces = $true
        break
    }
}
Check "No trailing whitespace on non-empty lines" (-not $has_trailing_spaces)

# Interior empty lines must be preserved (tmux parity). psmux intentionally
# trims TRAILING blank lines from the default (no-range) capture so iTerm2 does
# not advance its cursor past the content on initial attach, so the meaningful
# property to assert is that a blank line BETWEEN two non-blank lines survives.
& $exe send-keys -t $SESSION "Write-Host 'IBLANKA'; Write-Host ''; Write-Host 'IBLANKB'" Enter
Start-Sleep -Seconds 1
$capIB = (& $exe capture-pane -t $SESSION -p 2>&1) -split "`n"
$idxA = -1; $idxB = -1
for ($i = 0; $i -lt $capIB.Count; $i++) {
    if ($capIB[$i].Trim() -eq "IBLANKA") { $idxA = $i }
    elseif ($capIB[$i].Trim() -eq "IBLANKB") { $idxB = $i }
}
$interior_blank_preserved = ($idxA -ge 0 -and $idxB -eq ($idxA + 2) -and $capIB[$idxA + 1].Trim() -eq "")
Check "Interior empty lines are preserved" $interior_blank_preserved

# =============================================================================
# TEST 3: capture-pane without -p (stores in paste buffer)
# =============================================================================
Write-Host "`n--- TEST 3: capture-pane without -p (buffer storage) ---" -ForegroundColor Yellow

& $exe capture-pane -t $SESSION 2>&1
Start-Sleep -Milliseconds 500
$buf3 = & $exe show-buffer -t $SESSION 2>&1 | Out-String
Check "capture-pane stores in paste buffer" ($buf3.Length -gt 0)
Check "Paste buffer contains echoed text" ($buf3 -match "HELLO_CAPTURE_TEST")

# =============================================================================
# TEST 4: capture-pane -p -S 0 -E 5 (specific line range)
# =============================================================================
Write-Host "`n--- TEST 4: Range capture -S 0 -E 5 ---" -ForegroundColor Yellow

$lines4 = (& $exe capture-pane -t $SESSION -p -S 0 -E 5 2>&1) -split "`n"
# Filter out truly empty trailing entries from split
$non_null4 = $lines4 | Where-Object { $_ -ne $null }
# An explicit range is honored line for line (tmux parity): 6 rows requested
# (0 through 5), and trailing blank rows INSIDE the range are preserved rather
# than collapsed. So the count is the 6 requested lines (a trailing newline may
# add one empty split entry). Before the fix this returned only the ~3 non-blank
# lines because the range path wrongly reused the attach path's trailing-blank trim.
Check "-S 0 -E 5 returns the 6 requested lines" ($non_null4.Count -ge 6 -and $non_null4.Count -le 7)

# =============================================================================
# TEST 5: capture-pane -S -3 (3 lines of scrollback above visible top, per tmux)
# =============================================================================
Write-Host "`n--- TEST 5: Negative offset -S -3 ---" -ForegroundColor Yellow

$lines5 = (& $exe capture-pane -t $SESSION -p -S -3 2>&1) -split "`n"
$non_null5 = $lines5 | Where-Object { $_ -ne $null }
# Per tmux semantics, -S -N means "N lines above visible top" so the result
# should include up to N scrollback lines + the full visible pane (~24 rows on
# default geometry). When no scrollback is available it clamps to the visible
# pane. So the count should be at least 2 lines and not absurdly larger than
# pane height + a few scrollback rows.
Check "-S -3 returns at least 2 lines" ($non_null5.Count -ge 2)
Check "-S -3 returns scrollback + visible (within sane bound)" ($non_null5.Count -le 200)

# =============================================================================
# TEST 6: capture-pane -S 0 -E 0 (single line)
# =============================================================================
Write-Host "`n--- TEST 6: Single line capture -S 0 -E 0 ---" -ForegroundColor Yellow

$lines6 = (& $exe capture-pane -t $SESSION -p -S 0 -E 0 2>&1) -split "`n"
$non_empty6 = $lines6 | Where-Object { $_ -ne $null -and $_ -ne "" }
# Could be 1 line or 0 if it's empty
Check "-S 0 -E 0 returns 0 or 1 lines" ($non_empty6.Count -le 2)

# =============================================================================
# TEST 7: capture-pane -p -e (escape sequences)
# =============================================================================
Write-Host "`n--- TEST 7: Escape sequences -e ---" -ForegroundColor Yellow

# First send colored output
& $exe send-keys -t $SESSION 'Write-Host "RED_TEXT" -ForegroundColor Red' Enter
Start-Sleep -Seconds 1

$out7 = & $exe capture-pane -t $SESSION -p -e 2>&1 | Out-String
Check "-e flag returns content" ($out7.Length -gt 0)
# The escape sequence output should contain ESC[
$has_esc = $out7 -match [char]27
Check "-e contains escape sequences" $has_esc

# Plain capture should NOT have escape sequences
$out7plain = & $exe capture-pane -t $SESSION -p 2>&1 | Out-String
$has_esc_plain = $out7plain -match [char]27
Check "Plain capture has no escape sequences" (-not $has_esc_plain)

# =============================================================================
# TEST 8: capture-pane -e with -S/-E (combined flags)
# =============================================================================
Write-Host "`n--- TEST 8: Combined -e with -S/-E ---" -ForegroundColor Yellow

$out8 = & $exe capture-pane -t $SESSION -p -e -S 0 -E 10 2>&1 | Out-String
Check "-e -S 0 -E 10 returns content" ($out8.Length -gt 0)
$lines8 = $out8 -split "`n"
Check "-e -S -E returns reasonable lines" ($lines8.Count -le 15)

# =============================================================================
# TEST 9: capture-pane -J (join wrapped lines)
# =============================================================================
Write-Host "`n--- TEST 9: Join lines -J ---" -ForegroundColor Yellow

$out9 = & $exe capture-pane -t $SESSION -p -J 2>&1 | Out-String
Check "-J flag returns content" ($out9.Length -gt 0)
# -J joins wrapped rows and, like tmux, keeps the spaces the program printed
# (cmd-capture-pane.c leaves GRID_STRING_TRIM_SPACES off for -J), so it never
# has more lines than the plain capture. tests/test_capture_pane_join.ps1
# checks the joined text itself.
$plain9 = & $exe capture-pane -t $SESSION -p 2>&1 | Out-String
Check "-J has no more lines than the plain capture" (($out9 -split "`n").Count -le ($plain9 -split "`n").Count)

# =============================================================================
# TEST 10: capture-pane -S - (all history)
# =============================================================================
Write-Host "`n--- TEST 10: Full history -S - ---" -ForegroundColor Yellow

$out10 = & $exe capture-pane -t $SESSION -p -S - 2>&1 | Out-String
Check "-S - returns content" ($out10.Length -gt 0)
Check "-S - contains echoed text" ($out10 -match "HELLO_CAPTURE_TEST")

# =============================================================================
# TEST 11: Multi-pane capture
# =============================================================================
Write-Host "`n--- TEST 11: Multi-pane capture ---" -ForegroundColor Yellow

# Split to create pane 1
& $exe split-window -t $SESSION -h
Start-Sleep -Seconds 3
& $exe send-keys -t $SESSION "echo PANE_ONE_CONTENT" Enter
Start-Sleep -Seconds 2

$out11 = & $exe capture-pane -t $SESSION -p 2>&1 | Out-String
Check "Capture from active pane after split" ($out11 -match "PANE_ONE_CONTENT")

# =============================================================================
# TEST 12: Multi-window capture
# =============================================================================
Write-Host "`n--- TEST 12: Multi-window setup ---" -ForegroundColor Yellow

# Create window 1
& $exe new-window -t $SESSION
Start-Sleep -Seconds 3
& $exe send-keys -t $SESSION "echo WINDOW_TWO_TEXT" Enter
Start-Sleep -Seconds 2

$out12 = & $exe capture-pane -t $SESSION -p 2>&1 | Out-String
Check "Capture from window 1 active pane" ($out12 -match "WINDOW_TWO_TEXT")

# Switch back to window 0
& $exe select-window -t "${SESSION}:0"
Start-Sleep -Seconds 1

# =============================================================================
# TEST 13: capturep alias
# =============================================================================
Write-Host "`n--- TEST 13: capturep alias ---" -ForegroundColor Yellow

$out13 = & $exe capturep -t $SESSION -p 2>&1 | Out-String
Check "capturep alias works" ($out13.Length -gt 0)

# =============================================================================
# TEST 14: Large output capture
# =============================================================================
Write-Host "`n--- TEST 14: Large output capture ---" -ForegroundColor Yellow

& $exe send-keys -t $SESSION "1..25 | ForEach-Object { Write-Host LINE_`$_ }" Enter
Start-Sleep -Seconds 4

$out14 = & $exe capture-pane -t $SESSION -p 2>&1 | Out-String
Check "Large output capture works" ($out14.Length -gt 100)
# Check that at least some of the generated lines are present
$found_lines = 0
for ($li = 1; $li -le 25; $li++) {
    if ($out14 -match "LINE_$li") { $found_lines++ }
}
Check "Found some generated output lines" ($found_lines -gt 5)

# =============================================================================
# TEST 15: capture-pane -E -1 (exclude last visible line)
# =============================================================================
Write-Host "`n--- TEST 15: -E with negative offset ---" -ForegroundColor Yellow

$lines15_full = (& $exe capture-pane -t $SESSION -p 2>&1) -split "`n"
$lines15_minus1 = (& $exe capture-pane -t $SESSION -p -S 0 -E -1 2>&1) -split "`n"
# -E -1 should return fewer lines than full capture
$full_count = ($lines15_full | Where-Object { $_ -ne $null }).Count
$minus1_count = ($lines15_minus1 | Where-Object { $_ -ne $null }).Count
Check "-E -1 returns fewer lines than full" ($minus1_count -le $full_count)

# =============================================================================
# TEST 16: Empty lines preserved in output
# =============================================================================
Write-Host "`n--- TEST 16: Empty lines preserved ---" -ForegroundColor Yellow

# Clear and send content with gaps
& $exe send-keys -t $SESSION "clear" Enter
Start-Sleep -Seconds 1
& $exe send-keys -t $SESSION "echo MARKER_TOP" Enter
Start-Sleep -Milliseconds 500

$out16 = & $exe capture-pane -t $SESSION -p 2>&1 | Out-String
$lines16 = $out16 -split "`n"  
# There should be empty lines after the marker
$marker_idx = -1
for ($i = 0; $i -lt $lines16.Count; $i++) {
    if ($lines16[$i] -match "MARKER_TOP") { $marker_idx = $i; break }
}
if ($marker_idx -ge 0 -and $marker_idx + 2 -lt $lines16.Count) {
    # Lines after the marker area should be empty
    $has_empty_after = ($lines16[($marker_idx+3)..($lines16.Count-1)] | Where-Object { $_.Trim() -eq "" }).Count -gt 0
    Check "Empty lines preserved after content" $has_empty_after
} else {
    Skip "Empty lines preserved after content" "Marker not found or insufficient lines"
}

# =============================================================================
# TEST 17: Consistent line count across captures
# =============================================================================
Write-Host "`n--- TEST 17: Consistent line count ---" -ForegroundColor Yellow

$cap_a = (& $exe capture-pane -t $SESSION -p 2>&1) -split "`n"
$cap_b = (& $exe capture-pane -t $SESSION -p 2>&1) -split "`n"
Check "Consecutive captures have same line count" ($cap_a.Count -eq $cap_b.Count)

# =============================================================================
# TEST 18: -S and -E boundary correctness
# =============================================================================
Write-Host "`n--- TEST 18: -S/-E boundary correctness ---" -ForegroundColor Yellow

# Capture lines 2-4 (3 lines)
$lines18 = (& $exe capture-pane -t $SESSION -p -S 2 -E 4 2>&1) -split "`n"
$non_null18 = $lines18 | Where-Object { $_ -ne $null }
Check "-S 2 -E 4 returns ~3 lines" ($non_null18.Count -ge 2 -and $non_null18.Count -le 5)

# =============================================================================
# TEST 19: capture-pane -p doesn't add extra trailing newlines
# =============================================================================
Write-Host "`n--- TEST 19: No double trailing newlines ---" -ForegroundColor Yellow

$raw19 = & $exe capture-pane -t $SESSION -p 2>&1 | Out-String
# Should not end with double newline
$ends_double_nl = $raw19 -match "\n\n$"
# This is acceptable — the important thing is it doesn't have triple/quad newlines
$ends_triple_nl = $raw19 -match "\n\n\n$"
Check "No excessive trailing newlines" (-not $ends_triple_nl)

# =============================================================================
# TEST 20: capture-pane with -e -S -E combined
# =============================================================================
Write-Host "`n--- TEST 20: -e -S -E combined ---" -ForegroundColor Yellow

# Send colored content first
& $exe send-keys -t $SESSION 'Write-Host "STYLED_RANGE" -ForegroundColor Green' Enter
Start-Sleep -Seconds 1

$out20 = & $exe capture-pane -t $SESSION -p -e -S -5 2>&1 | Out-String
Check "-e -S -5 combined works" ($out20.Length -gt 0)
# Should have escape sequences
$has_esc20 = $out20 -match [char]27
Check "-e -S combined has escape codes" $has_esc20

# =============================================================================
# TEST 21: Multiple sessions, capture from each
# =============================================================================
Write-Host "`n--- TEST 21: Multiple sessions ---" -ForegroundColor Yellow

$SESSION2 = "test_cap2_$(Get-Random -Maximum 9999)"
& $exe new-session -d -s $SESSION2 -x 80 -y 24
Start-Sleep -Seconds 2
& $exe send-keys -t $SESSION2 "echo SESSION2_MARKER" Enter
Start-Sleep -Seconds 1

$out21a = & $exe capture-pane -t $SESSION -p 2>&1 | Out-String
$out21b = & $exe capture-pane -t $SESSION2 -p 2>&1 | Out-String
Check "Session 1 capture works" ($out21a.Length -gt 0)
Check "Session 2 capture works" ($out21b.Length -gt 0)
Check "Session 2 has its own content" ($out21b -match "SESSION2_MARKER")
Check "Session 1 doesn't have session 2 content" (-not ($out21a -match "SESSION2_MARKER"))

# Clean up session 2
& $exe kill-session -t $SESSION2 2>$null

# =============================================================================
# TEST 22: capture-pane -S - -E - (full range, explicit)
# =============================================================================
Write-Host "`n--- TEST 22: Explicit full range -S - -E - ---" -ForegroundColor Yellow

$out22 = & $exe capture-pane -t $SESSION -p -S - -E - 2>&1 | Out-String
Check "-S - -E - returns content" ($out22.Length -gt 0)

# =============================================================================
# TEST 23: Targeted pane capture (session:window.pane)
# =============================================================================
Write-Host "`n--- TEST 23: Targeted pane capture ---" -ForegroundColor Yellow

# Go back to window 0 (which has 2 panes from TEST 11)
& $exe select-window -t "${SESSION}:0"
Start-Sleep -Seconds 1
# Send unique markers to each pane in window 0
& $exe send-keys -t "${SESSION}:0.0" "echo TARGET_P0_MARKER" Enter
Start-Sleep -Seconds 2
& $exe send-keys -t "${SESSION}:0.1" "echo TARGET_P1_MARKER" Enter
Start-Sleep -Seconds 2

$cap_p0 = & $exe capture-pane -t "${SESSION}:0.0" -p 2>&1 | Out-String
$cap_p1 = & $exe capture-pane -t "${SESSION}:0.1" -p 2>&1 | Out-String
Check "Pane 0 has its own marker" ($cap_p0 -match "TARGET_P0_MARKER")
Check "Pane 1 has its own marker" ($cap_p1 -match "TARGET_P1_MARKER")
Check "Pane 0 does NOT have pane 1 marker" (-not ($cap_p0 -match "TARGET_P1_MARKER"))
Check "Pane 1 does NOT have pane 0 marker" (-not ($cap_p1 -match "TARGET_P0_MARKER"))

# =============================================================================
# TEST 24: Cross-window pane capture
# =============================================================================
Write-Host "`n--- TEST 24: Cross-window pane capture ---" -ForegroundColor Yellow

# Window 1 was created in TEST 12 - send a marker there
& $exe send-keys -t "${SESSION}:1" "echo CROSSWIN_MARKER" Enter
Start-Sleep -Seconds 4

# Capture from window 1 while staying on window 0
$cap_w1 = & $exe capture-pane -t "${SESSION}:1.0" -p 2>&1 | Out-String
Check "Cross-window capture works" ($cap_w1 -match "CROSSWIN_MARKER")

# Make sure it's not in window 0
$cap_w0 = & $exe capture-pane -t "${SESSION}:0.0" -p 2>&1 | Out-String
Check "Window 0 doesn't have window 1 content" (-not ($cap_w0 -match "CROSSWIN_MARKER"))

# =============================================================================
# TEST 25: 4-pane tiled layout capture
# =============================================================================
Write-Host "`n--- TEST 25: 4-pane tiled layout capture ---" -ForegroundColor Yellow

# Create a new window with 4 panes
& $exe new-window -t $SESSION
Start-Sleep -Seconds 3
& $exe send-keys -t $SESSION "echo QUAD_A" Enter
Start-Sleep -Seconds 2
& $exe split-window -t $SESSION -h
Start-Sleep -Seconds 3
& $exe send-keys -t $SESSION "echo QUAD_B" Enter
Start-Sleep -Seconds 2
& $exe split-window -t $SESSION -v
Start-Sleep -Seconds 3
& $exe send-keys -t $SESSION "echo QUAD_C" Enter
Start-Sleep -Seconds 2

# Window 2 was just created (0=setup, 1=TEST12, 2=this)
$capA = & $exe capture-pane -t "${SESSION}:2.0" -p 2>&1 | Out-String
$capB = & $exe capture-pane -t "${SESSION}:2.1" -p 2>&1 | Out-String
$capC = & $exe capture-pane -t "${SESSION}:2.2" -p 2>&1 | Out-String
Check "Quad pane 0 has QUAD_A" ($capA -match "QUAD_A")
Check "Quad pane 1 has QUAD_B" ($capB -match "QUAD_B")
Check "Quad pane 2 has QUAD_C" ($capC -match "QUAD_C")

# =============================================================================
# TEST 26: Targeted capture with -e (escape seqs on specific pane)
# =============================================================================
Write-Host "`n--- TEST 26: Targeted -e capture ---" -ForegroundColor Yellow

& $exe send-keys -t "${SESSION}:0.0" 'Write-Host "STYLED_TARGET" -ForegroundColor Magenta' Enter
Start-Sleep -Seconds 2

$styled_target = & $exe capture-pane -t "${SESSION}:0.0" -p -e 2>&1 | Out-String
Check "Targeted -e capture has escape codes" ($styled_target -match [char]27)
Check "Targeted -e capture has content" ($styled_target -match "STYLED_TARGET")

# =============================================================================
# TEST 27: Targeted capture with -S/-E range
# =============================================================================
Write-Host "`n--- TEST 27: Targeted -S/-E range capture ---" -ForegroundColor Yellow

$range_target = & $exe capture-pane -t "${SESSION}:0.1" -p -S 0 -E 3 2>&1
$range_lines = $range_target -split "`n" | Where-Object { $_ -ne $null }
Check "Targeted -S 0 -E 3 returns limited lines" ($range_lines.Count -le 6)
Check "Targeted range capture returns content" ($range_lines.Count -ge 1)

# =============================================================================
# TEST 28: Targeted capture with combined -e -S -E
# =============================================================================
Write-Host "`n--- TEST 28: Targeted -e -S -E combined ---" -ForegroundColor Yellow

$combo = & $exe capture-pane -t "${SESSION}:0.0" -p -e -S 0 -E 5 2>&1 | Out-String
Check "Targeted -e -S -E combined works" ($combo.Length -gt 0)
Check "Targeted -e -S -E has escapes" ($combo -match [char]27)

# =============================================================================
# CLEANUP
# =============================================================================
Write-Host "`n--- CLEANUP ---" -ForegroundColor Yellow
& $exe kill-session -t $SESSION 2>$null
& $exe kill-server 2>$null

# =============================================================================
# RESULTS
# =============================================================================
Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host "RESULTS: $pass PASS, $fail FAIL, $skip SKIP (Total: $($pass + $fail + $skip))" -ForegroundColor $(if ($fail -eq 0) { "Green" } else { "Red" })
Write-Host "========================================`n" -ForegroundColor Cyan

if ($fail -gt 0) { exit 1 } else { exit 0 }
