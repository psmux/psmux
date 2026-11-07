# Issue #1012: with `status-position top`, mouse PRESS and hover reached the
# pane on the right row, but RELEASE and DRAG landed one row lower (two rows
# with `status 2`) (dhaern). Sibling of #669.
#
# The client converts a press against the pane rects, which already sit below
# a top status bar, and sends it as `pane-mouse`. Release and drag travel as
# RAW screen coordinates for the server to convert, and the server mapped them
# against a window area that always starts at row 0, so the status rows above
# the window were never taken off.
#
# Ground truth is an SGR mouse echo child that logs its own raw stdin
# (tests/mouse_echo_child.cs).
#
# Layers: raw control-port verbs (pure server route) and real MOUSE_EVENT
#         records injected into an attached client's console.
#
# Run:  pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue1012_mouse_status_top.ps1
#       PSMUX_EXE selects the binary under test.

$ErrorActionPreference = "Continue"
$env:PSMUX_NO_WARM = "1"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$exeName = [IO.Path]::GetFileName($PSMUX)
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$NS = "i1012m"
$S  = "s1012"

$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan
Write-Host ("version: " + (& $PSMUX -V)) -ForegroundColor Cyan

$TMP = Join-Path $env:TEMP "i1012_suite"
New-Item -ItemType Directory -Force -Path $TMP | Out-Null
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }

$child    = Join-Path $TMP "echo_child.exe"
$clickInj = Join-Path $TMP "click.exe"
$dragInj  = Join-Path $TMP "drag.exe"
$childLog = Join-Path $TMP "echo.txt"
foreach ($pair in @(@($child,"mouse_echo_child.cs"),@($clickInj,"click_injector.cs"),@($dragInj,"mouse_drag_hold_injector.cs"))) {
    if (-not (Test-Path $pair[0])) { & $csc /nologo /optimize /out:$($pair[0]) (Join-Path $PSScriptRoot $pair[1]) 2>&1 | Out-Null }
    if (-not (Test-Path $pair[0])) { Write-Host "FATAL: could not compile $($pair[1])" -ForegroundColor Red; exit 1 }
}

function Port-File {
    Get-ChildItem $psmuxDir -Filter "*$S.port" -EA SilentlyContinue |
        Where-Object { $_.Name -like "*$NS*" } | Select-Object -First 1
}
function Stop-Ns {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    for ($i = 0; $i -lt 40; $i++) {
        if (-not (Port-File)) { break }
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Milliseconds 700
}
function LogCount { if (Test-Path $childLog) { (Get-Content $childLog).Count } else { 0 } }
function New-Events([int]$before) {
    $all = @(if (Test-Path $childLog) { Get-Content $childLog } else { @() })
    if ($all.Count -le $before) { return @() }
    $out = @()
    foreach ($line in $all[$before..($all.Count-1)]) {
        foreach ($m in [regex]::Matches($line, '<ESC>\[<(\d+);(\d+);(\d+)([Mm])')) {
            $out += [pscustomobject]@{
                Btn  = [int]$m.Groups[1].Value
                Col  = [int]$m.Groups[2].Value
                Row  = [int]$m.Groups[3].Value
                Kind = $m.Groups[4].Value
                Raw  = $m.Value
            }
        }
    }
    return $out
}
function Send-Tcp([string[]]$Cmds) {
    $pf = Port-File
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
            Start-Sleep -Milliseconds 150
            $tcp.Close()
        } catch { return "EXCEPTION $($_.Exception.Message)" }
    }
    return "sent"
}
function Wait-Child([string]$Cmd, [int]$Secs = 25) {
    & $PSMUX -L $NS send-keys -t $S $Cmd Enter 2>&1 | Out-Null
    $retried = $false
    for ($i = 0; $i -lt ($Secs * 2); $i++) {
        Start-Sleep -Milliseconds 500
        if (((& $PSMUX -L $NS capture-pane -t $S -p 2>&1) | Out-String) -match 'MOUSE_ECHO_READY') { return $true }
        if (-not $retried -and $i -eq $Secs) {
            $retried = $true
            & $PSMUX -L $NS send-keys -t $S $Cmd Enter 2>&1 | Out-Null
        }
    }
    return $false
}
function Set-Status([string]$pos, [string]$lines) {
    & $PSMUX -L $NS set-option -g status $lines 2>&1 | Out-Null
    & $PSMUX -L $NS set-option -g status-position $pos 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1200
}

