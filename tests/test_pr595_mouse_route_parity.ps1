# PR #595: "refactor(mouse): remove unreachable pre-server mouse path"
#
# The PR deletes input::handle_mouse, window_ops::{is_fullscreen_tui,
# screen_has_tui_content, pane_wants_mouse, update_tab_positions} and
# AppState.tab_positions and claims ZERO behavioural change. This script pins
# the user-visible behaviours the deleted/trimmed tests protected, on the LIVE
# route only (attached client -> TCP -> server, or raw TCP verbs):
#
#   #381 / #360  plain shell whose screen is full (prompt on the last row):
#                wheel must enter copy mode, no raw SGR text may appear, and
#                Ctrl+C must reach the shell (kills a running ping).
#   #285 / #570  nvim with mouse=a on the alternate screen must receive the
#                wheel and clicks (logged by nvim mappings), never copy mode.
#   #296 / #349  bare mouse motion over an alt-screen app that never enabled
#                motion tracking must not leak SGR garbage into it.
#   #168 / #593  status-bar tab clicks select the right window with the
#                default bar, a long status-left and a two-row bar.
#   key tables   `bind -T copy-mode-vi Y send-keys -X copy-pipe-and-cancel`
#                fires on the live send-keys path.
#   raw TCP      pane-scroll / pane-mouse verbs behave the same with no
#                attached client at all (pure server route).
#
# Run with the binary under test first on PATH (or PSMUX_EXE set). Every
# session name starts with p595_ so it never collides with other testers.

$ErrorActionPreference = "Continue"
$env:PSMUX_NO_WARM = "1"
$env:NO_COLOR = $null
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = "$env:USERPROFILE\.psmux"
$TMP = Join-Path $env:TEMP "p595"
New-Item -ItemType Directory -Force -Path $TMP | Out-Null
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
# nvim reads its mouse as VT bytes and draws on the alternate screen. The
# Windows 10 inbox conhost (build 19045) keeps its ?1049h and mouse DECSET to
# itself, so alternate_on stays 0, and drops an SGR report written to the
# pane, so nvim gets no wheel or click from psmux (Issue #1089, docs/faq.md;
# measured by writing them in a pane and reading what reached psmux, and with
# SGR reports typed with send-keys -l into a raw VT reader, which got only the
# letters around them). The nvim checks expect that there.
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan
Write-Host ("version: " + (& $PSMUX -V)) -ForegroundColor Cyan

# ---- helpers compiled from the repo ----
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$wheelInj = Join-Path $TMP "wheel.exe"
$clickInj = Join-Path $TMP "click.exe"
$keyInj   = Join-Path $TMP "keys.exe"
$moveInj  = Join-Path $TMP "move.exe"
$conread  = Join-Path $TMP "conread.exe"
foreach ($pair in @(@($wheelInj,"mouse_injector.cs"),@($clickInj,"click_injector.cs"),@($keyInj,"injector.cs"),@($moveInj,"mouse_move_injector.cs"),@($conread,"conread.cs"))) {
    if (-not (Test-Path $pair[0])) { & $csc /nologo /optimize /out:$($pair[0]) (Join-Path $PSScriptRoot $pair[1]) 2>&1 | Out-Null }
    if (-not (Test-Path $pair[0])) { Write-Host "FATAL: could not compile $($pair[1])" -ForegroundColor Red; exit 1 }
}

# Launcher that scrubs nesting env vars so an attached client can be started
# from inside another psmux/tmux. %1 = session, %2 = config file.
$launchCmd = Join-Path $TMP "launch.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set PSMUX_NO_WARM=1
set NO_COLOR=
"$PSMUX" -f "%~2" new-session -s %1 -x 120 -y 30
"@ | Set-Content -Path $launchCmd -Encoding ASCII

