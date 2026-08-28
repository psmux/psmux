# Issue #613: a node child's raw mode strips ENABLE_MOUSE_INPUT and the pane's wheel
# goes silent for good.
#
# Report: "One node child anywhere in the pane's process tree permanently strips the bit
# the gate depends on... the pane is permanently silent to the wheel, and restarting the
# application in the pane does not recover it, because the console outlives the process.
# Opening a new window does."
#
# Root cause: both #598 signals resolve to the SAME console input mode word, and that
# word belongs to the console, not to the application that set it.
#
#   window_ops::detect_mouse_input      reads ENABLE_MOUSE_INPUT off the child console
#   window_ops::update_mouse_proto_owner is driven by mouse_protocol_mode(), which under
#                                        ConPTY is conhost republishing that same word as
#                                        ESC[?1003;1006h / ESC[?1003;1006l
#
# libuv's uv_tty_set_mode(UV_TTY_MODE_RAW) ASSIGNS ENABLE_WINDOW_INPUT |
# ENABLE_VIRTUAL_TERMINAL_INPUT (0x0208) over the whole word and restores nothing, so one
# node child takes both signals away at once.  Measured before the fix:
#
#   PROBE[A] before=0x03B0 mouse=True    WHEEL up x1 -> 1 SGR chunk
#   MODE longnode-running 0x0208 mouse=False
#   PROBE[C] before=0x0208 mouse=False   WHEEL up x1 -> 0 SGR chunks   (and 0 forever after)
#
# tmux cannot have this bug.  Its authorization is `s->mode & ALL_MOUSE_MODES` on the
# pane's OWN screen (input-keys.c:805, tmux.h:698), set by the app's DECSET (input.c:2053)
# and cleared only by its DECRST (input.c:1959) or screen_reinit on respawn (screen.c:115).
# There is no console, so no third process can revoke it.
#
# Fix: `Pane::wheel_auth`, a pane-owned latch anchored to the pid that earned the
# authorization, consulted only AFTER both live signals have said no, and dropped when
# that process leaves the pane.  Plus an explicit opt-in (`set -p @mouse-force on`,
# or server wide `PSMUX_FORCE_WHEEL=1`) for a pane that never earned anything.
#
# Layers: E2E byte capture (rawmode_mouse_child + node_wheel_probe), real console wheel
#         injection, console-mode probing, copy-mode state, Win32 TUI verification.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$SESSION = "test_i613"
$script:TestsPassed = 0
$script:TestsFailed = 0

# An attached client launched from an agent shell inherits the caller's routing
# variables and re-enters the wrong session; scrub them before Start-Process.
foreach ($v in @("PSMUX_SESSION_NAME", "PSMUX_SESSION", "PSMUX_PANE")) {
    Remove-Item "env:$v" -EA SilentlyContinue
}

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }

$inj    = "$env:TEMP\psmux_i613_injector.exe"
$probe  = "$env:TEMP\psmux_i613_mode_probe.exe"
$child  = "$env:TEMP\psmux_i613_rawmode_child.exe"
$nodeJs = Join-Path $repoTests "node_wheel_probe.js"
$childLog = "$env:TEMP\psmux_i613_child.txt"
$nodeLog  = "$env:TEMP\psmux_i613_node.txt"

foreach ($pair in @(@($inj, "mouse_injector.cs"), @($probe, "mouse_mode_probe.cs"), @($child, "rawmode_mouse_child.cs"))) {
    Remove-Item $pair[0] -Force -EA SilentlyContinue
    & $csc /nologo /optimize /out:$($pair[0]) (Join-Path $repoTests $pair[1]) 2>&1 | Out-Null
    if (-not (Test-Path $pair[0])) { Write-Host "FATAL: could not compile $($pair[1])" -ForegroundColor Red; exit 1 }
}
$haveNode = $null -ne (Get-Command node -EA SilentlyContinue)

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

