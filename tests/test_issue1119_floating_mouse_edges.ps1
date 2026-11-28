# Issue #1119 follow up: a floating pane takes the mouse in the layouts where
# the screen and the window are not the same grid.
#
#  - status-position top: the window starts one row down, so the client's
#    float hit test and the server's border drag must both take the status
#    row off (tmux server_client_check_mouse_in_pane subtracts statuslines).
#  - a zoomed tiled pane under the float: the float is still drawn and hit
#    first (tmux window_get_active_at walks the floating panes before the
#    tiled layout, zoomed or not).
#
# One attached client in a 100x30 conhost, real MOUSE_EVENT records.
# Safety: -L namespace, its own PSMUX_DATA_DIR, stops only what it started.
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue1119_floating_mouse_edges.ps1

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } elseif ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$EXE_NAME = [IO.Path]::GetFileName($PSMUX)
$NS = "fme$PID"
$S = "fme"
$script:TestsPassed = 0; $script:TestsFailed = 0
$script:Started = @()
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }
function Check($cond, $pass, $fail) { if ($cond) { Write-Pass $pass } else { Write-Fail $fail } }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'TMUX', 'TMUX_PANE') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm = $env:PSMUX_NO_WARM
$root = Join-Path $env:TEMP "psmux_fme_$PID"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = "1"

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$CHILD = Join-Path $root "rec860.exe"
$CLICK = Join-Path $root "click.exe"
$DRAG = Join-Path $root "drag.exe"
& $csc /nologo /platform:x64 /out:$CHILD "$PSScriptRoot\rec860.cs" 2>&1 | Out-Null
& $csc /nologo /out:$CLICK "$PSScriptRoot\click_injector.cs" 2>&1 | Out-Null
& $csc /nologo /out:$DRAG "$PSScriptRoot\mouse_drag_hold_injector.cs" 2>&1 | Out-Null
if (-not (Test-Path $CHILD) -or -not (Test-Path $CLICK) -or -not (Test-Path $DRAG)) { Write-Host "could not build the C# helpers" -ForegroundColor Red; exit 1 }
Write-Info ("psmux: {0}  ({1})" -f $PSMUX, ((& $PSMUX -V) -join ' '))

$MOUSE_ON = '\e[?1000h\e[?1006hready'
$LOG_T = Join-Path $root "tiled.txt"
$LOG_A = Join-Path $root "float_a.txt"

function P { & $PSMUX -L $NS @args 2>&1 }
function Active { (P display-message -t $S -p '#{pane_id}' | Out-String).Trim() }
function Geo($id) { (P display-message -t $id -p '#{pane_left} #{pane_top} #{pane_width} #{pane_height}' | Out-String).Trim() }
function Click($x, $y) { & $CLICK $script:client $x $y | Out-Null; Start-Sleep -Milliseconds 700 }
function Drag($x1, $y1, $x2, $y2) {
    $steps = [Math]::Max([Math]::Abs($x2 - $x1), [Math]::Abs($y2 - $y1))
    & $DRAG $script:client $x1 $y1 $x2 $y2 $steps 40 150 | Out-Null
    Start-Sleep -Milliseconds 900
}

