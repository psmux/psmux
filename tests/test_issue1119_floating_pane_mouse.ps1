# Issue #1119: a click on a floating pane does not make it the active pane, and dragging its border neither moves nor resizes it.
#
# The client hit tested the tiled panes only, so a press on a floating pane
# went to the tiled pane under it: a click inside the float selected the tiled
# pane (and handed it the click), and a press on the float's border never
# reached the server's float drag. tmux 3.8 looks at the floating panes first
# (window_get_active_at, window.c): a click inside one makes it the active
# pane and brings it to the front (MouseDown1Pane, `select-pane -t=; send -M`,
# window_redraw_active_switch), and dragging its border moves it by the top
# edge and resizes it by the other edges and the corners
# (cmd_resize_pane_mouse_resize_move_floating, cmd-resize-pane.c).
#
# One attached client in a 100x30 console, a tiled pane and two floats, each
# running a child that asked for the mouse (DECSET 1000 + 1006, rec860) and logs every
# byte it reads. Real MOUSE_EVENT records go into the client's console.
#
# Windows 10 (a build before 22000 without PSMUX_CONPTY_DIR): the inbox host
# hands a VT reader no mouse, so the children read nothing there; the focus
# and geometry checks are the same.
#
# Safety: -L namespace, its own PSMUX_DATA_DIR, stops only the processes it
# started.
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue1119_floating_pane_mouse.ps1

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } elseif ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$EXE_NAME = [IO.Path]::GetFileName($PSMUX)
$NS = "fpm$PID"
$S = "fpm"
$script:TestsPassed = 0; $script:TestsFailed = 0
$script:Started = @()
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }
function Check($cond, $pass, $fail) { if ($cond) { Write-Pass $pass } else { Write-Fail $fail } }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'TMUX', 'TMUX_PANE') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm = $env:PSMUX_NO_WARM
$root = Join-Path $env:TEMP "psmux_fpm_$PID"
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

$build = [Environment]::OSVersion.Version.Build
$win10 = $build -lt 22000 -and -not $env:PSMUX_CONPTY_DIR
Write-Info ("psmux: {0}  ({1})  build={2}  Windows 10 inbox host: {3}" -f $PSMUX, ((& $PSMUX -V) -join ' '), $build, $win10)

# The children ask for the mouse and then write text: a request written on
# its own can stay with the console host and never reach psmux (as the
# extended key request of #1116 does).
$MOUSE_ON = '\e[?1000h\e[?1006hready'
$LOG_T = Join-Path $root "tiled.txt"
$LOG_A = Join-Path $root "float_a.txt"
$LOG_B = Join-Path $root "float_b.txt"

function P { & $PSMUX -L $NS @args 2>&1 }
function Active { (P display-message -t $S -p '#{pane_id}' | Out-String).Trim() }
function Geo($id) { (P display-message -t $id -p '#{pane_left} #{pane_top} #{pane_width} #{pane_height}' | Out-String).Trim() }
# The floats from back to front, as the client draws them (list-panes lists
# them after the tiled panes, in that order).
function Front-Order {
    ((P list-panes -t $S -F '#{pane_floating_flag} #{pane_id}') | Where-Object { $_ -match '^1 ' } | ForEach-Object { ($_ -split ' ')[1] }) -join ' '
}
# The SGR mouse reports a child read.
function Mouse-Bytes($log) { @((Get-Content $log -EA SilentlyContinue) | Where-Object { $_ -match '^BYTES .*str=\[<ESC>\[<' }).Count }
function Click($x, $y) { & $CLICK $script:client $x $y | Out-Null; Start-Sleep -Milliseconds 700 }
# One cell a motion, as the pointer moves.
function Drag($x1, $y1, $x2, $y2) {
    $steps = [Math]::Max([Math]::Abs($x2 - $x1), [Math]::Abs($y2 - $y1))
    & $DRAG $script:client $x1 $y1 $x2 $y2 $steps 40 150 | Out-Null
    Start-Sleep -Milliseconds 900
}

