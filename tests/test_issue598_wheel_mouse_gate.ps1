# Issue #598: mouse wheel over a full screen app (codex, htop) corrupted the app
#
# Report: "When I have codex cli running in a psmux panel and scroll back using the
# mouse or touchpad, it completely messes up codex... When running htop, scrolling
# back also does strange things like 'Search: ' and a bunch of garbage."
#
# Root cause: the wheel gate forwarded a mouse report whenever the pane was on the
# ALTERNATE SCREEN, without asking whether the application had enabled a mouse
# protocol.  A full screen app that never asked for the mouse therefore received raw
# SGR bytes (ESC[<64;col;rowM) on its stdin, which it read as keystrokes.
#
# tmux next-3.8 parity, key-bindings.c:510 and input-keys.c:805:
#
#   bind -n WheelUpPane { if -F '#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}'
#                            { send -M } { copy-mode -e } }
#
#   static void input_key_mouse(struct window_pane *wp, struct mouse_event *m) {
#           struct screen *s = wp->screen;
#           if (m->ignore || (s->mode & ALL_MOUSE_MODES) == 0)
#                   return;
#
# So alternate_on only decides "do not enter copy mode".  The bytes themselves are
# gated on the application's own mouse mode, and on the alternate screen without one
# `send -M` is a silent no-op.  psmux now matches that exactly.
#
# Layers: E2E byte capture (altscreen_mouse_child), real console wheel injection,
#         copy-mode state verification, Win32 TUI verification.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$SESSION = "test_i598"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }

$inj      = "$env:TEMP\psmux_mouse_injector.exe"
$child    = "$env:TEMP\psmux_altscreen_mouse_child.exe"
$childLog = "$env:TEMP\psmux_i598_echo.txt"

foreach ($pair in @(@($inj, "mouse_injector.cs"), @($child, "altscreen_mouse_child.cs"))) {
    Remove-Item $pair[0] -Force -EA SilentlyContinue
    & $csc /nologo /optimize /out:$($pair[0]) (Join-Path $repoTests $pair[1]) 2>&1 | Out-Null
    if (-not (Test-Path $pair[0])) { Write-Host "FATAL: could not compile $($pair[1])" -ForegroundColor Red; exit 1 }
}

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

# Start an attached client with one extra pane and run the model app in it.
# Returns @{ Proc; Target; Left; Top; W; H } or $null.
function Start-Case {
    param([string]$Alt, [string]$Decset, [string]$Conmouse)
    Cleanup
    Remove-Item $childLog -Force -EA SilentlyContinue
    $p = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
    Start-Sleep -Seconds 5
    & $PSMUX has-session -t $SESSION 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    & $PSMUX set-option -t $SESSION -g mouse on 2>&1 | Out-Null
    & $PSMUX split-window -h -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $target = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}') | Select-Object -Last 1).Trim()
    $childFwd = $child.Replace('\','/')
    $logFwd = $childLog.Replace('\','/')
    & $PSMUX send-keys -t $target "$childFwd alt=$Alt decset=$Decset conmouse=$Conmouse log=$logFwd" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 4
    if (-not (Test-Path $childLog)) { return $null }
    $g = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}|#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}') |
          Where-Object { $_ -like "$target|*" }) -split '\|'
    return @{ Proc = $p; Target = $target;
              Left = [int]$g[1]; Top = [int]$g[2]; W = [int]$g[3]; H = [int]$g[4] }
}

function Stop-Case($c) {
    Cleanup
    if ($c -and $c.Proc) { try { Stop-Process -Id $c.Proc.Id -Force -EA SilentlyContinue } catch {} }
}

# One wheel notch at the pane centre. Returns the RECV lines the child logged.
function Invoke-Wheel {
    param($Case, [string]$Dir = "up", [int]$Count = 1)
    $px = $Case.Left + [int]($Case.W / 2)
    $py = $Case.Top + [int]($Case.H / 2)
    $before = (Get-Content $childLog).Count
    & $inj $Case.Proc.Id $Dir $Count $px $py | Out-Null
    Start-Sleep -Milliseconds 1200
    $all = Get-Content $childLog
    if ($all.Count -le $before) { return @() }
    return @($all[$before..($all.Count-1)] | Where-Object { $_ -like 'RECV*' })
}

Write-Host "`n=== Issue #598: the wheel must not type into an app that never asked for the mouse ===" -ForegroundColor Cyan

