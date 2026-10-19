# What a copy mode selection copies, as tmux copies it.
#
# Two gaps against tmux:
#  1. With emacs mode-keys (the default) tmux leaves the cell under the end of
#     a selection out: "keep the top-left-most character, and drop the
#     bottom-right-most, regardless of copy direction" (window-copy.c
#     window_copy_get_selection), and paints the same (screen.c
#     screen_check_selection). psmux copied and painted it, as vi does.
#  2. The last row of a selection that spans rows was trimmed, so blanks
#     between words at the end of the selection were dropped. tmux clamps the
#     end to the row's text and keeps them.
#
# Every expectation was measured on tmux 3.4 and 3.8 (WSL) with the same
# screen and the same send -X motions (top-line, start-of-line,
# begin-selection, then the motions, then copy-selection-and-cancel):
#
#   screen: abcdefgh | foo   bar   baz | xy
#   motions             emacs                        vi
#   right x3            abc                          abcd
#   down, right x4      abcdefgh\nfoo                abcdefgh\nfoo
#   down, right x5      abcdefgh\nfoo                abcdefgh\nfoo
#   down, right x10     abcdefgh\nfoo   bar          abcdefgh\nfoo   bar
#   down x2, end        abcdefgh\nfoo   bar   baz\nxy (both)
#   screen: abc | (empty) | xyz
#   down                abc\n                        abc\n\n
#   rectangle, down x2, right x2   ab\n\nxy          abc\n\nxyz
#
# The buffer is read with save-buffer so trailing newlines count. The
# highlight is read from dump-layout, the frame the client paints from.
# Runs only under -L in a private PSMUX_DATA_DIR; no window is opened. A copy
# also goes to the system clipboard, so the user's clipboard is saved and put
# back.

$ErrorActionPreference = 'Continue'
$Exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux).Source }
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP "psmux_test_copy_selection_parity_$PID"
}
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null

$script:pass = 0
$script:fail = 0
function Vis([string]$s) { $s -replace "`r", '\r' -replace "`n", '\n' }
function Check([string]$name, [bool]$ok, [string]$detail) {
    if ($ok) { $script:pass++; Write-Host "PASS $name" -ForegroundColor Green }
    else { $script:fail++; Write-Host "FAIL $name : $detail" -ForegroundColor Red }
}

$ns = "test_copy_selection_parity_${PID}_$(Get-Random)"
$tmp = Join-Path $env:PSMUX_DATA_DIR "csp_$PID"
New-Item -ItemType Directory -Force $tmp | Out-Null
$bufFile = Join-Path $tmp "buf.txt"

function Child([string]$text) {
    "pwsh -NoProfile -NoLogo -Command `"Clear-Host; [Console]::Out.Write('$text'.Replace('|', [char]10)); Start-Sleep 120`""
}

# The layout frame an attached client paints from, over the session's TCP
# port (dump-layout is a wire command, not a CLI one).
function Get-Layout([string]$s) {
    $port = (Get-Content (Join-Path $env:PSMUX_DATA_DIR "$($ns)__$s.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $env:PSMUX_DATA_DIR "$($ns)__$s.key") -Raw).Trim()
    $tcp = New-Object System.Net.Sockets.TcpClient
    try {
        $tcp.Connect('127.0.0.1', [int]$port)
        $st = $tcp.GetStream(); $st.ReadTimeout = 5000
        $wr = New-Object System.IO.StreamWriter($st)
        $rd = New-Object System.IO.StreamReader($st)
        $wr.WriteLine("AUTH $key"); $wr.Flush()
        if ($rd.ReadLine() -ne 'OK') { return '<auth failed>' }
        $wr.WriteLine('dump-layout'); $wr.Flush()
        return $rd.ReadLine()
    } catch { return "<$($_.Exception.Message)>" } finally { $tcp.Close() }
}

function X([string]$s, [string]$verb) { & $Exe -L $ns send-keys -t $s -X $verb 2>&1 | Out-Null }

# Start a selection at the top left, run the motions, then either return the
# copy (save-buffer, exact bytes) or, with -Layout, the frame while the
# selection is still on screen.
function Select-Copy([string]$s, [string]$keys, [string[]]$motions, [switch]$Layout) {
    & $Exe -L $ns set -t $s -g mode-keys $keys 2>&1 | Out-Null
    & $Exe -L $ns copy-mode -t $s 2>&1 | Out-Null
    X $s 'top-line'
    X $s 'start-of-line'
    X $s 'begin-selection'
    foreach ($m in $motions) { X $s $m }
    if ($Layout) {
        Start-Sleep -Milliseconds 200
        $frame = Get-Layout $s
        X $s 'cancel'
        return $frame
    }
    X $s 'copy-selection-and-cancel'
    Start-Sleep -Milliseconds 300
    Remove-Item $bufFile -ErrorAction SilentlyContinue
    & $Exe -L $ns save-buffer $bufFile 2>&1 | Out-Null
    if (Test-Path $bufFile) { return [IO.File]::ReadAllText($bufFile) }
    return '<no buffer>'
}

$savedClip = Get-Clipboard -Raw
# One session at a time, so every command reaches the pane it means.
function Start-Text([string]$s, [string]$text, [string]$marker) {
    & $Exe -L $ns new-session -d -s $s -x 40 -y 10 (Child $text) | Out-Null
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 250
        if (((& $Exe -L $ns capture-pane -p -t $s) -join "`n") -match $marker) { return $true }
    }
    return $false
}
function Stop-Text([string]$s) { & $Exe -L $ns kill-session -t $s 2>&1 | Out-Null; Start-Sleep -Milliseconds 300 }