try {
    Write-Host "`n=== Issue #1119 follow up: floating panes and the mouse, offset layouts ===" -ForegroundColor Cyan
    $conf = Join-Path $root "fme.conf"
    "set -g mouse on`nset -g status-position top" | Set-Content $conf -Encoding ASCII
    $cmd = Join-Path $root "launch.cmd"
    @"
@echo off
set PSMUX_SESSION_NAME=
set PSMUX_SESSION=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set PSMUX_DATA_DIR=$($env:PSMUX_DATA_DIR)
set PSMUX_NO_WARM=1
mode con cols=100 lines=30
"$PSMUX" -L $NS -f "$conf" new-session -s $S -- "$CHILD" vt "$LOG_T" $MOUSE_ON
"@ | Set-Content $cmd -Encoding ASCII
    $h = Start-Process conhost.exe -ArgumentList "cmd /c `"$cmd`"" -PassThru
    $script:Started += $h.Id
    $script:client = $null
    for ($i = 0; $i -lt 60 -and -not $script:client; $i++) {
        Start-Sleep -Milliseconds 250
        $script:client = Get-CimInstance Win32_Process -Filter "Name='$EXE_NAME'" |
            Where-Object { $_.CommandLine -match "-L $NS " -and $_.CommandLine -match "new-session -s $S " } |
            Select-Object -ExpandProperty ProcessId -First 1
    }
    if (-not $script:client) { Write-Fail "the attached client did not start"; throw "no client" }
    $script:Started += $script:client
    for ($i = 0; $i -lt 40 -and -not (Test-Path $LOG_T); $i++) { Start-Sleep -Milliseconds 250 }
    $T = Active
    # A: outer (20,5) 30x10 in the window, so its border is screen rows 6..15
    # (the status line is row 0) and columns 20..49.
    $A = (P new-pane -t $S -P -F '#{pane_id}' -X 20 -Y 5 -x 30 -y 10 -- $CHILD vt $LOG_A $MOUSE_ON | Out-String).Trim()
    for ($i = 0; $i -lt 40 -and -not (Test-Path $LOG_A); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Seconds 1
    P select-pane -t $T | Out-Null
    Start-Sleep -Milliseconds 500
    Write-Info "client pid=$($script:client) tiled=$T A=$A [$(Geo $A)] status-position=$((P show-options -gv status-position | Out-String).Trim())"
    if ((Active) -ne $T) { Write-Fail "setup: the tiled pane is not active"; throw "setup" }
    $g0 = Geo $A

    Write-Host "`n[Test 1] status on top: a click on the float's first content row" -ForegroundColor Yellow
    # Window row 6 is A's first content row: screen row 7.
    Click 30 7
    Check ((Active) -eq $A) "A is active" "active is [$(Active)], want A [$A]"
    Click 5 25
    Check ((Active) -eq $T) "a click below it gives the tiled pane back" "active is [$(Active)], want [$T]"

    Write-Host "`n[Test 2] status on top: a click on the row above the float's border is not the float's" -ForegroundColor Yellow
    # Screen row 5 is window row 4, just above A's border: the tiled pane.
    Click 30 5
    Check ((Active) -eq $T) "the row above the border is the tiled pane" "active is [$(Active)], want the tiled pane [$T]"

    Write-Host "`n[Test 3] status on top: dragging the top border moves the float" -ForegroundColor Yellow
    # A's top border is screen row 6.
    Drag 30 6 35 9
    $g = Geo $A
    $p0 = $g0 -split ' '
    $want = "{0} {1} {2} {3}" -f ([int]$p0[0] + 5), ([int]$p0[1] + 3), $p0[2], $p0[3]
    Check ($g -eq $want) "A moved by (+5,+3) [$g]" "A is [$g], want [$want] (was [$g0])"

    Write-Host "`n[Test 4] status on top: the bottom border resizes" -ForegroundColor Yellow
    # A's outer is now (25,8) 30x10 in the window: bottom border window row
    # 17, screen row 18.
    Drag 35 18 35 20
    $g = Geo $A
    $p1 = $want -split ' '
    $want2 = "{0} {1} {2} {3}" -f $p1[0], $p1[1], $p1[2], ([int]$p1[3] + 2)
    Check ($g -eq $want2) "A is 2 taller [$g]" "A is [$g], want [$want2]"

    Write-Host "`n[Test 5] a zoomed tiled pane under the float" -ForegroundColor Yellow
    P split-window -t $T -h -- $CHILD vt (Join-Path $root "t2.txt") $MOUSE_ON | Out-Null
    Start-Sleep -Milliseconds 800
    P select-pane -t $T | Out-Null
    P resize-pane -Z -t $T | Out-Null
    Start-Sleep -Milliseconds 800
    $zoomed = (P display-message -t $S -p '#{window_zoomed_flag}' | Out-String).Trim()
    Write-Info "zoomed=$zoomed A=[$(Geo $A)]"
    # A's content starts at window (26,9): screen (26,10).
    Click 30 12
    Check ((Active) -eq $A) "A is active over the zoomed pane" "active is [$(Active)], want A [$A]"
    $z2 = (P display-message -t $S -p '#{window_zoomed_flag}' | Out-String).Trim()
    Check ($z2 -eq $zoomed) "the zoom stayed [$z2]" "the zoom changed from [$zoomed] to [$z2]"
    # The right border is window column 54.
    $gb = Geo $A
    Drag 54 13 57 13
    $g = Geo $A
    $pb = $gb -split ' '
    $want3 = "{0} {1} {2} {3}" -f $pb[0], $pb[1], ([int]$pb[2] + 3), $pb[3]
    Check ($g -eq $want3) "the right border made A 3 wider while zoomed [$g]" "A is [$g], want [$want3]"
}
catch {
    Write-Info "stopped: $($_.Exception.Message)"
}
finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    foreach ($p in $script:Started) { Stop-Process -Id $p -Force -EA SilentlyContinue }
    $env:PSMUX_DATA_DIR = $savedDataDir
    $env:PSMUX_NO_WARM = $savedNoWarm
    for ($i = 0; $i -lt 10 -and (Test-Path $root); $i++) {
        Start-Sleep -Milliseconds 300
        Remove-Item -Recurse -Force $root -EA SilentlyContinue
    }
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
