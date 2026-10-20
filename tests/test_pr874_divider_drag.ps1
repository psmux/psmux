# PR #874 (MattKotsenas): dragging a pane divider with the mouse must move the
# divider by exactly as many cells as the pointer moved, as tmux does
# (cmd-resize-pane.c cmd_resize_pane_mouse_resize_tiled: layout_resize_layout
# by x - lx / y - ly cells per motion event).
#
# Before the fix the server's drag (tree.rs adjust_split_sizes) scaled the
# pointer delta by the weight of the TWO panes next to the divider over the
# pixel span of the WHOLE split, so in a three pane split the divider moved
# about two thirds as far as the pointer. With small weights (a narrow window,
# where weights are cell counts) the clamp bounds crossed and the drag panicked.
#
# Layers:
#   A. control-port mouse verbs (mouse-down / mouse-drag / mouse-up), the
#      server's drag path, on detached sessions.
#   B. real MOUSE_EVENT records written into an attached client's console
#      (client.rs client_drag path), when an attached client can start here.
#
# Safety: -L namespace, honours PSMUX_DATA_DIR, never kills by name.
# Run:  pwsh -NoProfile -File tests\test_pr874_divider_drag.ps1
#       PSMUX_TEST_EXE selects the binary under test.

$ErrorActionPreference = "Continue"
$env:PSMUX_NO_WARM = "1"
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } elseif ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$NS = "pr874_$PID"

$script:Pass = 0; $script:Fail = 0; $script:Skip = 0; $script:InjectWorks = $false
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