# Start an attached client with one extra pane.  Returns the client handle plus the
# geometry of the pane the model app will run in.
function Start-Client {
    Cleanup
    $p = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
    Start-Sleep -Seconds 5
    & $PSMUX has-session -t $SESSION 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    & $PSMUX set-option -t $SESSION -g mouse on 2>&1 | Out-Null
    return $p
}

function New-ModelPane {
    param($Proc, [string]$Command)
    & $PSMUX split-window -h -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $target = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}') | Select-Object -Last 1).Trim()
    if ($Command) { & $PSMUX send-keys -t $target $Command Enter 2>&1 | Out-Null; Start-Sleep -Seconds 5 }
    $g = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}|#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}') |
          Where-Object { $_ -like "$target|*" }) -split '\|'
    return @{ Proc = $Proc; Target = $target;
              PanePid = (& $PSMUX display-message -t $target -p '#{pane_pid}' 2>&1).Trim()
              Left = [int]$g[1]; Top = [int]$g[2]; W = [int]$g[3]; H = [int]$g[4] }
}

function Stop-Client($p) {
    Cleanup
    if ($p) { try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {} }
}

# The pane child's console input mode word, read the way the reporter read it:
# AttachConsole + CreateFileW("CONIN$") + GetConsoleMode.
function Get-ConsoleMode($case) {
    $r = & $probe $case.PanePid query 2>&1
    if ("$r" -match 'before=0x([0-9A-Fa-f]{4})') { return [Convert]::ToUInt32($Matches[1], 16) }
    return 0xFFFF
}

# One wheel burst at the pane centre.  Returns the SGR report lines the model app logged.
function Invoke-Wheel {
    param($Case, [string]$Log, [string]$Dir = "up", [int]$Count = 1)
    $px = $Case.Left + [int]($Case.W / 2)
    $py = $Case.Top + [int]($Case.H / 2)
    $before = 0
    if (Test-Path $Log) { $before = (Get-Content $Log).Count }
    & $inj $Case.Proc.Id $Dir $Count $px $py | Out-Null
    Start-Sleep -Milliseconds 1500
    if (-not (Test-Path $Log)) { return @() }
    $all = Get-Content $Log
    if ($all.Count -le $before) { return @() }
    return @($all[$before..($all.Count-1)] | Where-Object { $_ -match '^RECV .*<ESC>\[<\d+;' })
}

Write-Host "`n=== Issue #613: a child's SetConsoleMode must not revoke the pane's wheel ===" -ForegroundColor Cyan

