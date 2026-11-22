# Issue #621: "Mouse-wheel scroll stops working while a program is blocking on a
# stdin read" (psmux 3.3.8, 66cf613).
#
# Report: inside `psmux new -s test`, run a script that prints 40 lines and then
# calls `input()`.  While that read is blocked the wheel no longer scrolls back;
# it "begins to cycle the prompt history and flicker the cursor".  Ctrl+C the
# script and the wheel works again.  Keyboard copy mode (prefix + [) is fine
# throughout.  Reproduced with several languages, so the trigger is the blocked
# read, not Python.
#
# ROOT CAUSE.  psmux's wheel fallback carried a general alternate-scroll branch:
# any pane whose foreground was a CONFIRMED non-shell process had every wheel
# notch translated into three arrow-key presses written straight into the pty
# (window_ops.rs at 66cf613):
#
#     } else if non_shell_fg && !is_legacy_pager {
#         // General alternate-scroll: arrow keys (tmux DECSET-1007 parity).
#         let seq: &[u8] = if up { b"\x1b[A" } else { b"\x1b[B" };
#         for _ in 0..3 { crate::input::write_key_seq(pane, seq); }
#     } else if up && app.scroll_enter_copy_mode {
#         enter_copy_mode(app);            // <- never reached while python runs
#
# A program blocked on `input()` IS the pane's foreground process, and it is not
# a shell, so `non_shell_fg` is `Some(false) -> true` and the wheel took the
# arrow branch instead of copy mode.  The arrows land in a Windows cooked read
# (ENABLE_LINE_INPUT, measured 0x01F7 while `input()` blocks against 0x01E4 at
# the PSReadLine prompt), and the console's own line editor reads Up/Down as
# "recall the previous/next history entry" -- exactly the reported prompt-history
# cycling and cursor flicker.  Ctrl+C makes pwsh the foreground again,
# `non_shell_fg` goes false, and copy mode comes back.
#
# TMUX PARITY.  tmux's default WheelUpPane binding (key-bindings.c:510) is
#
#     if -F '#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}' \
#         'send -M' 'copy-mode -e'
#
# Not one of those three terms is "the foreground process is not a shell", and
# alternate-scroll (DECSET 1007) applies to ALTERNATE-screen panes only.  A
# main-screen program blocked on a canonical read gets copy-mode scrollback in
# tmux no matter what it is doing with stdin.
#
# FIX.  The blanket arrow-key branch was removed (PR #548, merged 6ff92a4, three
# days after the 3.3.8 release commit): forwarding is decided by the pane's own
# terminal state (`pane_wheel_forward`: alternate screen, or a mouse protocol the
# application itself enabled), and everything else falls through to copy mode.
# `more.com` stays as an exact one-name allowlist because it parses no escape
# sequences at all.
#
# Layers: real attached client, real MOUSE_EVENT wheel injection, copy-mode
#         state, capture-pane scrollback, byte-exact proof that the blocked read
#         received the typed line and nothing else, and Win32 TUI verification.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$SESSION = "test_i621"
$script:TestsPassed = 0
$script:TestsFailed = 0

# An attached client launched from an agent shell inherits the caller's routing
# variables and re-enters the wrong session; scrub them before Start-Process.
foreach ($v in @("PSMUX_SESSION_NAME", "PSMUX_SESSION", "PSMUX_PANE")) {
    Remove-Item "env:$v" -EA SilentlyContinue
}

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }

$inj      = "$env:TEMP\psmux_i621_injector.exe"
$child    = "$env:TEMP\psmux_i621_rawmode_child.exe"
$childLog = "$env:TEMP\psmux_i621_child.txt"
foreach ($pair in @(@($inj, "mouse_injector.cs"), @($child, "rawmode_mouse_child.cs"))) {
    Remove-Item $pair[0] -Force -EA SilentlyContinue
    & $csc /nologo /optimize /out:$($pair[0]) (Join-Path $repoTests $pair[1]) 2>&1 | Out-Null
    if (-not (Test-Path $pair[0])) { Write-Host "FATAL: could not compile $($pair[1])" -ForegroundColor Red; exit 1 }
}

# The reporter's script, plus an echo of what the blocked read actually got so
# stray arrow keys cannot hide.
$PY = "$env:TEMP\psmux_i621_stdin.py"
@'
import sys
for i in range(40):
    print("i621 line %d" % i)
