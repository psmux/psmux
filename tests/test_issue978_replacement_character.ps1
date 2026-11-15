# Issue #978: U+FFFD, the replacement character, is a character of one cell,
# as in tmux (input.c:783-793 at tag 3.8, input_stop_utf8, writes it for a
# byte that is no UTF-8). psmux's vt100 dropped it (perform.rs), and both
# console hosts hand psmux U+FFFD for a lone 8-bit C1 byte, a byte such as
# 0xFF and a cut off sequence, so for all of them psmux kept nothing while the
# console host, and tmux, kept one cell: the rest of the line was one column
# to the left.
#
# Each case is written as raw bytes by a program in a pane, which then records
# where the console host left ITS cursor ([Console]::CursorLeft). Every
# expectation is what tmux 3.8 kept for the same bytes (row text and
# cursor_x), and psmux's cursor has to be where the console host's is.
# U+009B and U+0085 written as UTF-8 are no cell in the console hosts and
# stay none in psmux (tmux keeps them as zero width characters).
#
# The inbox conhost of Windows 10 (19045) is another matter, and there the
# row and cursor have to be its own: it drops a byte that is no UTF-8 rather
# than writing U+FFFD, reads U+009B as a CSI (so `b` repeats the `a`), and
# keeps U+0085 as a cell of one column, which psmux shows as U+FFFD.
#
# [1] the inbox conhost; [2] OpenConsole (PSMUX_CONPTY_DIR), when
# PSMUX_TEST_CONPTY_DIR names a folder holding conpty.dll, skipped otherwise.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per server;
# cleanup is `-L <ns> kill-server`. PSMUX_EXE picks the binary (defaults to
# target\release\psmux.exe).

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -and $_.Name -ne 'PSMUX_TEST_CONPTY_DIR' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
$PSMUX = $env:PSMUX_EXE
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
$ConptyDir = if ($env:PSMUX_TEST_CONPTY_DIR) { $env:PSMUX_TEST_CONPTY_DIR } else { '' }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_rc_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force "$TempHome\p" | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$R = [string][char]0xFFFD
# name, raw bytes, the row tmux 3.8 kept, its cursor_x, and the row and
# cursor_x of the Windows 10 inbox conhost
$cases = @(
    @('fffd',      [byte[]](0x61, 0xEF, 0xBF, 0xBD, 0x62, 0x7C),             "a${R}b|", 4, "a${R}b|", 4),
    @('c1_9b',     [byte[]](0x61, 0x9B, 0x62, 0x7C),                         "a${R}b|", 4, 'ab|', 3),
    @('c1_85',     [byte[]](0x61, 0x85, 0x62, 0x7C),                         "a${R}b|", 4, 'ab|', 3),
    @('byte_ff',   [byte[]](0x61, 0xFF, 0x62, 0x7C),                         "a${R}b|", 4, 'ab|', 3),
    @('cut_off',   [byte[]](0x61, 0xE2, 0x82, 0x62, 0x7C),                   "a${R}b|", 4, 'ab|', 3),
    @('cut_esc',   [byte[]](0x61, 0xE2, 0x82, 0x1B, 0x5B, 0x6D, 0x62, 0x7C), "a${R}b|", 4, 'ab|', 3),
    @('two',       [byte[]](0x78, 0xFF, 0xFE, 0x79),                         "x${R}${R}y", 4, 'xy', 2),
    @('u009b',     [byte[]](0x61, 0xC2, 0x9B, 0x62, 0x7C),                   "ab|", 3, 'aa|', 3),
    @('u0085',     [byte[]](0x61, 0xC2, 0x85, 0x62, 0x7C),                   "ab|", 3, "a${R}b|", 4)
)
# The inbox conhost before Windows 11 (measured on 19045).
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000
foreach ($c in $cases) { [IO.File]::WriteAllBytes("$TempHome\p\$($c[0]).bin", $c[1]) }
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
    $script:NS = "rc_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 80 -y 10 | Out-Null
    Start-Sleep -Milliseconds 2000
    foreach ($c in $cases) {
        P new-window -d -t s -n $c[0] "$pwsh -NoProfile -File $fwd/show.ps1 $fwd/p/$($c[0]).bin" | Out-Null
    }
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline -and @(Get-ChildItem "$TempHome\p\*.host" -EA SilentlyContinue).Count -lt $cases.Count) {
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Milliseconds 800
    foreach ($c in $cases) {
        $name = $c[0]
        $row = "$((P capture-pane -p -t "s:$name") | Select-Object -First 1)".TrimEnd()
        $cursor = "$(P display -p -t "s:$name" '#{cursor_x}')"
        $hostCol = "$((Get-Content "$TempHome\p\$name.bin.host" -EA SilentlyContinue) -join '')"
        $w = if ($route -eq 'inbox conhost' -and $Win10) { 4 } else { 2 }
        Check "${route}: $name row" $row $c[$w]
        Check "${route}: $name cursor_x" $cursor $c[$w + 1]
        Check "${route}: $name cursor_x is the console host's" $cursor $hostCol
    }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== the replacement character ===" -ForegroundColor Cyan
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
