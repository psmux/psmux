# Issue #862: copy mode word motions split words the way tmux does.
#
# tmux sorts each cell into whitespace, a separator (word-separators, by
# default the ASCII punctuation minus the underscore) or anything else, CJK
# characters and the second cell of a wide character included, and walks the
# whole grid with its grid reader (grid-reader.c, tag 3.7c). psmux's default
# separators were " -_@", it split punctuation that was not a separator off as
# a word of its own, it read the second cell of a wide character as a space,
# so every CJK character was a word, and emacs keys' next-word-end stopped on
# the last character rather than the cell after the word.
#
# Each check names tmux 3.7c's value for the same lines in a pane of the same
# size.
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
. "$PSScriptRoot\isolated_config.ps1" -Lines @("set -g default-shell $POWERSHELL")

$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = $env:PSMUX_TEST_EXE }
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_TARGET_FULL', 'TMUX', 'TMUX_PANE') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}
$env:PSMUX_NO_WARM = "1"
# Read psmux's output (show-buffer, display-message) as UTF-8.
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_cwm_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$NS = "cwm" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$S = "wm"

Write-Host "`n=== Copy mode word motions split words the way tmux does ===" -ForegroundColor Cyan
Write-Host "  Binary:   $PSMUX"
Write-Host "  Data dir: $TMP"

function P { & $PSMUX -L $NS @args 2>&1 }
function U($cp) { [char]::ConvertFromUtf32($cp) }

# The lines tmux was measured on, built from code points so this file stays
# ASCII.
$CJKWORD = (U 0x65E5) + (U 0x672C) + (U 0x8A9E) + (U 0x306E) + (U 0x30C6) + (U 0x30AD) + (U 0x30B9) + (U 0x30C8)
$MIXED = 'abc' + (U 0x65E5) + (U 0x672C)
$LINES = @(
    'foo-bar_baz qux.quux  a@b:c',
    'path/to/file.txt (x+y)=z  ~/dir',
    "$CJKWORD $MIXED def",
    '  lead  spaces, "quoted" end.'
)
$FILE = Join-Path $TMP "lines.txt"
[IO.File]::WriteAllLines($FILE, $LINES, [Text.UTF8Encoding]::new($false))

function Pos { ((P display -p -t $S '#{copy_cursor_y}:#{copy_cursor_x}') -join '').Trim() }
function Col { [int]((P display -p -t $S '#{copy_cursor_x}') -join '').Trim() }

# Enter copy mode with the cursor on screen row $row, column $col. The column
# is reached a cell at a time, checking where the cursor is, so it does not
# depend on how cursor-right treats wide characters.
function Enter-At($row, $col) {
    P send-keys -t $S -X cancel | Out-Null
    P copy-mode -t $S | Out-Null
    P send-keys -t $S -X top-line | Out-Null
    for ($i = 0; $i -lt $row; $i++) { P send-keys -t $S -X cursor-down | Out-Null }
    P send-keys -t $S -X start-of-line | Out-Null
    for ($i = 0; $i -lt 60 -and (Col) -lt $col; $i++) { P send-keys -t $S -X cursor-right | Out-Null }
}

# The positions `steps` presses of `verb` stop at.
function Walk($verb, $steps) {
    $out = @()
    for ($i = 0; $i -lt $steps; $i++) { P send-keys -t $S -X $verb | Out-Null; $out += Pos }
    $out -join ' '
}

function Check-Walk($row, $col, $verb, $want, $note) {
    Enter-At $row $col
    $got = Walk $verb (@($want -split ' ').Count)
    Check ($got -eq $want) "$verb from ${row}:$col stops at $want$note" "$verb from ${row}:$col stopped at $got, want $want$note"
}

function Copied {
    P send-keys -t $S -X copy-selection-and-cancel | Out-Null
    ((P show-buffer) -join "`n")
}

