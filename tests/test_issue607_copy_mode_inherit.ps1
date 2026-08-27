# Issue #607: "Copy Mode on Newly Created Panes/Windows" (CalebMostyn, 3.3.7)
#
# Reported with a blank config (`psmux -f NUL new-session`):
#
#   * with the current pane in copy mode, splitting a pane or opening a new
#     window puts the BRAND NEW pane into copy mode too
#   * closing a pane that is in copy mode puts the pane you land on into copy
#     mode
#
# tmux treats a mode as a property of one window pane. `window_pane_create`
# starts every pane with an empty mode stack (window.c:1317 `TAILQ_INIT
# (&wp->modes)`), and `window_lost_pane` (window.c:1060) only moves `w->active`
# when the dying pane was active; it never touches the survivor's modes.
#
# Measured against tmux 3.4 (`tmux -L parity607`, `#{pane_in_mode}` per pane):
#
#   split -h  while %0 in copy mode -> %0=1 %1=0
#   split -v  while %0 in copy mode -> %0=1 %1=0
#   new-window while %0 in copy mode -> win0 %0=1, win1 %1=0
#   kill %1 (in copy mode), %0 not  -> %0=0
#   kill %1 (in copy mode), %0 also -> %0=1     (the survivor keeps ITS OWN mode)
#   select-pane back and forth      -> modes never move between panes
#
# This suite pins each of those, over the CLI/TCP command route (what scripts
# and the client's own verbs use) and over the real keyboard route with an
# attached client driven by WriteConsoleInput.
#
# Set PSMUX_TEST_BIN to test a non-installed binary.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

# Inherited routing would aim every call at somebody else's server.
$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION      = $null
$env:PSMUX_PANE         = $null
$env:TMUX               = $null
$env:TMUX_PANE          = $null

$rig = Join-Path $env:TEMP ("psmux607-" + [guid]::NewGuid().ToString('N').Substring(0,8))
New-Item -ItemType Directory -Force -Path $rig | Out-Null
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }

function Kill-Sess([string]$S) {
    & $PSMUX kill-session -t $S 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    Remove-Item "$psmuxDir\$S.*" -Force -EA SilentlyContinue
}
function Pane-Ids([string]$Target) {
    @((& $PSMUX list-panes -t $Target -F '#{pane_id}' 2>&1) | ForEach-Object { "$_".Trim() } | Where-Object { $_ -match '^%\d+$' })
}
function Pane-Mode([string]$PaneId) {
    (& $PSMUX display-message -p -t $PaneId '#{pane_in_mode}' 2>&1 | Out-String).Trim()
}
# "%0=1 %1=0" over every pane of a target, in list-panes order.
function Modes([string]$Target) {
    (@(Pane-Ids $Target | ForEach-Object { "$_=$(Pane-Mode $_)" }) -join ' ')
}
function Mode-Of([string]$Target, [int]$Index) {
    $ids = Pane-Ids $Target
    if ($ids.Count -le $Index) { return "?" }
    Pane-Mode $ids[$Index]
}

# ---------------------------------------------------------------------------
# Part A: CLI/TCP command route, detached server, no client.
# ---------------------------------------------------------------------------
Write-Host "`n=== Part A: CLI route (detached server) ===" -ForegroundColor Cyan

foreach ($dir in @('-h', '-v')) {
    $S = "i607_split" + $dir.Substring(1)
    Kill-Sess $S
    & $PSMUX new-session -d -s $S -x 120 -y 30 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $before = Modes $S
    & $PSMUX copy-mode -t $S 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
    $inCopy = Modes $S
    & $PSMUX split-window $dir -t $S 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $after = Modes $S
    Write-Info "split $dir : before=[$before] inCopy=[$inCopy] after=[$after]"
    $ids = Pane-Ids $S
    if ($inCopy -notmatch '=1') { Write-Fail "split $dir : copy-mode never took effect" }
    elseif ($ids.Count -ne 2) { Write-Fail "split $dir : expected 2 panes, got $($ids.Count)" }
    else {
        $mOld = Pane-Mode $ids[0]
        $mNew = Pane-Mode $ids[1]
        if ($mNew -eq '1') { Write-Fail "split $dir : BUG, the NEW pane $($ids[1]) opened in copy mode" }
        else { Write-Pass "split $dir : the new pane $($ids[1]) is not in copy mode" }
        if ($mOld -eq '1') { Write-Pass "split $dir : the original pane $($ids[0]) kept its copy mode" }
        else { Write-Fail "split $dir : the original pane $($ids[0]) lost copy mode (mode=$mOld)" }
    }
    Kill-Sess $S
}