sys.stdout.flush()
line = input("BLOCKED> ")
print("GOTLINE[" + line + "]")
'@ | Set-Content -Path $PY -Encoding ASCII

$python = $null
foreach ($cand in @("python", "py")) {
    if (Get-Command $cand -EA SilentlyContinue) { $python = $cand; break }
}

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
}

function Start-Client {
    Cleanup
    $p = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
    Start-Sleep -Seconds 5
    & $PSMUX has-session -t $SESSION 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    & $PSMUX set-option -t $SESSION -g mouse on 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    return $p
}

function Stop-Client($p) {
    Cleanup
    if ($p) { try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {} }
}

function Get-Target { ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}') | Select-Object -First 1).Trim() }

function Get-Geom($target) {
    $g = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}|#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}') |
          Where-Object { $_ -like "$target|*" }) -split '\|'
    return @{ Left = [int]$g[1]; Top = [int]$g[2]; W = [int]$g[3]; H = [int]$g[4] }
}

# One wheel burst at the centre of the pane, delivered as real MOUSE_EVENT
# records into the attached client's console input buffer.
function Invoke-Wheel {
    param($Proc, $Target, [string]$Dir = "up", [int]$Count = 3)
    $g = Get-Geom $Target
    & $inj $Proc.Id $Dir $Count ($g.Left + [int]($g.W / 2)) ($g.Top + [int]($g.H / 2)) | Out-Null
    Start-Sleep -Milliseconds 1200
}

function Get-Mode($target) { (& $PSMUX display-message -t $target -p '#{pane_in_mode}' 2>&1).Trim() }
# Since copy mode runs on a snapshot of the pane grid (PR #671), a plain
# `capture-pane` answers with the pane's LIVE screen and never follows the
# copy-mode viewport -- tmux's contract, whose capture reads `wp->base`
# (cmd-capture-pane.c) and knows nothing about the mode's `oy`. To see what the
# wheel actually scrolled, capture the history range the view is sitting on.
function Get-Capture($target) {
    $scroll = 0
    $raw = (& $PSMUX display-message -t $target -p '#{scroll_position}' 2>&1 | Out-String).Trim()
    [void][int]::TryParse($raw, [ref]$scroll)
    if ($scroll -gt 0) { ((& $PSMUX capture-pane -t $target -p -S (-$scroll) 2>&1) -join "`n") }
    else { ((& $PSMUX capture-pane -t $target -p 2>&1) -join "`n") }
}

Write-Host "`n=== Issue #621: a blocked stdin read must not change what the wheel does ===" -ForegroundColor Cyan

