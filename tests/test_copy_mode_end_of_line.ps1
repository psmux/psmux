# Copy mode end-of-line stops where tmux stops.
#
# end-of-line (vi $, emacs C-e) went to the last column of the pane, 79 in an
# 80 column pane, whatever the line held. tmux goes to the end of the text:
# grid_reader_cursor_end_of_line follows a wrapped line to its last row and
# takes grid_line_length there, trailing spaces left out, and
# window_copy_cursor_limit keeps vi keys on the last character (tmux 3.7 and
# later) while emacs keys stay one past it. Every value checked is what tmux
# 3.8 reports for the same lines with the same keys.
#
# The suite runs under -L in a PSMUX_DATA_DIR of its own and kills only its own
# namespace. Set PSMUX_TEST_EXE (or PSMUX_TEST_BIN) to test a binary that is
# not on PATH.

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
. "$PSScriptRoot\isolated_config.ps1" -Lines @("set -g default-shell $POWERSHELL", "set -g mode-keys vi")

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $PSMUX = $env:PSMUX_TEST_BIN }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_TARGET_FULL', 'TMUX', 'TMUX_PANE') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}
$env:PSMUX_NO_WARM = "1"
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_eol_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$NS = "eol" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$S = "el"

Write-Host "`n=== Copy mode end-of-line stops at the end of the text, as tmux does ===" -ForegroundColor Cyan
Write-Host "  Binary:   $PSMUX"
Write-Host "  Data dir: $TMP"

function P { & $PSMUX -L $NS @args 2>&1 }
function U($cp) { [char]::ConvertFromUtf32($cp) }

# The lines are built from code points so this file stays ASCII.
$CJK = (U 0x65E5) + (U 0x672C) + (U 0x8A9E)
$LONG = '0' * 100
$CONTENT = @(
    'abcdefghi',          # row 0
    'trail   ',           # row 1, trailing spaces
    '',                   # row 2, empty
    '  indented x',       # row 3
    $LONG,                # rows 4 and 5, one line wrapped
    ($CJK + 'ab'),        # row 6
    ('x' + (U 0x1F600) + 'y'), # row 7
    $CJK                  # row 8, ends on a wide character
)
$LINES = Join-Path $TMP "lines.txt"
[IO.File]::WriteAllLines($LINES, $CONTENT, [Text.UTF8Encoding]::new($false))

function Pos { ((P display -p -t $S '#{copy_cursor_x},#{copy_cursor_y}') -join '').Trim() }

function Enter-Row($row) {
    P send-keys -t $S -X cancel | Out-Null
    P copy-mode -t $S | Out-Null
    P send-keys -t $S -X top-line | Out-Null
    for ($i = 0; $i -lt $row; $i++) { P send-keys -t $S -X cursor-down | Out-Null }
    P send-keys -t $S -X start-of-line | Out-Null
}

# Columns and rows tmux 3.8 reports for end-of-line on each row.
$EXPECT = @{
    vi    = @('8,0', '4,1', '0,2', '11,3', '19,5', '19,5', '7,6', '3,7', '4,8')
    emacs = @('9,0', '5,1', '0,2', '12,3', '20,5', '20,5', '8,6', '4,7', '6,8')
}

# The copies in section D also reach the system clipboard: keep what the user
# had there and put it back at the end.
$savedClipboard = $null
try { $savedClipboard = Get-Clipboard -Raw -EA Stop } catch {}