try {
    Write-Host "`n=== Issue #1119: floating panes and the mouse ===" -ForegroundColor Cyan
    $conf = Join-Path $root "fpm.conf"
    "set -g mouse on" | Set-Content $conf -Encoding ASCII
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
    # A at (20,5) 30x10, B at (40,8) 30x10 over it; both ask for the mouse.
    $A = (P new-pane -t $S -P -F '#{pane_id}' -X 20 -Y 5 -x 30 -y 10 -- $CHILD vt $LOG_A $MOUSE_ON | Out-String).Trim()
    $B = (P new-pane -t $S -P -F '#{pane_id}' -X 40 -Y 8 -x 30 -y 10 -- $CHILD vt $LOG_B $MOUSE_ON | Out-String).Trim()
    for ($i = 0; $i -lt 40 -and -not ((Test-Path $LOG_A) -and (Test-Path $LOG_B)); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Seconds 1
    P select-pane -t $T | Out-Null
    Start-Sleep -Milliseconds 500
    Write-Info "client pid=$($script:client) tiled=$T A=$A [$(Geo $A)] B=$B [$(Geo $B)] front order: $(Front-Order)"
    if ((Active) -ne $T) { Write-Fail "setup: the tiled pane is not active"; throw "setup" }

    Write-Host "`n[Test 1] click inside a float that is behind" -ForegroundColor Yellow
    # (25,7) is A's content and outside B.
    $before = @{ T = Mouse-Bytes $LOG_T; A = Mouse-Bytes $LOG_A }
    Click 25 7
    Check ((Active) -eq $A) "A is the active pane" "active is [$(Active)], want A [$A]"
    $order = Front-Order
    Check ($order -eq "$B $A") "A came to the front [$order]" "front order is [$order], want [$B $A]"
    if ($win10) {
        Check ((Mouse-Bytes $LOG_T) -eq $before.T) "the tiled program under it got nothing" "the tiled program got a mouse event"
    } else {
        Check ((Mouse-Bytes $LOG_A) -gt $before.A) "A's program got the click" "A's program got no mouse event"
        Check ((Mouse-Bytes $LOG_T) -eq $before.T) "the tiled program under it got nothing" "the tiled program got the click meant for A"
    }

    Write-Host "`n[Test 2] click on the tiled pane outside the floats" -ForegroundColor Yellow
    Click 5 22
    Check ((Active) -eq $T) "the tiled pane is active again" "active is [$(Active)], want the tiled pane [$T]"

    Write-Host "`n[Test 3] where two floats overlap, the front one takes the click" -ForegroundColor Yellow
    # (45,10) is inside both; A is in front now.
    $before = @{ A = Mouse-Bytes $LOG_A; B = Mouse-Bytes $LOG_B }
    Click 45 10
    Check ((Active) -eq $A) "A, in front, is active" "active is [$(Active)], want A [$A]"
    if (-not $win10) {
        Check ((Mouse-Bytes $LOG_B) -eq $before.B) "B behind it got nothing" "B behind A got the click"
    }
    # (60,15) is B only: B comes to the front.
    Click 60 15
    Check ((Active) -eq $B) "B is active" "active is [$(Active)], want B [$B]"
    $order = Front-Order
    Check ($order -eq "$A $B") "B came to the front [$order]" "front order is [$order], want [$A $B]"

    Write-Host "`n[Test 4] dragging the top border moves the float" -ForegroundColor Yellow
    # B's border is x 40..69, y 8..17 (content 41,9 28x8).
    Drag 50 8 55 11
    $g = Geo $B
    Check ($g -eq "46 12 28 8") "B moved by (+5,+3) [$g]" "B is [$g], want [46 12 28 8]"

    Write-Host "`n[Test 5] the right and bottom borders resize one way" -ForegroundColor Yellow
    # Right border x = 74.
    Drag 74 15 78 15
    $g = Geo $B
    Check ($g -eq "46 12 32 8") "the right border made B 4 wider [$g]" "B is [$g], want [46 12 32 8]"
    # Bottom border y = 20.
    Drag 55 20 55 22
    $g = Geo $B
    Check ($g -eq "46 12 32 10") "the bottom border made B 2 taller [$g]" "B is [$g], want [46 12 32 10]"

    Write-Host "`n[Test 6] the corners resize both ways" -ForegroundColor Yellow
    # Bottom-right corner (78,22).
    Drag 78 22 80 24
    $g = Geo $B
    Check ($g -eq "46 12 34 12") "the bottom-right corner grew B by (2,2) [$g]" "B is [$g], want [46 12 34 12]"
    # Top-left corner (45,11): the top-left follows the pointer.
    Drag 45 11 42 9
    $g = Geo $B
    Check ($g -eq "43 10 37 14") "the top-left corner moved B's corner and grew it [$g]" "B is [$g], want [43 10 37 14]"

    Write-Host "`n[Test 7] a drag inside the content does not move the float" -ForegroundColor Yellow
    Drag 60 15 65 18
    $g = Geo $B
    Check ($g -eq "43 10 37 14") "B stayed [$g]" "B is [$g], want [43 10 37 14]"
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