function Port-File([string]$S) {
    Get-ChildItem $psmuxDir -Filter "*$S.port" -EA SilentlyContinue |
        Where-Object { $_.Name -like "$NS*" } | Select-Object -First 1
}
function Stop-Ns {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
}
function Send-Tcp([string]$S, [string[]]$Cmds) {
    $pf = Port-File $S
    if (-not $pf) { return "NOPORT" }
    $port = (Get-Content $pf.FullName -Raw).Trim()
    $key  = (Get-Content ($pf.FullName -replace '\.port$','.key') -Raw).Trim()
    foreach ($c in $Cmds) {
        try {
            $tcp = New-Object System.Net.Sockets.TcpClient
            $tcp.Connect("127.0.0.1", [int]$port)
            $st = $tcp.GetStream(); $st.ReadTimeout = 1500
            $w = New-Object System.IO.StreamWriter($st); $w.AutoFlush = $true; $w.NewLine = "`n"
            $r = New-Object System.IO.StreamReader($st)
            $w.WriteLine("AUTH $key"); $null = $r.ReadLine()
            $w.WriteLine($c)
            try { $null = $r.ReadLine() } catch {}
            Start-Sleep -Milliseconds 120
            $tcp.Close()
        } catch { return "EXCEPTION $($_.Exception.Message)" }
    }
    return "sent"
}
function Panes([string]$S) {
    @(& $PSMUX -L $NS list-panes -t $S -F '#{pane_index} #{pane_left} #{pane_top} #{pane_width} #{pane_height}' 2>$null) |
        ForEach-Object { $f = $_ -split ' '; [pscustomobject]@{ I=[int]$f[0]; L=[int]$f[1]; T=[int]$f[2]; W=[int]$f[3]; H=[int]$f[4] } }
}
function Geo([string]$S) { ((Panes $S) | ForEach-Object { "$($_.L),$($_.T) $($_.W)x$($_.H)" }) -join ' | ' }
function New-Split([string]$S, [int]$X, [int]$Y, [string]$Flag, [int]$N, [string]$Layout) {
    & $PSMUX -L $NS new-session -d -s $S -x $X -y $Y 2>&1 | Out-Null
    for ($i = 0; $i -lt 60; $i++) { if (Port-File $S) { break }; Start-Sleep -Milliseconds 250 }
    if (-not (Port-File $S)) { return $false }
    & $PSMUX -L $NS set-option -t $S -g mouse on 2>&1 | Out-Null
    & $PSMUX -L $NS set-option -t $S -g status off 2>&1 | Out-Null
    for ($k = 1; $k -lt $N; $k++) { & $PSMUX -L $NS split-window $Flag -t $S 2>&1 | Out-Null }
    if ($Layout) { & $PSMUX -L $NS select-layout -t $S $Layout 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 600
    return $true
}
function Alive([string]$S) { & $PSMUX -L $NS has-session -t $S 2>$null; return ($LASTEXITCODE -eq 0) }

# Drag divider number $Div (0 based, the border after pane $Div) by $Delta
# cells through the server verbs, in $Steps motion events.
function Drag-Server([string]$S, [bool]$Horiz, [int]$Div, [int]$Delta, [int]$Steps = 4) {
    $p = Panes $S
    if ($Horiz) { $pos = $p[$Div].L + $p[$Div].W; $o = [int]($p[$Div].H / 2) }
    else        { $pos = $p[$Div].T + $p[$Div].H; $o = [int]($p[$Div].W / 2) }
    $cmds = @()
    $cmds += $(if ($Horiz) { "mouse-down $pos $o" } else { "mouse-down $o $pos" })
    for ($k = 1; $k -le $Steps; $k++) {
        $at = $pos + [int][math]::Round($Delta * $k / $Steps)
        $cmds += $(if ($Horiz) { "mouse-drag $at $o" } else { "mouse-drag $o $at" })
    }
    $end = $pos + $Delta
    $cmds += $(if ($Horiz) { "mouse-up $end $o" } else { "mouse-up $o $end" })
    $null = Send-Tcp $S $cmds
    Start-Sleep -Milliseconds 500
    return $pos
}
function Border-At([string]$S, [bool]$Horiz, [int]$Div) {
    $p = Panes $S
    if ($p.Count -le $Div) { return -1 }
    if ($Horiz) { return $p[$Div].L + $p[$Div].W } else { return $p[$Div].T + $p[$Div].H }
}

# =====================================================================
Write-Host "`n[A] server drag path (control-port mouse verbs)" -ForegroundColor Yellow
# =====================================================================
Stop-Ns
$cases = @(
    @{ Name="3 columns, first divider +20";  X=120; Y=30; Flag="-h"; N=3; Layout="even-horizontal"; H=$true;  Div=0; D=20 },
    @{ Name="3 columns, second divider -15"; X=120; Y=30; Flag="-h"; N=3; Layout="even-horizontal"; H=$true;  Div=1; D=-15 },
    @{ Name="4 columns, first divider +12";  X=160; Y=30; Flag="-h"; N=4; Layout="even-horizontal"; H=$true;  Div=0; D=12 },
    @{ Name="3 rows, first divider +6";      X=100; Y=40; Flag="-v"; N=3; Layout="even-vertical";   H=$false; Div=0; D=6 },
    @{ Name="2 columns, divider +10";        X=120; Y=30; Flag="-h"; N=2; Layout="even-horizontal"; H=$true;  Div=0; D=10 }
)
$n = 0
foreach ($c in $cases) {
    $n++
    $S = "d$n"
    if (-not (New-Split $S $c.X $c.Y $c.Flag $c.N $c.Layout)) { Write-Fail "$($c.Name): session did not start"; continue }
    $before = Geo $S
    $start = Drag-Server $S $c.H $c.Div $c.D
    $after = Border-At $S $c.H $c.Div
    $want = $start + $c.D
    Write-Info "$($c.Name): $before  ->  $(Geo $S)"
    if ($after -eq $want) {
        Write-Pass "$($c.Name): divider went from $start to $after (pointer ended at $want)"
    } else {
        Write-Fail "$($c.Name): divider went from $start to $after but the pointer ended at $want"
    }
    # Back to the press position restores the layout.
    $null = Drag-Server $S $c.H $c.Div (-($after - $start))
    $back = Border-At $S $c.H $c.Div
    if ($back -eq $start) { Write-Pass "$($c.Name): dragging back returns the divider to $start ($back)" }
    else { Write-Fail "$($c.Name): dragging back left the divider at $back, not $start" }
}

# Small weights: a narrow window, cell count weights. On master the clamp
# bounds cross (5 > pair - 5) and the server panics on the first motion.
foreach ($c in @(
    @{ Name="narrow 24 col window, 3 columns"; X=24; Y=10; Flag="-h"; N=3; Layout="even-horizontal"; H=$true; Div=0; D=2 },
    @{ Name="short 12 row window, 3 rows";     X=40; Y=12; Flag="-v"; N=3; Layout="even-vertical";   H=$false; Div=0; D=1 }
)) {
    $n++
    $S = "d$n"
    if (-not (New-Split $S $c.X $c.Y $c.Flag $c.N $c.Layout)) { Write-Fail "$($c.Name): session did not start"; continue }
    $before = Geo $S
    $start = Drag-Server $S $c.H $c.Div $c.D 2
    Start-Sleep -Milliseconds 500
    if (-not (Alive $S)) { Write-Fail "$($c.Name): the server died during the drag (layout was $before)"; continue }
    $after = Border-At $S $c.H $c.Div
    Write-Info "$($c.Name): $before  ->  $(Geo $S)"
    Write-Pass "$($c.Name): server survived the drag (divider $start -> $after)"
}
Stop-Ns

# =====================================================================
Write-Host "`n[B] attached client, real MOUSE_EVENT records" -ForegroundColor Yellow
# =====================================================================
$TMP = Join-Path ([IO.Path]::GetTempPath()) "pr874_suite"
New-Item -ItemType Directory -Force -Path $TMP | Out-Null
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$dragInj = Join-Path $TMP "drag.exe"
if (-not (Test-Path $dragInj) -and (Test-Path $csc)) {
    & $csc /nologo /optimize /out:$dragInj (Join-Path $PSScriptRoot "mouse_drag_hold_injector.cs") 2>&1 | Out-Null
}

# One attached client in its own console of $Cols x $Lines, three panes split
# with $Flag, then one real drag of the first divider by $Delta cells.
# The status line stays on: with `status off` in the startup config the server
# window is one row shorter than the client draws (a separate bug), which
# shifts vertical drags by a cell whatever the drag maths.
function Test-Attached([string]$Name, [int]$Cols, [int]$Lines, [string]$Flag, [string]$Layout, [int]$Delta) {
    $S = "att$Cols$Lines"
    $horiz = ($Flag -eq "-h")
    $conf = Join-Path $TMP "pr874.conf"
    "set -g mouse on" | Set-Content -Path $conf -Encoding ASCII
    $launch = Join-Path $TMP "launch_$S.cmd"
    $dd = if ($env:PSMUX_DATA_DIR) { "set PSMUX_DATA_DIR=$env:PSMUX_DATA_DIR" } else { "" }
@"
@echo off
set PSMUX_SESSION=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set PSMUX_NO_WARM=1
$dd
mode con cols=$Cols lines=$Lines
"$PSMUX" -L $NS -f "$conf" new-session -s $S
"@ | Set-Content -Path $launch -Encoding ASCII
    $win = Start-Process -FilePath "cmd.exe" -ArgumentList "/c `"$launch`"" -PassThru
    $cli = $null
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 500
        $cli = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($win.Id)" |
               Where-Object { $_.CommandLine -match "new-session -s $S" } | Select-Object -First 1
        if ($cli) { break }
    }
    try {
        if (-not $cli -or -not (Port-File $S)) { Write-Skip "$Name : no attached client started here"; return }
        Start-Sleep -Seconds 2
        & $PSMUX -L $NS split-window $Flag -t $S 2>&1 | Out-Null
        & $PSMUX -L $NS split-window $Flag -t $S 2>&1 | Out-Null
        & $PSMUX -L $NS select-layout -t $S $Layout 2>&1 | Out-Null
        Start-Sleep -Seconds 1
        $p = Panes $S
        if ($horiz) { $pos = $p[0].L + $p[0].W; $o = [int]($p[0].H / 2) } else { $pos = $p[0].T + $p[0].H; $o = [int]($p[0].W / 2) }
        $cpid = [int]$cli.ProcessId
        Write-Info "$Name : client pid=$cpid, layout $(Geo $S), divider at $pos"
        if ($horiz) { & $dragInj $cpid $pos $o ($pos + $Delta) $o 4 40 200 | Out-Null }
        else        { & $dragInj $cpid $o $pos $o ($pos + $Delta) 4 40 200 | Out-Null }
        Start-Sleep -Milliseconds 1500
        $clientAlive = [bool](Get-Process -Id $cpid -EA SilentlyContinue)
        if (-not (Alive $S)) { Write-Fail "$Name : the server died during the real drag"; return }
        if (-not $clientAlive) { Write-Fail "$Name : the attached client died during the real drag"; return }
        $after = Border-At $S $horiz 0
        Write-Info "$Name : after real drag $(Geo $S)"
        if ($after -eq $pos -and -not $script:InjectWorks) {
            Write-Skip "$Name : the injected drag changed nothing (console focus refused)"
        } elseif ($after -eq $pos) {
            Write-Fail "$Name : real drag of $Delta cells left the divider at $pos (an earlier injected drag here did move one)"
        } elseif ($after -eq ($pos + $Delta)) {
            Write-Pass "$Name : real drag moved the divider from $pos to $after, under the pointer"
            $script:InjectWorks = $true
        } else {
            Write-Fail "$Name : real drag moved the divider from $pos to $after but the pointer ended at $($pos + $Delta)"
            $script:InjectWorks = $true
        }
    } finally {
        & $PSMUX -L $NS kill-session -t $S 2>&1 | Out-Null
        Start-Sleep -Milliseconds 500
        try { Stop-Process -Id $win.Id -Force -EA SilentlyContinue } catch {}
        if ($cli) { try { Stop-Process -Id ([int]$cli.ProcessId) -Force -EA SilentlyContinue } catch {} }
    }
}

if (-not (Test-Path $dragInj)) {
    Write-Skip "could not build the drag injector"
} else {
    Test-Attached "120x30, 3 columns, +20" 120 30 "-h" "even-horizontal" 20
    # A short console: cell count weights are small, so the old client clamp
    # had its lower bound above its upper bound and panicked.
    Test-Attached "40x12, 3 rows, +1" 40 12 "-v" "even-vertical" 1
    Test-Attached "40x40, 3 rows, +1" 40 40 "-v" "even-vertical" 1
    Test-Attached "120x30, 3 columns, +1" 120 30 "-h" "even-horizontal" 1
}

Stop-Ns
Write-Host "`n=== PR #874 divider drag: $script:Pass passed, $script:Fail failed, $script:Skip skipped ===" -ForegroundColor Cyan
if ($script:Fail -gt 0) { exit 1 } else { exit 0 }
