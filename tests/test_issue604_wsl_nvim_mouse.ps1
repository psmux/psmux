# Issue #604: "Mouse support seems to conflicts in some scenarios" (fekir)
#
# Reported table:
#
#   powershell -> psmux ->        nvim.exe -u NONE -> click : cursor MOVES
#   powershell ->          wsl -> nvim     -u NONE -> click : cursor MOVES
#   powershell -> psmux -> wsl -> nvim     -u NONE -> click : cursor does NOT move
#
#   plus: the cursor FLICKERS when the mouse merely MOVES over a psmux pane
#   running nvim, and does not flicker without psmux.
#
# SYMPTOM A ROOT CAUSE (self-inflicted, measured with PSMUX_PANE_RAW=1):
#   For a VT bridge child (wsl.exe / ssh.exe) psmux injects the SGR report with
#   WriteConsoleInputW.  send_vt_sequence wrapped that write in a
#   SetConsoleMode(set) / SetConsoleMode(restore) pair that also toggled
#   ENABLE_QUICK_EDIT_MODE.  conhost derives "is this client tracking the mouse"
#   from the console input mode, so it mirrored every toggle back up the ConPTY
#   into the PANE'S OUTPUT as `ESC[?1003;1006h` immediately followed by
#   `ESC[?1003;1006l`, one pair per injected event.  psmux's own vt100 parser
#   applied both, and DECRST 1003 clears the mouse protocol outright (tmux does
#   the same, input.c:1955), so the trailing DECRST wiped the DECSET 1002 that
#   nvim had really asked for.  The pane's mode went ButtonMotion -> None on the
#   first forwarded event and the bridge gate suppressed every click after it.
#
# SYMPTOM B ROOT CAUSE (tmux parity):
#   Bare motion was forwarded on DECSET 1002 OR 1003.  1002 is BUTTON-event
#   tracking: report motion only WHILE A BUTTON IS HELD.  tmux discards a bare
#   motion report unless the pane asked for 1003 (input-keys.c:737) and does not
#   even ask the outer terminal for it otherwise (tty.c:897).  `nvim -u NONE`
#   has mouse=nvi, which enables 1002+1006 and never 1003, so every pointer
#   sample sprayed an ESC[<35;col;rowM report at nvim and drove a full state
#   frame plus a client repaint.
#
# Layers: real attached client in its own console, real nvim (Windows and
#         inside WSL), real MOUSE_EVENT injection via WriteConsoleInput, and
#         nvim's own mappings as the ground-truth oracle for what it received.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION_PREFIX = "test_i604"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }

$clickInj = "$env:TEMP\psmux_i604_click.exe"
$moveInj  = "$env:TEMP\psmux_i604_move.exe"
$altChild = "$env:TEMP\psmux_i604_altchild.exe"
foreach ($pair in @(@($clickInj, "click_injector.cs"), @($moveInj, "mouse_move_injector.cs"), @($altChild, "altscreen_mouse_child.cs"))) {
    if (-not (Test-Path $pair[0])) {
        & $csc /nologo /optimize /out:$($pair[0]) (Join-Path $repoTests $pair[1]) 2>&1 | Out-Null
    }
    if (-not (Test-Path $pair[0])) { Write-Host "FATAL: could not compile $($pair[1])" -ForegroundColor Red; exit 1 }
}

# ---------------------------------------------------------------------------
# Availability probes.  A missing nvim or a missing WSL distro is a SKIP, never
# a pass: this suite proves a mouse report reaches a real editor, and there is
# nothing to prove without one.
# ---------------------------------------------------------------------------
$winNvim = (Get-Command nvim -EA SilentlyContinue)
$wslDistro = $null
$wslNvim = $false
try {
    $distros = @((& wsl.exe -l -q 2>$null) | ForEach-Object { ($_ -replace "`0", "").Trim() } | Where-Object { $_ })
    foreach ($d in $distros) {
        if ($d -match 'docker|podman') { continue }
        $probe = (& wsl.exe -d $d -e sh -c 'command -v nvim' 2>$null) -replace "`0", ""
        if ($probe -and $probe.Trim()) { $wslDistro = $d; $wslNvim = $true; break }
    }
} catch { }

