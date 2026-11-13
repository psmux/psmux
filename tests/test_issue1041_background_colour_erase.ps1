# Issue #1041: background colour erase in a pane: a line scrolled in by LF, IND, NEL, RI,
# SU or SD, an inserted or deleted line or character (IL, DL, ICH, DCH) and an
# erase (ED, EL, ECH) leave blank cells with the current background and
# nothing else, as tmux does (`grid_clear_cell`, grid.c:264 at tag 3.8, called
# with `ictx->cell.cell.bg` from input.c:1365-1425 and 1613-1852). psmux's
# vt100 left the first group in the default colours and gave the erases every
# attribute, so with OpenConsole (PSMUX_CONPTY_DIR, which hands psmux the
# program's output as written) an underline or reverse ran across erased
# cells and scrolled in lines had no colour. The inbox conhost does all of
# this itself and hands psmux the result.
#
# Each case is written as raw bytes by a program in a 40x10 pane, which then
# reads the console host's OWN cells (ReadConsoleOutputAttribute and
# ReadConsoleOutputCharacter) and cursor. Every expectation is what tmux 3.8
# showed for the same bytes, and psmux's cells have to be the console host's.
# Two cases differ; they say so.
#
# One letter a cell: . default, R red background and nothing else, B bright
# red background, U underline, V reverse, F red foreground, ? anything else.
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_bce_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force "$TempHome\p" | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$E = [string][char]27
# Rows 0-9 hold aaaa... to jjjj..., 20 letters each, so that shifts show.
$fill = ""
for ($r = 1; $r -le 10; $r++) { $fill += "${E}[$r;1H" + ([string][char](96 + $r)) * 20 }
# name, the bytes written after the fill, and the rows that are not all
# default as row=count letter..., e.g. 4=10.3R is ten default cells and three
# red ones on row 4.
$cases = @(
    @('lf_bottom',    "${E}[10;1H${E}[41m`n",            '9=40R'),
    @('ind_bottom',   "${E}[10;1H${E}[41m${E}D",         '9=40R'),
    @('nel_bottom',   "${E}[10;1H${E}[41m${E}E",         '9=40R'),
    @('ri_top',       "${E}[1;1H${E}[41m${E}M",          '0=40R'),
    @('su2',          "${E}[41m${E}[2S",                 '8=40R 9=40R'),
    @('sd2',          "${E}[41m${E}[2T",                 '0=40R 1=40R'),
    @('il2',          "${E}[5;1H${E}[41m${E}[2L",        '4=40R 5=40R'),
    @('dl2',          "${E}[5;1H${E}[41m${E}[2M",        '8=40R 9=40R'),
    @('ich3',         "${E}[5;11H${E}[41m${E}[3@",       '4=10.3R'),
    @('dch3',         "${E}[5;11H${E}[41m${E}[3P",       '4=37.3R'),
    @('dch_past',     "${E}[5;31H${E}[41m${E}[20P",      '4=30.10R'),
    @('ech3',         "${E}[5;11H${E}[41m${E}[3X",       '4=10.3R'),
    @('el0',          "${E}[5;11H${E}[41m${E}[K",        '4=10.30R'),
    @('el1',          "${E}[5;11H${E}[41m${E}[1K",       '4=11R'),
    @('el2',          "${E}[5;11H${E}[41m${E}[2K",       '4=40R'),
    @('ed0',          "${E}[5;11H${E}[41m${E}[J",        '4=10.30R 5=40R 6=40R 7=40R 8=40R 9=40R'),
    @('ed1',          "${E}[5;11H${E}[41m${E}[1J",       '0=40R 1=40R 2=40R 3=40R 4=11R'),
    @('region_lf',    "${E}[3;6r${E}[6;1H${E}[41m`n",    '5=40R'),
    @('region_ri',    "${E}[3;6r${E}[3;1H${E}[41m${E}M", '2=40R'),
    @('region_su',    "${E}[3;6r${E}[1;1H${E}[41m${E}[S", '5=40R'),
    @('region_il',    "${E}[3;6r${E}[4;1H${E}[41m${E}[L", '3=40R'),
    @('ul_el',        "${E}[5;11H${E}[4m${E}[K",         ''),
    @('rev_el',       "${E}[5;11H${E}[7m${E}[K",         ''),
    @('ul_bg_el',     "${E}[5;11H${E}[4;41m${E}[K",      '4=10.30R'),
    @('ul_il',        "${E}[5;1H${E}[4m${E}[L",          ''),
    @('rev_lf',       "${E}[10;1H${E}[7m`n",             ''),
    @('bright_bg_il', "${E}[5;1H${E}[101m${E}[L",        '4=40B'),
    @('bg256_il',     "${E}[5;1H${E}[48;5;1m${E}[L",     '4=40R'),
    @('wide_ich',     "${E}[5;1H${E}[2K${E}[5;39H$([char]0x3042)${E}[5;1H${E}[41m${E}[@", '4=1R'),
    # hosts: tmux leaves the line a wrap scrolls in default, 9=1R.
    @('wrap_bottom',  ("${E}[10;1H${E}[41m" + ('x' * 41)), '8=40R 9=40R'),
    # tmux: the hosts keep the red foreground on the erased cells, which shows
    # nothing; psmux keeps the background alone, as the inbox route shows it.
    @('fg_el',        "${E}[5;11H${E}[31m${E}[K",        '')
)
$NotHost = @('fg_el')
foreach ($c in $cases) { [IO.File]::WriteAllBytes("$TempHome\p\$($c[0]).bin", [Text.Encoding]::UTF8.GetBytes($fill + $c[1])) }

