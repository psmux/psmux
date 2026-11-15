# Issue #1058: a pane keeps its clock on screen while another pane has focus.
# tmux keeps clock mode per pane (window-clock.c): every pane in the mode has
# its own timer and is drawn whether it has the focus or not, so a clock can
# stay up beside the pane being worked in. psmux remembered the clock of a
# pane the focus left (AppState::clock_panes) but drew a clock only on the
# focused pane, so moving away hid it and moving back showed it again.
# A left press on the clock pane closes the clock, as tmux's MouseDown1Pane
# `select-pane -t=; send -M` (key-bindings.c) hands it to the mode.
#
# A real client in its own console. Keys and clicks go in with
# WriteConsoleInput (tests\injector.cs, tests\click_injector.cs); the screen
# is read back with tests\console_screen_read.cs (the clock is drawn by the
# client, so capture-pane cannot see it).
#
# Private PSMUX_DATA_DIR and config, a -L namespace per run; cleanup is
# `-L <ns> kill-server` and the client PID this suite started.
# PSMUX_EXE (or PSMUX_TEST_BIN) picks the binary.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -and $_.Name -ne 'PSMUX_TEST_BIN' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
Remove-Item Env:SHELL -EA SilentlyContinue
$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = $env:PSMUX_EXE }
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

. "$PSScriptRoot\injector_guard.ps1"

