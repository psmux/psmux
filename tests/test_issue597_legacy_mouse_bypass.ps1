# Issue #597 follow up: widen the Win32 MOUSE_EVENT bypass past the wheel on
# Windows builds below CONPTY_MOUSE_MIN_BUILD (22523).
#
# What the reporter measured on real Windows 10 19045 (issue #597 thread):
#
#   crossterm app in a pane, wheel notch  -> "MOUSE #1: ScrollDown at (35,7)"   works
#   node app in a pane, wheel notch       -> zero bytes                          dead
#
# The wheel works there because psmux injects a Win32 MOUSE_EVENT record straight
# into the pane's console input buffer (send_mouse_event, "fixes #277").  The node
# case is dead because the other channel, the SGR report written into the pane's
# ConPTY input pipe, is eaten by conhost's inbound VT parser on that build
# generation.  That is the same parser CONPTY_MOUSE_MIN_BUILD already documents in
# src/ssh_input.rs.
#
# The record bypass was wheel only, so on those builds a record reading app got the
# wheel and nothing else: no clicks, no releases, no drags.  This suite pins the
# widening.
#
# tmux parity note: tmux always writes SGR bytes and has no record channel at all.
# The MOUSE_EVENT bypass is a Windows only extension that exists because conhost
# sits between psmux and the pane child.  Nothing here changes what tmux would do.
#
# Build seam: this machine is far above 22523, so the below-threshold branch is
# exercised through PSMUX_FAKE_WIN_BUILD, the diagnostic override that
# ssh_input::windows_build_number already honours (see docs/diagnostics.md).  The
# env var is set on the SERVER process, which is what runs inject_mouse_combined.
#
# Layers: real attached client, real console click injection (click_injector.cs),
#         a record reading child (native_mouse_child.cs) and a VT byte reading
#         child (altscreen_mouse_child.cs) as the two arms from the #597 thread.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$SESSION = "i597_bypass"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }

$click    = "$env:TEMP\psmux_i597b_click.exe"
$wheel    = "$env:TEMP\psmux_i597b_wheel.exe"
$drag     = "$env:TEMP\psmux_i597b_drag.exe"
$recChild = "$env:TEMP\psmux_i597b_record_child.exe"
$vtChild  = "$env:TEMP\psmux_i597b_vt_child.exe"
$recLog   = "$env:TEMP\psmux_native_mouse.txt"   # native_mouse_child.cs writes here
$vtLog    = "$env:TEMP\psmux_i597b_vt.txt"

$builds = @(
    @($click,    "click_injector.cs"),
    @($wheel,    "mouse_injector.cs"),
    @($drag,     "mouse_drag_injector.cs"),
    @($recChild, "native_mouse_child.cs"),
    @($vtChild,  "altscreen_mouse_child.cs")
)
foreach ($pair in $builds) {
    Remove-Item $pair[0] -Force -EA SilentlyContinue
    & $csc /nologo /optimize /out:$($pair[0]) (Join-Path $repoTests $pair[1]) 2>&1 | Out-Null
    if (-not (Test-Path $pair[0])) { Write-Host "FATAL: could not compile $($pair[1])" -ForegroundColor Red; exit 1 }
}

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