function Expand($spec) {
    if (-not $spec) { return 'none' }
    ($spec -split ' ' | ForEach-Object {
        $row, $runs = $_ -split '=', 2
        "${row}:" + (([regex]::Matches($runs, '(\d+)(\D)') | ForEach-Object { "$($_.Groups[2].Value)" * [int]$_.Groups[1].Value }) -join '')
    }) -join ' '
}

Set-Content -Path "$TempHome\show.ps1" -Value @'
param([string]$Payload)
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class HostCells {
    [StructLayout(LayoutKind.Sequential)] public struct COORD { public short X, Y; }
    [DllImport("kernel32.dll")] static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll")] static extern bool ReadConsoleOutputAttribute(IntPtr h, ushort[] a, uint len, COORD c, out uint read);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode)] static extern bool ReadConsoleOutputCharacterW(IntPtr h, StringBuilder s, uint len, COORD c, out uint read);
    public static string Row(int y, int cols) {
        IntPtr h = GetStdHandle(-11);
        ushort[] a = new ushort[cols]; uint n;
        COORD c; c.X = 0; c.Y = (short)y;
        ReadConsoleOutputAttribute(h, a, (uint)cols, c, out n);
        StringBuilder s = new StringBuilder(cols);
        ReadConsoleOutputCharacterW(h, s, (uint)cols, c, out n);
        StringBuilder m = new StringBuilder();
        foreach (ushort v in a) {
            int bg = (v >> 4) & 15, fg = v & 15; bool ul = (v & 0x8000) != 0, rv = (v & 0x4000) != 0;
            char k = '?';
            if (!ul && !rv && fg == 7 && bg == 0) k = '.';
            else if (!ul && !rv && fg == 7 && bg == 4) k = 'R';
            else if (!ul && !rv && fg == 7 && bg == 12) k = 'B';
            else if (ul && !rv && fg == 7 && bg == 0) k = 'U';
            else if (!ul && rv && fg == 7 && bg == 0) k = 'V';
            else if (!ul && !rv && fg == 4 && bg == 0) k = 'F';
            m.Append(k);
        }
        string t = s.ToString(); if (t.Length > (int)n) t = t.Substring(0, (int)n);
        return t.TrimEnd() + "|" + m.ToString();
    }
}
"@
$o = [Console]::OpenStandardOutput(); $b = [IO.File]::ReadAllBytes($Payload); $o.Write($b, 0, $b.Length); $o.Flush()
$top = [Console]::WindowTop
$lines = @("{0},{1}" -f [Console]::CursorLeft, ([Console]::CursorTop - $top))
for ($y = 0; $y -lt 10; $y++) { $lines += [HostCells]::Row($top + $y, 40) }
Set-Content -Path ($Payload + ".tmp") -Value $lines -Encoding utf8
Move-Item ($Payload + ".tmp") ($Payload + ".host")
Start-Sleep 600
'@