# ---------------------------------------------------------------------------
# Test 1: THE BUG.  Alternate screen, application has NOT enabled any mouse
# protocol.  tmux writes nothing (input_key_mouse returns early) and does not
# enter copy mode either.  Before the fix psmux injected ESC[<64;col;rowM,
# which htop read as keystrokes ("Search: " plus garbage).
# ---------------------------------------------------------------------------
Write-Host "`n[Test 1] Alt screen + no mouse protocol: no bytes may reach the app" -ForegroundColor Yellow
$c = Start-Case -Alt 1 -Decset 0 -Conmouse 0
if (-not $c) {
    Write-Fail "could not start the alt-screen model app"
} else {
    $recv = Invoke-Wheel $c "up" 3
    if ($recv.Count -eq 0) {
        Write-Pass "wheel-up over an alt-screen app with no mouse protocol forwarded nothing"
    } else {
        Write-Fail "BUG #598: app received $($recv.Count) unsolicited input chunk(s): $($recv -join ' | ')"
    }

    $recvDown = Invoke-Wheel $c "down" 3
    if ($recvDown.Count -eq 0) {
        Write-Pass "wheel-down forwarded nothing either"
    } else {
        Write-Fail "BUG #598: app received $($recvDown.Count) unsolicited chunk(s) on wheel-down: $($recvDown -join ' | ')"
    }

    # tmux runs `send -M` here, which is a no-op; it must NOT fall through to
    # copy mode, or the full screen app would be hidden behind the copy overlay.
    $mode = (& $PSMUX display-message -t $c.Target -p '#{pane_in_mode}' 2>&1).Trim()
    if ($mode -eq "0") { Write-Pass "psmux did not enter copy mode over the alt-screen app (tmux alternate_on parity)" }
    else { Write-Fail "psmux entered copy mode over an alt-screen app (pane_in_mode=$mode)" }

    Stop-Case $c
}

# ---------------------------------------------------------------------------
# Test 2: Alternate screen AND the app registered the mouse the way a real
# Windows TUI does (SetConsoleMode ENABLE_MOUSE_INPUT, which conhost relays to
# psmux as ESC[?1003;1006h).  The wheel must still be forwarded, with the real
# pointer coordinates (#570).
# ---------------------------------------------------------------------------
Write-Host "`n[Test 2] Alt screen + registered mouse: the wheel is still forwarded" -ForegroundColor Yellow
$c = Start-Case -Alt 1 -Decset 0 -Conmouse 1
if (-not $c) {
    Write-Fail "could not start the mouse-registered alt-screen model app"
} else {
    $recv = Invoke-Wheel $c "up" 1
    $line = $recv | Select-Object -Last 1
    if ($line -match '<ESC>\[<64;(\d+);(\d+)M') {
        $col = [int]$Matches[1]; $row = [int]$Matches[2]
        $expCol = [int]($c.W / 2) + 1
        $expRow = [int]($c.H / 2) + 1
        Write-Pass "wheel-up forwarded as SGR 64 at col=$col row=$row"
        if ($col -eq $expCol -and $row -eq $expRow) { Write-Pass "coordinates track the pointer (expected $expCol,$expRow)" }
        else { Write-Fail "coordinates were $col,$row, expected $expCol,$expRow" }
    } else {
        Write-Fail "no SGR wheel report reached a mouse-registered alt-screen app (got: $($recv -join ' | '))"
    }

    $recvDown = Invoke-Wheel $c "down" 1
    if (($recvDown | Select-Object -Last 1) -match '<ESC>\[<65;\d+;\d+M') { Write-Pass "wheel-down forwarded as SGR 65" }
    else { Write-Fail "wheel-down not forwarded (got: $($recvDown -join ' | '))" }

    Stop-Case $c
}

# ---------------------------------------------------------------------------
# Test 3: Main screen and the app registered the mouse.  tmux mouse_any_flag
# is set, so `send -M` forwards.  This is the #570 main-screen consumer case
# and must not regress into copy mode.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 3] Main screen + registered mouse: forwarded, not copy mode" -ForegroundColor Yellow
$c = Start-Case -Alt 0 -Decset 0 -Conmouse 1
if (-not $c) {
    Write-Fail "could not start the main-screen mouse consumer"
} else {
    $recv = Invoke-Wheel $c "up" 1
    if (($recv | Select-Object -Last 1) -match '<ESC>\[<64;\d+;\d+M') { Write-Pass "main-screen mouse consumer received the wheel" }
    else { Write-Fail "main-screen mouse consumer got nothing (got: $($recv -join ' | '))" }

    $mode = (& $PSMUX display-message -t $c.Target -p '#{pane_in_mode}' 2>&1).Trim()
    if ($mode -eq "0") { Write-Pass "no copy mode for a main-screen mouse consumer" }
    else { Write-Fail "copy mode stole the wheel from a main-screen mouse consumer (pane_in_mode=$mode)" }

    Stop-Case $c
}

