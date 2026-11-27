# Issue #1109: clear-history keeps the screen; -H removes the links.
#
# clear-history frees the pane's history and nothing else, as tmux does:
# cmd-capture-pane.c:424-430 leaves every mode, calls
# grid_clear_history(wp->base.grid) (grid.c:515-525, which only trims the
# history) and with -H screen_reset_hyperlinks (tmux.1:8608-8616; all at tag
# 3.8). psmux gave the pane a new terminal parser, which also blanked the
# visible screen, left the alternate screen, put the cursor at the top left,
# dropped the scroll region and the mouse modes, turned alternate-screen off
# back on, and ignored -H.
#
# Every expectation is what tmux 3.8 did with the same bytes (WSL, 80x10
# pane), except where a comment says otherwise. A pwsh program in the pane
# prints 40 rows, then, by case, enters the alternate screen with mouse
# tracking, bracketed paste, a scroll region and the cursor at row 5 column
# 10, or writes an OSC 8 link; it then echoes the keys it reads, and "!"
# makes it enter the alternate screen.
#   1. the inbox conhost. It does not pass mouse modes or the scroll region
#      on to psmux, so those are checked on route 2 only. The one before
#      Windows 11 (19045) keeps the alternate screen to itself, so there the
#      pane never reads as on it (issue #88 and its Windows 10 branches).
#   2. OpenConsole, when PSMUX_TEST_CONPTY_DIR or PSMUX_CONPTY_DIR_FOR_TEST
#      names a folder holding conpty.dll, skipped otherwise.
# -H is checked in what the client draws (tests/pipe_client_output_log.cs
# plays its terminal): a link it drew before is gone when it draws the
# window again after `clear-history -H` and still there after a plain `clear-history`. A
# route whose host does not pass OSC 8 on reports that and skips it.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per route;
# cleanup is `-L <ns> kill-server` and the logger this suite started.
# PSMUX_TEST_BIN or PSMUX_EXE picks the binary (defaults to
# target\release\psmux.exe).

. "$PSScriptRoot\helpers\stop_own_tree.ps1"
$ErrorActionPreference = "Continue"
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_clh_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$TermExe = Join-Path $TempHome 'pipe_client_output_log.exe'
if ($csc) {
    & $csc.FullName /nologo /optimize /out:$TermExe (Join-Path $PSScriptRoot 'pipe_client_output_log.cs') 2>&1 | Out-Null
}
if (-not (Test-Path $TermExe)) {
    Write-Fail "build tests\pipe_client_output_log.cs (csc.exe unavailable)"
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
    exit $script:TestsFailed
}

$App = Join-Path $TempHome 'clh_app.ps1'
Set-Content -Path $App -Encoding utf8 -Value @'
param([string]$Mode)
$e = [char]27
$o = [Console]::OpenStandardOutput()
function W([string]$s) { $b = [Text.Encoding]::UTF8.GetBytes($s); $o.Write($b, 0, $b.Length); $o.Flush() }
for ($i = 1; $i -le 40; $i++) { W "row $i`r`n" }
switch ($Mode) {
    'alt' { W "$e[?1049h$e[?1000h$e[?1006h$e[?2004h$e[HALTSCREEN text$e[3;8r$e[5;10H" }
    'link' { W "$e]8;;https://example.com/clh$e\LINKTEXT$e]8;;$e\ plain`r`n" }
}
while ($true) {
    $k = [Console]::ReadKey($true)
    if ($k.KeyChar -eq '!') { W "$e[?1049h$e[HENTERED" } else { W ([string]$k.KeyChar) }
}
'@