$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_clkp_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_RUN_NOPAUSE = "1"
$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
$READ = Join-Path $TMP "screenread.exe"
$KEYINJ = Join-Path $TMP "keyinject.exe"
$CLICKINJ = Join-Path $TMP "clickinject.exe"
$WHEELINJ = Join-Path $TMP "wheelinject.exe"
if ($csc) {
    & $csc /nologo /optimize "/out:$READ" (Join-Path $PSScriptRoot "console_screen_read.cs") 2>&1 | Out-Null
    & $csc /nologo /optimize "/out:$KEYINJ" (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
    & $csc /nologo /optimize "/out:$CLICKINJ" (Join-Path $PSScriptRoot "click_injector.cs") 2>&1 | Out-Null
    & $csc /nologo /optimize "/out:$WHEELINJ" (Join-Path $PSScriptRoot "mouse_injector.cs") 2>&1 | Out-Null
}
foreach ($exe in @($READ, $KEYINJ, $CLICKINJ, $WHEELINJ)) {
    if (-not (Test-Path $exe)) { Write-Error "could not build $(Split-Path $exe -Leaf)"; exit 1 }
}
# The bare name: a full path under "C:\Program Files" has a space, and the
# default-shell line below is split at it.
$POWERSHELL = if (Get-Command pwsh -EA SilentlyContinue) { "pwsh" } else { "powershell" }

$script:client = $null
$script:ns = ""
function Start-Run {
    $script:ns = "clkp_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $conf = Join-Path $TMP "$($script:ns).conf"
    @("set -g default-shell '$POWERSHELL -NoLogo -NoProfile'", "set -g status off", "set -g mouse on") -join "`n" | Set-Content $conf -Encoding ASCII
    $env:PSMUX_CONFIG_FILE = $conf
    & $PSMUX -L $script:ns new-session -d -s A -x 100 -y 25 2>&1 | Out-Null
    for ($i = 0; $i -lt 40; $i++) { & $PSMUX -L $script:ns has-session -t A 2>$null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 800
    $script:client = Start-Process -FilePath $PSMUX -ArgumentList @('-L', $script:ns, 'attach', '-t', 'A') -PassThru
    Start-Sleep -Seconds 4
}
function Stop-Run {
    if ($script:ns) { & $PSMUX -L $script:ns kill-server 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 500
    if ($script:client -and -not $script:client.HasExited) { Stop-Process -Id $script:client.Id -Force -EA SilentlyContinue }
    $script:client = $null; $script:ns = ""
}
function P { & $PSMUX -L $script:ns @args 2>&1 }
# Visible rows of the client console.
function Screen {
    $f = Join-Path $TMP "screen.txt"
    & $READ $script:client.Id $f | Out-Null
    $lines = @(Get-Content $f -Encoding UTF8)
    return @($lines | Where-Object { $_.StartsWith('T|') } | ForEach-Object { $_.Substring(2) })
}
function Geometry($pane) {
    $g = (P display-message -p -t $pane '#{pane_left} #{pane_top} #{pane_width} #{pane_height}') -split ' '
    return @([int]$g[0], [int]$g[1], [int]$g[2], [int]$g[3])
}
# The text of a pane's own cells on the client screen.
function PaneText($pane) {
    $rows = Screen
    $l, $t, $w, $h = Geometry $pane
    $out = @()
    for ($y = $t; $y -lt $t + $h -and $y -lt $rows.Count; $y++) {
        $r = $rows[$y].PadRight($l + $w)
        $out += $r.Substring($l, $w)
    }
    return $out
}
# Clock mode owns the whole pane: no output shows and only the 5 rows of big
# digits have text (the PR #1046 suite pins that drawing).
function ShowsClock($pane) {
    $rows = @(PaneText $pane)
    $marks = @($rows | Where-Object { $_ -match 'MARK' }).Count
    $drawn = @($rows | Where-Object { $_.Trim() -ne '' }).Count
    return ($marks -eq 0 -and $drawn -eq 5)
}
function ShowsOutput($pane) {
    return (@(PaneText $pane | Where-Object { $_ -match 'MARK' }).Count -ge 5)
}
function Fill($pane) {
    P send-keys -t $pane -l '1..40 | % { "MARK$_ " * 4 }' | Out-Null
    P send-keys -t $pane Enter | Out-Null
    Start-Sleep -Seconds 2
}
function Keys($keys) {
    if (-not (Invoke-GuardedInjector $KEYINJ $script:client.Id $keys -RequireDelivery)) { throw "INJECTOR_BLOCKED: $($script:InjectorBlocked)" }
    Start-Sleep -Milliseconds 1200
}
# $button is the console button mask: 1 left, 2 right, 4 middle.
function Click($pane, $button = 1) {
    $l, $t, $w, $h = Geometry $pane
    & $CLICKINJ $script:client.Id ($l + [math]::Floor($w / 2)) ($t + [math]::Floor($h / 2)) 80 $button | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "click_injector failed with $LASTEXITCODE" }
    Start-Sleep -Milliseconds 1200
}
function WheelUp($pane) {
    $l, $t, $w, $h = Geometry $pane
    & $WHEELINJ $script:client.Id up 1 ($l + [math]::Floor($w / 2)) ($t + [math]::Floor($h / 2)) | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "mouse_injector failed with $LASTEXITCODE" }
    Start-Sleep -Milliseconds 1200
}
function Mode($pane) { return (P display-message -p -t $pane '#{pane_mode}').Trim() }
function Active { return (P display-message -p '#{pane_id}').Trim() }
function InMode($pane) { return (P display-message -p -t $pane '#{pane_in_mode}').Trim() }
# Split in two, both full of output, focus on the left pane.
function Setup {
    P split-window -h -t A | Out-Null
    Start-Sleep -Seconds 2
    $script:left = (P display-message -p -t 'A:0.0' '#{pane_id}').Trim()
    $script:right = (P display-message -p -t 'A:0.1' '#{pane_id}').Trim()
    Fill $script:left
    Fill $script:right
    P select-pane -t $script:left | Out-Null
    Start-Sleep -Milliseconds 800
}

Write-Host "`n=== Issue #1058 a pane keeps its clock while another pane has focus ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"

try {
    Start-Run
    Setup
    $left = $script:left; $right = $script:right

    Write-Host "`n[1] prefix+o leaves the clock on the left pane" -ForegroundColor Yellow
    P clock-mode -t $left | Out-Null
    Start-Sleep -Milliseconds 1500
    if (ShowsClock $left) { Write-Pass "the left pane shows its clock" } else { Write-Fail "the left pane should show its clock:`n$((PaneText $left) -join "`n")" }
    Keys '^bo'
    if ((Active) -eq $right) { Write-Pass "prefix+o moved the focus to the right pane" } else { Write-Fail "prefix+o should focus $right, focus is on $(Active)" }
    if (ShowsClock $left) { Write-Pass "the left pane still shows its clock" } else { Write-Fail "the left pane lost its clock when the focus left:`n$((PaneText $left) -join "`n")" }
    if (ShowsOutput $right) { Write-Pass "the right pane shows its own output, no clock" } else { Write-Fail "the right pane should show its output" }
    Keys 'echo TYPED_ON_RIGHT{ENTER}'
    $cap = (P capture-pane -p -t $right) -join "`n"
    if ($cap -match 'TYPED_ON_RIGHT') { Write-Pass "typing reaches the right pane" } else { Write-Fail "typing should reach the right pane" }
    if (ShowsClock $left) { Write-Pass "the clock stays up while the right pane is used" } else { Write-Fail "the clock should stay up while the right pane is used" }
    if ((InMode $left) -eq '1') { Write-Pass "#{pane_in_mode} of the left pane is 1" } else { Write-Fail "#{pane_in_mode} of the left pane should be 1, got $(InMode $left)" }

    Write-Host "`n[2] a click on the right pane leaves the clock" -ForegroundColor Yellow
    Keys '^bo'
    if ((Active) -eq $left -and (ShowsClock $left)) { Write-Pass "back on the left pane, the clock is live" } else { Write-Fail "prefix+o back to the left pane should meet its clock" }
    Click $right
    if ((Active) -eq $right) { Write-Pass "the click focused the right pane" } else { Write-Fail "the click should focus $right, focus is on $(Active)" }
    if (ShowsClock $left) { Write-Pass "the left pane still shows its clock" } else { Write-Fail "the left pane lost its clock on the click:`n$((PaneText $left) -join "`n")" }

    Write-Host "`n[3] a click on the clock pane closes the clock" -ForegroundColor Yellow
    Click $left
    if ((Active) -eq $left) { Write-Pass "the click focused the left pane" } else { Write-Fail "the click should focus $left, focus is on $(Active)" }
    if ((InMode $left) -eq '0') { Write-Pass "the left pane left clock mode" } else { Write-Fail "the click should close the clock, #{pane_in_mode} is $(InMode $left)" }
    if (ShowsOutput $left) { Write-Pass "the left pane shows its output again" } else { Write-Fail "the left pane should show its output again:`n$((PaneText $left) -join "`n")" }

    Write-Host "`n[4] clock-mode -t on the pane without focus" -ForegroundColor Yellow
    # Start from a left pane out of clock mode even when [3] failed: any key
    # leaves it (window_clock_key).
    if ((InMode $left) -ne '0') { P send-keys -t $left q | Out-Null; Start-Sleep -Milliseconds 800 }
    P clock-mode -t $right | Out-Null
    Start-Sleep -Milliseconds 1500
    if ((Active) -eq $left) { Write-Pass "the focus stays on the left pane" } else { Write-Fail "the focus should stay on $left" }
    if (ShowsClock $right) { Write-Pass "the right pane shows the clock it entered" } else { Write-Fail "the right pane should show its clock:`n$((PaneText $right) -join "`n")" }
    if (ShowsOutput $left) { Write-Pass "the left pane shows its output, no clock" } else { Write-Fail "the left pane should show its output" }
    # tmux runs `select-pane -t=; send -M` for a middle press on a pane in a
    # mode and `send -M` for a wheel notch, so both reach window_clock_key and
    # close the clock; on another pane they only move the focus (or enter
    # copy mode there) and the clock stays with its own pane.
    Write-Host "`n[5] a middle press on the clock pane without focus closes it" -ForegroundColor Yellow
    Click $right 4
    if ((Active) -eq $right) { Write-Pass "the middle press focused the clock pane" } else { Write-Fail "the middle press should focus $right, focus is on $(Active)" }
    if ((InMode $right) -eq '0') { Write-Pass "the middle press closed the clock" } else { Write-Fail "the middle press should close the clock, pane_mode is '$(Mode $right)'" }
    if (ShowsOutput $right) { Write-Pass "the right pane shows its output again" } else { Write-Fail "the right pane should show its output again:`n$((PaneText $right) -join "`n")" }

    Write-Host "`n[6] a middle press on another pane leaves the clock on its own pane" -ForegroundColor Yellow
    P clock-mode -t $right | Out-Null
    Start-Sleep -Milliseconds 1500
    Click $left 4
    if ((Active) -eq $left) { Write-Pass "the middle press focused the left pane" } else { Write-Fail "the middle press should focus $left, focus is on $(Active)" }
    if ((Mode $right) -eq 'clock-mode' -and (ShowsClock $right)) { Write-Pass "the right pane keeps its clock" } else { Write-Fail "the right pane should keep its clock, pane_mode is '$(Mode $right)':`n$((PaneText $right) -join "`n")" }
    if ((InMode $left) -eq '0' -and (ShowsOutput $left)) { Write-Pass "the clock did not move to the left pane" } else { Write-Fail "the left pane should not be in a mode, pane_mode is '$(Mode $left)'" }

    Write-Host "`n[7] a wheel notch on another pane leaves the clock on its own pane" -ForegroundColor Yellow
    WheelUp $left
    if ((Mode $left) -eq 'copy-mode') { Write-Pass "the left pane entered copy mode" } else { Write-Fail "the wheel should put the left pane in copy mode, pane_mode is '$(Mode $left)'" }
    if ((Mode $right) -eq 'clock-mode' -and (ShowsClock $right)) { Write-Pass "the right pane keeps its clock" } else { Write-Fail "the right pane should keep its clock, pane_mode is '$(Mode $right)':`n$((PaneText $right) -join "`n")" }
    P send-keys -t $left -X cancel | Out-Null
    Start-Sleep -Milliseconds 800

    Write-Host "`n[8] a wheel notch on the clock pane without focus closes it" -ForegroundColor Yellow
    WheelUp $right
    if ((InMode $right) -eq '0') { Write-Pass "the wheel closed the clock" } else { Write-Fail "the wheel should close the clock, pane_mode is '$(Mode $right)'" }
    if ((Active) -eq $left) { Write-Pass "the focus stays on the left pane" } else { Write-Fail "the focus should stay on $left, it is on $(Active)" }
    if (ShowsOutput $right) { Write-Pass "the right pane shows its output again" } else { Write-Fail "the right pane should show its output again:`n$((PaneText $right) -join "`n")" }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    Stop-Run
    Remove-Item -Recurse -Force $TMP -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