# ---------------------------------------------------------------------------
# Test 1: THE BUG.  A mouse-registered TUI keeps the wheel after an unrelated
# node child enters raw mode and dies without restoring the console mode word.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 1] A node child's raw mode must not silence the pane's wheel" -ForegroundColor Yellow
$p = Start-Client
if (-not $p) {
    Write-Fail "could not start the attached client"
} else {
    Remove-Item $childLog -Force -EA SilentlyContinue
    $c = New-ModelPane $p "& '$child' alt=1 decset=0 conmouse=1 log='$childLog'"
    if (-not (Test-Path $childLog)) {
        Write-Fail "the mouse-registered model app never started"
    } else {
        $m0 = Get-ConsoleMode $c
        if ($m0 -band 0x0010) { Write-Pass ("baseline console mode 0x{0:X4} has ENABLE_MOUSE_INPUT" -f $m0) }
        else { Write-Fail ("baseline console mode 0x{0:X4} has no ENABLE_MOUSE_INPUT; the model app did not register" -f $m0) }

        $r0 = Invoke-Wheel $c $childLog "up" 1
        if ($r0.Count -ge 1) { Write-Pass "baseline: the wheel reaches a mouse-registered app ($($r0[-1]))" }
        else { Write-Fail "baseline: the wheel did not reach a mouse-registered app; the rest of this test proves nothing" }

        # 'L' spawns `node -e "process.stdin.setRawMode(true); setInterval(...)"` with
        # inherited stdio, so libuv writes 0x0208 over this pane's console mode word.
        & $PSMUX send-keys -t $c.Target "L" 2>&1 | Out-Null
        Start-Sleep -Seconds 7
        $m1 = Get-ConsoleMode $c
        $stripped = ($m1 -band 0x0010) -eq 0
        if ($stripped) { Write-Pass ("the node child stripped ENABLE_MOUSE_INPUT: console mode is now 0x{0:X4}" -f $m1) }
        else { Write-Host ("  [INFO] console mode after the node child is 0x{0:X4}; node may have restored it" -f $m1) -ForegroundColor DarkYellow }

        $r1 = Invoke-Wheel $c $childLog "up" 1
        if ($r1.Count -ge 1) { Write-Pass "BUG #613 FIXED: the wheel still reaches the app while a node child holds raw mode" }
        else { Write-Fail "BUG #613: the wheel went silent while a node child held raw mode" }

        # 'K' kills it the way Ctrl+C does, so libuv's tty reset never runs and the
        # console keeps the raw mode word after the child is gone.
        & $PSMUX send-keys -t $c.Target "K" 2>&1 | Out-Null
        Start-Sleep -Seconds 6
        $r2 = Invoke-Wheel $c $childLog "up" 1
        $r3 = Invoke-Wheel $c $childLog "down" 2
        if ($r2.Count -ge 1) { Write-Pass "BUG #613 FIXED: wheel-up still forwarded after the node child was killed" }
        else { Write-Fail "BUG #613: wheel-up silent after the node child was killed" }
        if ($r3.Count -ge 2) { Write-Pass "wheel-down forwarded too ($($r3.Count) reports for 2 notches)" }
        else { Write-Fail "BUG #613: wheel-down leaked notches after the node child was killed ($($r3.Count)/2)" }

        # The report is of a permanently dead pane, so one recovered notch is not enough.
        $late = 0
        for ($i = 0; $i -lt 4; $i++) { $late += (Invoke-Wheel $c $childLog "up" 2).Count }
        if ($late -ge 8) { Write-Pass "8 further notches all arrived ($late reports); the authorization is durable" }
        else { Write-Fail "only $late of 8 further notches arrived; the authorization is not durable" }
    }
    Stop-Client $p
}

# ---------------------------------------------------------------------------
# Test 2: the #598 negative case must stay negative.  A full screen app that
# asked through NEITHER signal still receives nothing, and no latch can be
# invented for it.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 2] #598 stays fixed: an app that never asked still gets nothing" -ForegroundColor Yellow
$p = Start-Client
if (-not $p) {
    Write-Fail "could not start the attached client"
} else {
    Remove-Item $childLog -Force -EA SilentlyContinue
    $c = New-ModelPane $p "& '$child' alt=1 decset=0 conmouse=0 log='$childLog'"
    if (-not (Test-Path $childLog)) {
        Write-Fail "the htop-like model app never started"
    } else {
        $leaks = 0
        for ($i = 0; $i -lt 4; $i++) {
            $dir = if ($i % 2 -eq 0) { "up" } else { "down" }
            $leaks += (Invoke-Wheel $c $childLog $dir 2).Count
        }
        if ($leaks -eq 0) { Write-Pass "8 notches over an alt-screen app with no mouse leaked 0 reports" }
        else { Write-Fail "#598 REGRESSED: $leaks unsolicited report(s) reached an app that never asked" }

        # And the same after a node child churns the console mode, which is the exact
        # event the latch is allowed to survive.  It must not manufacture consent.
        & $PSMUX send-keys -t $c.Target "L" 2>&1 | Out-Null
        Start-Sleep -Seconds 7
        & $PSMUX send-keys -t $c.Target "K" 2>&1 | Out-Null
        Start-Sleep -Seconds 6
        $after = (Invoke-Wheel $c $childLog "up" 3).Count
        if ($after -eq 0) { Write-Pass "still 0 reports after a node child churned the console mode" }
        else { Write-Fail "#598 REGRESSED: $after report(s) leaked after a node child churned the console mode" }

        $mode = (& $PSMUX display-message -t $c.Target -p '#{pane_in_mode}' 2>&1).Trim()
        if ($mode -eq "0") { Write-Pass "no copy mode over the alt-screen app (tmux alternate_on parity)" }
        else { Write-Fail "copy mode opened over an alt-screen app (pane_in_mode=$mode)" }
    }
    Stop-Client $p
}

