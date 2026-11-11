# Issue #972: a letter typed into Claude Code's prompt on Windows 10 stayed on
# screen after Backspace (`❯a`, then `❯ab`), and only a full repaint cleared it.
#
# Cause: a console cell can hold a C0 control (WriteConsoleOutputCharacterW
# puts one there, and so does a write while ENABLE_PROCESSED_OUTPUT is off).
# The inbox conhost writes such a cell to the ConPTY pipe as the bare byte and
# counts it as one column, so its next relative move (`SI BS a` to put `a` in
# column 3) landed one column short in psmux, which ran SI as a control.
# psmux now reads such a byte from the inbox conhost as the one column cell it
# is, drawn as the code page 437 picture conhost itself uses.
#
# The pane program puts each control into a cell the two ways an application
# can, then repaints the cell the way Ink does (CUP to it, write `a`). Every
# row must read `❯ a` with nothing left over. Real controls must keep working:
# a tab written with processed output still expands, a bell still rings
# without taking a column. With a NuGet OpenConsole (PSMUX_CONPTY_DIR, used
# when $env:PSMUX_TEST_OPENCONSOLE_DIR or %TEMP%\occ-conpty\nugetstable holds
# one) the stream stays plain VT.
#
# Runs under a private -L namespace and PSMUX_DATA_DIR; cleans up with
# `psmux -L <ns> kill-server` only. PSMUX_EXE picks another binary.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow }

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_i972_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $work | Out-Null
$savedEnv = @{}
foreach ($n in 'PSMUX_DATA_DIR', 'PSMUX_CONPTY_DIR', 'PSMUX_NO_WARM') { $savedEnv[$n] = [Environment]::GetEnvironmentVariable($n) }
$env:PSMUX_DATA_DIR = $work
$env:PSMUX_NO_WARM = '1'
Remove-Item Env:PSMUX_CONPTY_DIR -ErrorAction SilentlyContinue
Remove-Item Env:SHELL -ErrorAction SilentlyContinue
Remove-Item Env:TMUX -ErrorAction SilentlyContinue
Remove-Item Env:TMUX_PANE -ErrorAction SilentlyContinue

$writer = Join-Path $work 'writer.ps1'
@'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class I972 {
    [StructLayout(LayoutKind.Sequential)] public struct COORD { public short X; public short Y; }
    [DllImport("kernel32.dll")] public static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll")] public static extern bool GetConsoleMode(IntPtr h, out uint m);
    [DllImport("kernel32.dll")] public static extern bool SetConsoleMode(IntPtr h, uint m);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] public static extern bool WriteConsoleW(IntPtr h, string s, uint n, out uint w, IntPtr r);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] public static extern bool WriteConsoleOutputCharacterW(IntPtr h, string s, uint n, COORD at, out uint w);
    public static IntPtr Out { get { return GetStdHandle(-11); } }
    public static void W(string s) { uint w; WriteConsoleW(Out, s, (uint)s.Length, out w, IntPtr.Zero); }
    public static void Cell(string s, short x, short y) { uint w; var c = new COORD(); c.X = x; c.Y = y; WriteConsoleOutputCharacterW(Out, s, (uint)s.Length, c, out w); }
}
"@
$E = [char]27
$prompt = [string][char]0x276F + [char]0x00A0 + 'Try'
$orig = 0; [void][I972]::GetConsoleMode([I972]::Out, [ref]$orig)
$vt = 7   # processed output, wrap at EOL, VT processing
$vtRaw = 6 # the same without ENABLE_PROCESSED_OUTPUT
[I972]::W("$E[2J$E[H")
$row = 0
function Repaint($r) {
    Start-Sleep -Milliseconds 250
    [void][I972]::SetConsoleMode([I972]::Out, $vt)
    [I972]::W("$E[?25l$E[$r;3Ha$E[K$E[$r;4H$E[?25h")
    Start-Sleep -Milliseconds 250
}
# 1: SI put into the cell by WriteConsoleOutputCharacterW.
foreach ($c in 0x0F, 0x0E, 0x01, 0x09, 0x0B, 0x0C, 0x1A) {
    $row++
    [void][I972]::SetConsoleMode([I972]::Out, $vt)
    [I972]::W("$E[$row;1H$prompt$E[$row;3H")
    Start-Sleep -Milliseconds 250
    [I972]::Cell([string][char]$c, 2, [int16]($row - 1))
    Repaint $row
}
# 2: the same controls written with processed output off.
foreach ($c in 0x0F, 0x0E, 0x01, 0x09, 0x0B, 0x0C, 0x1A) {
    $row++
    [void][I972]::SetConsoleMode([I972]::Out, $vt)
    [I972]::W("$E[$row;1H$prompt$E[$row;3H")
    Start-Sleep -Milliseconds 250
    [void][I972]::SetConsoleMode([I972]::Out, $vtRaw)
    [I972]::W([string][char]$c)
    Repaint $row
}
# 3: real controls with processed output on: a tab expands, a bell takes no column.
[void][I972]::SetConsoleMode([I972]::Out, $vt)
$row++; [I972]::W("$E[$row;1Hx`ty`tz")
$row++; [I972]::W("$E[$row;1Hab$([char]7)cd")
Start-Sleep -Milliseconds 300
[I972]::W("$E[$($row + 1);1HI972-DONE")
[void][I972]::SetConsoleMode([I972]::Out, $orig)
Start-Sleep 600
'@ | Set-Content -Path $writer -Encoding UTF8