try {
    P new-session -d -s $S -x 60 -y 10 | Out-Null
    $port = Join-Path $TMP "${NS}__${S}.port"
    for ($i = 0; $i -lt 60 -and -not (Test-Path $port); $i++) { Start-Sleep -Milliseconds 100 }
    Start-Sleep -Milliseconds 1500
    P send-keys -t $S "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Clear-Host; Get-Content -Encoding UTF8 '$FILE'" Enter | Out-Null
    $shown = $false
    for ($i = 0; $i -lt 50 -and -not $shown; $i++) {
        Start-Sleep -Milliseconds 200
        $shown = ((P capture-pane -p -t $S) -join "`n") -match 'quoted'
    }

    Write-Head "A. the lines and the default separators"
    $cap = @(P capture-pane -p -t $S)
    Check ($shown -and $cap[2] -eq $LINES[2]) "the lines reach the pane whole" "capture [$($cap[0..3] -join ' | ')]"
    $seps = ((P show -gv word-separators) -join '')
    $want = '!"#$%&' + "'" + '()*+,-./:;<=>?@[\]^`{|}~'
    Check ($seps -eq $want) "word-separators defaults to tmux's: ASCII punctuation minus the underscore" "word-separators is [$seps]"

    Write-Head "B. vi keys"
    P set -g mode-keys vi | Out-Null
    Check-Walk 0 0 next-word '0:3 0:4 0:12 0:15 0:16 0:22 0:23' ' (tmux)'
    Check-Walk 0 0 next-word-end '0:2 0:3 0:10 0:14 0:15 0:19 0:22' ' (tmux)'
    Check-Walk 0 26 previous-word '0:25 0:24 0:23 0:22 0:16 0:15 0:12' ' (tmux)'
    Check-Walk 1 0 next-space '1:17 1:26 2:0 2:17' ' (tmux)'
    Check-Walk 2 0 next-word '2:17 2:25 3:2 3:8' ' (tmux)'
    Check-Walk 2 0 next-word-end '2:15 2:23 2:27' ' (tmux: 2:15 2:23 2:27, the second cells)'
    Check-Walk 2 27 previous-word '2:25 2:17 2:0 1:28' ' (tmux)'

    Write-Head "C. emacs keys"
    P set -g mode-keys emacs | Out-Null
    Check-Walk 0 0 next-word-end '0:3 0:4 0:11 0:15 0:16 0:20 0:23' ' (tmux)'
    Check-Walk 2 0 next-word-end '2:16 2:24 2:28 3:6' ' (tmux)'
    Check-Walk 0 27 previous-word '0:26 0:25 0:24 0:23 0:22 0:16 0:15' ' (tmux)'

    Write-Head "D. select-word and copy_cursor_word"
    P set -g mode-keys vi | Out-Null
    $cases = @(
        @(0, 5, 'bar_baz', 'bar_baz'),
        @(0, 3, '-bar_baz', 'bar_baz'),
        @(0, 21, 'quux', 'a'),
        @(2, 0, $CJKWORD, $CJKWORD),
        @(2, 20, $MIXED, $MIXED),
        @(3, 16, '"quoted', 'quoted')
    )
    foreach ($c in $cases) {
        Enter-At $c[0] $c[1]
        $word = ((P display -p -t $S '#{copy_cursor_word}') -join '')
        Check ($word -eq $c[3]) "copy_cursor_word at $($c[0]):$($c[1]) is [$($c[3])] (tmux)" "copy_cursor_word at $($c[0]):$($c[1]) is [$word], want [$($c[3])]"
        P send-keys -t $S -X select-word | Out-Null
        $buf = Copied
        Check ($buf -eq $c[2]) "select-word at $($c[0]):$($c[1]) copies [$($c[2])] (tmux, vi keys)" "select-word at $($c[0]):$($c[1]) copied [$buf], want [$($c[2])]"
    }

    Write-Head "E. select-word with emacs keys"
    # tmux puts the cursor on the cell after the word, which an emacs
    # selection leaves out.
    P set -g mode-keys emacs | Out-Null
    foreach ($c in @(@(0, 5, '0:11', 'bar_baz'), @(2, 20, '2:24', $MIXED))) {
        Enter-At $c[0] $c[1]
        P send-keys -t $S -X select-word | Out-Null
        $pos = Pos
        Check ($pos -eq $c[2]) "select-word at $($c[0]):$($c[1]) ends at $($c[2]) (tmux)" "select-word at $($c[0]):$($c[1]) ended at $pos, want $($c[2])"
        $buf = Copied
        Check ($buf -eq $c[3]) "select-word at $($c[0]):$($c[1]) copies [$($c[3])] (tmux, emacs keys)" "select-word at $($c[0]):$($c[1]) copied [$buf], want [$($c[3])]"
    }

    Write-Head "F. a wrapped line and the scrollback (20x6 pane, values from tmux 3.8)"
    # Ten lines in a 20 column pane: the ninth wraps, and the first five go
    # into the scrollback. Screen row 2 is "alpha bravocharliede", row 3 its
    # continuation "ltaecho foxtrot", row 0 is "row07 aa-bb".
    $FILE2 = Join-Path $TMP "lines2.txt"
    $LINES2 = @(1..8 | ForEach-Object { 'row{0:D2} aa-bb' -f $_ }) + @('alpha bravocharliedeltaecho foxtrot', 'end')
    [IO.File]::WriteAllLines($FILE2, $LINES2)
    $S = "wrap"
    P new-session -d -s $S -x 20 -y 6 "Get-Content '$FILE2'; Start-Sleep 100000" | Out-Null
    $up = $false
    for ($i = 0; $i -lt 50 -and -not $up; $i++) { Start-Sleep -Milliseconds 200; $up = ((P capture-pane -p -t $S) -join "`n") -match '(?m)^end' }
    Check $up "the wrapped lines reach the pane" "the wrapped lines never appeared"
    P set -g mode-keys vi | Out-Null
    Check-Walk 2 0 next-word '2:6 3:8 4:0' ' (tmux: over the wrap)'
    Check-Walk 2 6 next-word-end '3:6 3:14' ' (tmux: the word ends on the next row)'
    Check-Walk 0 0 previous-word '0:9 0:8 0:6 0:0 0:9' ' (tmux: up into the scrollback)'
    $sp = ((P display -p -t $S '#{scroll_position}') -join '').Trim()
    Check ($sp -eq '2') "the view scrolled 2 lines to show the motion's line (tmux)" "scroll_position is $sp, want 2"
    Enter-At 2 8
    $word = ((P display -p -t $S '#{copy_cursor_word}') -join '')
    Check ($word -eq 'bravocharliedeltaecho') "copy_cursor_word takes the whole wrapped word (tmux)" "copy_cursor_word is [$word]"
    P send-keys -t $S -X select-word | Out-Null
    $buf = Copied
    Check ($buf -eq 'bravocharliedeltaecho') "select-word copies the whole wrapped word (tmux)" "select-word copied [$buf]"
    P set -g mode-keys emacs | Out-Null
    Check-Walk 2 6 next-word-end '3:7 3:15' ' (tmux, emacs keys)'

    Write-Head "G. an alternate screen has no lines above it"
    # 30 lines go into the history, then the pane takes the alternate
    # screen. psmux's alternate grid keeps no scrollback, so previous-word
    # from its top row stays there. It used to read row 0 again as the line
    # above and walk 0:8 0:4 0:0 over it.
    $S = "alt"
    P new-session -d -s $S -x 40 -y 8 "1..30|%{ 'word{0:D2} aa' -f `$_ }; [Console]::Write([string][char]27+'[?1049h'+[char]27+'[HALT top line'); Start-Sleep 100000" | Out-Null
    $up = $false
    for ($i = 0; $i -lt 50 -and -not $up; $i++) { Start-Sleep -Milliseconds 200; $up = ((P capture-pane -p -t $S) -join "`n") -match 'ALT top' }
    Check $up "the alternate screen is up" "the alternate screen never appeared"
    P set -g mode-keys vi | Out-Null
    Check-Walk 0 0 previous-word '0:0 0:0' ''
    $word = ((P display -p -t $S '#{copy_cursor_word}') -join '')
    Check ($word -eq 'ALT') "copy_cursor_word on the top row is [ALT]" "copy_cursor_word is [$word]"
    P send-keys -t $S -X cancel | Out-Null

    Write-Head "H. show-options quotes the default as tmux does"
    $line = @(P show -g) | Where-Object { $_ -like 'word-separators *' }
    $want = 'word-separators "!\"#$%&' + "'" + '()*+,-./:;<=>?@[\\]^`{|}~"'
    Check ($line -eq $want) "show -g lists [$want] (tmux 3.8)" "show -g lists [$line], want [$want]"
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