# new-window while in copy mode
$S = "i607_newwin"
Kill-Sess $S
& $PSMUX new-session -d -s $S -x 120 -y 30 2>&1 | Out-Null
Start-Sleep -Seconds 2
& $PSMUX copy-mode -t $S 2>&1 | Out-Null
Start-Sleep -Milliseconds 700
$w0In = Modes "${S}:0"
& $PSMUX new-window -t $S 2>&1 | Out-Null
Start-Sleep -Seconds 2
$w0After = Modes "${S}:0"
$w1 = Modes "${S}:1"
Write-Info "new-window : win0 inCopy=[$w0In] win0 after=[$w0After] win1=[$w1]"
if ($w0In -notmatch '=1') { Write-Fail "new-window : copy-mode never took effect" }
else {
    if ($w1 -match '=1') { Write-Fail "new-window : BUG, the new window's pane opened in copy mode" }
    else { Write-Pass "new-window : the new window's pane is not in copy mode" }
    if ($w0After -match '=1') { Write-Pass "new-window : the original pane kept its copy mode" }
    else { Write-Fail "new-window : the original pane lost copy mode ([$w0After])" }
}
Kill-Sess $S

# kill-pane of a copy-mode pane, neighbour NOT in copy mode
$S = "i607_killno"
Kill-Sess $S
& $PSMUX new-session -d -s $S -x 120 -y 30 2>&1 | Out-Null
Start-Sleep -Seconds 2
& $PSMUX split-window -h -t $S 2>&1 | Out-Null
Start-Sleep -Seconds 2
$ids = Pane-Ids $S
if ($ids.Count -ne 2) { Write-Fail "kill-pane (clean neighbour): setup produced $($ids.Count) panes" }
else {
    $P0 = $ids[0]; $P1 = $ids[1]
    & $PSMUX copy-mode -t $P1 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
    $before = Modes $S
    & $PSMUX kill-pane -t $P1 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $after = Modes $S
    Write-Info "kill-pane (clean neighbour): before=[$before] after=[$after]"
    if ($before -notmatch "$P1=1") { Write-Fail "kill-pane (clean neighbour): $P1 never entered copy mode" }
    elseif ($after -match '=1') { Write-Fail "kill-pane (clean neighbour): BUG, the survivor $P0 was pushed into copy mode" }
    else { Write-Pass "kill-pane (clean neighbour): the survivor $P0 stays out of copy mode" }
}
Kill-Sess $S

# kill-pane of a copy-mode pane, neighbour ALSO in copy mode: it must stay
$S = "i607_killboth"
Kill-Sess $S
& $PSMUX new-session -d -s $S -x 120 -y 30 2>&1 | Out-Null
Start-Sleep -Seconds 2
& $PSMUX split-window -h -t $S 2>&1 | Out-Null
Start-Sleep -Seconds 2
$ids = Pane-Ids $S
if ($ids.Count -ne 2) { Write-Fail "kill-pane (both in copy mode): setup produced $($ids.Count) panes" }
else {
    $Q0 = $ids[0]; $Q1 = $ids[1]
    & $PSMUX copy-mode -t $Q0 2>&1 | Out-Null
    & $PSMUX copy-mode -t $Q1 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900
    $before = Modes $S
    & $PSMUX kill-pane -t $Q1 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $after = Modes $S
    Write-Info "kill-pane (both in copy mode): before=[$before] after=[$after]"
    if ($before -notmatch "$Q0=1" -or $before -notmatch "$Q1=1") {
        Write-Fail "kill-pane (both in copy mode): setup did not put both panes in copy mode ([$before])"
    } elseif ($after -match "$Q0=1") {
        Write-Pass "kill-pane (both in copy mode): the survivor $Q0 keeps its own copy mode"
    } else {
        Write-Fail "kill-pane (both in copy mode): the survivor $Q0 lost its own copy mode ([$after])"
    }
}
Kill-Sess $S

# select-pane must never move a mode between panes
$S = "i607_focus"
Kill-Sess $S
& $PSMUX new-session -d -s $S -x 120 -y 30 2>&1 | Out-Null
Start-Sleep -Seconds 2
& $PSMUX split-window -h -t $S 2>&1 | Out-Null
Start-Sleep -Seconds 2
$ids = Pane-Ids $S
if ($ids.Count -ne 2) { Write-Fail "select-pane: setup produced $($ids.Count) panes" }
else {
    $R0 = $ids[0]; $R1 = $ids[1]
    & $PSMUX copy-mode -t $R1 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
    $m0 = Modes $S
    & $PSMUX select-pane -t $R0 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900
    $m1 = Modes $S
    & $PSMUX select-pane -t $R1 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900
    $m2 = Modes $S
    Write-Info "select-pane: start=[$m0] after->$R0=[$m1] after->$R1=[$m2]"
    if ($m1 -eq $m0 -and $m2 -eq $m0) { Write-Pass "select-pane leaves every pane's mode alone" }
    else { Write-Fail "select-pane moved copy mode between panes" }
}
Kill-Sess $S