# ---------------------------------------------------------------------------
# nvim instrumentation: log every click with the cell nvim decoded, every
# cursor move, and every bare motion report it received.  Mapping <MouseMove>
# does NOT make nvim ask for DECSET 1003 (verified against a bare ConPTY: nvim
# still emits only 1002+1006), so this stays an honest receiver.
# ---------------------------------------------------------------------------
$vimFile = "$env:TEMP\psmux_i604.vim"
@'
let g:i604log = empty($I604LOG) ? '/tmp/psmux_i604_click.log' : $I604LOG
call writefile(['START mouse=' . &mouse], g:i604log)
call setline(1, ['AAAA1111aaaa', 'BBBB2222bbbb', 'CCCC3333cccc', 'DDDD4444dddd', 'EEEE5555eeee', 'FFFF6666ffff', 'GGGG7777gggg', 'HHHH8888hhhh'])
call cursor(1, 1)
nnoremap <silent> <LeftMouse> <LeftMouse>:call writefile(['CLICK cur=' . string(getcurpos()[1:2])], g:i604log, 'a')<CR>
silent! nnoremap <silent> <MouseMove> :call writefile(['MOUSEMOVE'], g:i604log, 'a')<CR>
'@ | Set-Content -Path $vimFile -Encoding ASCII

$winLog = "$env:TEMP\psmux_i604_click_win.log"
$vimFwd = $vimFile -replace '\\','/'

# Launcher that scrubs nesting env vars (this suite may itself run inside psmux)
# while leaving PSMUX_DATA_DIR alone so the client joins the same registry.
$launchCmd = "$env:TEMP\psmux_i604_launch.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
"$PSMUX" new-session -s %1 -x 100 -y 30
"@ | Set-Content -Path $launchCmd -Encoding ASCII

# Runner .cmd files: send-keys a single path, no quoting games.
$runWsl = "$env:TEMP\psmux_i604_run_wsl.cmd"
$runWin = "$env:TEMP\psmux_i604_run_win.cmd"
$runWinPlain = "$env:TEMP\psmux_i604_run_win_plain.cmd"
$runAlt = "$env:TEMP\psmux_i604_run_alt.cmd"
$altLog = "$env:TEMP\psmux_i604_alt.log"
if ($wslDistro) {
@"
@echo off
wsl.exe -d $wslDistro -e env I604LOG=/tmp/psmux_i604_click.log nvim -u NONE -c "source /tmp/psmux_i604.vim"
"@ | Set-Content -Path $runWsl -Encoding ASCII
}
@"
@echo off
set I604LOG=$winLog
nvim.exe -u NONE -c "source $vimFwd"
"@ | Set-Content -Path $runWin -Encoding ASCII
@"
@echo off
set I604LOG=$winLog
nvim.exe -u NONE -c "source $vimFwd"
"@ | Set-Content -Path $runWinPlain -Encoding ASCII
# conmouse=1: a native Windows TUI registers the mouse with
# SetConsoleMode(ENABLE_MOUSE_INPUT), which conhost relays to psmux as
# `ESC[?1003;1006h`, which is any-event tracking.  Such an app is the real
# regression risk for the #604 narrowing, so it is what this arm models.
@"
@echo off
"$altChild" alt=1 decset=1 conmouse=1 log=$($altLog -replace '\\','/')
"@ | Set-Content -Path $runAlt -Encoding ASCII

function Cleanup($name) {
    & $PSMUX kill-session -t $name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
}

# Start an attached client, run $Runner in its pane, return the pane geometry.
function Start-Case {
    param([string]$Name, [string]$Runner, [int]$SettleSec = 8)
    Cleanup $Name
    $null = Start-Process -FilePath $launchCmd -ArgumentList $Name -PassThru
    Start-Sleep -Seconds 7
    & $PSMUX has-session -t $Name 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    & $PSMUX set-option -t $Name -g mouse on 2>&1 | Out-Null
    $cli = Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
           Where-Object { $_.CommandLine -match "new-session -s\s+$Name\b" } | Select-Object -First 1
    if (-not $cli) { return $null }
    & $PSMUX send-keys -t $Name ($Runner -replace '\\','/') Enter 2>&1 | Out-Null
    Start-Sleep -Seconds $SettleSec
    $g = ((& $PSMUX list-panes -t $Name -F '#{pane_id}|#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}') |
          Select-Object -First 1) -split '\|'
    if ($g.Count -lt 5) { return $null }
    return @{ Name = $Name; Pid = $cli.ProcessId; Pane = $g[0]
              Left = [int]$g[1]; Top = [int]$g[2]; W = [int]$g[3]; H = [int]$g[4] }
}