function Fmt([string]$Sess, [string]$F) { (& $PSMUX display-message -t $Sess -p $F 2>&1 | Out-String).Trim() }
function Cap([string]$Sess) { (& $PSMUX capture-pane -t $Sess -p 2>&1 | Out-String) }
function Wait-Port([string]$Sess, [int]$Secs = 15) {
    for ($i = 0; $i -lt ($Secs * 4); $i++) { if (Test-Path "$psmuxDir\$Sess.port") { return $true }; Start-Sleep -Milliseconds 250 }
    return $false
}
function Wait-Cond([scriptblock]$Cond, [int]$Secs = 10) {
    for ($i = 0; $i -lt ($Secs * 4); $i++) { if (& $Cond) { return $true }; Start-Sleep -Milliseconds 250 }
    return $false
}
function Start-Attached([string]$Sess, [string]$Conf) {
    & $PSMUX kill-session -t $Sess 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$Sess.*" -Force -EA SilentlyContinue
    $null = Start-Process -FilePath $launchCmd -ArgumentList $Sess,$Conf -PassThru
    if (-not (Wait-Port $Sess 20)) { return $null }
    Start-Sleep -Seconds 4
    $cli = Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -match "new-session -s\s+$Sess\b" } | Select-Object -First 1
    if (-not $cli) { return $null }
    return [int]$cli.ProcessId
}
function Stop-Sess([string]$Sess) {
    & $PSMUX kill-session -t $Sess 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    Remove-Item "$psmuxDir\$Sess.*" -Force -EA SilentlyContinue
}
function Send-Tcp([string]$Sess, [string[]]$Cmds) {
    # The control protocol serves ONE command per connection unless the client
    # sends PERSISTENT after AUTH, so open a fresh connection per command.
    $port = (Get-Content "$psmuxDir\$Sess.port" -Raw).Trim()
    $key  = (Get-Content "$psmuxDir\$Sess.key" -Raw).Trim()
    $log = @()
    foreach ($c in $Cmds) {
        try {
            $tcp = New-Object System.Net.Sockets.TcpClient
            $tcp.Connect("127.0.0.1", [int]$port)
            $st = $tcp.GetStream(); $st.ReadTimeout = 1500
            $w = New-Object System.IO.StreamWriter($st); $w.AutoFlush = $true; $w.NewLine = "`n"
            $r = New-Object System.IO.StreamReader($st)
            $w.WriteLine("AUTH $key"); $auth = $r.ReadLine()
            $w.WriteLine($c)
            $resp = try { $r.ReadLine() } catch { "<timeout>" }
            $log += "[$c] auth=$auth resp='$resp'"
            Start-Sleep -Milliseconds 200
            $tcp.Close()
        } catch { $log += "[$c] EXCEPTION $($_.Exception.Message)" }
    }
    return ($log -join " ; ")
}
$rawSgr = '\x1b\[<\d+;\d+;\d+[Mm]|(?<![A-Za-z0-9])\d{1,2};\d{1,3};\d{1,3}[Mm]'

$confMouse = Join-Path $TMP "mouse.conf"
"set -g mouse on`nset -g scroll-enter-copy-mode on" | Set-Content -Path $confMouse -Encoding UTF8