# Start a real attached client whose SERVER carries $FakeBuild, split a pane and
# run $ChildCmd in it.  Returns the geometry of that pane plus the client process.
#
# PSMUX_NO_WARM is mandatory: a claimed warm server was spawned before this
# function ran and would not carry the build override at all.
function Start-Case {
    param([string]$FakeBuild, [string]$ChildCmd, [string]$ChildLog, [string]$ReadyMark)

    Cleanup
    Remove-Item $ChildLog -Force -EA SilentlyContinue

    $savedBuild = $env:PSMUX_FAKE_WIN_BUILD
    $savedWarm  = $env:PSMUX_NO_WARM
    $env:PSMUX_NO_WARM = "1"
    if ($FakeBuild) { $env:PSMUX_FAKE_WIN_BUILD = $FakeBuild }
    else            { Remove-Item Env:\PSMUX_FAKE_WIN_BUILD -EA SilentlyContinue }

    $p = $null
    try {
        $p = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
    } finally {
        if ($savedBuild) { $env:PSMUX_FAKE_WIN_BUILD = $savedBuild } else { Remove-Item Env:\PSMUX_FAKE_WIN_BUILD -EA SilentlyContinue }
        if ($savedWarm)  { $env:PSMUX_NO_WARM = $savedWarm }        else { Remove-Item Env:\PSMUX_NO_WARM -EA SilentlyContinue }
    }
    if (-not $p) { return $null }

    Start-Sleep -Seconds 5
    & $PSMUX has-session -t $SESSION 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }

    & $PSMUX set-option -t $SESSION -g mouse on 2>&1 | Out-Null
    & $PSMUX split-window -h -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 3

    $target = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}') | Select-Object -Last 1).Trim()
    & $PSMUX send-keys -t $target $ChildCmd Enter 2>&1 | Out-Null

    for ($i = 0; $i -lt 25; $i++) {
        Start-Sleep -Milliseconds 400
        if ((Test-Path $ChildLog) -and ((Get-Content $ChildLog -Raw -EA SilentlyContinue) -match [regex]::Escape($ReadyMark))) { break }
    }
    if (-not (Test-Path $ChildLog)) { return $null }

    $g = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}|#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}') |
          Where-Object { $_ -like "$target|*" }) -split '\|'
    return @{ Proc = $p; Target = $target; Log = $ChildLog;
              Left = [int]$g[1]; Top = [int]$g[2]; W = [int]$g[3]; H = [int]$g[4] }
}

function Stop-Case($c) {
    Cleanup
    if ($c -and $c.Proc) { try { Stop-Process -Id $c.Proc.Id -Force -EA SilentlyContinue } catch {} }
}

function Get-NewLines($c, [int]$before) {
    $all = @(Get-Content $c.Log -EA SilentlyContinue)
    if ($all.Count -le $before) { return @() }
    return @($all[$before..($all.Count - 1)])
}

function Measure-Log($c) { return @(Get-Content $c.Log -EA SilentlyContinue).Count }

# One left click (press then release) at the centre of the child's pane.
function Invoke-Click($c) {
    $px = $c.Left + [int]($c.W / 2)
    $py = $c.Top  + [int]($c.H / 2)
    $before = Measure-Log $c
    & $click $c.Proc.Id $px $py 120 | Out-Null
    Start-Sleep -Milliseconds 1500
    return ,(Get-NewLines $c $before)
}

function Invoke-Wheel($c, [string]$Dir = "up", [int]$Count = 1) {
    $px = $c.Left + [int]($c.W / 2)
    $py = $c.Top  + [int]($c.H / 2)
    $before = Measure-Log $c
    & $wheel $c.Proc.Id $Dir $Count $px $py | Out-Null
    Start-Sleep -Milliseconds 1500
    return ,(Get-NewLines $c $before)
}

function Invoke-Drag($c) {
    $x0 = $c.Left + 2
    $y0 = $c.Top  + 2
    $x1 = $c.Left + [int]($c.W / 2)
    $y1 = $c.Top  + [int]($c.H / 2)
    $before = Measure-Log $c
    & $drag $c.Proc.Id "drag" $x0 $y0 $x1 $y1 8 60 | Out-Null
    Start-Sleep -Milliseconds 1800
    return ,(Get-NewLines $c $before)
}

# A record reading child logs "MOUSE x=.. y=.. buttons=0x.. flags=0x.. wheel=.."
#
# The leading comma is load bearing: `return @(...)` unrolls a one element array
# back to a bare string on the way out of a PowerShell function, and then `[0]`
# indexes a CHARACTER of it.  Every failure message here would report "M".
function Select-Mouse($lines, [string]$Flags, [string]$Buttons) {
    return ,@($lines | Where-Object {
        $_ -like 'MOUSE *' -and $_ -like "*flags=$Flags*" -and ($Buttons -eq '' -or $_ -like "*buttons=$Buttons*")
    })
}

