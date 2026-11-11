# Issue #1034: DECALN (ESC # 8), DECSTBM (CSI Ps ; Ps r) and VPA (CSI Ps d) in origin mode in
# a pane, and #{scroll_region_upper}, #{scroll_region_lower} and #{origin_flag},
# as tmux handles them (screen-write.c:1316 and :1775, input.c:1439, :1620 and
# :1870, format.c:2834 and :2843 at tag 3.8), except where both console hosts
# do otherwise. psmux's vt100 ignored DECALN, read VPA from the top of the
# screen in origin mode, and sent the cursor to the top of the region on
# DECSTBM, falling back to the whole screen for a region it could not set, so
# with OpenConsole (PSMUX_CONPTY_DIR, which hands psmux the program's output as
# written) the cursor ended where the host's was not. The inbox conhost handles
# these itself and hands psmux the result.
#
# Each case is written as raw bytes by a program in a 40x10 pane, which then
# records where the console host left ITS cursor ([Console]::CursorLeft and
# CursorTop). Every expectation is what tmux 3.8 showed for the same bytes,
# and psmux's cursor has to be where the console host's is. Five cases follow
# the hosts where tmux differs; they say so.
#
# [1] the inbox conhost; [2] OpenConsole, when PSMUX_TEST_CONPTY_DIR or
# PSMUX_CONPTY_DIR_FOR_TEST names a folder holding conpty.dll, skipped
# otherwise.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per server;
# cleanup is `-L <ns> kill-server`. PSMUX_TEST_BIN or PSMUX_EXE picks the
# binary (defaults to target\release\psmux.exe).

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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_rgn_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force "$TempHome\p" | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$E = [string][char]27
$AllE = 'E' * 40
# name, the bytes written, the cursor as x,y, the region as upper-lower and
# origin_flag, and row 0 ('' when it stays blank).
$cases = @(
    @('decaln',            "${E}#8",                                  '0,0', '0-9', '0', $AllE),
    @('decaln_region',     "${E}[3;6r${E}[5;10H${E}#8",               '0,0', '0-9', '0', $AllE),
    @('decaln_text',       "${E}[41m${E}#8abc",                       '3,0', '0-9', '0', "abc$('E' * 37)"),
    # hosts: tmux keeps origin mode, origin_flag 1.
    @('decaln_origin',     "${E}[3;6r${E}[?6h${E}#8",                 '0,0', '0-9', '0', $AllE),
    # hosts: with origin mode kept, tmux lands on row 2.
    @('decaln_then_cup',   "${E}[?6h${E}#8${E}[3;6r${E}[1;1H",        '0,0', '2-5', '0', $AllE),
    @('stbm',              "${E}[5;10H${E}[3;6r",                     '0,0', '2-5', '0', ''),
    # hosts: tmux homes to the top left of the screen, 0,0.
    @('stbm_origin',       "${E}[?6h${E}[3;6r",                       '0,2', '2-5', '1', ''),
    @('stbm_invalid',      "${E}[3;6r${E}[5;10H${E}[6;3r",            '9,4', '2-5', '0', ''),
    @('stbm_one_line',     "${E}[3;6r${E}[5;10H${E}[5;5r",            '9,4', '2-5', '0', ''),
    # hosts: tmux cuts the bottom to row 9 and sets 2-9, cursor 0,0.
    @('stbm_past',         "${E}[5;10H${E}[3;20r",                    '9,4', '0-9', '0', ''),
    @('stbm_both_past',    "${E}[5;10H${E}[20;30r",                   '9,4', '0-9', '0', ''),
    @('stbm_reset',        "${E}[3;6r${E}[5;10H${E}[r",               '0,0', '0-9', '0', ''),
    # hosts: tmux reads 0;0 as one line and ignores it, 9,4 and 2-5.
    @('stbm_zero',         "${E}[3;6r${E}[5;10H${E}[0;0r",            '0,0', '0-9', '0', ''),
    @('stbm_bottom_only',  "${E}[5;10H${E}[;6r",                      '0,0', '0-5', '0', ''),
    @('stbm_top_only',     "${E}[5;10H${E}[3r",                       '0,0', '2-9', '0', ''),
    @('vpa',               "${E}[3;6r${E}[2d",                        '0,1', '2-5', '0', ''),
    @('vpa_origin',        "${E}[3;6r${E}[?6h${E}[2d",                '0,3', '2-5', '1', ''),
    @('vpa_origin_past',   "${E}[3;6r${E}[?6h${E}[9d",                '0,5', '2-5', '1', ''),
    @('vpa_column',        "${E}[3;6r${E}[?6h${E}[1;10H${E}[2d",      '9,3', '2-5', '1', ''),
    @('cup_origin',        "${E}[3;6r${E}[?6h${E}[2;4H",              '3,3', '2-5', '1', ''),
    @('decom_on',          "${E}[3;6r${E}[5;10H${E}[?6h",             '0,2', '2-5', '1', ''),
    @('decom_off',         "${E}[3;6r${E}[?6h${E}[2;5H${E}[?6l",      '0,0', '2-5', '0', ''),
    @('decsc_origin',      "${E}[3;6r${E}[?6h${E}7${E}[?6l${E}8",     '0,2', '2-5', '1', '')
)
foreach ($c in $cases) { [IO.File]::WriteAllBytes("$TempHome\p\$($c[0]).bin", [Text.Encoding]::UTF8.GetBytes($c[1])) }
Set-Content -Path "$TempHome\show.ps1" -Value @(
    'param([string]$Payload)'
    '[Console]::OutputEncoding = [Text.Encoding]::UTF8'
    '$o = [Console]::OpenStandardOutput(); $b = [IO.File]::ReadAllBytes($Payload); $o.Write($b, 0, $b.Length); $o.Flush()'
    'Set-Content -Path ($Payload + ".host") -Value ("{0},{1}" -f [Console]::CursorLeft, [Console]::CursorTop)'
    'Start-Sleep 600'
)
$pwsh = (Get-Command pwsh).Source.Replace('\', '/')
$fwd = $TempHome.Replace('\', '/')
$script:NS = ""
function P { & $PSMUX -L $script:NS @args 2>&1 }

function Run-Cases($route, $regionSeen) {
    Get-ChildItem "$TempHome\p\*.host" -EA SilentlyContinue | Remove-Item -Force
    $script:NS = "rgn_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 40 -y 10 | Out-Null
    Start-Sleep -Milliseconds 2000
    foreach ($c in $cases) {
        P new-window -d -t s -n $c[0] "& '$pwsh' -NoProfile -File $fwd/show.ps1 $fwd/p/$($c[0]).bin" | Out-Null
    }
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline -and @(Get-ChildItem "$TempHome\p\*.host" -EA SilentlyContinue).Count -lt $cases.Count) {
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Milliseconds 800
    foreach ($c in $cases) {
        $name = $c[0]
        $size = "$(P display -p -t "s:$name" '#{pane_width}x#{pane_height}')"
        if ($size -ne '40x10') { Write-Fail "${route}: $name pane is $size, not 40x10"; continue }
        $cursor = "$(P display -p -t "s:$name" '#{cursor_x},#{cursor_y}')"
        $hostPos = "$((Get-Content "$TempHome\p\$name.bin.host" -EA SilentlyContinue) -join '')"
        Check "${route}: $name cursor" $cursor $c[2]
        Check "${route}: $name cursor is the console host's" $cursor $hostPos
        $row0 = "$((P capture-pane -p -t "s:$name") | Select-Object -First 1)".TrimEnd()
        Check "${route}: $name row 0" $row0 $c[5]
        if ($regionSeen) {
            Check "${route}: $name region" "$(P display -p -t "s:$name" '#{scroll_region_upper}-#{scroll_region_lower}')" $c[3]
            Check "${route}: $name origin_flag" "$(P display -p -t "s:$name" '#{origin_flag}')" $c[4]
        }
    }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== Scroll region, DECALN and origin mode ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] the inbox conhost" -ForegroundColor Yellow
    Run-Cases 'inbox conhost' $false

    Write-Host "`n[2] OpenConsole" -ForegroundColor Yellow
    if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
        $env:PSMUX_CONPTY_DIR = $ConptyDir
        Run-Cases 'OpenConsole' $true
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