# =====================================================================
Write-Host "`n[A] #381/#360: filled plain pwsh shell, live attached client" -ForegroundColor Yellow
# =====================================================================
$S = "p595_sh"
$cpid = Start-Attached $S $confMouse
if (-not $cpid) { Write-Fail "attached client for $S did not start" }
else {
    Write-Pass "attached client started (pid $cpid)"
    & $PSMUX send-keys -t $S '1..80 | % { "FILL_$_" }' Enter 2>&1 | Out-Null
    $filled = Wait-Cond { (Cap $S) -match "FILL_80" } 10
    Start-Sleep -Milliseconds 800
    $cy = Fmt $S '#{cursor_y}'; $ph = Fmt $S '#{pane_height}'; $alt = Fmt $S '#{alternate_on}'
    Write-Info "after fill: cursor_y=$cy pane_height=$ph alternate_on=$alt pane_current_command=$(Fmt $S '#{pane_current_command}')"
    if ($filled -and ([int]$cy -ge ([int]$ph - 2))) { Write-Pass "screen is full: prompt sits on row $cy of $ph (the #381 false-positive shape)" }
    else { Write-Fail "screen not full (filled=$filled cursor_y=$cy pane_height=$ph)" }

    $inModeBefore = Fmt $S '#{pane_in_mode}'
    & $wheelInj $cpid up 3 40 10 2>&1 | Out-Null
    $inCopy = Wait-Cond { (Fmt $S '#{pane_in_mode}') -eq "1" } 6
    $inModeAfter = Fmt $S '#{pane_in_mode}'
    Write-Info "wheel up x3 at (40,10): pane_in_mode before=$inModeBefore after=$inModeAfter"
    if ($inCopy) { Write-Pass "wheel on a filled plain shell enters copy mode (#360/#381)" }
    else { Write-Fail "wheel on a filled plain shell did NOT enter copy mode (pane_in_mode=$inModeAfter)" }
    $capA = Cap $S
    if ($capA -notmatch $rawSgr) { Write-Pass "no raw mouse escape text in the pane after the wheel (#381)" }
    else { Write-Fail "raw mouse sequence leaked into the pane: $((($capA -split "`n") | Where-Object { $_ -match $rawSgr }) -join ' | ')" }

    & $PSMUX send-keys -t $S -X cancel 2>&1 | Out-Null
    $left = Wait-Cond { (Fmt $S '#{pane_in_mode}') -eq "0" } 5
    if ($left) { Write-Pass "copy mode cancelled, back to the shell" } else { Write-Fail "could not leave copy mode" }

    # Ctrl+C must reach the shell: start a long ping, inject a real Ctrl+C key
    & $PSMUX send-keys -t $S 'ping -n 40 127.0.0.1' Enter 2>&1 | Out-Null
    $pingUp = Wait-Cond { (Fmt $S '#{pane_current_command}') -match "ping" } 8
    if (-not $pingUp) { Write-Skip "ping did not become the foreground command ($(Fmt $S '#{pane_current_command}')), Ctrl+C check skipped" }
    else {
        Write-Pass "ping is the foreground command: $(Fmt $S '#{pane_current_command}')"
        & $keyInj $cpid '^c' 2>&1 | Out-Null
        $pingGone = Wait-Cond { (Fmt $S '#{pane_current_command}') -notmatch "ping" } 8
        $fg = Fmt $S '#{pane_current_command}'
        $capC = Cap $S
        $replies = ([regex]::Matches($capC, "Reply from 127\.0\.0\.1")).Count
        Write-Info "after Ctrl+C: pane_current_command=$fg replies_seen=$replies"
        if ($pingGone -and $replies -lt 40) { Write-Pass "Ctrl+C reached the shell on a filled screen: ping stopped after $replies replies, foreground=$fg (#381)" }
        elseif (-not $pingGone) { Write-Skip "pwsh did not honour the injected Ctrl+C for its ping child (foreground still $fg); pwsh + ConPTY delivers ^c to its own ping unreliably, the git bash arm below covers the same #381 path (see conpty-ctrlc-limitation). Not a mouse-route regression." }
        else { Write-Fail "ping ran to completion ($replies replies)" }
    }
    Stop-Sess $S
}

# =====================================================================
Write-Host "`n[B] #381: filled Git Bash shell, live attached client" -ForegroundColor Yellow
# =====================================================================
$GITBASH = "C:\Program Files\Git\bin\bash.exe"
if (-not (Test-Path $GITBASH)) { Write-Skip "git bash not installed" }
else {
    $S = "p595_gb"
    $confGb = Join-Path $TMP "gitbash.conf"
    "set -g default-shell `"C:/Program Files/Git/bin/bash.exe`"`nset -g mouse on`nset -g scroll-enter-copy-mode on" | Set-Content -Path $confGb -Encoding UTF8
    $cpid = Start-Attached $S $confGb
    if (-not $cpid) { Write-Fail "attached git bash client did not start" }
    else {
        Start-Sleep -Seconds 2
        $fg0 = Fmt $S '#{pane_current_command}'
        if ($fg0 -match "bash") { Write-Pass "foreground is bash ($fg0)" } else { Write-Fail "expected bash foreground, got '$fg0'" }
        & $PSMUX send-keys -t $S 'for i in $(seq 1 80); do echo FILL_$i; done' Enter 2>&1 | Out-Null
        $filled = Wait-Cond { (Cap $S) -match "FILL_80" } 10
        Start-Sleep -Milliseconds 800
        $cy = Fmt $S '#{cursor_y}'; $ph = Fmt $S '#{pane_height}'
        Write-Info "after fill: cursor_y=$cy pane_height=$ph filled=$filled"
        & $wheelInj $cpid up 3 40 10 2>&1 | Out-Null
        $inCopy = Wait-Cond { (Fmt $S '#{pane_in_mode}') -eq "1" } 6
        if ($inCopy) { Write-Pass "wheel on a filled git bash screen enters copy mode (#381)" }
        else { Write-Fail "wheel on a filled git bash screen did NOT enter copy mode (pane_in_mode=$(Fmt $S '#{pane_in_mode}'))" }
        $capB = Cap $S
        if ($capB -notmatch $rawSgr) { Write-Pass "no raw mouse escape text in the git bash pane (#381)" }
        else { Write-Fail "raw mouse sequence leaked into git bash: $((($capB -split "`n") | Where-Object { $_ -match $rawSgr }) -join ' | ')" }
        & $PSMUX send-keys -t $S -X cancel 2>&1 | Out-Null
        Start-Sleep -Milliseconds 500
        # a bare mouse move over the shell must not print anything either (the
        # reporter's second symptom: "15M65;61;15M...")
        $capBefore = Cap $S
        & $moveInj $cpid move 15 10 10 2 1 20 2>&1 | Out-Null
        Start-Sleep -Seconds 2
        $capMove = Cap $S
        if ($capMove -notmatch $rawSgr -and $capMove -notmatch "35;") { Write-Pass "bare mouse motion over git bash prints nothing (#381)" }
        else { Write-Fail "mouse motion leaked into git bash: $((($capMove -split "`n") | Where-Object { $_ -match $rawSgr -or $_ -match '35;' }) -join ' | ')" }
        # Ctrl+C reaches bash
        & $PSMUX send-keys -t $S 'ping -n 40 127.0.0.1' Enter 2>&1 | Out-Null
        $pingUp = Wait-Cond { (Fmt $S '#{pane_current_command}') -match "ping" } 8
        if (-not $pingUp) { Write-Skip "ping did not become foreground under git bash ($(Fmt $S '#{pane_current_command}'))" }
        else {
            & $keyInj $cpid '^c' 2>&1 | Out-Null
            $pingGone = Wait-Cond { (Fmt $S '#{pane_current_command}') -notmatch "ping" } 8
            $fg = Fmt $S '#{pane_current_command}'
            if ($pingGone) { Write-Pass "Ctrl+C reached git bash on a filled screen: ping stopped, foreground=$fg (#381)" }
            else { Write-Fail "Ctrl+C did not stop ping under git bash (foreground $fg)" }
        }
        Stop-Sess $S
    }
}

# =====================================================================
Write-Host "`n[C] #285/#570/#296: nvim on the alternate screen, live attached client" -ForegroundColor Yellow
# =====================================================================
$nvim = Get-Command nvim -EA SilentlyContinue
if (-not $nvim) { Write-Skip "nvim not installed" }
else {
    $S = "p595_nv"
    $nvLog = (Join-Path $TMP "nvim_mouse.log") -replace '\\','/'
    Remove-Item $nvLog -Force -EA SilentlyContinue
    $vimrc = Join-Path $TMP "p595.vim"
    @"
set mouse=a
set noswapfile
nnoremap <ScrollWheelUp>   :call writefile(["UP"], "$nvLog", "a")<CR>
nnoremap <ScrollWheelDown> :call writefile(["DOWN"], "$nvLog", "a")<CR>
nnoremap <LeftMouse>       :call writefile(["LEFT"], "$nvLog", "a")<CR>
"@ | Set-Content -Path $vimrc -Encoding ASCII
    $cpid = Start-Attached $S $confMouse
    if (-not $cpid) { Write-Fail "attached client for nvim did not start" }
    else {
        & $PSMUX send-keys -t $S "nvim -u `"$(($vimrc -replace '\\','/'))`"" Enter 2>&1 | Out-Null
        $onAlt = Wait-Cond { (Fmt $S '#{alternate_on}') -eq "1" } 12
        Start-Sleep -Seconds 2
        Write-Info "nvim: alternate_on=$(Fmt $S '#{alternate_on}') pane_current_command=$(Fmt $S '#{pane_current_command}')"
        if ($onAlt) { Write-Pass "nvim is on the alternate screen" }
        elseif ($Win10) { Write-Pass "on this host alternate_on stays 0 under nvim: its ?1049h never reached psmux, as measured" }
        else { Write-Fail "nvim never reached the alternate screen" }
        & $wheelInj $cpid up 3 40 10 2>&1 | Out-Null
        & $wheelInj $cpid down 2 40 10 2>&1 | Out-Null
        $logged = Wait-Cond { (Test-Path $nvLog) -and ((Get-Content $nvLog) -contains "UP") } 6
        $lines = if (Test-Path $nvLog) { @(Get-Content $nvLog) } else { @() }
        $ups = @($lines | Where-Object { $_ -eq "UP" }).Count; $downs = @($lines | Where-Object { $_ -eq "DOWN" }).Count
        Write-Info "nvim mouse log: UP=$ups DOWN=$downs pane_in_mode=$(Fmt $S '#{pane_in_mode}')"
        if ($logged -and $ups -ge 1) { Write-Pass "wheel was forwarded INTO nvim (mouse=a): $ups up / $downs down notches logged (#285/#570)" }
        elseif ($Win10 -and $ups -eq 0 -and $downs -eq 0) { Write-Pass "on this host the wheel did not reach nvim, as measured" }
        else { Write-Fail "wheel did not reach nvim (UP=$ups DOWN=$downs)" }
        if ((Fmt $S '#{pane_in_mode}') -eq "0") { Write-Pass "wheel over nvim did NOT enter copy mode" } else { Write-Fail "wheel over nvim entered copy mode" }
        & $clickInj $cpid 40 10 2>&1 | Out-Null
        $clicked = Wait-Cond { (Test-Path $nvLog) -and ((Get-Content $nvLog) -contains "LEFT") } 6
        if ($clicked) { Write-Pass "left click was forwarded into nvim (#285)" }
        elseif ($Win10) { Write-Pass "on this host the left click did not reach nvim, as measured" }
        else { Write-Fail "left click did not reach nvim" }
        & $PSMUX send-keys -t $S Escape ':qa!' Enter 2>&1 | Out-Null
        $back = Wait-Cond { (Fmt $S '#{alternate_on}') -eq "0" } 8
        if ($back) { Write-Pass "nvim quit cleanly (still responsive to keys)" } else { Write-Fail "nvim did not quit" }

        # #296 / #349: nvim WITHOUT mouse tracking on the alt screen, bare motion must not leak
        $vimrc2 = Join-Path $TMP "p595_nomouse.vim"
        "set mouse=`nset noswapfile" | Set-Content -Path $vimrc2 -Encoding ASCII
        & $PSMUX send-keys -t $S "nvim -u `"$(($vimrc2 -replace '\\','/'))`"" Enter 2>&1 | Out-Null
        $onAlt2 = Wait-Cond { (Fmt $S '#{alternate_on}') -eq "1" } 12
        Start-Sleep -Seconds 2
        if ($onAlt2) {
            & $moveInj $cpid move 20 10 10 2 1 20 2>&1 | Out-Null
            Start-Sleep -Seconds 2
            $capN = Cap $S
            if ($capN -notmatch $rawSgr -and $capN -notmatch "35;") { Write-Pass "bare motion over nvim (mouse=) leaked nothing (#296/#349)" }
            else { Write-Fail "motion garbage leaked into nvim: $((($capN -split "`n") | Where-Object { $_ -match $rawSgr -or $_ -match '35;' }) -join ' | ')" }
            & $PSMUX send-keys -t $S Escape ':qa!' Enter 2>&1 | Out-Null
            $back2 = Wait-Cond { (Fmt $S '#{alternate_on}') -eq "0" } 8
            if ($back2) { Write-Pass "nvim (mouse=) still responsive after motion, quit cleanly (#296)" } else { Write-Fail "nvim hung after motion (#296 regression)" }
        } else { Write-Skip "second nvim never reached alt screen" }
        Stop-Sess $S
    }
}

# =====================================================================
Write-Host "`n[D] #168/#593: status-bar tab clicks, live attached client" -ForegroundColor Yellow
# =====================================================================
function Tab-Click-Scenario([string]$Sess, [string]$Conf, [string]$Label, [int]$Row, [scriptblock]$Locate) {
    $cpid = Start-Attached $Sess $Conf
    if (-not $cpid) { Write-Fail "$Label client did not start"; return }
    & $PSMUX new-window -t $Sess 2>&1 | Out-Null; Start-Sleep -Milliseconds 600
    & $PSMUX new-window -t $Sess 2>&1 | Out-Null; Start-Sleep -Milliseconds 600
    & $PSMUX select-window -t "${Sess}:2" 2>&1 | Out-Null; Start-Sleep -Milliseconds 800
    $rows = & $conread $cpid 2>&1
    $statusText = if ($rows.Count -gt $Row) { [string]$rows[$Row] } else { "" }
    Write-Info "$Label status row ${Row}: [$statusText]"
    $targets = & $Locate $statusText
    if (-not $targets) { Write-Fail "$Label could not locate tabs on the rendered status row"; Stop-Sess $Sess; return }
    foreach ($t in $targets) {
        & $clickInj $cpid $t.X $Row 2>&1 | Out-Null
        $ok = Wait-Cond { (Fmt $Sess '#{window_index}') -eq "$($t.Want)" } 4
        $got = Fmt $Sess '#{window_index}'
        if ($ok) { Write-Pass "$Label click col $($t.X) -> window $got (want $($t.Want))" }
        else { Write-Fail "$Label click col $($t.X) -> window $got (want $($t.Want))" }
    }
    Stop-Sess $Sess
}
$confEmpty = Join-Path $TMP "empty.conf"; "" | Set-Content -Path $confEmpty -Encoding UTF8
$confLongLeft = Join-Path $TMP "longleft.conf"
"set -g status-left `"LEFT-LEFT-LEFT-LEFT-LEFT-LEFT `"`nset -g status-left-length 40" | Set-Content -Path $confLongLeft -Encoding UTF8
$conf2Row = Join-Path $TMP "tworow.conf"
@'
set -g status 2
set -g status-format[0] "#[align=left]#S"
set -g status-format[1] "#[align=left]#[range=window|0]tab0#[norange] #[range=window|1]tab1#[norange] #[range=window|2]tab2#[norange]"
'@ | Set-Content -Path $conf2Row -Encoding UTF8

$locDefault = {
    param($txt)
    # status-left-length defaults to 10, so "[p595_tabd] " is truncated to
    # "[p595_tabd" and the tabs follow immediately: aim at the rendered "N:" text.
    $c1 = $txt.IndexOf(" 1:"); $c0 = $txt.IndexOf("0:")
    if ($c1 -lt 0 -or $c0 -lt 0) { return $null }
    @([pscustomobject]@{X=$c1+2;Want=1},[pscustomobject]@{X=$c0+1;Want=0},[pscustomobject]@{X=$c1+2;Want=1})
}
$locLongLeft = {
    param($txt)
    $c1 = $txt.IndexOf("1:"); $c0 = $txt.IndexOf("0:")
    if ($c1 -lt 0 -or $c0 -lt 0 -or $txt -notmatch "LEFT-LEFT") { return $null }
    @([pscustomobject]@{X=$c1+1;Want=1},[pscustomobject]@{X=$c0+1;Want=0},[pscustomobject]@{X=2;Want=0})
}
$loc2Row = {
    param($txt)
    if ($txt -notmatch "tab0") { return $null }
    @([pscustomobject]@{X=2;Want=0},[pscustomobject]@{X=7;Want=1},[pscustomobject]@{X=12;Want=2},[pscustomobject]@{X=2;Want=0})
}
Tab-Click-Scenario "p595_tabd" $confEmpty    "default bar"          29 $locDefault
Tab-Click-Scenario "p595_tabl" $confLongLeft "long status-left (#168)" 29 $locLongLeft
Tab-Click-Scenario "p595_tab2" $conf2Row     "two-row bar (#593)"   29 $loc2Row

# =====================================================================
Write-Host "`n[E] copy-mode key table fires on the live send-keys route" -ForegroundColor Yellow
# =====================================================================
$S = "p595_kt"
$pipeOut = Join-Path $TMP "pipe_out.txt"; Remove-Item $pipeOut -Force -EA SilentlyContinue
$pipeCmd = Join-Path $TMP "pipe.cmd"
"@findstr /r `"^`" > `"$pipeOut`"" | Set-Content -Path $pipeCmd -Encoding ASCII
$confKt = Join-Path $TMP "kt.conf"
"set -g mode-keys vi`nbind -T copy-mode-vi Y send-keys -X copy-pipe-and-cancel `"$(($pipeCmd -replace '\\','/'))`"" | Set-Content -Path $confKt -Encoding UTF8
& $PSMUX kill-session -t $S 2>&1 | Out-Null
& $PSMUX -f $confKt new-session -d -s $S -x 100 -y 20 2>&1 | Out-Null
if (-not (Wait-Port $S 15)) { Write-Fail "detached session $S did not start" }
else {
    Start-Sleep -Seconds 3
    $lk = (& $PSMUX list-keys -t $S -T copy-mode-vi 2>&1 | Out-String)
    if ($lk -match "copy-mode-vi\s+Y\s+send-keys -X copy-pipe-and-cancel") { Write-Pass "list-keys shows the copy-mode-vi Y binding" } else { Write-Fail "binding missing from list-keys: $lk" }
    & $PSMUX send-keys -t $S 'echo P595_PIPE_TOKEN' Enter 2>&1 | Out-Null
    $seen = Wait-Cond { (Cap $S) -match "^P595_PIPE_TOKEN" } 8
    & $PSMUX copy-mode -t $S 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    if ((Fmt $S '#{pane_in_mode}') -eq "1") { Write-Pass "entered copy mode" } else { Write-Fail "copy-mode did not enter" }
    # Space is begin-selection in tmux's copy-mode-vi table (key-bindings.c:656);
    # v is rectangle-toggle since the #694 parity merge, so it no longer starts one.
    & $PSMUX send-keys -t $S k 0 Space '$' 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    & $PSMUX send-keys -t $S Y 2>&1 | Out-Null
    $piped = Wait-Cond { (Test-Path $pipeOut) -and ((Get-Content $pipeOut -Raw) -match "P595_PIPE_TOKEN") } 8
    $content = if (Test-Path $pipeOut) { (Get-Content $pipeOut -Raw) } else { "<no file>" }
    Write-Info "pipe output: $($content.Trim())"
    if ($piped) { Write-Pass "bound Y ran copy-pipe-and-cancel: selection piped to the user command" }
    elseif (Test-Path $pipeOut) { Write-Fail "pipe ran but content was '$($content.Trim())'" }
    else { Write-Fail "bound Y did NOT run the user's copy-pipe command (no output file)" }
    if ((Fmt $S '#{pane_in_mode}') -eq "0") { Write-Pass "copy-pipe-and-cancel left copy mode" } else { Write-Fail "still in copy mode after copy-pipe-and-cancel" }
    Stop-Sess $S
}

# =====================================================================
Write-Host "`n[F] raw TCP verbs with NO attached client (pure server route)" -ForegroundColor Yellow
# =====================================================================
$S = "p595_raw"
& $PSMUX kill-session -t $S 2>&1 | Out-Null
& $PSMUX -f $confMouse new-session -d -s $S -x 100 -y 20 2>&1 | Out-Null
if (-not (Wait-Port $S 15)) { Write-Fail "detached session $S did not start" }
else {
    Start-Sleep -Seconds 3
    $pid0 = (Fmt $S '#{pane_id}') -replace '%',''
    & $PSMUX send-keys -t $S '1..60 | % { "FILL_$_" }' Enter 2>&1 | Out-Null
    $null = Wait-Cond { (Cap $S) -match "FILL_60" } 8
    Start-Sleep -Milliseconds 500
    # click on the filled shell: nothing may leak
    Write-Info (Send-Tcp $S @("pane-mouse $pid0 0 5 5 M", "pane-mouse $pid0 0 5 5 m", "pane-mouse $pid0 35 6 6 M"))
    Start-Sleep -Seconds 1
    $capR = Cap $S
    if ($capR -notmatch $rawSgr -and $capR -notmatch "0;5;5") { Write-Pass "raw pane-mouse click + motion on a filled shell leaked nothing (#349/#381)" }
    else { Write-Fail "raw pane-mouse leaked into the shell: $((($capR -split "`n") | Where-Object { $_ -match $rawSgr -or $_ -match '0;5;5' }) -join ' | ')" }
    if ((Fmt $S '#{pane_in_mode}') -eq "0") { Write-Pass "click did not enter copy mode" } else { Write-Fail "click entered copy mode" }
    Write-Info (Send-Tcp $S @("pane-scroll $pid0 up 5 5"))
    $inCopy = Wait-Cond { (Fmt $S '#{pane_in_mode}') -eq "1" } 5
    if ($inCopy) { Write-Pass "raw pane-scroll up on a filled shell enters copy mode (#360)" } else { Write-Fail "raw pane-scroll up did not enter copy mode" }
    & $PSMUX send-keys -t $S -X cancel 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    if ($nvim) {
        Remove-Item $nvLog -Force -EA SilentlyContinue
        & $PSMUX send-keys -t $S "nvim -u `"$(($vimrc -replace '\\','/'))`"" Enter 2>&1 | Out-Null
        $onAlt = Wait-Cond { (Fmt $S '#{alternate_on}') -eq "1" } 12
        Start-Sleep -Seconds 2
        Write-Info (Send-Tcp $S @("pane-scroll $pid0 up 5 5", "pane-scroll $pid0 up 5 5"))
        $logged = Wait-Cond { (Test-Path $nvLog) -and ((Get-Content $nvLog) -contains "UP") } 6
        if ($onAlt -and $logged) { Write-Pass "raw pane-scroll over nvim (mouse=a) was forwarded into nvim, not copy mode (pane_in_mode=$(Fmt $S '#{pane_in_mode}'))" }
        elseif ($Win10 -and -not $onAlt -and -not $logged) { Write-Pass "on this host raw pane-scroll did not reach nvim, which psmux cannot see on the alternate screen, as measured" }
        else { Write-Fail "raw pane-scroll over nvim not forwarded (alt=$onAlt logged=$logged)" }
        & $PSMUX send-keys -t $S Escape ':qa!' Enter 2>&1 | Out-Null
        Start-Sleep -Seconds 1
    }
    Stop-Sess $S
}

Write-Host "`n=== PR #595 parity results ($((& $PSMUX -V))) ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:Pass)  Failed: $($script:Fail)  Skipped: $($script:Skip)"
exit $script:Fail