$recCmd = "$($recChild.Replace('\','/'))"
$vtCmdBase = $vtChild.Replace('\','/')
$vtLogFwd  = $vtLog.Replace('\','/')

Write-Host "`n=== Issue #597: the MOUSE_EVENT bypass past the wheel on builds below 22523 ===" -ForegroundColor Cyan
Write-Host "host build: $([Environment]::OSVersion.Version.Build)" -ForegroundColor DarkGray

# ---------------------------------------------------------------------------
# Test 1 (harness sanity): on the REAL build the wheel already reaches a record
# reading child, because the wheel record bypass has shipped since #277.  If this
# fails nothing below means anything.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 1] real build: the wheel reaches a record reading child (existing #277 bypass)" -ForegroundColor Yellow
$c = Start-Case -FakeBuild $null -ChildCmd $recCmd -ChildLog $recLog -ReadyMark "NATIVE_MOUSE START"
if (-not $c) { Write-Fail "could not start the record reading child" }
else {
    $lines = Invoke-Wheel $c "down" 2
    $w = Select-Mouse $lines '0x4' ''
    if ($w.Count -gt 0) { Write-Pass "wheel delivered as MOUSE_EVENT records: $($w[0])" }
    else { Write-Fail "no wheel MOUSE_EVENT record reached the child: $($lines -join ' | ')" }
    Stop-Case $c
}

# ---------------------------------------------------------------------------
# Test 2: on the REAL build a click must NOT produce a MOUSE_EVENT record.  psmux
# forces ENABLE_VIRTUAL_TERMINAL_INPUT on every pane (ensure_vti, #277/#245), and
# with VTI on conhost hands the SGR click through as KEY_EVENT text rather than
# converting it to a record.  That is today's behaviour on 22523+ and this widening
# must not touch it.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 2] real build: a click delivers no MOUSE_EVENT record (unchanged on 22523+)" -ForegroundColor Yellow
$c = Start-Case -FakeBuild $null -ChildCmd $recCmd -ChildLog $recLog -ReadyMark "NATIVE_MOUSE START"
if (-not $c) { Write-Fail "could not start the record reading child" }
else {
    $lines = Invoke-Click $c
    $m = Select-Mouse $lines '0x0' ''
    if ($m.Count -eq 0) { Write-Pass "no click record on the real build, behaviour unchanged" }
    else { Write-Fail "REGRESSION: the widening leaked onto a 22523+ build: $($m -join ' | ')" }
    Stop-Case $c
}

# ---------------------------------------------------------------------------
# Test 3: THE FIX.  Simulated build 19045 (the reporter's box).  A click must reach
# a record reading child as a press record and a release record.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 3] build 19045: a click reaches a record reading child (press and release)" -ForegroundColor Yellow
$c = Start-Case -FakeBuild "19045" -ChildCmd $recCmd -ChildLog $recLog -ReadyMark "NATIVE_MOUSE START"
if (-not $c) { Write-Fail "could not start the record reading child" }
else {
    $lines = Invoke-Click $c
    $press   = Select-Mouse $lines '0x0' '0x1'
    $release = Select-Mouse $lines '0x0' '0x0'
    if ($press.Count -gt 0) { Write-Pass "press record delivered: $($press[0])" }
    else { Write-Fail "BUG #597: no press record on build 19045: $($lines -join ' | ')" }
    if ($release.Count -gt 0) { Write-Pass "release record delivered: $($release[0])" }
    else { Write-Fail "BUG #597: no release record on build 19045: $($lines -join ' | ')" }

    # The record must carry PANE RELATIVE coordinates, not client coordinates.
    # The click goes to the centre of the pane, so both must be inside the pane.
    $ok = $true
    $both = @($press) + @($release)
    foreach ($l in $both) {
        if ($l -match 'x=(-?\d+) y=(-?\d+)') {
            $rx = [int]$Matches[1]; $ry = [int]$Matches[2]
            if ($rx -lt 0 -or $rx -ge $c.W -or $ry -lt 0 -or $ry -ge $c.H) { $ok = $false }
        } else { $ok = $false }
    }
    if ($both.Count -gt 0 -and $ok) {
        Write-Pass "records carry pane relative coordinates inside $($c.W)x$($c.H): $($both -join ' | ')"
    } else {
        Write-Fail "records carry wrong coordinates: $($both -join ' | ')"
    }
    Stop-Case $c
}

# ---------------------------------------------------------------------------
# Test 4: a DRAG on the simulated old build reaches the child as a moved record
# with the left button still held.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 4] build 19045: a drag reaches a record reading child" -ForegroundColor Yellow
$c = Start-Case -FakeBuild "19045" -ChildCmd $recCmd -ChildLog $recLog -ReadyMark "NATIVE_MOUSE START"
if (-not $c) { Write-Fail "could not start the record reading child" }
else {
    $lines = Invoke-Drag $c
    $moved = Select-Mouse $lines '0x1' '0x1'
    if ($moved.Count -gt 0) { Write-Pass "drag delivered as a moved record with the button held: $($moved[0])" }
    else { Write-Fail "BUG #597: no drag record on build 19045: $($lines -join ' | ')" }
    Stop-Case $c
}

# ---------------------------------------------------------------------------
# Test 5: the #598 rule still holds on the simulated old build.  A VT byte reading
# app that never enabled a mouse mode and is not on the alternate screen must
# receive NOTHING for a click or a wheel notch.  The widened bypass must not become
# a second way to type a mouse report into an app that never asked for one.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 5] build 19045: an app that never asked for the mouse still receives nothing" -ForegroundColor Yellow
$vtCmd = "$vtCmdBase alt=0 decset=0 conmouse=0 log=$vtLogFwd"
$c = Start-Case -FakeBuild "19045" -ChildCmd $vtCmd -ChildLog $vtLog -ReadyMark "ALT_ECHO START"
if (-not $c) { Write-Fail "could not start the VT reading child" }
else {
    $clickLines = @(Invoke-Click $c | Where-Object { $_ -like 'RECV*' })
    if ($clickLines.Count -eq 0) { Write-Pass "click forwarded nothing into an app with no mouse mode" }
    else { Write-Fail "#598 rule broken by the widening (click): $($clickLines -join ' | ')" }

    $wheelLines = @(Invoke-Wheel $c "up" 2 | Where-Object { $_ -like 'RECV*' })
    if ($wheelLines.Count -eq 0) { Write-Pass "wheel forwarded nothing into an app with no mouse mode" }
    else { Write-Fail "#598 rule broken by the widening (wheel): $($wheelLines -join ' | ')" }
    Stop-Case $c
}

# ---------------------------------------------------------------------------
# Test 6: Windows Server 2022 (build 20348) sits below the threshold too and gets
# the same widening.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 6] build 20348 (Server 2022): a click reaches a record reading child" -ForegroundColor Yellow
$c = Start-Case -FakeBuild "20348" -ChildCmd $recCmd -ChildLog $recLog -ReadyMark "NATIVE_MOUSE START"
if (-not $c) { Write-Fail "could not start the record reading child" }
else {
    $lines = Invoke-Click $c
    $m = Select-Mouse $lines '0x0' ''
    if ($m.Count -gt 0) { Write-Pass "click record delivered on 20348: $($m[0])" }
    else { Write-Fail "BUG #597: no click record on build 20348: $($lines -join ' | ')" }
    Stop-Case $c
}

# ---------------------------------------------------------------------------
# Test 7: the boundary.  CONPTY_MOUSE_MIN_BUILD itself is NOT below the threshold,
# so at exactly 22523 the widening is off and behaviour matches the real build.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 7] build 22523 (the threshold itself): no click record, widening is off" -ForegroundColor Yellow
$c = Start-Case -FakeBuild "22523" -ChildCmd $recCmd -ChildLog $recLog -ReadyMark "NATIVE_MOUSE START"
if (-not $c) { Write-Fail "could not start the record reading child" }
else {
    $lines = Invoke-Click $c
    $m = Select-Mouse $lines '0x0' ''
    if ($m.Count -eq 0) { Write-Pass "no click record at 22523, the gate is strictly below the threshold" }
    else { Write-Fail "the widening fired at 22523, the comparison is wrong: $($m -join ' | ')" }
    Stop-Case $c
}

Cleanup
Write-Host "`n=== Results: $script:TestsPassed passed, $script:TestsFailed failed ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
