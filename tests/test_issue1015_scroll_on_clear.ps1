# Issue #1015: scroll-on-clear (options-table.c:1723 at tag 3.8, default on). A
# clear of the whole screen sends what it showed to history first, as tmux does
# for CSI 2 J (screen-write.c:2093), CSI 0 J from the top left corner
# (screen-write.c:1941) and RIS and send-keys -R (screen-write.c:435), never in
# the alternate screen. psmux only blanked the screen, so PSReadLine's Ctrl+L
# lost what was on it, and RIS threw the history away as well.
#
# Every expectation is what tmux 3.8 (`-f /dev/null`, WSL) showed for the same
# bytes in a 40x10 pane, except where the console hosts differ:
#   - CSI H CSI J on the inbox conhost reaches psmux as CSI H and a CSI K on
#     each row, the same as a redraw after a resize, so it keeps nothing there.
#   - RIS in the alternate screen: both hosts leave it and blank the normal
#     screen, tmux stays in it. psmux follows the hosts and keeps the rows in
#     history.
#   - CSI ? 3 h (DECCOLM) clears in tmux. The inbox conhost neither passes it on
#     nor changes its screen, so psmux leaves the screen too.
#   - The inbox conhost of Windows 10 (19045) passes no CSI 2 J, CSI 3 J or RIS
#     on: it hands psmux a redraw that erases every row (PSMUX_PANE_RAW), so
#     psmux cannot tell a clear there and what was on the screen is lost, as
#     with scroll-on-clear off; only what scrolled off before stays in history.
#     The option itself is checked through OpenConsole there.
#
# A program in each pane writes what the suite hands it, each piece as its own
# write, as real programs clear long after they wrote.
#
# [1] the inbox conhost; [2] OpenConsole (PSMUX_CONPTY_DIR), when
# PSMUX_TEST_CONPTY_DIR names a folder holding conpty.dll, skipped otherwise.
#
# Private USERPROFILE, APPDATA, PSMUX_DATA_DIR and config, a -L namespace per
# server; cleanup is `-L <ns> kill-server`. PSMUX_TEST_BIN or PSMUX_EXE picks the
# binary (defaults to target\release\psmux.exe); PSMUX_CONPTY_DIR_FOR_TEST,
# which the sweep sets, names the OpenConsole folder as well.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }
$ConptyDir = if ($env:PSMUX_TEST_CONPTY_DIR) { $env:PSMUX_TEST_CONPTY_DIR } elseif ($env:PSMUX_CONPTY_DIR_FOR_TEST) { $env:PSMUX_CONPTY_DIR_FOR_TEST } else { '' }
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
# The panes run pwsh commands; SHELL would make psmux start them in bash.
Remove-Item Env:SHELL -EA SilentlyContinue
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_soc_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force "$TempHome\p" | Out-Null
$env:USERPROFILE = $TempHome
$env:APPDATA = $TempHome
$env:LOCALAPPDATA = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$E = [string][char]27
function Lines($n, $word = 'line') { -join (1..$n | ForEach-Object { "$word$_`r`n" }) }
function Want($n, $word = 'line') { (1..$n | ForEach-Object { "$word$_" }) -join '|' }