# Screen row 5 (0 based) is the press cell, row 6 the drag target.
# Status rows above the window shift the pane down by that many rows, so the
# SGR row (1 based, pane relative) is 6 - top status rows.
$cases = @(
    @{ Pos = "bottom"; Lines = "on"; Top = 0 },
    @{ Pos = "top";    Lines = "on"; Top = 1 },
    @{ Pos = "top";    Lines = "2";  Top = 2 },
    @{ Pos = "bottom"; Lines = "2";  Top = 0 },
    @{ Pos = "top";    Lines = "off"; Top = 0 }
)

# =====================================================================
Write-Host "`n[A] raw control-port verbs, detached 80x24 session" -ForegroundColor Yellow
# =====================================================================
Stop-Ns
Remove-Item $childLog -Force -EA SilentlyContinue
$env:PSMUX_MOUSE_ECHO_LOG = $childLog
& $PSMUX -L $NS new-session -d -s $S -x 80 -y 24 2>&1 | Out-Null
for ($i = 0; $i -lt 60; $i++) { if (Port-File) { break }; Start-Sleep -Milliseconds 250 }
if (-not (Port-File)) { Write-Fail "detached session $S did not come up in namespace $NS"; exit 1 }
Start-Sleep -Seconds 2
& $PSMUX -L $NS set-option -t $S -g mouse on 2>&1 | Out-Null
if (Wait-Child "`$env:PSMUX_MOUSE_ECHO_LOG='$childLog'; & '$child'") {
    Write-Pass "mouse-reporting child is running"
} else {
    Write-Fail "mouse-reporting child did not start"
    Write-Info ((& $PSMUX -L $NS capture-pane -t $S -p 2>&1) | Out-String)
    Stop-Ns
    exit 1
}

foreach ($case in $cases) {
    $tag = "status $($case.Lines) position $($case.Pos)"
    Set-Status $case.Pos $case.Lines
    $pressRow = 6 - $case.Top
    $dragRow  = 7 - $case.Top
    Write-Info "$tag : $(((& $PSMUX -L $NS list-panes -t $S -F 'top=#{pane_top} h=#{pane_height}')) -join '; ')"
    $rows = @{}
    foreach ($verb in @(@("mouse-down 10 5","press"), @("mouse-up 10 5","release"), @("mouse-drag 10 6","drag"), @("mouse-up 10 6","drag release"))) {
        $b = LogCount
        $null = Send-Tcp @($verb[0])
        Start-Sleep -Milliseconds 700
        $ev = New-Events $b | Select-Object -Last 1
        if ($null -eq $ev) { Write-Skip "$tag : '$($verb[0])' forwarded nothing"; continue }
        $want = if ($verb[1] -like "drag*") { $dragRow } else { $pressRow }
        if ($ev.Row -eq $want) { Write-Pass "$tag : '$($verb[0])' -> $($ev.Raw) (row $want)" }
        else { Write-Fail "$tag : '$($verb[0])' -> $($ev.Raw), expected SGR row $want (#1012)" }
    }
}
Stop-Ns

# =====================================================================
Write-Host "`n[B] attached client, real MOUSE_EVENT records" -ForegroundColor Yellow
# =====================================================================
$conf = Join-Path $TMP "i1012.conf"
"set -g mouse on" | Set-Content -Path $conf -Encoding ASCII
$launch = Join-Path $TMP "launch.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set PSMUX_NO_WARM=1
set NO_COLOR=
set PSMUX_MOUSE_ECHO_LOG=$childLog
"$PSMUX" -L $NS -f "$conf" new-session -s $S -x 80 -y 24
"@ | Set-Content -Path $launch -Encoding ASCII