try {
    P new-session -d -s $S -x 80 -y 20 | Out-Null
    $port = Join-Path $TMP "${NS}__${S}.port"
    for ($i = 0; $i -lt 60 -and -not (Test-Path $port); $i++) { Start-Sleep -Milliseconds 100 }
    Start-Sleep -Milliseconds 1500
    P send-keys -t $S "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Clear-Host; Get-Content -Encoding UTF8 '$LINES'" Enter | Out-Null
    $shown = $false
    for ($i = 0; $i -lt 50 -and -not $shown; $i++) {
        Start-Sleep -Milliseconds 200
        $shown = ((P capture-pane -p -t $S) -join "`n") -match 'indented x'
    }

    Write-Head "A. the lines reach the pane"
    $cap = @(P capture-pane -p -t $S)
    $wrapped = ($cap[4] -eq ('0' * 80)) -and ($cap[5] -eq ('0' * 20))
    Check ($shown -and $cap[0] -eq 'abcdefghi' -and $cap[3] -eq '  indented x' -and $cap[8] -eq $CJK -and $wrapped) `
        "rows 0 to 8 hold the lines and the long line fills row 4 and 20 cells of row 5" "capture [$($cap[0..8] -join ' | ')]"

    foreach ($mk in 'vi', 'emacs') {
        P set -g mode-keys $mk | Out-Null
        Write-Head "B. end-of-line on each row, mode-keys $mk"
        for ($row = 0; $row -le 8; $row++) {
            Enter-Row $row
            P send-keys -t $S -X end-of-line | Out-Null
            $got = Pos
            $want = $EXPECT[$mk][$row]
            Check ($got -eq $want) "row ${row}: end-of-line lands on $want (tmux)" "row ${row}: landed on $got, tmux lands on $want"
        }

        Write-Head "C. start-of-line, back-to-indentation and -N, mode-keys $mk"
        Enter-Row 3
        P send-keys -t $S -X end-of-line | Out-Null
        P send-keys -t $S -X back-to-indentation | Out-Null
        $got = Pos
        Check ($got -eq '2,3') "back-to-indentation on '  indented x' lands on 2,3" "landed on $got"
        P send-keys -t $S -X end-of-line | Out-Null
        P send-keys -t $S -X start-of-line | Out-Null
        $got = Pos
        Check ($got -eq '0,3') "start-of-line lands on 0,3" "landed on $got"
        Enter-Row 5
        P send-keys -t $S -X end-of-line | Out-Null
        P send-keys -t $S -X start-of-line | Out-Null
        $got = Pos
        Check ($got -eq '0,4') "start-of-line on the wrapped row goes back to its first row, 0,4 (tmux)" "landed on $got"
        Enter-Row 0
        P send-keys -t $S -X -N 3 end-of-line | Out-Null
        $got = Pos
        $want = $EXPECT[$mk][0]
        Check ($got -eq $want) "-N 3 end-of-line stays on row 0 at $want (tmux)" "landed on $got"

        Write-Head "D. a selection to the end of the line, mode-keys $mk"
        Enter-Row 0
        P send-keys -t $S -X begin-selection | Out-Null
        P send-keys -t $S -X end-of-line | Out-Null
        P send-keys -t $S -X copy-selection-and-cancel | Out-Null
        $buf = ((P show-buffer) -join "`n")
        Check ($buf -eq 'abcdefghi') "begin-selection, end-of-line, copy gives 'abcdefghi' (tmux)" "buffer [$buf]"
        Enter-Row 1
        P send-keys -t $S -X begin-selection | Out-Null
        P send-keys -t $S -X end-of-line | Out-Null
        P send-keys -t $S -X copy-selection-and-cancel | Out-Null
        $buf = ((P show-buffer) -join "`n")
        Check ($buf -eq 'trail') "on 'trail   ' the copy stops at 'trail' (tmux)" "buffer [$buf]"
        Enter-Row 1
        P send-keys -t $S -X begin-selection | Out-Null
        P send-keys -t $S -X -N 7 cursor-right | Out-Null
        P send-keys -t $S -X copy-selection-and-cancel | Out-Null
        $buf = ((P show-buffer) -join "`n")
        Check ($buf -eq 'trail') "a selection walked past the text of 'trail   ' copies 'trail' (tmux)" "buffer [$buf]"
        Enter-Row 0
        P send-keys -t $S -X begin-selection | Out-Null
        P send-keys -t $S -X cursor-down | Out-Null
        P send-keys -t $S -X end-of-line | Out-Null
        P send-keys -t $S -X copy-selection-and-cancel | Out-Null
        $buf = ((P show-buffer) -join "|")
        Check ($buf -eq 'abcdefghi|trail') "two rows to the end of the second copy 'abcdefghi' and 'trail' (tmux)" "buffer [$buf]"
    }

    Write-Head "E. the cursor is drawn where end-of-line put it"
    P set -g mode-keys vi | Out-Null
    Enter-Row 0
    P send-keys -t $S -X end-of-line | Out-Null
    $line = ((P display -p -t $S '#{copy_cursor_line}') -join '')
    $word = ((P display -p -t $S '#{copy_cursor_word}') -join '')
    Check ($line -eq 'abcdefghi' -and $word -eq 'abcdefghi') "copy_cursor_line and copy_cursor_word read 'abcdefghi' at the end of row 0" "line [$line] word [$word]"

    Write-Head "F. the keys themselves: vi `$ and End, emacs C-e"
    Enter-Row 3
    P send-keys -t $S '$' | Out-Null
    $got = Pos
    Check ($got -eq '11,3') "vi `$ key lands on 11,3" "landed on $got"
    Enter-Row 0
    P send-keys -t $S End | Out-Null
    $got = Pos
    Check ($got -eq '8,0') "vi End key lands on 8,0" "landed on $got"
    P set -g mode-keys emacs | Out-Null
    Enter-Row 3
    P send-keys -t $S C-e | Out-Null
    $got = Pos
    Check ($got -eq '12,3') "emacs C-e lands on 12,3" "landed on $got"
    P send-keys -t $S C-a | Out-Null
    $got = Pos
    Check ($got -eq '0,3') "emacs C-a goes back to 0,3" "landed on $got"
} finally {
    P kill-server | Out-Null
    if ($null -ne $savedClipboard) { try { Set-Clipboard -Value $savedClipboard } catch {} }
    Start-Sleep -Milliseconds 500
    Remove-PsmuxIsolatedConfig
    Remove-Item Env:PSMUX_DATA_DIR, Env:PSMUX_NO_WARM -EA SilentlyContinue
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" `
    -ForegroundColor $(if ($script:TestsFailed) { 'Red' } else { 'Green' })
exit $script:TestsFailed
