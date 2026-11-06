# Issue #995: REP (CSI Ps b), autowrap off (CSI ? 7 l) and insert mode
# (CSI 4 h) in a pane, as tmux handles them (input.c:1799-1818 for REP,
# :1946 and :2043 for DECAWM, :2007 and :1910 for IRM, and screen-write.c:
# 2695-2707 and :2790-2794 at tag 3.8). psmux's vt100 ignored all three, so
# with OpenConsole (PSMUX_CONPTY_DIR, which hands psmux the program's output
# as written) `a ESC[3b` stayed `a`, a line kept wrapping with autowrap off,
# and insert mode overwrote. The inbox conhost handles them itself before
# psmux reads the output, so that route was right before and must stay so.
#
# Each case is written as raw bytes by a program in a 20 column pane, which
# then records where the console host left ITS cursor
# ([Console]::CursorLeft). Every expectation is what tmux 3.8 kept for the
# same bytes (the first two rows and cursor_x), and psmux's cursor has to be
# where the console host's is.
#
# [1] the inbox conhost; [2] OpenConsole (PSMUX_CONPTY_DIR), when
# PSMUX_TEST_CONPTY_DIR names a folder holding conpty.dll, skipped otherwise.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per server;
# cleanup is `-L <ns> kill-server`. PSMUX_TEST_BIN or PSMUX_EXE picks the
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_rai_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force "$TempHome\p" | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$E = [string][char]27
$W = [string][char]0x3042   # a wide character
$Line = '0123456789ABCDEFGHIJKL'   # 22 characters for a 20 column pane
# name, the text written, rows 0 and 1 as tmux 3.8 kept them, its cursor_x
$cases = @(
    # B1: REP repeats the last character printed, n times.
    @('rep',          "a${E}[3bX${E}[2b|",            'aaaaXXX|', '', 8),
    # A CSI or a C0 between them, and nothing is repeated.
    @('rep_csi',      "a${E}[C${E}[3b|",              'a |', '', 3),
    @('rep_c0',       "a$([char]7)${E}[3b|",          'a|', '', 2),
    # No character printed yet in this line of work: nothing to repeat.
    @('rep_none',     "${E}[3bX",                     'X', '', 1),
    # Past the right margin REP wraps and goes on, as both console hosts do
    # (and xterm); tmux stops at the margin, `                 aaa` with
    # cursor_x 20. psmux follows the host here (Discussion #937).
    @('rep_wrap',     "${E}[18Ga${E}[9b",             '                 aaa', 'aaaaaaa', 7),
    # The largest count: the screen fills, and psmux ends where the host does.
    @('rep_max',      "a${E}[65535b|",                'aaaaaaaaaaaaaaaaaaaa', 'aaaaaaaaaaaaaaaaaaaa', 17),
    @('rep_wide',     "${W}${E}[2b|",                 "${W}${W}${W}|", '', 7),
    # B3: with autowrap off the last column is overwritten.
    @('awm_off',      "${E}[?7l${Line}",              '0123456789ABCDEFGHIL', '', 19),
    @('awm_back_on',  "${E}[?7l${Line}${E}[?7hMN",    '0123456789ABCDEFGHIM', 'N', 1),
    # A wide character that does not fit is dropped.
    @('awm_wide',     "${E}[?7l$('a' * 19)${W}|",     "$('a' * 19)|", '', 19),
    # B4: insert mode moves the rest of the line right.
    @('irm',          "abcdef`r${E}[4hXY${E}[4l",     'XYabcdef', '', 2),
    @('irm_off',      "abcdef`r${E}[4hXY${E}[4lZ",    'XYZbcdef', '', 3),
    @('irm_wide',     "abcdef`r${E}[4h${W}${E}[4l",   "${W}abcdef", '', 2),
    # What is pushed past the right margin is lost, not wrapped.
    @('irm_full',     "$($Line.Substring(0, 20))`r${E}[4hX${E}[4l", 'X0123456789ABCDEFGHI', '', 1)
)
foreach ($c in $cases) { [IO.File]::WriteAllBytes("$TempHome\p\$($c[0]).bin", [Text.Encoding]::UTF8.GetBytes($c[1])) }
Set-Content -Path "$TempHome\show.ps1" -Value @(
    'param([string]$Payload)'
    '[Console]::OutputEncoding = [Text.Encoding]::UTF8'
    '$o = [Console]::OpenStandardOutput(); $b = [IO.File]::ReadAllBytes($Payload); $o.Write($b, 0, $b.Length); $o.Flush()'
    'Set-Content -Path ($Payload + ".host") -Value ([Console]::CursorLeft)'
    'Start-Sleep 600'
)
$pwsh = (Get-Command pwsh).Source.Replace('\', '/')
$fwd = $TempHome.Replace('\', '/')
$script:NS = ""
function P { & $PSMUX -L $script:NS @args 2>&1 }

function Run-Cases($route) {
    Get-ChildItem "$TempHome\p\*.host" -EA SilentlyContinue | Remove-Item -Force
    $script:NS = "rai_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 20 -y 10 | Out-Null
    Start-Sleep -Milliseconds 2000
    foreach ($c in $cases) {
        P new-window -d -t s -n $c[0] "& '$pwsh' -NoProfile -File $fwd/show.ps1 $fwd/p/$($c[0]).bin" | Out-Null
    }
    $deadline = (Get-Date).AddSeconds(40)
    while ((Get-Date) -lt $deadline -and @(Get-ChildItem "$TempHome\p\*.host" -EA SilentlyContinue).Count -lt $cases.Count) {
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Milliseconds 800
    foreach ($c in $cases) {
        $name = $c[0]
        $rows = @(P capture-pane -p -t "s:$name")
        $row0 = "$($rows | Select-Object -First 1)".TrimEnd()
        $row1 = "$($rows | Select-Object -Skip 1 -First 1)".TrimEnd()
        $width = "$(P display -p -t "s:$name" '#{pane_width}')"
        $cursor = "$(P display -p -t "s:$name" '#{cursor_x}')"
        $hostCol = "$((Get-Content "$TempHome\p\$name.bin.host" -EA SilentlyContinue) -join '')"
        if ($width -ne '20') { Write-Fail "${route}: $name pane is $width columns, not 20"; continue }
        Check "${route}: $name row 0" $row0 $c[2]
        Check "${route}: $name row 1" $row1 $c[3]
        Check "${route}: $name cursor_x" $cursor $c[4]
        Check "${route}: $name cursor_x is the console host's" $cursor $hostCol
    }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

# DECRQM for DECAWM and IRM: 1 set, 2 reset, as tmux answers from MODE_WRAP
# and MODE_INSERT (input.c:1641-1643 and :1662-1664 at tag 3.8). The inbox
# conhost answers itself; through OpenConsole psmux answers, and said 0, "not
# recognised", for both before it kept the modes.
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$probe = "$TempHome\decrqm_probe.exe"
if (Test-Path $csc) { & $csc /nologo /out:$probe (Join-Path $PSScriptRoot "decrqm_mode_probe_child.cs") 2>&1 | Out-Null }
$rqmWant = [ordered]@{
    awm_default = 'ESC[?7;1$y'; awm_off = 'ESC[?7;2$y'; awm_on_again = 'ESC[?7;1$y'
    irm_default = 'ESC[4;2$y'; irm_on = 'ESC[4;1$y'; irm_off_again = 'ESC[4;2$y'
}
function Run-Decrqm($route) {
    if (-not (Test-Path $probe)) { Write-Info "${route}: DECRQM skipped, the probe did not compile"; return }
    $log = "$TempHome\decrqm.txt"
    Remove-Item $log -Force -EA SilentlyContinue
    $script:NS = "rai_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s q -x 80 -y 20 "$($probe.Replace('\', '/')) $($log.Replace('\', '/'))" | Out-Null
    $deadline = (Get-Date).AddSeconds(40)
    while ((Get-Date) -lt $deadline -and -not (Test-Path $log)) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 300
    $got = @{}
    foreach ($line in @(Get-Content $log -EA SilentlyContinue)) {
        $k, $v = $line -split ' = ', 2
        $got[$k] = "$v".Trim()
    }
    foreach ($k in $rqmWant.Keys) { Check "${route}: DECRQM $k" $got[$k] $rqmWant[$k] }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== REP, autowrap off and insert mode ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] the inbox conhost" -ForegroundColor Yellow
    Run-Cases 'inbox conhost'
    Run-Decrqm 'inbox conhost'

    Write-Host "`n[2] OpenConsole" -ForegroundColor Yellow
    if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
        $env:PSMUX_CONPTY_DIR = $ConptyDir
        Run-Cases 'OpenConsole'
        Run-Decrqm 'OpenConsole'
        Remove-Item Env:PSMUX_CONPTY_DIR
    } else {
        Write-Info "skipped: set PSMUX_TEST_CONPTY_DIR to a folder holding conpty.dll and OpenConsole.exe"
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