# name, the writes, the history and screen tmux 3.8 showed joined with |, and
# where the inbox conhost differs, what it gives (see the top).
# `win10` is what the Windows 10 inbox conhost gives.
$cases = @(
    @{ n = 'clear2';      w = @((Lines 5), "$E[2J");                  a = (Want 5); win10 = '' }
    @{ n = 'one_write';   w = @(((Lines 5) + "$E[2J"));               a = (Want 5); win10 = '' }
    @{ n = 'history';     w = @((Lines 15), "$E[2J");                 a = (Want 15); win10 = (Want 6) }
    @{ n = 'twice';       w = @((Lines 5), "$E[2J", "$E[2J");         a = (Want 5); win10 = '' }
    @{ n = 'home_clear2'; w = @((Lines 5), "$E[H$E[2J");              a = (Want 5); win10 = '' }
    @{ n = 'gap';         w = @("a`r`n`r`n`r`nb`r`n", "$E[2J");       a = 'a|||b'; win10 = '' }
    @{ n = 'partial_j';   w = @((Lines 5), "$E[3;1H$E[J");            a = (Want 2) }
    @{ n = 'above_j';     w = @((Lines 5), "$E[1J");                  a = '' }
    @{ n = 'home_j';      w = @((Lines 5), "$E[H$E[J");               a = (Want 5); inbox = '' }
    @{ n = 'ris';         w = @((Lines 5), "${E}c");                  a = (Want 5); win10 = '' }
    @{ n = 'ris_history'; w = @((Lines 15), "${E}c");                 a = (Want 15); win10 = (Want 6) }
    @{ n = 'clear3';      w = @((Lines 15), "$E[3J");                 a = ((7..15 | ForEach-Object { "line$_" }) -join '|'); win10 = (Want 15) }
    @{ n = 'clear2_3';    w = @((Lines 15), "$E[2J$E[3J");            a = ''; win10 = (Want 6) }
    @{ n = 'home_2_3';    w = @((Lines 15), "$E[H$E[2J$E[3J");        a = ''; win10 = (Want 6) }
    @{ n = 'alt_clear';   w = @((Lines 2), "$E[?1049h", (Lines 5), "$E[2J", "$E[?1049l"); a = (Want 2) }
    @{ n = 'alt_ris';     w = @((Lines 5), "$E[?1049h", "inalt`r`n", "${E}c"); a = (Want 5); win10 = '' }
    @{ n = 'deccolm';     w = @((Lines 5), "$E[?3h");                 a = (Want 5) }
)
# The inbox conhost before Windows 11 (measured on 19045).
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000
$SEP = [string][char]1
foreach ($c in $cases) {
    [IO.File]::WriteAllBytes("$TempHome\p\$($c.n).bin", [Text.Encoding]::UTF8.GetBytes(($c.w -join $SEP)))
}
Set-Content -Path "$TempHome\show.ps1" -Value @(
    'param([string]$Payload)'
    '[Console]::OutputEncoding = [Text.Encoding]::UTF8'
    '$o = [Console]::OpenStandardOutput()'
    'Start-Sleep -Milliseconds 1500'
    '$pieces = [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($Payload)) -split [string][char]1'
    'foreach ($p in $pieces) { $b = [Text.Encoding]::UTF8.GetBytes($p); $o.Write($b, 0, $b.Length); $o.Flush(); Start-Sleep -Milliseconds 700 }'
    'Set-Content -Path ($Payload + ".done") -Value 1'
    'Start-Sleep 600'
)
# The real clears, after 15 lines: each also clears the history.
$real = @(
    @{ n = 'real_cls';   c = 'cls' }
    @{ n = 'real_host';  c = 'Clear-Host' }
    @{ n = 'real_conso'; c = '[Console]::Clear()' }
    @{ n = 'real_cmd';   c = 'cmd.exe /c cls' }
)
foreach ($r in $real) {
    Set-Content -Path "$TempHome\p\$($r.n).ps1" -Value @(
        'param([string]$Done)'
        'Start-Sleep -Milliseconds 1500'
        '1..15 | ForEach-Object { [Console]::WriteLine("line$_") }'
        'Start-Sleep -Milliseconds 700'
        $r.c
        'Start-Sleep -Milliseconds 700'
        'Set-Content -Path $Done -Value 1'
        'Start-Sleep 600'
    )
}
# Writes what the suite hands it, as often as it is asked.
Set-Content -Path "$TempHome\emit.ps1" -Value @(
    'param([string]$Dir, [string]$Name)'
    '[Console]::OutputEncoding = [Text.Encoding]::UTF8'
    '$o = [Console]::OpenStandardOutput()'
    'Set-Content -Path "$Dir/$Name.ready" -Value 1'
    'while ($true) {'
    '    if (Test-Path "$Dir/$Name.cmd") {'
    '        $b = [IO.File]::ReadAllBytes("$Dir/$Name.cmd"); Remove-Item "$Dir/$Name.cmd"'
    '        $o.Write($b, 0, $b.Length); $o.Flush()'
    '        Set-Content -Path "$Dir/$Name.ack" -Value 1'
    '    }'
    '    Start-Sleep -Milliseconds 50'
    '}'
)
# A prompt for Ctrl+L, with no predictions drawn under what is typed. The
# prompt is short and fixed: the default one shows the working directory, which
# wraps onto a second row in a 40 column pane when the suite runs from a long
# path, and the cursor then sits on row 1 after Ctrl+L.
Set-Content -Path "$TempHome\prompt.ps1" -Value @(
    'Set-PSReadLineOption -PredictionSource None'
    'function global:prompt { "PS> " }'
)
$pwsh = (Get-Command pwsh).Source.Replace('\', '/')
$fwd = $TempHome.Replace('\', '/')
$script:NS = ""
function P { & $PSMUX -L $script:NS @args 2>&1 }
function All($t) {
    $rows = @(P capture-pane -p -t $t -S - | ForEach-Object { "$_".TrimEnd() })
    while ($rows.Count -gt 0 -and -not $rows[-1]) { $rows = @($rows | Select-Object -SkipLast 1) }
    $rows -join '|'
}
function Hist($t) { "$(P display -p -t $t '#{history_size}')" }
function Wait-File($path, $seconds = 20) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline -and -not (Test-Path $path)) { Start-Sleep -Milliseconds 50 }
    Test-Path $path
}
function Start-Emit($window, $name) {
    Remove-Item "$TempHome\$name.ready", "$TempHome\$name.ack", "$TempHome\$name.cmd" -Force -EA SilentlyContinue
    P new-window -d -t s -n $window "& '$pwsh' -NoProfile -File $fwd/emit.ps1 $fwd $name" | Out-Null
    Wait-File "$TempHome\$name.ready" | Out-Null
}
# Hand a pane's program bytes, and wait until they reached the pane's screen.
function Emit($name, $text) {
    Remove-Item "$TempHome\$name.ack" -Force -EA SilentlyContinue
    $tmp = "$TempHome\$name.tmp"
    [IO.File]::WriteAllBytes($tmp, [Text.Encoding]::UTF8.GetBytes($text))
    Move-Item -Force $tmp "$TempHome\$name.cmd"
    Wait-File "$TempHome\$name.ack" | Out-Null
    Start-Sleep -Milliseconds 600
}
# One row of text then a clear: 1 more row in history means it was kept. From
# the top left corner each time, as CSI 2 J leaves the cursor where it is and the
# blank rows above it would go to history too.
function Grew($name, $t) {
    $before = [int](Hist $t)
    Emit $name "$E[Hx`r`n"
    Emit $name "$E[2J"
    [int](Hist $t) - $before
}