# ---------------------------------------------------------------------------
# Part B: real attached client, real keystrokes (WriteConsoleInput).
# prefix-[ enters copy mode, prefix-% splits, prefix-c opens a window.
# ---------------------------------------------------------------------------
Write-Host "`n=== Part B: attached client, injected keystrokes ===" -ForegroundColor Cyan

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$keyInj = Join-Path $rig "keys.exe"
$injSrc = Join-Path $PSScriptRoot "injector.cs"
if (Test-Path $csc) { & $csc /nologo /optimize /out:$keyInj $injSrc 2>&1 | Out-Null }

if (-not (Test-Path $keyInj)) {
    Write-Info "csc.exe unavailable, skipping the keyboard route"
} else {
    $conf = Join-Path $rig "i607.conf"
    "set -g history-limit 2000" | Set-Content -Path $conf -Encoding ASCII
    $launchCmd = Join-Path $rig "launch.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set PSMUX_NO_WARM=1
set NO_COLOR=
"$PSMUX" -f "%~2" new-session -s %1 -x 120 -y 30
"@ | Set-Content -Path $launchCmd -Encoding ASCII

    $ST = "i607_tui"
    Kill-Sess $ST
    $null = Start-Process -FilePath $launchCmd -ArgumentList $ST,$conf -PassThru
    for ($i = 0; $i -lt 80; $i++) { if (Test-Path "$psmuxDir\$ST.port") { break }; Start-Sleep -Milliseconds 250 }
    Start-Sleep -Seconds 4
    $cli = Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -match "new-session -s\s+$ST\b" } | Select-Object -First 1
    if (-not $cli) {
        Write-Info "attached client did not start, skipping the keyboard route"
    } else {
        $cpid = [int]$cli.ProcessId

        # prefix-[ then prefix-% : the split must not inherit copy mode
        & $keyInj $cpid "^b{SLEEP:400}[" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1200
        $tuiIn = Modes $ST
        & $keyInj $cpid "^b{SLEEP:400}%" 2>&1 | Out-Null
        Start-Sleep -Seconds 3
        $tuiSplit = Modes $ST
        Write-Info "keys: after prefix-[ =[$tuiIn]  after prefix-% =[$tuiSplit]"
        $tids = Pane-Ids $ST
        if ($tuiIn -notmatch '=1') {
            Write-Info "prefix-[ did not enter copy mode (injection refused), skipping keyboard assertions"
        } elseif ($tids.Count -lt 2) {
            Write-Info "prefix-% did not split (injection refused), skipping keyboard assertions"
        } else {
            if ((Pane-Mode $tids[1]) -eq '1') { Write-Fail "keys: BUG, prefix-% opened the new pane in copy mode" }
            else { Write-Pass "keys: prefix-% opens the new pane out of copy mode" }

            # prefix-c : the new window's pane must not inherit copy mode either
            & $PSMUX copy-mode -t $tids[0] 2>&1 | Out-Null
            & $PSMUX select-pane -t $tids[0] 2>&1 | Out-Null
            Start-Sleep -Milliseconds 900
            & $keyInj $cpid "^b{SLEEP:400}c" 2>&1 | Out-Null
            Start-Sleep -Seconds 3
            $w1t = Modes "${ST}:1"
            Write-Info "keys: new window from prefix-c =[$w1t]"
            if ($w1t -match '=1') { Write-Fail "keys: BUG, prefix-c opened the new window's pane in copy mode" }
            elseif ($w1t -match '=0') { Write-Pass "keys: prefix-c opens the new window's pane out of copy mode" }
            else { Write-Info "keys: prefix-c did not create a window (injection refused)" }
        }
        try { Stop-Process -Id $cpid -Force -EA SilentlyContinue } catch {}
    }
    Kill-Sess $ST
}

Remove-Item -Recurse -Force $rig -EA SilentlyContinue
Write-Host "`nPassed: $script:Pass  Failed: $script:Fail" -ForegroundColor Cyan
if ($script:Fail -gt 0) { exit 1 } else { exit 0 }