$script:NS = ""
$script:run = 0
function P { & $PSMUX -L $script:NS @args 2>&1 }
function Fmt($t, $f) { (P display -p -t $t $f | Select-Object -First 1) }
function Rows($t) { @(P capture-pane -p -t $t) }
# Row $n of the screen, '' when capture-pane returns fewer rows.
function Row($t, $n) { $r = @(Rows $t); if ($r.Count -gt $n) { "$($r[$n])".TrimEnd() } else { '' } }
function NewWindow($mode) {
    $script:run++
    $w = "w$($script:run)"
    P new-window -t s -n $w "pwsh -NoProfile -File `"$App`" $mode" | Out-Null
    Start-Sleep -Milliseconds 3500
    return "s:$w"
}
# The inbox conhost before Windows 11 keeps the alternate screen to itself.
$Win10Inbox = [Environment]::OSVersion.Version.Build -lt 22000

function Run-Route($route) {
    Write-Host "`n=== $route ===" -ForegroundColor Cyan
    $oc = $route -eq 'OpenConsole'
    $altOn = if ($oc -or -not $Win10Inbox) { '1' } else { '0' }

    # 1. A full screen program on the alternate screen.
    $t = NewWindow 'alt'
    $before = Fmt $t '#{cursor_x},#{cursor_y} #{scroll_region_upper}-#{scroll_region_lower} #{mouse_any_flag}#{mouse_sgr_flag}'
    Check "${route}: before, alternate_on" (Fmt $t '#{alternate_on}') $altOn
    P clear-history -t $t | Out-Null
    Start-Sleep -Milliseconds 600
    Check "${route}: alternate screen, history_size" (Fmt $t '#{history_size}') '0'
    Check "${route}: alternate screen, alternate_on stays" (Fmt $t '#{alternate_on}') $altOn
    Check "${route}: alternate screen, the text stays" (Row $t 0) 'ALTSCREEN text'
    Check "${route}: alternate screen, the cursor stays" (Fmt $t '#{cursor_x},#{cursor_y}') '9,4'
    if ($oc) {
        Check "${route}: scroll region and mouse modes stay" (Fmt $t '#{cursor_x},#{cursor_y} #{scroll_region_upper}-#{scroll_region_lower} #{mouse_any_flag}#{mouse_sgr_flag}') $before
        Check "${route}: before, scroll region and mouse modes" $before '9,4 2-7 11'
    }
    P send-keys -t $t -l 'Z' | Out-Null
    Start-Sleep -Milliseconds 800
    $row = Row $t 4
    Check "${route}: the program's next text lands at row 5 column 10" ($(if ($row.Length -ge 10) { $row.Substring(9, 1) } else { '' })) 'Z'

    # 2. The normal screen keeps its visible rows and cursor.
    $t = NewWindow 'main'
    $rows = (Rows $t | ForEach-Object { $_.TrimEnd() }) -join '|'
    $cur = Fmt $t '#{cursor_x},#{cursor_y}'
    P clear-history -t $t | Out-Null
    Start-Sleep -Milliseconds 600
    Check "${route}: normal screen, history_size" (Fmt $t '#{history_size}') '0'
    Check "${route}: normal screen, the visible rows stay" ((Rows $t | ForEach-Object { $_.TrimEnd() }) -join '|') $rows
    Check "${route}: normal screen, the cursor stays" (Fmt $t '#{cursor_x},#{cursor_y}') $cur

    # 3. alternate-screen off is kept for what the program does next.
    $t = NewWindow 'main'
    P set -w -t $t alternate-screen off | Out-Null
    P clear-history -t $t | Out-Null
    P send-keys -t $t -l '!' | Out-Null
    Start-Sleep -Milliseconds 800
    Check "${route}: alternate-screen off still keeps the pane off it" (Fmt $t '#{alternate_on}') '0'
    if ($Win10Inbox -and -not $oc) { Write-Info "${route}: this host never passes the switch on, so the check above holds either way" }

    # 4. -t reaches a pane that is not the active one.
    $t = NewWindow 'main'
    P split-window -t $t "pwsh -NoProfile -File `"$App`" main" | Out-Null
    Start-Sleep -Milliseconds 3500
    $ids = @(P list-panes -t $t -F '#{pane_id}')
    P select-pane -t $ids[1] | Out-Null
    P clear-history -t $ids[0] | Out-Null
    Start-Sleep -Milliseconds 600
    Check "${route}: -t, the target's history_size" (Fmt $ids[0] '#{history_size}') '0'
    Check "${route}: -t, the active pane keeps its history" ([int](Fmt $ids[1] '#{history_size}') -gt 0) 'True'

    # 5. -H, in what the client draws.
    foreach ($h in @($false, $true)) {
        $t = NewWindow 'link'
        P select-window -t $t | Out-Null
        Start-Sleep -Milliseconds 1500
        $drawn = DrawnSince 0
        if (-not $drawn.Contains(']8;;https://example.com/clh')) {
            Write-Info "${route}: the client drew no OSC 8 link, so this host does not pass it on; -H not checked here"
            break
        }
        $flag = if ($h) { '-H' } else { '' }
        if ($h) { P clear-history -H -t $t | Out-Null } else { P clear-history -t $t | Out-Null }
        Start-Sleep -Milliseconds 600
        # Away to the first window and back, so the client draws the whole
        # window again.
        P select-window -t s:0 | Out-Null
        Start-Sleep -Milliseconds 1000
        $from = LogLength
        P select-window -t $t | Out-Null
        Start-Sleep -Milliseconds 1500
        $after = DrawnSince $from
        Check "${route}: clear-history $flag, the full redraw drew LINKTEXT" ($after.Contains('LINKTEXT')) 'True'
        Check "${route}: clear-history $flag, the link is still drawn" ($after.Contains(']8;;https://example.com/clh')) $(if ($h) { 'False' } else { 'True' })
    }
}

function LogLength { if (Test-Path $script:Log) { (Get-Item $script:Log).Length } else { 0 } }
function DrawnSince($from) {
    if (-not (Test-Path $script:Log)) { return '' }
    $fs = [System.IO.File]::Open($script:Log, 'Open', 'Read', 'ReadWrite')
    try {
        $fs.Position = [Math]::Min($from, $fs.Length)
        $buf = New-Object byte[] ($fs.Length - $fs.Position)
        [void]$fs.Read($buf, 0, $buf.Length)
        [System.Text.Encoding]::UTF8.GetString($buf)
    } finally { $fs.Close() }
}

$routes = @('inbox')
if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) { $routes += 'OpenConsole' } else { Write-Info "OpenConsole route skipped (set PSMUX_TEST_CONPTY_DIR)" }
foreach ($route in $routes) {
    $script:NS = "clh_$($route.ToLower())_$PID"
    if ($route -eq 'OpenConsole') { $env:PSMUX_CONPTY_DIR = $ConptyDir } else { Remove-Item Env:PSMUX_CONPTY_DIR -EA SilentlyContinue }
    P new-session -d -s s -x 80 -y 10 | Out-Null
    Start-Sleep -Milliseconds 2500
    $script:Log = Join-Path $TempHome "client_$route.log"
    $term = Start-Process -FilePath $TermExe -PassThru -WindowStyle Hidden -ArgumentList @($script:Log, $PSMUX, '-L', $script:NS, 'attach', '-t', 's')
    Start-Sleep -Milliseconds 2500
    try { Run-Route $route } finally {
        P kill-server | Out-Null
        if ($term) { Stop-OwnTree $term.Id }
        Start-Sleep -Milliseconds 800
    }
}
Remove-Item Env:PSMUX_CONPTY_DIR -EA SilentlyContinue
Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue

Write-Host "`nResults: $($script:TestsPassed) passed, $($script:TestsFailed) failed"
exit $script:TestsFailed