function Run-Cases($route) {
    Get-ChildItem "$TempHome\p\*.done" -EA SilentlyContinue | Remove-Item -Force
    $script:NS = "soc_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 40 -y 10 | Out-Null
    Start-Sleep -Milliseconds 2000
    foreach ($c in $cases) {
        P new-window -d -t s -n $c.n "& '$pwsh' -NoProfile -File $fwd/show.ps1 $fwd/p/$($c.n).bin" | Out-Null
    }
    foreach ($r in $real) {
        P new-window -d -t s -n $r.n "& '$pwsh' -NoProfile -File $fwd/p/$($r.n).ps1 $fwd/p/$($r.n).done" | Out-Null
    }
    $count = $cases.Count + $real.Count
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline -and @(Get-ChildItem "$TempHome\p\*.done" -EA SilentlyContinue).Count -lt $count) {
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Milliseconds 1500
    foreach ($c in $cases) {
        $want = if ($route -eq 'inbox conhost' -and $Win10 -and $c.ContainsKey('win10')) { $c.win10 }
                elseif ($route -eq 'inbox conhost' -and $c.ContainsKey('inbox')) { $c.inbox } else { $c.a }
        Check "${route}: $($c.n)" (All "s:$($c.n)") $want
    }
    Check "${route}: alt_ris left the alternate screen" "$(P display -p -t 's:alt_ris' '#{alternate_on}')" '0'
    foreach ($r in $real) {
        if ($route -eq 'inbox conhost' -and $Win10) {
            # The clear never reaches psmux: what scrolled off stays.
            Check "${route}: $($r.n) leaves what scrolled off" (All "s:$($r.n)") (Want 6)
        } else {
            Check "${route}: $($r.n) leaves nothing" (All "s:$($r.n)") ''
        }
    }

    # The console host's own redraws send nothing to history.
    Start-Emit 'sizes' 'sizes'
    Emit 'sizes' (Lines 15)
    $before = Hist 's:sizes'
    foreach ($s in @(@(40, 14), @(40, 8), @(60, 8), @(30, 8), @(40, 10))) {
        P resize-window -t 's:sizes' -x $s[0] -y $s[1] | Out-Null
        Start-Sleep -Milliseconds 900
    }
    Check "${route}: resizing sends nothing to history" (Hist 's:sizes') $before
    $seen = @((All 's:sizes') -split '\|' | Where-Object { $_ })
    Check "${route}: resizing copies no line" (@($seen | Group-Object | Where-Object { $_.Count -gt 1 }).Count) 0

    # send-keys -R: the screen joins the history, and the program gets nothing.
    Start-Emit 'keysr' 'keysr'
    Emit 'keysr' (Lines 5)
    P send-keys -R -t 's:keysr' | Out-Null
    Start-Sleep -Milliseconds 800
    Check "${route}: send-keys -R sends the screen to history" (Hist 's:keysr') '5'

    # Ctrl+L at a prompt, as a person presses it.
    P new-window -d -t s -n ctrll "& '$pwsh' -NoProfile -NoExit -File $fwd/prompt.ps1" | Out-Null
    Start-Sleep -Milliseconds 3500
    P send-keys -t 's:ctrll' -l '1..12 | % { "mark$_" }' | Out-Null
    Start-Sleep -Milliseconds 300
    P send-keys -t 's:ctrll' Enter | Out-Null
    Start-Sleep -Milliseconds 1200
    P send-keys -t 's:ctrll' C-l | Out-Null
    Start-Sleep -Milliseconds 1200
    $rows = (All 's:ctrll') -split '\|'
    foreach ($m in 'mark1', 'mark6', 'mark12') {
        # On the Windows 10 inbox conhost the clear is a redraw, so the marks
        # still on the screen are lost; mark1 had scrolled off.
        $once = if ($route -eq 'inbox conhost' -and $Win10 -and $m -ne 'mark1') { 0 } else { 1 }
        Check "${route}: Ctrl+L keeps $m once" (@($rows | Where-Object { $_ -ceq $m }).Count) $once
    }
    Check "${route}: Ctrl+L leaves the prompt on the top row" "$(P display -p -t 's:ctrll' '#{cursor_y}')" '0'

    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

function Run-Options($route) {
    $script:NS = "soc_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 40 -y 10 | Out-Null
    Start-Sleep -Milliseconds 2000
    Check "${route}: the default" "$(P show -gv scroll-on-clear)" 'on'
    Check "${route}: the format" "$(P display -p '#{scroll-on-clear}')" 'on'

    Start-Emit 'a' 'a'
    Start-Emit 'b' 'b'
    Check "${route}: on by default" (Grew 'a' 's:a') 1

    # set -g reaches panes that already run, and new ones.
    P set -g scroll-on-clear off | Out-Null
    Start-Sleep -Milliseconds 300
    Check "${route}: set -g off" (Grew 'a' 's:a') 0
    Start-Emit 'c' 'c'
    Check "${route}: a new window under set -g off" (Grew 'c' 's:c') 0
    P set -g scroll-on-clear on | Out-Null
    Start-Sleep -Milliseconds 300
    Check "${route}: set -g on" (Grew 'a' 's:a') 1
    Check "${route}: set -g on reaches the other window" (Grew 'c' 's:c') 1

    # A flag with no value flips.
    P set -g scroll-on-clear | Out-Null
    Check "${route}: set -g with no value flips it" "$(P show -gv scroll-on-clear)" 'off'
    P set -g scroll-on-clear | Out-Null
    Check "${route}: and back" "$(P show -gv scroll-on-clear)" 'on'

    # set -gu goes back to the default.
    P set -g scroll-on-clear off | Out-Null
    P set -gu scroll-on-clear | Out-Null
    Start-Sleep -Milliseconds 300
    Check "${route}: set -gu is on again" "$(P show -gv scroll-on-clear)" 'on'
    Check "${route}: and the pane follows" (Grew 'a' 's:a') 1

    # set -w: this window only, until set -wu.
    P set -w -t s:a scroll-on-clear off | Out-Null
    Start-Sleep -Milliseconds 300
    Check "${route}: set -w off" (Grew 'a' 's:a') 0
    Check "${route}: another window is untouched" (Grew 'b' 's:b') 1
    Check "${route}: show -w" "$(P show -wv -t s:a scroll-on-clear)" 'off'
    P set -wu -t s:a scroll-on-clear | Out-Null
    Start-Sleep -Milliseconds 300
    Check "${route}: set -wu inherits again" (Grew 'a' 's:a') 1

    # set -wg, the form tmux users write for a window option.
    P set -wg scroll-on-clear off | Out-Null
    Start-Sleep -Milliseconds 300
    Check "${route}: set -wg off" (Grew 'a' 's:a') 0
    Check "${route}: show -wg" "$(P show -wgv scroll-on-clear)" 'off'
    P set -wg scroll-on-clear on | Out-Null
    Start-Sleep -Milliseconds 300
    Check "${route}: set -wg on" (Grew 'a' 's:a') 1

    # set -p: this pane only, over its window, until set -pu.
    Remove-Item "$TempHome\d.ready" -Force -EA SilentlyContinue
    P split-window -d -t s:a "& '$pwsh' -NoProfile -File $fwd/emit.ps1 $fwd d" | Out-Null
    Wait-File "$TempHome\d.ready" | Out-Null
    P set -p -t s:a.1 scroll-on-clear off | Out-Null
    Start-Sleep -Milliseconds 300
    Check "${route}: set -p off" (Grew 'd' 's:a.1') 0
    Check "${route}: the pane beside it is untouched" (Grew 'a' 's:a.0') 1
    P set -w -t s:a scroll-on-clear on | Out-Null
    Start-Sleep -Milliseconds 300
    Check "${route}: set -p wins over set -w" (Grew 'd' 's:a.1') 0

    # clear-history gives the pane a new parser, and the option stays.
    P select-window -t s:a | Out-Null
    P select-pane -t s:a.1 | Out-Null
    P clear-history | Out-Null
    Start-Sleep -Milliseconds 300
    Check "${route}: clear-history emptied it" (Hist 's:a.1') '0'
    Check "${route}: the option survives clear-history" (Grew 'd' 's:a.1') 0

    P set -pu -t s:a.1 scroll-on-clear | Out-Null
    Start-Sleep -Milliseconds 300
    Check "${route}: set -pu inherits again" (Grew 'd' 's:a.1') 1

    # A pane moved to another window resolves against that window.
    P set -w -t s:a scroll-on-clear off | Out-Null
    Start-Sleep -Milliseconds 300
    Check "${route}: under set -w off" (Grew 'd' 's:a.1') 0
    P break-pane -d -s s:a.1 -n moved | Out-Null
    Start-Sleep -Milliseconds 500
    Check "${route}: break-pane leaves the window's value behind" (Grew 'd' 's:moved') 1
    P set -wu -t s:a scroll-on-clear | Out-Null

    # A respawned pane keeps its own value.
    P set -p -t s:b scroll-on-clear off | Out-Null
    Start-Sleep -Milliseconds 300
    Remove-Item "$TempHome\b.ready" -Force -EA SilentlyContinue
    P respawn-pane -k -t s:b "& '$pwsh' -NoProfile -File $fwd/emit.ps1 $fwd b" | Out-Null
    Wait-File "$TempHome\b.ready" | Out-Null
    Start-Sleep -Milliseconds 500
    Check "${route}: respawn-pane keeps the pane's value" (Grew 'b' 's:b') 0

    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

# The same option from a config file, which has its own route to the setter.
function Run-Config($route) {
    foreach ($line in 'set -g scroll-on-clear off', 'setw -g scroll-on-clear off') {
        Set-Content -Path $env:PSMUX_CONFIG_FILE -Value $line
        $script:NS = "soc_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
        P new-session -d -s s -x 40 -y 10 | Out-Null
        Start-Sleep -Milliseconds 2000
        Check "${route}: '$line' in the config" "$(P show -gv scroll-on-clear)" 'off'
        Start-Emit 'cfg' 'cfg'
        Check "${route}: and a pane follows '$line'" (Grew 'cfg' 's:cfg') 0
        P kill-server | Out-Null
        Start-Sleep -Milliseconds 500
    }
    Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""
}

# A shell from the warm pool moves with `cd` and a clear, which must not leave
# its first prompt in the history of the new pane.
function Run-Warm($route) {
    Remove-Item Env:PSMUX_NO_WARM -EA SilentlyContinue
    $script:NS = "soc_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 40 -y 10 | Out-Null
    Start-Sleep -Milliseconds 5000
    P new-window -d -t s -n warm -c $TempHome | Out-Null
    Start-Sleep -Milliseconds 3000
    Check "${route}: a warm pane moved with cd starts with no history" (Hist 's:warm') '0'
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    $env:PSMUX_NO_WARM = "1"
}

Write-Host "`n=== scroll-on-clear ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] the inbox conhost" -ForegroundColor Yellow
    Run-Cases 'inbox conhost'
    # The option needs a clear that reaches psmux, which the Windows 10 inbox
    # conhost never passes on: there it is checked through OpenConsole.
    if (-not $Win10) {
        Run-Options 'inbox conhost'
        Run-Config 'inbox conhost'
    }
    Run-Warm 'inbox conhost'

    Write-Host "`n[2] OpenConsole" -ForegroundColor Yellow
    if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
        $env:PSMUX_CONPTY_DIR = $ConptyDir
        Run-Cases 'OpenConsole'
        if ($Win10) {
            Run-Options 'OpenConsole'
            Run-Config 'OpenConsole'
        }
        Run-Warm 'OpenConsole'
        Remove-Item Env:PSMUX_CONPTY_DIR
    } else {
        Write-Info "skipped: set PSMUX_TEST_CONPTY_DIR to a folder holding conpty.dll and OpenConsole.exe"
        if ($Win10) { Write-Info "the option is not checked: the Windows 10 inbox conhost passes no clear on" }
    }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    if ($script:NS) { P kill-server | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
