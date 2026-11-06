# Issue #997: the DEC Special Graphics set, the line drawing set, in a pane:
# `ESC ( 0` into G0, `ESC ) 0` into G1, `ESC ( B` / `ESC ) B` for ASCII,
# SI and SO to choose (input.c:241-249, :1372-1377, :1442-1453 at tag 3.8).
# psmux's vt100 ignored them, so with OpenConsole (PSMUX_CONPTY_DIR, which
# hands psmux the program's output as written) `ESC(0 lqqk` stayed `lqqk`
# while the console host held the box. The inbox conhost turns the letters
# into box drawing characters before psmux reads the output, so that route
# was right before and must stay so.
#
# Each case is written as raw bytes by a program in a 40 column pane, which
# then reads the console host's own row (ReadConsoleOutputCharacterW) and
# cursor. psmux's row and cursor have to be the host's, and the expectations
# are what both hosts hold. tmux keeps the letter and draws the box character
# only on output (capture-pane -p shows `lqqk`), and its table (tty-acs.c:
# 31-68) also turns `+ , - . 0` into arrows and a block and `` ` `` into a
# different diamond; psmux follows the host (Discussion #937).
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_ldc_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force "$TempHome\p" | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$E = [string][char]27
$SO = [string][char]14
$SI = [string][char]15
$W = [string][char]0x3042   # a wide character
function U([int[]]$codes) { -join ($codes | ForEach-Object { [string][char]$_ }) }
$Box = U 0x250C, 0x2500, 0x2500, 0x2510
$H = [string][char]0x2500
# name, the text written, row 0 as both console hosts hold it, cursor_x
$cases = @(
    @('g0',      "${E}(0lqqk${E}(B lqqk", "$Box lqqk", 9),
    # The whole set, as both hosts map it: `_` is a blank and `` ` `` is ♦.
    @('set_a',   "${E}(0``abcdefghijklm${E}(B", (U 0x2666, 0x2592, 0x2409, 0x240C, 0x240D, 0x240A, 0xB0, 0xB1, 0x2424, 0x240B, 0x2518, 0x2510, 0x250C, 0x2514), 14),
    @('set_n',   "${E}(0nopqrstuvwxyz{|}~${E}(B", (U 0x253C, 0x23BA, 0x23BB, 0x2500, 0x23BC, 0x23BD, 0x251C, 0x2524, 0x2534, 0x252C, 0x2502, 0x2264, 0x2265, 0x3C0, 0x2260, 0xA3, 0xB7), 17),
    @('blank',   "${E}(0a_a${E}(B", ((U 0x2592) + ' ' + (U 0x2592)), 3),
    # Not in the set: kept, where tmux's table has arrows and a block for
    # `+ , - . 0`.
    @('kept',    "${E}(0+,-.0ABC^]${E}(B", '+,-.0ABC^]', 10),
    @('wide',    "${E}(0q${W}q${E}(B", "$H$W$H", 4),
    # G1 with SO and SI.
    @('g1',      "${E})0${SO}lqqk${SI}lqqk", "${Box}lqqk", 8),
    @('g0_so',   "${E}(0${SO}q${SI}q${E}(B", "q$H", 2),
    # REP repeats the letter in the set in use.
    @('rep',     "${E}(0q${E}[3b${E}(B|", "$H$H$H$H|", 5),
    # DECSC saves the sets, DECRC restores them; RIS goes back to ASCII.
    @('decsc',   "${E}(0${E}7${E}(B${E}8q", $H, 1),
    @('ris',     "${E}(0${E}cq", 'q', 1)
)
foreach ($c in $cases) { [IO.File]::WriteAllBytes("$TempHome\p\$($c[0]).bin", [Text.Encoding]::UTF8.GetBytes($c[1])) }
Set-Content -Path "$TempHome\show.ps1" -Value @(
    'param([string]$Payload)'
    '[Console]::OutputEncoding = [Text.Encoding]::UTF8'
    'Add-Type -TypeDefinition @"'
    'using System; using System.Runtime.InteropServices; using System.Text;'
    'public static class HostRow {'
    '  [DllImport("kernel32.dll")] static extern IntPtr GetStdHandle(int n);'
    '  [StructLayout(LayoutKind.Sequential)] struct COORD { public short X; public short Y; }'
    '  [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern bool ReadConsoleOutputCharacterW(IntPtr h, StringBuilder s, uint n, COORD c, out uint read);'
    '  public static string Read(short y, uint n) { var sb = new StringBuilder((int)n + 4); var c = new COORD(); c.Y = y; uint r; ReadConsoleOutputCharacterW(GetStdHandle(-11), sb, n, c, out r); return sb.ToString(0, (int)r); }'
    '}'
    '"@'
    '$o = [Console]::OpenStandardOutput(); $b = [IO.File]::ReadAllBytes($Payload); $o.Write($b, 0, $b.Length); $o.Flush()'
    '$x = [Console]::CursorLeft; $row = [HostRow]::Read([int16][Console]::CursorTop, 40).TrimEnd()'
    '[IO.File]::WriteAllText($Payload + ".host", "$x`n$row", [Text.Encoding]::UTF8)'
    'Start-Sleep 600'
)
$pwsh = (Get-Command pwsh).Source.Replace('\', '/')
$fwd = $TempHome.Replace('\', '/')
$script:NS = ""
function P { & $PSMUX -L $script:NS @args 2>&1 }

function Run-Cases($route) {
    Get-ChildItem "$TempHome\p\*.host" -EA SilentlyContinue | Remove-Item -Force
    $script:NS = "ldc_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
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
        $row = "$(P capture-pane -p -t "s:$name" | Select-Object -First 1)".TrimEnd()
        $width = "$(P display -p -t "s:$name" '#{pane_width}')"
        $cursor = "$(P display -p -t "s:$name" '#{cursor_x}')"
        $hostLines = @(([IO.File]::ReadAllText("$TempHome\p\$name.bin.host", [Text.Encoding]::UTF8) -split "`n"))
        if ($width -ne '40') { Write-Fail "${route}: $name pane is $width columns, not 40"; continue }
        Check "${route}: $name row" $row $c[2]
        Check "${route}: $name row is the console host's" $row $hostLines[1]
        Check "${route}: $name cursor_x" $cursor $c[3]
        Check "${route}: $name cursor_x is the console host's" $cursor $hostLines[0]
    }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== the line drawing set ===" -ForegroundColor Cyan
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