# ---------------------------------------------------------------------------
# Test 3: the latch expires with its owner.  Once the app that earned it exits,
# a wheel notch at the shell prompt must go back to copy mode (#360 parity),
# not be typed into the shell.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 3] The latch dies with the app that earned it" -ForegroundColor Yellow
$p = Start-Client
if (-not $p) {
    Write-Fail "could not start the attached client"
} else {
    Remove-Item $childLog -Force -EA SilentlyContinue
    $c = New-ModelPane $p "& '$child' alt=1 decset=0 conmouse=1 log='$childLog'"
    if (-not (Test-Path $childLog)) {
        Write-Fail "the mouse-registered model app never started"
    } else {
        $r = Invoke-Wheel $c $childLog "up" 1
        if ($r.Count -ge 1) { Write-Pass "the app earned the wheel authorization" }
        else { Write-Fail "the app never earned the wheel authorization; the rest of this test proves nothing" }

        & $PSMUX send-keys -t $c.Target C-z 2>&1 | Out-Null   # the model app quits on Ctrl+Z
        Start-Sleep -Seconds 5
        $fg = (& $PSMUX display-message -t $c.Target -p '#{pane_current_command}' 2>&1).Trim()
        $post = Invoke-Wheel $c $childLog "up" 2
        if ($post.Count -eq 0) { Write-Pass "no reports forwarded after the app exited (foreground is now '$fg')" }
        else { Write-Fail "the latch outlived its owner: $($post.Count) report(s) went to the bare shell" }

        $mode = (& $PSMUX display-message -t $c.Target -p '#{pane_in_mode}' 2>&1).Trim()
        if ($mode -eq "1") { Write-Pass "wheel-up at the shell prompt entered copy mode again (#360 parity)" }
        else { Write-Fail "wheel-up at the shell prompt did not enter copy mode (pane_in_mode=$mode)" }
        & $PSMUX send-keys -t $c.Target -X cancel 2>&1 | Out-Null
    }
    Stop-Client $p
}

