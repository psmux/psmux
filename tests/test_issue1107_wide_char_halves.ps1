# Issue #1107: ICH on the right half of a wide character blanks both halves.
#
# tmux moves the cells and leaves the halves apart (grid-view.c:197-225 at
# tag 3.8); psmux keeps no lone half (#639), so its row is tmux's with a lone
# half as a blank. The inbox conhost of build 26300 and OpenConsole 1.25 leave
# the same row in their buffers (read back with ReadConsoleOutputW) but in
# three cases:
#
#   - ICH on a right half: the hosts blank both halves, and psmux kept the
#     character and inserted after it; this is what changes
#   - DCH on a left half: the hosts drop the character before the cursor, and
#     psmux, as tmux, does not
#   - a character written in insert mode on a right half: OpenConsole keeps
#     the wide character after the new one, and psmux, as tmux and the inbox
#     conhost, blanks it
#
#   1. the inbox console host redraws its own buffer, so psmux shows the
#      host's row on master too; the Windows 10 inbox conhost (build 19045)
#      keeps lone halves and redraws a lone left half as the whole character
#   2. OpenConsole, with PSMUX_CONPTY_DIR_FOR_TEST (a directory with conpty.dll
#      and OpenConsole.exe; skipped without it), passes the program's bytes on,
#      so psmux's row is its own; it is checked against the expected row and
#      against the host's
#
# tests/wide_cells_probe.cs writes each case in one write in a new window and
# reads its own console buffer back; psmux's row is read with capture-pane.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per console
# host; cleanup is `-L <ns> kill-server`. PSMUX_TEST_BIN or PSMUX_EXE picks the
# binary (defaults to target\release\psmux.exe).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }
$ConptyDir = $env:PSMUX_CONPTY_DIR_FOR_TEST
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
Remove-Item Env:SHELL -EA SilentlyContinue
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
# capture-pane writes UTF-8.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_widehalves_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$probe = Join-Path $TempHome 'wide_cells_probe.exe'
if ($csc) {
    & $csc.FullName /nologo /optimize /out:$probe (Join-Path $PSScriptRoot 'wide_cells_probe.cs') 2>&1 | Out-Null
}
if (-not (Test-Path $probe)) {
    Write-Fail "build tests\wide_cells_probe.cs (csc.exe unavailable)"
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
    exit $script:TestsFailed
}

# A row is written as capture-pane shows it, a space as `_`, up to the last
# cell that is not a space. The written row is `X`, a wide character, `y`, `z`.
$A = [string][char]0x3042
$I = [string][char]0x3044
$ROW = "X${A}yz"
# Name, bytes, the row through the inbox conhost of build 22000 and later,
# through the Windows 10 one, and through OpenConsole.
$Cases = @(
    @('DCH 1 on X', "$ROW\e[1;1H\e[P", "${A}yz", "${A}yz", "${A}yz"),
    @('DCH 1 on the left half', "$ROW\e[1;2H\e[P", "${A}yz", "X${A}yz", 'X_yz'),
    @('DCH 1 on the right half', "$ROW\e[1;3H\e[P", 'X_yz', "X${A}z", 'X_yz'),
    @('DCH 2 on the left half', "$ROW\e[1;2H\e[2P", 'Xyz', 'Xyz', 'Xyz'),
    @('DCH 2 on X', "$ROW\e[1;1H\e[2P", '_yz', '_yz', '_yz'),
    @('DCH 2 on the right half', "$ROW\e[1;3H\e[2P", 'X_z', "X${A}", 'X_z'),
    @('DCH 3 on the left half', "$ROW\e[1;2H\e[3P", 'Xz', 'Xz', 'Xz'),
    @('DCH 1 on the right half, a wide character after', "X${A}${I}z\e[1;3H\e[P", "X_${I}z", "X${A}${I}z", "X_${I}z"),
    @('DCH 1 on the left half, then Q', "$ROW\e[1;2H\e[PQ", '_Qyz', 'XQyz', 'XQyz'),
    @('DCH 1 on the right half, then Q', "$ROW\e[1;3H\e[PQ", 'X_Qz', "X${A}z", 'X_Qz'),
    @('ICH 1 on the left half', "$ROW\e[1;2H\e[@", "X_${A}yz", "X_${A}yz", "X_${A}yz"),
    @('ICH 1 on the right half', "$ROW\e[1;3H\e[@", 'X___yz', "X${A}${A}yz", 'X___yz'),
    @('ICH 2 on the right half', "$ROW\e[1;3H\e[2@", 'X____yz', "X${A}_${A}yz", 'X____yz'),
    @('ICH 1 on the right half, then Q', "$ROW\e[1;3H\e[@Q", 'X_Q_yz', "X${A}${A}yz", 'X_Q_yz'),
    @('insert mode, Q on the right half', "$ROW\e[1;3H\e[4hQ\e[4l", 'X_Q_yz', "X${A}yz", 'X_Q_yz'),
    @('insert mode, a wide character on the right half', "$ROW\e[1;3H\e[4h${I}\e[4l", "X_${I}_yz", "X${A}${I}z", "X_${I}_yz"),
    @('ECH 1 on the left half', "$ROW\e[1;2H\e[X", 'X__yz', "X_${A}yz", 'X__yz'),
    @('ECH 1 on the right half', "$ROW\e[1;3H\e[X", 'X__yz', "X${A}yz", 'X__yz')
)
$CasesFile = Join-Path $TempHome 'cases.txt'
[IO.File]::WriteAllLines($CasesFile, [string[]]@($Cases | ForEach-Object { "$($_[0])|$($_[1])" }), (New-Object System.Text.UTF8Encoding($false)))

