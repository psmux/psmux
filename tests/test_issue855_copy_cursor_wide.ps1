# Issue #855: copy mode cursor-left and cursor-right cross a wide character in one step.
#
# They moved one cell at a time, and the second cell of a CJK character or an
# emoji sequence is padding, where the cursor still shows on the character, so
# each wide character took two presses. Right now steps over the padding as
# tmux does (grid-reader.c, grid_reader_cursor_right, tag 3.7c). Left backs off
# a second cell and steps one cell, as grid_reader_cursor_left does, and up and
# down keep the column. Every value checked is what tmux 3.7c and 3.8 report
# for the same lines.
#
# The suite runs under -L in a PSMUX_DATA_DIR of its own and kills only its own
# namespace. Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
. "$PSScriptRoot\isolated_config.ps1" -Lines @("set -g default-shell $POWERSHELL", "set -g mode-keys vi")

$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_TARGET_FULL', 'TMUX', 'TMUX_PANE') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}
$env:PSMUX_NO_WARM = "1"
# Read psmux's output (show-buffer, capture-pane) as UTF-8.
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_ccw_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$NS = "ccw" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$S = "cw"

Write-Host "`n=== Issue #855: copy mode cursor-left and cursor-right cross a wide character in one step ===" -ForegroundColor Cyan
Write-Host "  Binary:   $PSMUX"
Write-Host "  Data dir: $TMP"

function P { & $PSMUX -L $NS @args 2>&1 }
function U($cp) { [char]::ConvertFromUtf32($cp) }

# The lines are built from code points so this file stays ASCII.
$CJK = (U 0x65E5) + (U 0x672C) + (U 0x8A9E) + 'abc'
$ZWJ = [string][char]0x200D; $VS16 = [string][char]0xFE0F
$FAMILY = (U 0x1F468) + $ZWJ + (U 0x1F469) + $ZWJ + (U 0x1F467)
$FLAG = (U 0x1F1EF) + (U 0x1F1F5)
$EMOJI = $FAMILY + $FLAG + (U 0x1F44D) + (U 0x1F3FD) + [char]0x2764 + $VS16 + '1' + $VS16 + [char]0x20E3 + 'a'
$ASCII = 'abc def'
$LINES = Join-Path $TMP "lines.txt"
[IO.File]::WriteAllLines($LINES, @($CJK, $EMOJI, $ASCII), [Text.UTF8Encoding]::new($false))

function X { [int]((P display -p -t $S '#{copy_cursor_x}') -join '').Trim() }

# Enter copy mode on screen row $row, at column $col.
function Enter-At($row, $col) {
    P send-keys -t $S -X cancel | Out-Null
    P copy-mode -t $S | Out-Null
    P send-keys -t $S -X top-line | Out-Null
    for ($i = 0; $i -lt $row; $i++) { P send-keys -t $S -X cursor-down | Out-Null }
    P send-keys -t $S -X start-of-line | Out-Null
    for ($i = 0; $i -lt $col; $i++) { P send-keys -t $S -X cursor-right | Out-Null }
}

# The columns `steps` presses of `verb` visit.
function Walk($verb, $steps) {
    $out = @()
    for ($i = 0; $i -lt $steps; $i++) { P send-keys -t $S -X $verb | Out-Null; $out += X }
    , $out
}

function Same($got, $want) { ($got -join ',') -eq ($want -join ',') }