Remove-Item $childLog -Force -EA SilentlyContinue
$launcher = Start-Process -FilePath $launch -PassThru
$cli = $null
for ($i = 0; $i -lt 40; $i++) {
    Start-Sleep -Milliseconds 500
    $cli = Get-CimInstance Win32_Process -Filter "Name='$exeName'" |
           Where-Object { $_.CommandLine -match "-L $NS" -and $_.CommandLine -match "new-session -s $S" } |
           Select-Object -First 1
    if ($cli) { break }
}
if (-not $cli) {
    Write-Skip "no attached client started; the console-injection layer cannot run here"
} else {
    Start-Sleep -Seconds 4
    $cpid = [int]$cli.ProcessId
    Write-Info "attached client pid=$cpid"
    if (Wait-Child "& '$child'") {
        Write-Pass "mouse-reporting child is running under the attached client"
        foreach ($case in $cases) {
            $tag = "status $($case.Lines) position $($case.Pos)"
            Set-Status $case.Pos $case.Lines
            $pressRow = 6 - $case.Top

            # One real click: press and release at the SAME cell.
            $b = LogCount
            & $clickInj $cpid 10 5 120 | Out-Null
            Start-Sleep -Milliseconds 1000
            $ev = New-Events $b
            $press   = $ev | Where-Object { $_.Kind -ceq 'M' -and $_.Btn -eq 0 } | Select-Object -First 1
            $release = $ev | Where-Object { $_.Kind -ceq 'm' } | Select-Object -First 1
            if ($null -eq $press -or $null -eq $release) {
                Write-Skip "$tag : the injected click delivered no press/release pair (focus refused)"
            } else {
                if ($press.Row -eq $pressRow) { Write-Pass "$tag : click press $($press.Raw) on SGR row $pressRow" }
                else { Write-Fail "$tag : click press $($press.Raw), expected SGR row $pressRow" }
                if ($release.Row -eq $press.Row) { Write-Pass "$tag : click release $($release.Raw) shares the press row" }
                else { Write-Fail "$tag : click press $($press.Raw) but release $($release.Raw), $($release.Row - $press.Row) row(s) off (#1012)" }
            }

            # One real drag: press on row 5, motion to row 6, release there.
            $b = LogCount
            & $dragInj $cpid 10 5 10 6 2 60 150 | Out-Null
            Start-Sleep -Milliseconds 1400
            $ev = New-Events $b
            $dpress  = $ev | Where-Object { $_.Kind -ceq 'M' -and $_.Btn -eq 0 } | Select-Object -First 1
            $motions = @($ev | Where-Object { $_.Kind -ceq 'M' -and $_.Btn -eq 32 })
            $drel    = $ev | Where-Object { $_.Kind -ceq 'm' } | Select-Object -Last 1
            if ($null -eq $dpress -or $motions.Count -eq 0) {
                Write-Skip "$tag : the injected drag delivered no press/motion (focus refused)"
            } else {
                $lastMotion = $motions[-1]
                if ($lastMotion.Row -eq ($dpress.Row + 1)) {
                    Write-Pass "$tag : drag one row below the press reported one SGR row below it ($($dpress.Raw) -> $($lastMotion.Raw))"
                } else {
                    Write-Fail "$tag : drag reported $($lastMotion.Raw) for one row below press $($dpress.Raw) (#1012)"
                }
                if ($null -ne $drel) {
                    if ($drel.Row -eq ($dpress.Row + 1)) { Write-Pass "$tag : drag release $($drel.Raw) on the row the drag ended on" }
                    else { Write-Fail "$tag : drag from $($dpress.Raw) released at $($drel.Raw), expected SGR row $($dpress.Row + 1) (#1012)" }
                }
            }
        }
    } else {
        Write-Fail "mouse-reporting child did not start under the attached client"
        Write-Info ((& $PSMUX -L $NS capture-pane -t $S -p 2>&1) | Out-String)
    }
    Stop-Ns
    try { Stop-Process -Id $cpid -Force -EA SilentlyContinue } catch {}
}
if ($launcher) { try { Stop-Process -Id $launcher.Id -Force -EA SilentlyContinue } catch {} }

Stop-Ns
Write-Host "`n=== Issue #1012 summary ===" -ForegroundColor Cyan
Write-Host "  Passed:  $script:Pass" -ForegroundColor Green
Write-Host "  Failed:  $script:Fail" -ForegroundColor $(if ($script:Fail -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $script:Skip" -ForegroundColor Yellow
if ($script:Fail -gt 0) { exit 1 } else { exit 0 }