$script:NS = ""
function P { & $PSMUX -L $script:NS @args 2>&1 }

# The host's cells (`<` and `>` after the halves) as capture-pane would show
# them: a whole wide character as itself, a lone half as `?`.
function Host-Row($cells) {
    $r = $cells -replace '(.)<(.)>', '$1' -replace '.[<>]', '?'
    $r.TrimEnd('_')
}

# Run case $i in a new window; psmux's row and the host's cells.
function Run-Case($i) {
    $out = Join-Path $TempHome "r$($script:NS)_$i.txt"
    P new-window -d -t s -n "w$i" "$probe $CasesFile $i $out" | Out-Null
    for ($k = 0; $k -lt 100 -and -not (Test-Path $out); $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 800
    $line = "$(P capture-pane -p -t "s:w$i" | Select-Object -First 1)"
    $cells = if (Test-Path $out) { (Get-Content $out -Raw -Encoding utf8).Trim() } else { '(no probe output)' }
    @(($line -replace ' ', '_').TrimEnd('_'), $cells)
}

function Run-Host($label, $column, [switch]$CompareHost) {
    $script:NS = "widehalves_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 40 -y 10 | Out-Null
    Start-Sleep -Milliseconds 2500
    for ($i = 0; $i -lt $Cases.Count; $i++) {
        $c = $Cases[$i]
        $got, $cells = Run-Case $i
        Write-Info "${label}: $($c[0]): host cells $cells"
        Check "${label}: $($c[0])" $got $c[$column]
        # DCH on a left half, where OpenConsole drops the character before the
        # cursor, and a character written in insert mode on a right half,
        # where it keeps the wide character after the new one (the inbox
        # conhost of build 26300 blanks it), are left as tmux has them.
        if ($CompareHost -and $c[0] -notlike 'DCH 1 on the left half*' -and $c[0] -notlike 'insert mode*') {
            Check "${label}: $($c[0]), the host's row" (Host-Row $cells) $got
        }
    }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "=== A wide character's halves under ICH, DCH and ECH ===" -ForegroundColor Cyan
Write-Host "  -- the inbox console host" -ForegroundColor DarkCyan
if ([Environment]::OSVersion.Version.Build -lt 22000) {
    # The Windows 10 inbox conhost keeps a lone half in its buffer, as tmux
    # does, and redraws a lone left half as the whole character, which covers
    # the cell after it.
    Write-Info "Windows 10 inbox conhost: lone halves are kept, and a lone left half is drawn whole"
    Run-Host 'inbox conhost' 3
} else {
    Run-Host 'inbox conhost' 2
}

if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
    Write-Host "  -- OpenConsole ($ConptyDir)" -ForegroundColor DarkCyan
    $env:PSMUX_CONPTY_DIR = $ConptyDir
    Run-Host 'OpenConsole' 4 -CompareHost
    Remove-Item Env:PSMUX_CONPTY_DIR -EA SilentlyContinue
} else {
    Write-Info "OpenConsole skipped: set PSMUX_CONPTY_DIR_FOR_TEST to a directory with conpty.dll and OpenConsole.exe"
}

Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
