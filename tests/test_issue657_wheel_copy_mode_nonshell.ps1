# Issue #657: "mouse wheel does not enter copy mode when a non shell application
# has filled the pane" (Alacritty 0.17, psmux 3.3.8).
#
# REPORT.  A Spring Boot / Java process runs in a normal pane, prints its boot log
# until every row is full and the cursor sits on the last one, and never asks for
# mouse reporting or the alternate screen.  Rolling the wheel up does nothing at
# all.  Scrollback is only reachable by typing prefix + [ first.
#
# WHAT MADE THAT PANE DIFFERENT.  psmux 3.3.8 (66cf613) decided the wheel on the
# pane's FOREGROUND PROCESS, not on the pane's terminal state.  Both wheel
# entry points ended in the same blanket branch (window_ops.rs at 66cf613):
#
#     let (non_shell_fg, fg_name) = ... foreground_is_shell ...
#     } else if non_shell_fg && !is_legacy_pager {
#         // General alternate-scroll: arrow keys (tmux DECSET-1007 parity).
#         let seq: &[u8] = if up { b"\x1b[A" } else { b"\x1b[B" };
#         for _ in 0..3 { crate::input::write_key_seq(pane, seq); }
#     } else if up && app.scroll_enter_copy_mode {
#         enter_copy_mode(app);          // <- never reached while java runs
#
# A JVM IS the pane's foreground process and it is not a shell, so every notch
# was translated into three Up arrows written into the JVM's pty.  A logging
# application reads nothing from stdin, so the arrows vanished and the pane sat
# still: "wheel up does nothing".  The same branch is what #621 measured as
# prompt-history cycling for a blocked `input()`.
#
# TMUX PARITY.  tmux's default binding (key-bindings.c:510) is
#
#     bind -n WheelUpPane { if -F '#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}' \
#         { send -M } { copy-mode -e } }
#
# and `mouse_any_flag` is `wp->base.mode & ALL_MOUSE_MODES` (format.c:1952,
# tmux.h:698) - a mouse protocol on the PANE'S OWN SCREEN, set by the
# application's DECSET.  Neither term asks what program is in the foreground and
# neither looks at what is painted on the screen.  A main screen program that
# never enabled mouse tracking gets copy mode scrollback in tmux whatever it is.
#
# WHAT PINS IT.  PR #548 replaced the foreground-process branch with
# `pane_wheel_forward` (alternate screen, or a mouse protocol the application
# itself enabled), and #613 made that decision durable.  This suite asserts the
# reporter's exact shape on every path a wheel notch can take into psmux:
#
#   1. Win32 MOUSE_EVENT records (a local terminal, WriteConsoleInput)
#   2. raw SGR bytes on stdin (Alacritty / WezTerm / kitty over ssh)
#   3. a real Alacritty window driven with a real WM_MOUSEWHEEL, when Alacritty
#      is installed on the machine
#
# and it keeps the opposite audience honest: an alternate screen TUI that reads
# the wheel itself must still receive it (#598, #548, #613).
#
# Layers: E2E over real attached clients, real console wheel injection, real VT
#         wheel bytes, real GUI wheel messages, copy-mode state, scroll position,
#         and byte-exact proof of what the child did or did not receive.

$ErrorActionPreference = "Continue"

$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }

# An attached client launched from an agent shell inherits the caller's routing
# variables and re-enters the wrong session; scrub them before Start-Process.
foreach ($v in @("PSMUX_SESSION_NAME", "PSMUX_SESSION", "PSMUX_PANE")) {
    Remove-Item "env:$v" -EA SilentlyContinue
}

$NS      = "i657"
$NS_OUT  = "i657o"
$NS_IN   = "i657i"
$SESSION = "i657s"
$S_OUT   = "i657outer"
$S_IN    = "i657inner"
$ESC     = [char]27
# The Windows 10 inbox conhost (build 19045) drops an SGR mouse report typed
# into a pane, and hands a VT reading program no wheel either way: measured,
# SGR wheel, press and release reports typed with send-keys -l into a pane
# whose program reads raw VT input arrived as nothing while the letters
# around them arrived.
$Win10   = [Environment]::OSVersion.Version.Build -lt 22000

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }

$inj      = "$env:TEMP\psmux_i657_injector.exe"
$logChild = "$env:TEMP\psmux_i657_log_child.exe"
$altChild = "$env:TEMP\psmux_i657_alt_child.exe"
$guiWheel = "$env:TEMP\psmux_i657_gui_wheel.exe"
$childLog = "$env:TEMP\psmux_i657_child.txt"
$altLog   = "$env:TEMP\psmux_i657_alt.txt"