$expected = [string][char]0x276F + [char]0x00A0 + 'a'
$names = @('cell SI', 'cell SO', 'cell 0x01', 'cell HT', 'cell VT', 'cell FF', 'cell SUB',
    'raw write SI', 'raw write SO', 'raw write 0x01', 'raw write HT', 'raw write VT', 'raw write FF', 'raw write SUB')

function Run-Writer([string]$label, [string]$conptyDir) {
    $ns = 'i972_' + [guid]::NewGuid().ToString('N').Substring(0, 8)
    if ($conptyDir) { $env:PSMUX_CONPTY_DIR = $conptyDir } else { Remove-Item Env:PSMUX_CONPTY_DIR -ErrorAction SilentlyContinue }
    try {
        & $PSMUX -L $ns new-session -d -s t -x 60 -y 24 "pwsh -NoProfile -ExecutionPolicy Bypass -File `"$writer`""
        $lines = $null
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        while ($sw.Elapsed.TotalSeconds -lt 40) {
            Start-Sleep -Milliseconds 500
            $lines = & $PSMUX -L $ns capture-pane -p -t t 2>$null
            if (($lines -join "`n") -match 'I972-DONE') { break }
        }
        return , $lines
    } finally {
        & $PSMUX -L $ns kill-server 2>$null | Out-Null
        Remove-Item Env:PSMUX_CONPTY_DIR -ErrorAction SilentlyContinue
    }
}

Write-Host "`n=== Issue #972: C0 cells from the inbox conhost ===" -ForegroundColor Cyan
try {
    $lines = Run-Writer 'inbox' ''
    if (-not (($lines -join "`n") -match 'I972-DONE')) {
        Write-Fail "the pane program never finished; capture:`n$($lines -join "`n")"
    } else {
        for ($i = 0; $i -lt $names.Count; $i++) {
            $got = $lines[$i].TrimEnd()
            if ($got -eq $expected) { Write-Pass "$($names[$i]): repaint lands on the cell ($got)" }
            else { Write-Fail "$($names[$i]): expected '$expected', got '$got'" }
        }
        $tab = $lines[14].TrimEnd()
        if ($tab -eq 'x       y       z') { Write-Pass "a tab written with processed output still expands" }
        else { Write-Fail "tab row: expected 'x       y       z', got '$tab'" }
        $bell = $lines[15].TrimEnd()
        if ($bell -eq 'abcd') { Write-Pass "a bell written with processed output takes no column" }
        else { Write-Fail "bell row: expected 'abcd', got '$bell'" }
    }

    $occ = if ($env:PSMUX_TEST_OPENCONSOLE_DIR) { $env:PSMUX_TEST_OPENCONSOLE_DIR } else { Join-Path $env:TEMP 'occ-conpty\nugetstable' }
    if (Test-Path (Join-Path $occ 'conpty.dll')) {
        $lines = Run-Writer 'openconsole' $occ
        if (-not (($lines -join "`n") -match 'I972-DONE')) {
            Write-Fail "OpenConsole: the pane program never finished"
        } else {
            $tab = $lines[14].TrimEnd()
            if ($tab -eq 'x       y       z') { Write-Pass "OpenConsole: a tab is still a tab (plain VT kept)" }
            else { Write-Fail "OpenConsole tab row: expected 'x       y       z', got '$tab'" }
            $si = $lines[0].TrimEnd()
            if ($si -eq $expected) { Write-Pass "OpenConsole: its own cell picture is drawn in place ($si)" }
            else { Write-Fail "OpenConsole SI cell row: expected '$expected', got '$si'" }
        }
    } else {
        Write-Skip "no NuGet OpenConsole at $occ"
    }
} finally {
    foreach ($n in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($n, $savedEnv[$n]) }
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