function Stop-Case($c) {
    if (-not $c) { return }
    & $PSMUX send-keys -t $c.Name Escape 2>&1 | Out-Null
    & $PSMUX send-keys -t $c.Name ":qa!" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    Cleanup $c.Name
    try { Stop-Process -Id $c.Pid -Force -EA SilentlyContinue } catch {}
}

# 1-based nvim cell -> client console cell, then inject a real left click.
function Invoke-Click($c, [int]$Col, [int]$Row) {
    & $clickInj $c.Pid ($c.Left + $Col - 1) ($c.Top + $Row - 1) 120 | Out-Null
    Start-Sleep -Milliseconds 700
}

Write-Host "`n=== Issue #604: mouse in nvim under psmux, and no motion nobody asked for ===" -ForegroundColor Cyan

$cells = @(@(5,3), @(9,5), @(3,7), @(11,2))

# ---------------------------------------------------------------------------
# Test 1: THE BUG.  psmux -> wsl.exe -> nvim.  Every click must reach nvim and
# move the cursor to the clicked cell.  Before the fix nvim received nothing
# from the very first click onward.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 1] psmux -> wsl -> nvim: a click moves the cursor" -ForegroundColor Yellow
if (-not $wslNvim) {
    Write-Skip "no WSL distro with nvim installed (wsl.exe -l -q / command -v nvim) - cannot test the reported chain"
} else {
    & wsl.exe -d $wslDistro -e rm -f /tmp/psmux_i604_click.log 2>&1 | Out-Null
    $vimUnix = "/mnt/" + ($vimFile.Substring(0,1).ToLower()) + ($vimFile.Substring(2) -replace '\\','/')
    & wsl.exe -d $wslDistro -e cp $vimUnix /tmp/psmux_i604.vim 2>&1 | Out-Null
    $c = Start-Case "${SESSION_PREFIX}_wsl" $runWsl 10
    if (-not $c) {
        Write-Fail "could not start an attached client running wsl nvim"
    } else {
        $started = @((& wsl.exe -d $wslDistro -e sh -c 'cat /tmp/psmux_i604_click.log 2>/dev/null') | Where-Object { $_ -like 'START*' })
        if ($started.Count -eq 0) {
            Write-Skip "nvim inside WSL never started in the pane - nothing to measure"
        } else {
            foreach ($cell in $cells) { Invoke-Click $c $cell[0] $cell[1] }
            Start-Sleep -Seconds 1
            $log = @((& wsl.exe -d $wslDistro -e sh -c 'cat /tmp/psmux_i604_click.log 2>/dev/null') |
                     ForEach-Object { ($_ -replace "`0", "").Trim() })
            $clicks = @($log | Where-Object { $_ -like 'CLICK*' })
            if ($clicks.Count -eq $cells.Count) {
                Write-Pass "all $($cells.Count) clicks reached nvim inside WSL"
            } else {
                Write-Fail "BUG #604: only $($clicks.Count) of $($cells.Count) clicks reached nvim inside WSL (log: $($log -join ' | '))"
            }
            $wrong = @()
            for ($i = 0; $i -lt $clicks.Count -and $i -lt $cells.Count; $i++) {
                $want = "CLICK cur=[$($cells[$i][1]), $($cells[$i][0])]"
                if ($clicks[$i] -ne $want) { $wrong += "got '$($clicks[$i])' wanted '$want'" }
            }
            if ($clicks.Count -eq $cells.Count -and $wrong.Count -eq 0) {
                Write-Pass "the cursor landed on the clicked cell every time"
            } elseif ($clicks.Count -eq $cells.Count) {
                Write-Fail "cursor landed on the wrong cell: $($wrong -join ' ; ')"
            }
        }
        Stop-Case $c
    }
}

# ---------------------------------------------------------------------------
# Test 2: NO REGRESSION for the column the reporter says already works.
# psmux -> nvim.exe uses the PTY pipe, not the bridge path.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 2] psmux -> nvim.exe: a click still moves the cursor" -ForegroundColor Yellow
if (-not $winNvim) {
    Write-Skip "nvim.exe not installed"
} else {
    Remove-Item $winLog -Force -EA SilentlyContinue
    $c = Start-Case "${SESSION_PREFIX}_win" $runWin 9
    if (-not $c) {
        Write-Fail "could not start an attached client running nvim.exe"
    } elseif (-not (Test-Path $winLog)) {
        Write-Skip "nvim.exe never started in the pane - nothing to measure"
    } else {
        foreach ($cell in $cells) { Invoke-Click $c $cell[0] $cell[1] }
        Start-Sleep -Seconds 1
        $clicks = @(Get-Content $winLog | Where-Object { $_ -like 'CLICK*' })
        if ($clicks.Count -eq $cells.Count) { Write-Pass "all $($cells.Count) clicks reached nvim.exe" }
        else { Write-Fail "only $($clicks.Count) of $($cells.Count) clicks reached nvim.exe" }
        Stop-Case $c
    }
}