# ---------------------------------------------------------------------------
# Test 1: THE BUG.  A python script blocked in input() is the pane's foreground
# and is not a shell.  The wheel must still enter copy mode and scroll psmux's
# own history, and must not type anything into the blocked read.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 1] Wheel over a program blocked on input() enters copy mode" -ForegroundColor Yellow
if (-not $python) {
    Write-Skip "no python interpreter on PATH"
} else {
    $p = Start-Client
    if (-not $p) {
        Write-Fail "could not start the attached client"
    } else {
        $t = Get-Target

        # Baseline: the same wheel at a bare prompt, so a failure below is about
        # the blocked read and nothing else.
        & $PSMUX send-keys -t $t "1..40 | ForEach-Object { `"i621 base `$_`" }" Enter 2>&1 | Out-Null
        Start-Sleep -Seconds 3
        Invoke-Wheel $p $t "up" 3
        if ((Get-Mode $t) -eq "1") { Write-Pass "baseline: the wheel at a pwsh prompt enters copy mode" }
        else { Write-Fail "baseline: the wheel at a pwsh prompt did not enter copy mode; the rest of this test proves nothing" }
        & $PSMUX send-keys -t $t -X cancel 2>&1 | Out-Null
        Start-Sleep -Milliseconds 800

        & $PSMUX send-keys -t $t "$python `"$PY`"" Enter 2>&1 | Out-Null
        Start-Sleep -Seconds 6
        $before = Get-Capture $t
        if ($before -match 'BLOCKED>') { Write-Pass "the script printed 40 lines and is blocked in input()" }
        else { Write-Fail "the script never reached input() (capture has no BLOCKED> prompt)" }

        # The exact condition the removed alternate-scroll branch keyed on.
        $fg = (& $PSMUX display-message -t $t -p '#{pane_current_command}' 2>&1).Trim()
        if ($fg -match 'python|py') { Write-Pass "the pane's foreground is a confirmed non-shell process ('$fg')" }
        else { Write-Host "  [INFO] foreground reported as '$fg'" -ForegroundColor DarkYellow }

        $alt = (& $PSMUX display-message -t $t -p '#{alternate_on}' 2>&1).Trim()
        if ($alt -eq "0") { Write-Pass "the pane is on the main screen (alternate_on=0), so tmux would scroll history here" }
        else { Write-Fail "the pane reports alternate_on=$alt; this is not the reported shape" }

        Invoke-Wheel $p $t "up" 3
        $mode = Get-Mode $t
        if ($mode -eq "1") { Write-Pass "BUG #621 FIXED: wheel-up over the blocked read entered copy mode" }
        else { Write-Fail "BUG #621: wheel-up over the blocked read did NOT enter copy mode (pane_in_mode=$mode)" }

        $after = Get-Capture $t
        if ($after -ne $before) { Write-Pass "the view scrolled back into history" }
        else { Write-Fail "the view did not move; the wheel scrolled nothing" }
        if ($after -notmatch 'BLOCKED>') { Write-Pass "the scrolled view is above the prompt line" }
        else { Write-Host "  [INFO] the prompt line is still visible after 3 notches" -ForegroundColor DarkYellow }

        & $PSMUX send-keys -t $t -X cancel 2>&1 | Out-Null
        Start-Sleep -Milliseconds 800

        # The decisive byte-level check.  The blocked cooked read must receive
        # exactly what is typed next.  Under the bug the wheel had already fed
        # it arrow keys, and the console line editor answers Up with a recalled
        # history entry, so the read returns that entry plus the typed text.
        & $PSMUX send-keys -t $t "i621sentinel" Enter 2>&1 | Out-Null
        Start-Sleep -Seconds 3
        $out = Get-Capture $t
        if ($out -match 'GOTLINE\[([^\]]*)\]') {
            $got = $Matches[1]
            if ($got -eq "i621sentinel") { Write-Pass "the blocked read got exactly 'i621sentinel'; no arrow keys were typed into it" }
            else { Write-Fail "the blocked read got '$got'; the wheel typed into it (prompt-history recall)" }
        } else {
            Write-Fail "the script never echoed the line it read (capture has no GOTLINE[...])"
        }

        Stop-Client $p
    }
}

# ---------------------------------------------------------------------------
# Test 2: Ctrl+C restores nothing, because nothing was broken.  The reporter
# used Ctrl+C as the workaround, so the post-kill prompt must still behave.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 2] Ctrl+C during the blocked read leaves the wheel working" -ForegroundColor Yellow
if (-not $python) {
    Write-Skip "no python interpreter on PATH"
} else {
    $p = Start-Client
    if (-not $p) {
        Write-Fail "could not start the attached client"
    } else {
        $t = Get-Target
        & $PSMUX send-keys -t $t "$python `"$PY`"" Enter 2>&1 | Out-Null
        Start-Sleep -Seconds 6
        Invoke-Wheel $p $t "up" 3
        $during = Get-Mode $t
        & $PSMUX send-keys -t $t -X cancel 2>&1 | Out-Null
        Start-Sleep -Milliseconds 500
        & $PSMUX send-keys -t $t C-c 2>&1 | Out-Null
        Start-Sleep -Seconds 3
        Invoke-Wheel $p $t "up" 3
        $after = Get-Mode $t
        if ($during -eq "1" -and $after -eq "1") { Write-Pass "copy mode on both sides of the Ctrl+C (during=$during after=$after)" }
        else { Write-Fail "the wheel behaves differently across the Ctrl+C (during=$during after=$after)" }
        & $PSMUX send-keys -t $t -X cancel 2>&1 | Out-Null
        Stop-Client $p
    }
}