# ---------------------------------------------------------------------------
# Test 4: the Claude Code shape.  A node TUI whose DECSET conhost swallows and
# whose console is in libuv raw mode from its first instant asked through
# neither signal and can never earn a latch.  That pane is silent by default,
# and `set -p @mouse-force on` is the way out.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 4] The unearnable pane: silent by default, opened by @mouse-force" -ForegroundColor Yellow
if (-not $haveNode) {
    Write-Host "  [SKIP] node is not on PATH" -ForegroundColor DarkYellow
} else {
    $p = Start-Client
    if (-not $p) {
        Write-Fail "could not start the attached client"
    } else {
        Remove-Item $nodeLog -Force -EA SilentlyContinue
        $c = New-ModelPane $p "node '$nodeJs' '$nodeLog' 0"
        if (-not (Test-Path $nodeLog)) {
            Write-Fail "the node model TUI never started"
        } else {
            $m = Get-ConsoleMode $c
            if (($m -band 0x0010) -eq 0) { Write-Pass ("the node TUI's console is in libuv raw mode: 0x{0:X4}, no ENABLE_MOUSE_INPUT" -f $m) }
            else { Write-Host ("  [INFO] node TUI console mode is 0x{0:X4}" -f $m) -ForegroundColor DarkYellow }

            $off = (Invoke-Wheel $c $nodeLog "up" 3).Count
            if ($off -eq 0) { Write-Pass "no reports by default: nothing to latch onto, so the gate holds (#598 safe side)" }
            else { Write-Host "  [INFO] this pane earned an authorization on its own ($off reports); the opt-in is not needed here" -ForegroundColor DarkYellow }

            $bad = (& $PSMUX set-option -p -t $c.Target "@mouse-force" "maybe" 2>&1) -join " "
            if ($bad -match "bad value") { Write-Pass "set -p @mouse-force rejects a junk value loudly (#580 parity)" }
            else { Write-Fail "set -p @mouse-force accepted a junk value silently: '$bad'" }

            $set = (& $PSMUX set-option -p -t $c.Target "@mouse-force" "on" 2>&1) -join " "
            if ($set -match "not supported|bad value") { Write-Fail "set -p @mouse-force on was refused: $set" }
            else { Write-Pass "set -p @mouse-force on accepted" }
            Start-Sleep -Milliseconds 800
            $on = (Invoke-Wheel $c $nodeLog "up" 3).Count
            if ($on -ge 3) { Write-Pass "@mouse-force on delivered all 3 notches to the unearnable pane" }
            else { Write-Fail "@mouse-force on delivered only $on of 3 notches" }

            & $PSMUX set-option -p -t $c.Target "@mouse-force" "off" 2>&1 | Out-Null
            Start-Sleep -Milliseconds 800
            $backOff = (Invoke-Wheel $c $nodeLog "up" 3).Count
            if ($backOff -eq 0) { Write-Pass "@mouse-force off puts the gate back" }
            else { Write-Host "  [INFO] $backOff report(s) after @mouse-force off; the pane had earned an authorization meanwhile" -ForegroundColor DarkYellow }
        }
        Stop-Client $p
    }
}

# ---------------------------------------------------------------------------
# Win32 TUI verification: a real attached window, a real shell pane, the wheel
# still driving copy mode, and the pane option surviving a round trip through
# show-options.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host ("=" * 60)
Write-Host "Win32 TUI VISUAL VERIFICATION"
Write-Host ("=" * 60)

$p = Start-Client
if (-not $p) {
    Write-Fail "TUI: session did not start"
} else {
    & $PSMUX split-window -v -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $panes = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1).Trim()
    if ($panes -eq "2") { Write-Pass "TUI: split-window created 2 panes" } else { Write-Fail "TUI: expected 2 panes, got $panes" }

    $target = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}') | Select-Object -Last 1).Trim()
    & $PSMUX send-keys -t $target "1..80 | ForEach-Object { `"tui line `$_`" }" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $g = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}|#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}') |
          Where-Object { $_ -like "$target|*" }) -split '\|'
    & $inj $p.Id "up" 3 ([int]$g[1] + [int]([int]$g[3]/2)) ([int]$g[2] + [int]([int]$g[4]/2)) | Out-Null
    Start-Sleep -Milliseconds 900
    $mode = (& $PSMUX display-message -t $target -p '#{pane_in_mode}' 2>&1).Trim()
    if ($mode -eq "1") { Write-Pass "TUI: wheel over a live shell pane still enters copy mode" }
    else { Write-Fail "TUI: wheel over a shell pane did not enter copy mode (pane_in_mode=$mode)" }

    & $PSMUX send-keys -t $target -X cancel 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $mode2 = (& $PSMUX display-message -t $target -p '#{pane_in_mode}' 2>&1).Trim()
    if ($mode2 -eq "0") { Write-Pass "TUI: copy mode exits cleanly afterwards" }
    else { Write-Fail "TUI: still in copy mode after cancel (pane_in_mode=$mode2)" }

    & $PSMUX set-option -p -t $target "@mouse-force" "on" 2>&1 | Out-Null
    $shown = (& $PSMUX show-options -p -t $target 2>&1) -join "`n"
    if ($shown -match '@mouse-force on') { Write-Pass "TUI: @mouse-force survives a round trip through show-options -p" }
    else { Write-Fail "TUI: show-options -p did not report @mouse-force (got: $shown)" }

    Stop-Client $p
}

Remove-Item $childLog, $nodeLog -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
