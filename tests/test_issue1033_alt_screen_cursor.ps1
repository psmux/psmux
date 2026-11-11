# Follow up to #1032/#1033: the cursor across the alternate screen.
#
# tmux keeps one cursor for both screens: DECSET 47 and 1049 leave it where
# it was, DECRST 1049 puts back the one 1049 saved, and DECRST 47, which saves
# none, leaves it where the alternate screen had it (screen_alternate_on and
# screen_alternate_off at tag 3.8). Both console hosts do the same, and both
# ignore 1047 and 1048. psmux keeps a cursor per grid, so through OpenConsole
# (PSMUX_CONPTY_DIR, which hands psmux the program's output as written)
# `ESC[5;4H AB ESC[?1049h` left the pane cursor at 0,0 where the host had 5,4,
# and `... ESC[?47l` jumped back to 5,4 where the host had 20,8. The inbox
# conhost repaints with absolute moves, so that route was right already.
#
# Each case is written as raw bytes by a program in a 40x10 pane, which then
# records where the console host left ITS cursor. psmux's #{cursor_x},
# #{cursor_y} must equal the expectation (tmux 3.8, which the hosts match) and
# the host's own.
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_alt_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force "$TempHome\p" | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$E = [string][char]27
# name, the bytes written, the expected "cursor_x,cursor_y" and alternate_on.
$cases = @(
    @('h1049',      "${E}[5;4HAB${E}[?1049h",                         '5,4',  '1'),
    @('h1049_cha',  "${E}[4G${E}[?1049h",                             '3,0',  '1'),
    @('hl1049',     "${E}[5;4HAB${E}[?1049h${E}[9;20Hx${E}[?1049l",   '5,4',  '0'),
    @('h1049_move', "${E}[5;4HAB${E}[?1049h${E}[9;20Hx",              '20,8', '1'),
    @('h47',        "${E}[5;4HAB${E}[?47h",                           '5,4',  '1'),
    @('h47_cha',    "${E}[4G${E}[?47h",                               '3,0',  '1'),
    @('hl47',       "${E}[5;4HAB${E}[?47h${E}[9;20Hx${E}[?47l",       '20,8', '0'),
    @('h1047',      "${E}[5;4HAB${E}[?1047h",                         '5,4',  '0'),
    @('hl1047',     "${E}[5;4HAB${E}[?1047h${E}[9;20Hx${E}[?1047l",   '20,8', '0'),
    @('hl1048',     "${E}[5;4HAB${E}[?1048h${E}[9;20Hx${E}[?1048l",   '20,8', '0')
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

function Run-Cases($route) {
    Get-ChildItem "$TempHome\p\*.host" -EA SilentlyContinue | Remove-Item -Force
    $script:NS = "alt_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
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
        $alt = "$(P display -p -t "s:$name" '#{alternate_on}')"
        $hostPos = "$((Get-Content "$TempHome\p\$name.bin.host" -EA SilentlyContinue) -join '')"
        Check "${route}: $name cursor" $cursor $c[2]
        Check "${route}: $name cursor is the console host's" $cursor $hostPos
        Check "${route}: $name alternate_on" $alt $c[3]
    }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== Cursor across the alternate screen ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] the inbox conhost" -ForegroundColor Yellow
    Run-Cases 'inbox conhost'

    Write-Host "`n[2] OpenConsole" -ForegroundColor Yellow
    if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
        $env:PSMUX_CONPTY_DIR = $ConptyDir
        Run-Cases 'OpenConsole'
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