foreach ($pair in @(@($inj, "mouse_injector.cs"),
                    @($logChild, "issue657_log_child.cs"),
                    @($altChild, "altscreen_mouse_child.cs"),
                    @($guiWheel, "issue657_gui_wheel.cs"))) {
    Remove-Item $pair[0] -Force -EA SilentlyContinue
    & $csc /nologo /optimize /out:$($pair[0]) (Join-Path $repoTests $pair[1]) 2>&1 | Out-Null
    if (-not (Test-Path $pair[0])) {
        Write-Host "FATAL: could not compile $($pair[1])" -ForegroundColor Red
        exit 1
    }
}

# When a pane reports the wrong foreground, say WHY, so the next failure names
# its mechanism instead of just the stranger's name.  The 2026-10-02 sweep
# reported 'vctip' here: an MSVC telemetry helper that outlives the linker
# that started it, so its ParentProcessId names a dead pid, and when Windows
# recycles that pid into a pane process an unguarded tree walk adopts it.
# This prints, for every live process with the reported name, its pid, its
# parent pid and creation time, and who holds that parent pid NOW with that
# process's creation time.  A child older than the holder of its parent pid
# can only be a recycled pid.
function Describe-Foreground($ns, $target, $fgName) {
    try {
        $panePid = (& $PSMUX -L $ns display-message -t $target -p '#{pane_pid}' 2>&1 | Out-String).Trim()
        Write-Info "pane_pid=$panePid pane_current_command='$fgName'"
        $fmt = { param($p) if ($p) { "pid=$($p.ProcessId) name=$($p.Name) ppid=$($p.ParentProcessId) created=$($p.CreationDate.ToString('HH:mm:ss.fff'))" } else { "(gone)" } }
        if ($panePid -match '^\d+$') {
            $pp = Get-CimInstance Win32_Process -Filter "ProcessId=$panePid" -EA SilentlyContinue
            Write-Info ("  pane process: " + (& $fmt $pp))
        }
        if ($fgName) {
            $leafs = @(Get-CimInstance Win32_Process -Filter "Name='$fgName.exe'" -EA SilentlyContinue)
            if ($leafs.Count -eq 0) { Write-Info "  no live process named $fgName.exe (it exited)" }
            foreach ($l in $leafs) {
                $holder = Get-CimInstance Win32_Process -Filter "ProcessId=$($l.ParentProcessId)" -EA SilentlyContinue
                Write-Info ("  foreground candidate: " + (& $fmt $l))
                Write-Info ("    holder of its parent pid now: " + (& $fmt $holder))
                if ($holder -and $l.CreationDate -lt $holder.CreationDate) {
                    Write-Info "    -> the 'child' is OLDER than the process holding its parent pid: a RECYCLED parent pid, not a real child"
                }
            }
        }
    } catch {
        Write-Info "  (could not describe the foreground: $($_.Exception.Message))"
    }
}