# capture-pane -e -N rows as "text|letters", the SGR state carried from row
# to row as capture-pane writes it.
function Convert-Capture([string[]]$lines) {
    $out = @()
    $bg = 'd'; $fg = 'd'; $ul = $false; $rv = $false
    foreach ($line in $lines) {
        $text = ''; $attr = ''; $i = 0
        while ($i -lt $line.Length) {
            if ($line[$i] -eq [char]27) {
                $m = [regex]::Match($line.Substring($i), '^\x1b\[([0-9;:]*)m')
                if ($m.Success) {
                    $ps = @(if ($m.Groups[1].Value -eq '') { '0' } else { $m.Groups[1].Value -split ';' })
                    for ($k = 0; $k -lt $ps.Count; $k++) {
                        $p = $ps[$k]
                        if ($p -eq '0' -or $p -eq '') { $bg = 'd'; $fg = 'd'; $ul = $false; $rv = $false }
                        elseif ($p -match '^4(:[1-5])?$') { $ul = $true }
                        elseif ($p -eq '24') { $ul = $false }
                        elseif ($p -eq '7') { $rv = $true }
                        elseif ($p -eq '27') { $rv = $false }
                        elseif ($p -match '^3[0-7]$') { $fg = $p }
                        elseif ($p -eq '39') { $fg = 'd' }
                        elseif ($p -match '^(4[0-7]|10[0-7])$') { $bg = $p }
                        elseif ($p -eq '49') { $bg = 'd' }
                        elseif ($p -eq '48' -or $p -eq '38') {
                            $v = if ($ps[$k + 1] -eq '5') { "5:$($ps[$k + 2])" } else { 'rgb' }
                            if ($p -eq '48') { $bg = $v } else { $fg = $v }
                            $k += if ($ps[$k + 1] -eq '5') { 2 } else { 4 }
                        }
                    }
                    $i += $m.Length; continue
                }
                $m = [regex]::Match($line.Substring($i), '^\x1b\][^\x07\x1b]*(\x07|\x1b\\)')
                if ($m.Success) { $i += $m.Length; continue }
                $i++; continue
            }
            $ch = $line[$i]
            $text += $ch
            $plain = (-not $ul) -and (-not $rv) -and $fg -eq 'd'
            $red = $bg -eq '41' -or $bg -eq '5:1'
            $a = if ($plain -and $bg -eq 'd') { '.' } elseif ($plain -and $red) { 'R' } elseif ($plain -and $bg -eq '101') { 'B' }
                 elseif ($ul -and -not $rv -and $fg -eq 'd' -and $bg -eq 'd') { 'U' }
                 elseif ($rv -and -not $ul -and $fg -eq 'd' -and $bg -eq 'd') { 'V' }
                 elseif (-not $ul -and -not $rv -and $fg -eq '31' -and $bg -eq 'd') { 'F' } else { '?' }
            # A wide character covers two columns, as the host's cells do.
            if ([int]$ch -ge 0x1100) { $a += $a }
            $attr += $a
            $i++
        }
        $out += "$($text.TrimEnd())|$($attr.PadRight(40, '.'))"
    }
    $out
}
function Letters([string[]]$rows) {
    $o = @()
    for ($r = 0; $r -lt $rows.Count; $r++) {
        $a = (($rows[$r] -split '\|', 2)[1]).TrimEnd('.')
        if ($a) { $o += "${r}:$a" }
    }
    if ($o.Count -eq 0) { 'none' } else { $o -join ' ' }
}
function Texts([string[]]$rows) { ($rows | ForEach-Object { ($_ -split '\|', 2)[0] }) -join '/' }

$pwsh = (Get-Command pwsh).Source.Replace('\', '/')
$fwd = $TempHome.Replace('\', '/')
$script:NS = ""
function P { & $PSMUX -L $script:NS @args 2>&1 }

function Run-Cases($route) {
    Get-ChildItem "$TempHome\p\*.host" -EA SilentlyContinue | Remove-Item -Force
    $script:NS = "bce_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 40 -y 10 | Out-Null
    Start-Sleep -Milliseconds 2000
    foreach ($c in $cases) {
        P new-window -d -t s -n $c[0] "& '$pwsh' -NoProfile -File $fwd/show.ps1 $fwd/p/$($c[0]).bin" | Out-Null
    }
    $deadline = (Get-Date).AddSeconds(120)
    while ((Get-Date) -lt $deadline -and @(Get-ChildItem "$TempHome\p\*.host" -EA SilentlyContinue).Count -lt $cases.Count) {
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Milliseconds 800
    foreach ($c in $cases) {
        $name = $c[0]
        $size = "$(P display -p -t "s:$name" '#{pane_width}x#{pane_height}')"
        if ($size -ne '40x10') { Write-Fail "${route}: $name pane is $size, not 40x10"; continue }
        $hostRows = @(Get-Content "$TempHome\p\$name.bin.host" -Encoding utf8 -EA SilentlyContinue)
        if ($hostRows.Count -lt 11) { Write-Fail "${route}: $name the console host's cells were not read"; continue }
        $rows = Convert-Capture @(P capture-pane -e -p -N -t "s:$name")
        $got = Letters $rows
        Check "${route}: $name cells" $got (Expand $c[2])
        if ($NotHost -notcontains $name) {
            Check "${route}: $name cells are the console host's" $got (Letters $hostRows[1..10])
        }
        Check "${route}: $name text is the console host's" (Texts $rows) (Texts $hostRows[1..10])
        Check "${route}: $name cursor is the console host's" "$(P display -p -t "s:$name" '#{cursor_x},#{cursor_y}')" $hostRows[0]
    }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== Issue #1041: background colour erase ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] the inbox conhost" -ForegroundColor Yellow
    # The Windows 10 inbox conhost (19045) keeps its own model of these
    # cells: underline and reverse stay on erased cells, IND and NEL at the
    # bottom scroll in a default line, and reverse reaches psmux as an
    # explicit background. Its cells match neither tmux nor the newer host,
    # so the expectations here hold from Windows 11 (build 22000) on.
    if ([Environment]::OSVersion.Version.Build -lt 22000) {
        Write-Info "skipped: the inbox conhost before Windows 11 keeps its own attributes on these cells"
    } else {
        Run-Cases 'inbox conhost'
    }

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