try {
    P new-session -d -s $S -x 80 -y 10 | Out-Null
    $port = Join-Path $TMP "${NS}__${S}.port"
    for ($i = 0; $i -lt 60 -and -not (Test-Path $port); $i++) { Start-Sleep -Milliseconds 100 }
    Start-Sleep -Milliseconds 1500
    P send-keys -t $S "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Clear-Host; Get-Content -Encoding UTF8 '$LINES'" Enter | Out-Null
    $shown = $false
    for ($i = 0; $i -lt 50 -and -not $shown; $i++) {
        Start-Sleep -Milliseconds 200
        $shown = ((P capture-pane -p -t $S) -join "`n") -match 'abc def'
    }

    Write-Head "A. the lines reach the pane whole"
    $cap = @(P capture-pane -p -t $S)
    Check ($shown -and $cap[0] -eq $CJK -and $cap[1] -eq $EMOJI) "row 0 is the CJK line and row 1 the emoji line" "capture [$($cap[0..2] -join ' | ')]"

    Write-Head "B. the CJK line"
    Enter-At 0 0
    $r = Walk cursor-right 5
    Check (Same $r @(2, 4, 6, 7, 8)) "cursor-right visits 2 4 6 7 8 (tmux)" "visited $($r -join ' ')"
    $l = Walk cursor-left 5
    Check (Same $l @(7, 6, 5, 3, 1)) "cursor-left visits 7 6 5 3 1 (tmux)" "visited $($l -join ' ')"

    Write-Head "C. the emoji line"
    Enter-At 1 0
    $r = Walk cursor-right 6
    Check (Same $r @(2, 4, 6, 8, 10, 11)) "cursor-right visits 2 4 6 8 10 11 (tmux)" "visited $($r -join ' ')"
    $l = Walk cursor-left 6
    Check (Same $l @(10, 9, 7, 5, 3, 1)) "cursor-left visits 10 9 7 5 3 1 (tmux)" "visited $($l -join ' ')"

    Write-Head "D. one-cell characters, and up and down"
    Enter-At 2 0
    $r = Walk cursor-right 3
    Check (Same $r @(1, 2, 3)) "cursor-right on 'abc def' visits 1 2 3" "visited $($r -join ' ')"
    # Column 3 above is the second cell of the flag, and above that of U+672C.
    P send-keys -t $S -X cursor-up | Out-Null
    $up1 = (P display -p -t $S '#{copy_cursor_y}:#{copy_cursor_x}') -join ''
    P send-keys -t $S -X cursor-up | Out-Null
    $up2 = (P display -p -t $S '#{copy_cursor_y}:#{copy_cursor_x}') -join ''
    Check (($up1 -eq '1:3') -and ($up2 -eq '0:3')) "cursor-up from column 3 keeps column 3 on both rows (tmux)" "got $up1 then $up2"
    P send-keys -t $S -X cursor-down | Out-Null
    P send-keys -t $S -X cursor-down | Out-Null
    $back = (P display -p -t $S '#{copy_cursor_y}:#{copy_cursor_x}') -join ''
    Check ($back -eq '2:3') "cursor-down twice comes back to 2:3, the column it left (tmux)" "got $back"

    Write-Head "E. a repeat count and a selection"
    Enter-At 0 0
    P send-keys -t $S -X -N 3 cursor-right | Out-Null
    $x = X
    Check ($x -eq 6) "-N 3 cursor-right lands on 6 (tmux)" "landed on $x"
    Enter-At 0 0
    P send-keys -t $S -X begin-selection | Out-Null
    P send-keys -t $S -X cursor-right | Out-Null
    P send-keys -t $S -X cursor-right | Out-Null
    P send-keys -t $S -X copy-selection-and-cancel | Out-Null
    $buf = ((P show-buffer) -join "`n")
    $want = (U 0x65E5) + (U 0x672C) + (U 0x8A9E)
    Check ($buf -eq $want) "two cursor-right from the first character select three of them (tmux, vi keys)" "buffer [$buf]"
    Enter-At 1 0
    P send-keys -t $S -X begin-selection | Out-Null
    P send-keys -t $S -X cursor-right | Out-Null
    P send-keys -t $S -X copy-selection-and-cancel | Out-Null
    $buf = ((P show-buffer) -join "`n")
    Check ($buf -eq ($FAMILY + $FLAG)) "one cursor-right from the family selects the family and the flag (tmux, vi keys)" "buffer [$buf]"
    Enter-At 0 3
    P send-keys -t $S -X begin-selection | Out-Null
    P send-keys -t $S -X cursor-left | Out-Null
    P send-keys -t $S -X copy-selection-and-cancel | Out-Null
    $buf = ((P show-buffer) -join "`n")
    Check ($buf -eq 'a') "one cursor-left from 'a' stops on the second cell of the CJK character before and copies 'a' (tmux, vi keys)" "buffer [$buf]"
} finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-PsmuxIsolatedConfig
    Remove-Item Env:PSMUX_DATA_DIR, Env:PSMUX_NO_WARM -EA SilentlyContinue
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" `
    -ForegroundColor $(if ($script:TestsFailed) { 'Red' } else { 'Green' })
exit $script:TestsFailed