function Cleanup-All {
    foreach ($n in @($NS, $NS_IN, $NS_OUT)) { & $PSMUX -L $n kill-server 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 800
}

Write-Host "`n=== Issue #657: the wheel must enter copy mode over a non shell pane ===" -ForegroundColor Cyan
Write-Host "  binary under test: $PSMUX" -ForegroundColor DarkGray

# ---------------------------------------------------------------------------
# Layer 1: the local terminal path - real attached client, real MOUSE_EVENT
# records written with WriteConsoleInput, the reporter's screen shape.
# ---------------------------------------------------------------------------
Write-Host "`n[Layer 1] Win32 MOUSE_EVENT wheel over a screen filling non shell app" -ForegroundColor Yellow

Cleanup-All
Remove-Item $childLog -Force -EA SilentlyContinue
$client = Start-Process -FilePath $PSMUX -ArgumentList "-L",$NS,"new-session","-s",$SESSION -PassThru
Start-Sleep -Seconds 6
& $PSMUX -L $NS has-session -t $SESSION 2>$null
$clientUp = ($LASTEXITCODE -eq 0)

if (-not $clientUp) {
    Write-Fail "could not start an attached client for the local wheel path"
} else {
    & $PSMUX -L $NS set-option -t $SESSION -g mouse on 2>&1 | Out-Null
    & $PSMUX -L $NS set-option -t $SESSION -g scroll-enter-copy-mode on 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    # The stand in for the JVM: not a shell, fills every row, cursor on the last
    # one, no DECSET, no alternate screen, no console mode change.
    & $PSMUX -L $NS send-keys -t $SESSION "& '$logChild' 80 '$childLog'" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 6

    # NOTE: tmux's third WheelUpPane term, `#{mouse_any_flag}`, became a psmux
    # format variable in #662; test_issue662_mouse_flag_formats.ps1 reads it
    # directly.  Here the pane's "asked for nothing" state is still asserted
    # from what it IS: alternate_on is 0, the model app writes plain text and
    # nothing else, and Layer 2 below shows the same wheel at the same moment
    # reaching an application that did ask.
    $fields = ((& $PSMUX -L $NS display-message -t $SESSION -p `
        '#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}|#{pane_current_command}|#{pane_in_mode}|#{alternate_on}') 2>&1).Trim() -split '\|'
    $fg = $fields[4]
    $rows = (& $PSMUX -L $NS capture-pane -t $SESSION -p 2>&1)
    $blank = ($rows | Where-Object { $_.Trim() -eq "" }).Count

    if ($fg -like "*log_child*") {
        Write-Pass "the pane's foreground is the non shell app ($fg)"
    } else {
        Write-Fail "the pane's foreground is '$fg', not the model app; this layer would prove nothing"
        Describe-Foreground $NS $SESSION $fg
    }
    if ($blank -eq 0 -and $rows.Count -ge 20) {
        Write-Pass "the app filled the pane: $($rows.Count) painted rows, 0 blank"
    } else {
        Write-Fail "the pane is not full ($($rows.Count) rows, $blank blank); the reporter's shape is not set up"
    }
    if ($fields[6] -eq "0") {
        Write-Pass "the app is on the main screen (alternate_on=0), so tmux would open copy mode here"
    } else {
        Write-Fail "the model app is on the alternate screen (alternate_on=$($fields[6])); it is not the reporter's shape"
    }

    $px = [int]$fields[0] + [int]([int]$fields[2] / 2)
    $py = [int]$fields[1] + [int]([int]$fields[3] / 2)
    $logBefore = if (Test-Path $childLog) { (Get-Content $childLog).Count } else { 0 }
    $modeBefore = (& $PSMUX -L $NS display-message -t $SESSION -p '#{pane_in_mode}' 2>&1).Trim()

    & $inj $client.Id up 3 $px $py 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1800

    $modeAfter = (& $PSMUX -L $NS display-message -t $SESSION -p '#{pane_in_mode}' 2>&1).Trim()
    $scroll = (& $PSMUX -L $NS display-message -t $SESSION -p '#{scroll_position}' 2>&1).Trim()
    $logAll = if (Test-Path $childLog) { Get-Content $childLog } else { @() }
    $childNew = if ($logAll.Count -gt $logBefore) { @($logAll[$logBefore..($logAll.Count-1)]) } else { @() }

    Write-Info "pane_in_mode before=$modeBefore after=$modeAfter scroll_position=$scroll"
    if ($modeBefore -ne "1" -and $modeAfter -eq "1") {
        Write-Pass "wheel up entered copy mode over the filled non shell pane (#657)"
    } else {
        Write-Fail "wheel up did NOT enter copy mode (before=$modeBefore after=$modeAfter) - #657 is present"
    }
    if ([int]($scroll -replace '[^0-9]','0') -gt 0) {
        Write-Pass "the view really moved back into the scrollback (scroll_position=$scroll)"
    } else {
        Write-Fail "the view did not move back into the scrollback (scroll_position=$scroll)"
    }
    # What the application received on stdin.  Read this as one sided evidence:
    # an SGR report or an arrow key showing up here proves the wheel was
    # forwarded, but silence proves nothing on its own, because a console left
    # in its cooked shape answers ESC[A in its own line editor (history recall)
    # and never hands the bytes to the program.  That is exactly why 3.3.8's
    # arrow branch looked like "the wheel does nothing" to the reporter.  The
    # copy-mode and scroll_position checks above are the load bearing ones; the
    # Rust tests in tests-rs watch the pty end byte for byte.
    $forwarded = @($childNew | Where-Object { $_ -match '<ESC>\[<6[45];' -or $_ -match '<ESC>\[[AB]' })
    if ($forwarded.Count -eq 0) {
        Write-Pass "the application was fed no wheel report and no arrow keys"
    } else {
        Write-Fail "the wheel was fed to the application instead of scrolling: $($forwarded[0])"
    }

    # tmux parity for the rest of the gesture: more up stays in copy mode, and
    # wheeling back to the bottom leaves it.
    & $inj $client.Id up 2 $px $py 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1200
    $stillUp = (& $PSMUX -L $NS display-message -t $SESSION -p '#{pane_in_mode}' 2>&1).Trim()
    if ($stillUp -eq "1") { Write-Pass "further notches keep scrolling inside copy mode" }
    else { Write-Fail "the pane fell out of copy mode on the second burst (pane_in_mode=$stillUp)" }

    & $inj $client.Id down 12 $px $py 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1800
    $back = (& $PSMUX -L $NS display-message -t $SESSION -p '#{pane_in_mode}' 2>&1).Trim()
    if ($back -eq "0") { Write-Pass "wheeling back to the live output leaves copy mode (tmux parity)" }
    else { Write-Fail "the pane stayed in copy mode after wheeling all the way down (pane_in_mode=$back)" }

    # ---------------------------------------------------------------------
    # The opposite audience must not regress: an alternate screen TUI that
    # registered the mouse itself still receives the wheel (#598/#548/#613).
    # ---------------------------------------------------------------------
    Write-Host "`n[Layer 2] An alt screen mouse app must still receive the wheel" -ForegroundColor Yellow
    Remove-Item $altLog -Force -EA SilentlyContinue
    & $PSMUX -L $NS split-window -h -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $target = ((& $PSMUX -L $NS list-panes -t $SESSION -F '#{pane_id}') | Select-Object -Last 1).Trim()
    & $PSMUX -L $NS send-keys -t $target "& '$altChild' alt=1 decset=1 conmouse=1 log='$altLog'" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 6
    $ag = ((& $PSMUX -L $NS display-message -t $target -p '#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}') 2>&1).Trim() -split '\|'
    $ax = [int]$ag[0] + [int]([int]$ag[2] / 2)
    $ay = [int]$ag[1] + [int]([int]$ag[3] / 2)
    $altBefore = if (Test-Path $altLog) { (Get-Content $altLog).Count } else { 0 }
    & $inj $client.Id up 2 $ax $ay 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1600
    $altAll = if (Test-Path $altLog) { Get-Content $altLog } else { @() }
    $altNew = if ($altAll.Count -gt $altBefore) { @($altAll[$altBefore..($altAll.Count-1)]) } else { @() }
    $altGot = @($altNew | Where-Object { $_ -match '<ESC>\[<64;' })
    $altMode = (& $PSMUX -L $NS display-message -t $target -p '#{pane_in_mode}' 2>&1).Trim()
    if ($Win10) {
        # This app reads its mouse as VT bytes, which the Windows 10 inbox
        # conhost cuts off (Issue #1089, docs/faq.md): the pane keeps the
        # wheel from copy mode below, but nothing reaches the app.
        if ($altGot.Count -eq 0) { Write-Pass "on this host the VT reading app got no SGR wheel report, as measured" }
        else { Write-Fail "the VT reading app got an SGR wheel report, which this host was measured not to deliver ($($altGot[0]))" }
    } elseif ($altGot.Count -ge 1) {
        Write-Pass "the alt screen mouse app still receives the wheel ($($altGot[0]))"
    } else {
        Write-Fail "the wheel no longer reaches an alt screen mouse app; #598/#548 regressed"
    }
    if ($altMode -ne "1") { Write-Pass "and that pane did NOT enter copy mode" }
    else { Write-Fail "the alt screen mouse app's pane entered copy mode instead of getting the wheel" }
}

if ($client) { try { Stop-Process -Id $client.Id -Force -EA SilentlyContinue } catch {} }
Cleanup-All

# ---------------------------------------------------------------------------
# Layer 3: the VT / SGR path.  An Alacritty or kitty link (and every ssh login)
# hands the client raw SGR bytes instead of console records; the same pane shape
# must reach the same decision.  The harness of #629: an OUTER psmux owns a
# ConPTY and the client under test runs inside it with SSH_CONNECTION set.
# ---------------------------------------------------------------------------
Write-Host "`n[Layer 3] Raw SGR wheel bytes over the same non shell pane" -ForegroundColor Yellow

$vtLog = "$env:TEMP\psmux_i657_child_vt.txt"
Remove-Item $vtLog -Force -EA SilentlyContinue
& $PSMUX -L $NS_OUT new-session -d -s $S_OUT -x 120 -y 40 2>&1 | Out-Null
Start-Sleep -Seconds 4
& $PSMUX -L $NS_OUT has-session -t $S_OUT 2>$null
$outerUp = ($LASTEXITCODE -eq 0)

if (-not $outerUp) {
    Write-Fail "could not bring up the outer ConPTY for the VT wheel path"
} else {
    $cmd = "Remove-Item Env:\PSMUX_SESSION,Env:\PSMUX_SESSION_NAME,Env:\PSMUX_PANE -EA SilentlyContinue; " +
           "`$env:SSH_CONNECTION='10.0.0.5 51000 10.0.0.9 22'; `$env:SSH_TTY='/dev/pts/0'; " +
           "`$env:TERM='xterm-256color'; & '$PSMUX' -L $NS_IN new-session -s $S_IN"
    & $PSMUX -L $NS_OUT send-keys -t $S_OUT $cmd Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 9
    & $PSMUX -L $NS_IN has-session -t $S_IN 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "the inner client under test never came up on the VT input path"
    } else {
        & $PSMUX -L $NS_IN set-option -t $S_IN -g mouse on 2>&1 | Out-Null
        & $PSMUX -L $NS_IN set-option -t $S_IN -g scroll-enter-copy-mode on 2>&1 | Out-Null
        Start-Sleep -Milliseconds 400
        & $PSMUX -L $NS_IN send-keys -t $S_IN "& '$logChild' 60 '$vtLog'" Enter 2>&1 | Out-Null
        Start-Sleep -Seconds 6

        $vfg = (& $PSMUX -L $NS_IN display-message -t $S_IN -p '#{pane_current_command}' 2>&1).Trim()
        if ($vfg -like "*log_child*") {
            Write-Pass "the inner pane's foreground is the non shell app ($vfg)"
        } else {
            Write-Fail "the inner pane's foreground is '$vfg'; this layer would prove nothing"
            Describe-Foreground $NS_IN $S_IN $vfg
        }

        $vBefore = (& $PSMUX -L $NS_IN display-message -t $S_IN -p '#{pane_in_mode}' 2>&1).Trim()
        $vLogBefore = if (Test-Path $vtLog) { (Get-Content $vtLog).Count } else { 0 }
        for ($i = 0; $i -lt 3; $i++) {
            & $PSMUX -L $NS_OUT send-keys -t $S_OUT -l "$ESC[<64;25;10M" 2>&1 | Out-Null
            Start-Sleep -Milliseconds 350
        }
        Start-Sleep -Milliseconds 1200
        $vAfter = (& $PSMUX -L $NS_IN display-message -t $S_IN -p '#{pane_in_mode}' 2>&1).Trim()
        $vScroll = (& $PSMUX -L $NS_IN display-message -t $S_IN -p '#{scroll_position}' 2>&1).Trim()
        $vAll = if (Test-Path $vtLog) { Get-Content $vtLog } else { @() }
        $vNew = if ($vAll.Count -gt $vLogBefore) { @($vAll[$vLogBefore..($vAll.Count-1)]) } else { @() }

        Write-Info "inner pane_in_mode before=$vBefore after=$vAfter scroll_position=$vScroll"
        if ($Win10) {
            # The reports are typed into the outer pane, which this host
            # drops, so the client under test gets none of them.
            if ($vAfter -ne "1" -and [int]($vScroll -replace '[^0-9]','0') -eq 0) { Write-Pass "on this host the outer pane handed the client no SGR report (pane_in_mode=$vAfter scroll_position=$vScroll)" }
            else { Write-Fail "a report reached the client (pane_in_mode=$vAfter scroll_position=$vScroll), which this host was measured not to deliver" }
        } elseif ($vBefore -ne "1" -and $vAfter -eq "1") {
            Write-Pass "SGR wheel up entered copy mode on the VT path too"
        } else {
            Write-Fail "SGR wheel up did NOT enter copy mode (before=$vBefore after=$vAfter) - #657 on the VT path"
        }
        if (-not $Win10) {
            if ([int]($vScroll -replace '[^0-9]','0') -gt 0) {
                Write-Pass "the VT path scrolled back as well (scroll_position=$vScroll)"
            } else {
                Write-Fail "the VT path did not move back into the scrollback (scroll_position=$vScroll)"
            }
        }
        $vFwd = @($vNew | Where-Object { $_ -match '<ESC>\[<6[45];' -or $_ -match '<ESC>\[[AB]' })
        if ($vFwd.Count -eq 0) {
            Write-Pass "the VT path forwarded neither an SGR report nor arrow keys to the application"
        } else {
            Write-Fail "the VT path fed the application: $($vFwd[0])"
        }
    }
}
Cleanup-All

# ---------------------------------------------------------------------------
# Layer 4: the reporter's own terminal.  When Alacritty is installed, run the
# whole thing inside a real Alacritty window and roll a real wheel at it with
# WM_MOUSEWHEEL, so nothing about the input path is simulated.
# ---------------------------------------------------------------------------
Write-Host "`n[Layer 4] A real wheel in a real Alacritty window" -ForegroundColor Yellow

$alacritty = "C:\Program Files\Alacritty\alacritty.exe"
if (-not (Test-Path $alacritty)) {
    $cmdInfo = Get-Command alacritty -EA SilentlyContinue
    if ($cmdInfo) { $alacritty = $cmdInfo.Source }
}
if (-not (Test-Path $alacritty)) {
    Write-Skip "Alacritty is not installed on this machine; the reporter's terminal layer is skipped"
} else {
    $alaLog = "$env:TEMP\psmux_i657_child_ala.txt"
    Remove-Item $alaLog -Force -EA SilentlyContinue
    $ala = Start-Process -FilePath $alacritty -ArgumentList `
        "-o","window.dimensions.columns=120","-o","window.dimensions.lines=40",`
        "-e",$PSMUX,"-L",$NS,"new-session","-s",$SESSION -PassThru
    Start-Sleep -Seconds 14
    & $PSMUX -L $NS has-session -t $SESSION 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "psmux did not come up inside Alacritty"
    } else {
        & $PSMUX -L $NS set-option -t $SESSION -g mouse on 2>&1 | Out-Null
        & $PSMUX -L $NS set-option -t $SESSION -g scroll-enter-copy-mode on 2>&1 | Out-Null
        # Alacritty answers psmux's capability probe and the reply can land at
        # the prompt as stray text; clear the line before typing the command.
        & $PSMUX -L $NS send-keys -t $SESSION C-c 2>&1 | Out-Null
        Start-Sleep -Seconds 2
        & $PSMUX -L $NS send-keys -t $SESSION "& '$logChild' 80 '$alaLog'" Enter 2>&1 | Out-Null
        Start-Sleep -Seconds 10

        $afg = (& $PSMUX -L $NS display-message -t $SESSION -p '#{pane_current_command}' 2>&1).Trim()
        $aBefore = (& $PSMUX -L $NS display-message -t $SESSION -p '#{pane_in_mode}' 2>&1).Trim()
        if ($afg -like "*log_child*") {
            Write-Pass "the Alacritty pane's foreground is the non shell app ($afg)"
        } else {
            Write-Fail "the Alacritty pane's foreground is '$afg'; this layer would prove nothing"
            Describe-Foreground $NS $SESSION $afg
        }
        $w = & $guiWheel $ala.Id up 3 400 300 2>&1
        Write-Info ("$w" -replace "`r?`n", " ")
        Start-Sleep -Milliseconds 2000
        $aAfter = (& $PSMUX -L $NS display-message -t $SESSION -p '#{pane_in_mode}' 2>&1).Trim()
        $aScroll = (& $PSMUX -L $NS display-message -t $SESSION -p '#{scroll_position}' 2>&1).Trim()
        Write-Info "Alacritty pane_in_mode before=$aBefore after=$aAfter scroll_position=$aScroll"
        if ($aBefore -ne "1" -and $aAfter -eq "1") {
            Write-Pass "a real wheel in a real Alacritty window entered copy mode"
        } else {
            Write-Fail "a real wheel in Alacritty did NOT enter copy mode (before=$aBefore after=$aAfter) - the reporter's case"
        }
        if ([int]($aScroll -replace '[^0-9]','0') -gt 0) {
            Write-Pass "and it scrolled back into the log (scroll_position=$aScroll)"
        } else {
            Write-Fail "the Alacritty pane did not move back into the log (scroll_position=$aScroll)"
        }
    }
    if ($ala) { try { Stop-Process -Id $ala.Id -Force -EA SilentlyContinue } catch {} }
}
Cleanup-All

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)"
Write-Host "  Failed: $($script:TestsFailed)"
exit $script:TestsFailed