# ---------------------------------------------------------------------------
# Test 3: SYMPTOM B.  Moving the pointer with no button held over a pane whose
# app enabled DECSET 1002 must forward NOTHING.  tmux, input-keys.c:737:
#
#     if (MOUSE_DRAG(m->sgr_b) && MOUSE_RELEASE(m->sgr_b) &&
#         (~s->mode & MODE_MOUSE_ALL))
#             return (0);
#
# Before the fix nvim received one ESC[<35;col;rowM per pointer sample.
#
# Test 4 below drives the SAME injector with the same geometry against an app
# that DID ask for 1003 and sees every report arrive, so a zero here is a real
# suppression by psmux and not a dead injection harness.
# ---------------------------------------------------------------------------
Write-Host "`n[Test 3] bare pointer motion over a DECSET 1002 app forwards nothing" -ForegroundColor Yellow
if (-not $winNvim) {
    Write-Skip "nvim.exe not installed"
} else {
    Remove-Item $winLog -Force -EA SilentlyContinue
    $c = Start-Case "${SESSION_PREFIX}_motion" $runWinPlain 9
    if (-not $c) {
        Write-Fail "could not start an attached client for the motion test"
    } elseif (-not (Test-Path $winLog)) {
        Write-Skip "nvim.exe never started in the pane - nothing to measure"
    } else {
        $before = @(Get-Content $winLog | Where-Object { $_ -eq 'MOUSEMOVE' }).Count
        & $moveInj $c.Pid move 30 ($c.Left + 4) ($c.Top + 1) 2 0 25 | Out-Null
        Start-Sleep -Seconds 2
        $after = @(Get-Content $winLog | Where-Object { $_ -eq 'MOUSEMOVE' }).Count
        $delta = $after - $before
        if ($delta -eq 0) {
            Write-Pass "30 bare pointer moves forwarded 0 motion reports to a 1002 app (tmux parity)"
        } else {
            Write-Fail "BUG #604: $delta unsolicited motion reports reached an app that only asked for DECSET 1002"
        }
        Stop-Case $c
    }
}

# ---------------------------------------------------------------------------
# Test 4: NO REGRESSION for a real motion consumer.  An app that asked for
# DECSET 1003 (any-event tracking) must still receive bare motion (#60/#296).
# ---------------------------------------------------------------------------
Write-Host "`n[Test 4] a DECSET 1003 app still receives bare motion" -ForegroundColor Yellow
Remove-Item $altLog -Force -EA SilentlyContinue
$c = Start-Case "${SESSION_PREFIX}_any" $runAlt 6
if (-not $c) {
    Write-Fail "could not start an attached client for the 1003 test"
} elseif (-not (Test-Path $altLog)) {
    Write-Skip "the model 1003 app never started in the pane - nothing to measure"
} else {
    $before = @(Get-Content $altLog | Where-Object { $_ -like 'RECV*' }).Count
    & $moveInj $c.Pid move 8 ($c.Left + 4) ($c.Top + 1) 2 0 60 | Out-Null
    Start-Sleep -Seconds 2
    $lines = @(Get-Content $altLog | Where-Object { $_ -like 'RECV*' })
    $motion = @($lines | Select-Object -Skip $before | Where-Object { $_ -match '\[<35;\d+;\d+M' })
    if ($motion.Count -gt 0) {
        Write-Pass "a DECSET 1003 app still received $($motion.Count) bare-motion report(s)"
    } else {
        Write-Fail "REGRESSION: an app that asked for DECSET 1003 received no bare motion (got: $(($lines | Select-Object -Skip $before) -join ' | '))"
    }
    Stop-Case $c
}

Write-Host "`n=== Issue #604 results ===" -ForegroundColor Cyan
Write-Host "  Passed:  $script:TestsPassed" -ForegroundColor Green
Write-Host "  Failed:  $script:TestsFailed" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $script:TestsSkipped" -ForegroundColor Yellow
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