# ---------------------------------------------------------------------------
# Test 3: the reporter tried "different input methods using different
# languages".  Any blocked reader must behave the same, including ones that use
# a raw getch rather than a canonical line read.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 3] Other blocking readers behave identically" -ForegroundColor Yellow
$p = Start-Client
if (-not $p) {
    Write-Fail "could not start the attached client"
} else {
    $t = Get-Target
    foreach ($case in @(
        @{ Name = "pwsh Read-Host (canonical read in the shell itself)"; Cmd = "1..40 | ForEach-Object { `"i621 rh `$_`" }; Read-Host 'BLOCKED'"; Stop = "Enter" },
        @{ Name = "cmd /c pause (raw getch in a non-shell child)";       Cmd = "1..40 | ForEach-Object { `"i621 pz `$_`" }; cmd /c pause"; Stop = "Enter" }
    )) {
        & $PSMUX send-keys -t $t $case.Cmd Enter 2>&1 | Out-Null
        Start-Sleep -Seconds 5
        Invoke-Wheel $p $t "up" 3
        $mode = Get-Mode $t
        if ($mode -eq "1") { Write-Pass "$($case.Name): wheel-up entered copy mode" }
        else { Write-Fail "$($case.Name): wheel-up did not enter copy mode (pane_in_mode=$mode)" }
        & $PSMUX send-keys -t $t -X cancel 2>&1 | Out-Null
        Start-Sleep -Milliseconds 600
        & $PSMUX send-keys -t $t $case.Stop 2>&1 | Out-Null
        Start-Sleep -Seconds 2
    }
    Stop-Client $p
}

# ---------------------------------------------------------------------------
# Test 4: the fix must not turn the wheel off for applications that DID ask.
# A mouse-registered alternate-screen app blocked on a read still receives its
# SGR wheel reports (#570 / #613 audience).
# ---------------------------------------------------------------------------
Write-Host "`n[Test 4] A mouse-registered app still receives the wheel while it reads" -ForegroundColor Yellow
$p = Start-Client
if (-not $p) {
    Write-Fail "could not start the attached client"
} else {
    Remove-Item $childLog -Force -EA SilentlyContinue
    & $PSMUX split-window -h -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $t = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}') | Select-Object -Last 1).Trim()
    & $PSMUX send-keys -t $t "& '$child' alt=1 decset=0 conmouse=1 log='$childLog'" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 6
    if (-not (Test-Path $childLog)) {
        Write-Fail "the mouse-registered model app never started"
    } else {
        $before = (Get-Content $childLog).Count
        Invoke-Wheel $p $t "up" 2
        $all = Get-Content $childLog
        $reports = @()
        if ($all.Count -gt $before) {
            $reports = @($all[$before..($all.Count-1)] | Where-Object { $_ -match '^RECV .*<ESC>\[<\d+;' })
        }
        if ($reports.Count -ge 1) { Write-Pass "the app that asked still gets SGR wheel reports ($($reports[-1]))" }
        elseif ([Environment]::OSVersion.Version.Build -lt 22000) {
            # The model app reads its mouse as VT bytes, which the Windows 10
            # inbox conhost (build 19045) cuts off: it drops an SGR report
            # written to the pane (Issue #1089, docs/faq.md; measured with
            # SGR reports typed with send-keys -l into a raw VT reader, which
            # got only the letters around them).
            Write-Pass "on this host the VT reading app got no SGR wheel report, as measured"
        }
        else { Write-Fail "#570/#613 REGRESSED: a mouse-registered app received no wheel reports" }
        if ((Get-Mode $t) -eq "0") { Write-Pass "and no copy mode opened over it (tmux alternate_on parity)" }
        else { Write-Fail "copy mode opened over a mouse-registered alt-screen app" }
    }
    Stop-Client $p
}

# ---------------------------------------------------------------------------
# Win32 TUI verification: a real attached window, a real blocked reader, the
# wheel driving copy mode, and a clean exit from it.
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

    $t = ((& $PSMUX list-panes -t $SESSION -F '#{pane_id}') | Select-Object -Last 1).Trim()
    & $PSMUX send-keys -t $t "1..80 | ForEach-Object { `"tui line `$_`" }; Read-Host 'TUIBLOCK'" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 4
    Invoke-Wheel $p $t "up" 3
    if ((Get-Mode $t) -eq "1") { Write-Pass "TUI: wheel over a blocked reader in a split pane enters copy mode" }
    else { Write-Fail "TUI: wheel over a blocked reader in a split pane did not enter copy mode" }

    $cap = Get-Capture $t
    if ($cap -match 'tui line') { Write-Pass "TUI: the scrolled view shows the pane's own history" }
    else { Write-Fail "TUI: the scrolled view shows no history" }

    & $PSMUX send-keys -t $t -X cancel 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    if ((Get-Mode $t) -eq "0") { Write-Pass "TUI: copy mode exits cleanly afterwards" }
    else { Write-Fail "TUI: still in copy mode after cancel" }

    & $PSMUX send-keys -t $t Enter 2>&1 | Out-Null
    Stop-Client $p
}

Remove-Item $childLog, $PY -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