# ---------------------------------------------------------------------------
# Test 4: Main screen, no mouse protocol.  Plain shell semantics: copy mode on
# wheel-up, and never any bytes into the child (#360 parity).
# ---------------------------------------------------------------------------
Write-Host "`n[Test 4] Main screen + no mouse: copy mode, no bytes" -ForegroundColor Yellow
$c = Start-Case -Alt 0 -Decset 0 -Conmouse 0
if (-not $c) {
    Write-Fail "could not start the plain main-screen model app"
} else {
    $recv = Invoke-Wheel $c "up" 1
    if ($recv.Count -eq 0) { Write-Pass "no bytes forwarded to a main-screen app without mouse" }
    else { Write-Fail "unsolicited bytes on the main screen: $($recv -join ' | ')" }

    $mode = (& $PSMUX display-message -t $c.Target -p '#{pane_in_mode}' 2>&1).Trim()
    if ($mode -eq "1") { Write-Pass "wheel-up entered copy mode (tmux copy-mode -e parity)" }
    else { Write-Fail "wheel-up did not enter copy mode (pane_in_mode=$mode)" }

    Stop-Case $c
}

# ---------------------------------------------------------------------------
# Test 5: repeat the exact reported scenario several times.  The report is of a
# corrupted app, so one clean run is not enough: a single leaked notch is the bug.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 5] Repeated scrolling over the alt-screen app never leaks a byte" -ForegroundColor Yellow
$c = Start-Case -Alt 1 -Decset 0 -Conmouse 0
if (-not $c) {
    Write-Fail "could not start the alt-screen model app for the repeat run"
} else {
    $leaks = 0
    for ($i = 0; $i -lt 6; $i++) {
        $dir = if ($i % 2 -eq 0) { "up" } else { "down" }
        $r = Invoke-Wheel $c $dir 2
        $leaks += $r.Count
    }
    if ($leaks -eq 0) { Write-Pass "12 wheel notches over 6 bursts leaked 0 input chunks" }
    else { Write-Fail "12 wheel notches leaked $leaks input chunk(s) into the app" }
    Stop-Case $c
}

# ---------------------------------------------------------------------------
# Win32 TUI verification: a real attached window stays usable while all of this
# happens, and the wheel still drives copy mode over a plain shell pane.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host ("=" * 60)
Write-Host "Win32 TUI VISUAL VERIFICATION"
Write-Host ("=" * 60)

Cleanup
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
Start-Sleep -Seconds 5
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "TUI: session did not start"
} else {
    & $PSMUX set-option -t $SESSION -g mouse on 2>&1 | Out-Null
    & $PSMUX split-window -v -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $panes = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1).Trim()
    if ($panes -eq "2") { Write-Pass "TUI: split-window created 2 panes" } else { Write-Fail "TUI: expected 2 panes, got $panes" }

    $target = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}') | Select-Object -Last 1).Trim()
    & $PSMUX send-keys -t $target "1..80 | ForEach-Object { `"tui line `$_`" }" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $g = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}|#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}') |
          Where-Object { $_ -like "$target|*" }) -split '\|'
    & $inj $proc.Id "up" 3 ([int]$g[1] + [int]([int]$g[3]/2)) ([int]$g[2] + [int]([int]$g[4]/2)) | Out-Null
    Start-Sleep -Milliseconds 900
    $mode = (& $PSMUX display-message -t $target -p '#{pane_in_mode}' 2>&1).Trim()
    if ($mode -eq "1") { Write-Pass "TUI: wheel over a live shell pane still enters copy mode" }
    else { Write-Fail "TUI: wheel over a shell pane did not enter copy mode (pane_in_mode=$mode)" }

    & $PSMUX send-keys -t $target -X cancel 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $mode2 = (& $PSMUX display-message -t $target -p '#{pane_in_mode}' 2>&1).Trim()
    if ($mode2 -eq "0") { Write-Pass "TUI: copy mode exits cleanly afterwards" }
    else { Write-Fail "TUI: still in copy mode after cancel (pane_in_mode=$mode2)" }

    Cleanup
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
}

Remove-Item $childLog -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