try {
    Check 'the words pane shows its lines' (Start-Text 'words' 'abcdefgh|foo   bar   baz|xy|' 'baz') ''

    $R = 'cursor-right'
    $cases = @(
        @('right x3', @($R, $R, $R), 'abc', 'abcd'),
        @('down, right x4', @('cursor-down', $R, $R, $R, $R), "abcdefgh`nfoo ", "abcdefgh`nfoo  "),
        @('down, right x5', @('cursor-down', $R, $R, $R, $R, $R), "abcdefgh`nfoo  ", "abcdefgh`nfoo   "),
        @('down, right x10', (@('cursor-down') + @($R) * 10), "abcdefgh`nfoo   bar ", "abcdefgh`nfoo   bar  "),
        @('down x2, end-of-line', @('cursor-down', 'cursor-down', 'end-of-line'), "abcdefgh`nfoo   bar   baz`nxy", "abcdefgh`nfoo   bar   baz`nxy")
    )
    foreach ($c in $cases) {
        $e = Select-Copy 'words' 'emacs' $c[1]
        Check "emacs $($c[0]) copies [$(Vis $c[2])]" ($e -ceq $c[2]) "got [$(Vis $e)]"
        $v = Select-Copy 'words' 'vi' $c[1]
        Check "vi $($c[0]) copies [$(Vis $c[3])]" ($v -ceq $c[3]) "got [$(Vis $v)]"
    }

    # The painted selection: emacs stops one cell before the cursor.
    $f = Select-Copy 'words' 'emacs' @($R, $R, $R) -Layout
    Check 'emacs highlight ends on column 2' ($f -match '"sel_start_col":0,"sel_end_row":0,"sel_end_col":2,') ($f -replace '.*("sel_start_row"[^}]*?"sel_mode":[^,]*).*', '$1')
    $f = Select-Copy 'words' 'vi' @($R, $R, $R) -Layout
    Check 'vi highlight ends on column 3' ($f -match '"sel_start_col":0,"sel_end_row":0,"sel_end_col":3,') ($f -replace '.*("sel_start_row"[^}]*?"sel_mode":[^,]*).*', '$1')
    Stop-Text 'words'

    Check 'the gaps pane shows its lines' (Start-Text 'gaps' 'abc||xyz|' 'xyz') ''
    $e = Select-Copy 'gaps' 'emacs' @('cursor-down')
    Check 'emacs selection ending on an empty row copies [abc\n]' ($e -ceq "abc`n") "got [$(Vis $e)]"
    $v = Select-Copy 'gaps' 'vi' @('cursor-down')
    Check 'vi selection ending on an empty row copies [abc\n\n]' ($v -ceq "abc`n`n") "got [$(Vis $v)]"

    $rect = @('rectangle-toggle', 'cursor-down', 'cursor-down', $R, $R)
    $e = Select-Copy 'gaps' 'emacs' $rect
    Check 'emacs rectangle leaves the cursor column out [ab\n\nxy]' ($e -ceq "ab`n`nxy") "got [$(Vis $e)]"
    $v = Select-Copy 'gaps' 'vi' $rect
    Check 'vi rectangle keeps the cursor column [abc\n\nxyz]' ($v -ceq "abc`n`nxyz") "got [$(Vis $v)]"

} finally {
    & $Exe -L $ns kill-server 2>$null | Out-Null
    if ($null -ne $savedClip) { Set-Clipboard -Value $savedClip }
}

Write-Host ""
Write-Host "Passed: $script:pass  Failed: $script:fail"
if ($script:fail -gt 0) { exit 1 } else { exit 0 }
