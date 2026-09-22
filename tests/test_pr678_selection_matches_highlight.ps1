#!/usr/bin/env pwsh
###############################################################################
# test_pr678_selection_matches_highlight.ps1
#
# PR #678: the yank must match the highlight, in every direction and at every
# edge.
#
# This drives a REAL attached psmux client: the binary is started in its own
# console window and real Windows MOUSE_EVENT records (press, held moves,
# release) are written into that console's input buffer with
# WriteConsoleInput, so the client's own copy-mode drag path runs, not a CLI
# shortcut.  The "highlight" is read from the frame the server published for
# that client (dump-state: sel_start_row/col, sel_end_row/col plus the pane's
# content grid), which is exactly the data the client renders the selection
# from, and it is read BEFORE the button goes up.  The "yank" is show-buffer
# after the release.  The two must be the same text.
#
# ReadConsoleOutputAttribute is deliberately NOT used for the highlight: on
# Windows 11 26200 the legacy attribute plane comes back as 0x0007 for every
# cell a VT sequence coloured (the status line reads 0x0007 too), so it cannot
# see the selection style on this host.
#
# Cases
#   1 left to right on one row
#   2 right to left on one row
#   3 upward multi row drag
#   4 downward multi row drag
#   5 ending on the last column
#   6 release cell one past the last drag cell, both in ONE input batch
#     (the reported "one character more than the highlight" symptom)
#   7 a double width CJK line, ending on the second half of a wide cell
#
# Usage:
#   test_pr678_selection_matches_highlight.ps1 [-Binary <path to psmux.exe>]
###############################################################################
param(
    [string]$Binary = "",
    [int]$Repeat = 1
)

$ErrorActionPreference = "Continue"

$PSMUX = if ($Binary) { (Resolve-Path $Binary).Path } else { (Get-Command psmux -EA Stop).Source }
$NS = "pr678"
$SESSION = "pr678_sel"
if (-not $env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR = "$env:TEMP\pr678-data" }
$env:PSMUX_NO_WARM = "1"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null

$script:Passed = 0
$script:Failed = 0
$script:OpenedPids = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:Passed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red;  $script:Failed++ }
function Write-Info($msg) { Write-Host "  [info] $msg" -ForegroundColor DarkGray }

Write-Host "`n================================================================" -ForegroundColor Cyan
Write-Host " PR #678: the yank must equal the highlight (real client, real" -ForegroundColor Cyan
Write-Host " console mouse records)" -ForegroundColor Cyan
Write-Host " binary: $PSMUX" -ForegroundColor Cyan
Write-Host "================================================================`n" -ForegroundColor Cyan

# --- the injector / screen reader ------------------------------------------
$probeSrc = Join-Path $PSScriptRoot "pr678_mouse_probe.cs"
$probeExe = Join-Path $env:TEMP "pr678_mouse_probe.exe"
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
if (-not (Test-Path $probeSrc)) { Write-Fail "missing $probeSrc"; exit 1 }
& $csc /nologo /optimize /out:$probeExe /target:exe $probeSrc 2>&1 | Out-Null
if (-not (Test-Path $probeExe)) { Write-Fail "csc failed to build the mouse probe"; exit 1 }

# --- session + client -------------------------------------------------------
function Stop-Everything {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    foreach ($procId in $script:OpenedPids) {
        $p = Get-Process -Id $procId -EA SilentlyContinue
        if ($p) { Stop-Process -Id $procId -Force -EA SilentlyContinue }
    }
}

Stop-Everything
$client = Start-Process -FilePath $PSMUX -ArgumentList '-L',$NS,'new-session','-s',$SESSION -PassThru
$script:OpenedPids += $client.Id
Write-Info "client pid $($client.Id)"

$deadline = (Get-Date).AddSeconds(25)
$up = $false
while ((Get-Date) -lt $deadline) {
    if (Test-Path (Join-Path $env:PSMUX_DATA_DIR "${NS}__${SESSION}.port")) { $up = $true; break }
    Start-Sleep -Milliseconds 300
}
if (-not $up) { Write-Fail "session did not start"; Stop-Everything; exit 1 }
Start-Sleep -Seconds 2

& $PSMUX -L $NS set -g mouse on 2>&1 | Out-Null
& $PSMUX -L $NS set -g mode-style "bg=yellow,fg=black" 2>&1 | Out-Null
Start-Sleep -Milliseconds 300

# Three ASCII marker lines plus one double width CJK line, printed in one go
# so they land on consecutive rows.
$paint = '"PR678-L1-ABCDEFGHIJKLMNOP","PR678-L2-QRSTUVWXYZ012345","PR678-L3-6789abcdefghijkl","W5 CJK 另外别忘了那 end"'
& $PSMUX -L $NS send-keys -t $SESSION $paint Enter 2>&1 | Out-Null
Start-Sleep -Seconds 3

function Get-State {
    $raw = (& $PSMUX -L $NS dump-state -t $SESSION 2>&1 | Out-String)
    try { return ($raw | ConvertFrom-Json) } catch { return $null }
}

function Get-Leaf($state) {
    if ($null -eq $state) { return $null }
    $n = $state.layout
    while ($n -and $n.type -eq "split") { $n = $n.children[0] }
    return $n
}

function Row-Text($leaf, [int]$r) {
    $sb = New-Object System.Text.StringBuilder
    foreach ($cell in $leaf.content[$r]) { [void]$sb.Append($cell.text) }
    return $sb.ToString()
}

# The pane grid only rides along with dump-state while the pane is in copy
# mode, so find the marker rows from inside copy mode and then leave again.
& $PSMUX -L $NS copy-mode -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 800
$st = Get-State
$leaf = Get-Leaf $st
if (-not $leaf) { Write-Fail "dump-state gave no leaf"; Stop-Everything; exit 1 }
$rowsN = $leaf.content.Count

$ROW_L1 = -1; $ROW_L2 = -1; $ROW_L3 = -1; $ROW_CJK = -1
for ($r = 0; $r -lt $rowsN; $r++) {
    $t = Row-Text $leaf $r
    if ($t.StartsWith("PR678-L1-")) { $ROW_L1 = $r }
    if ($t.StartsWith("PR678-L2-")) { $ROW_L2 = $r }
    if ($t.StartsWith("PR678-L3-")) { $ROW_L3 = $r }
    if ($t.StartsWith("W5 CJK "))   { $ROW_CJK = $r }
}
if ($ROW_L1 -lt 0 -or $ROW_L2 -lt 0 -or $ROW_L3 -lt 0 -or $ROW_CJK -lt 0) {
    Write-Fail "marker rows not found (L1=$ROW_L1 L2=$ROW_L2 L3=$ROW_L3 CJK=$ROW_CJK)"
    Write-Info ("row 0: '" + (Row-Text $leaf 0) + "'")
    Stop-Everything; exit 1
}
Write-Pass "setup: marker rows L1=$ROW_L1 L2=$ROW_L2 L3=$ROW_L3 CJK=$ROW_CJK, pane $($leaf.cols)x$($leaf.rows)"
& $PSMUX -L $NS send-keys -t $SESSION -X cancel 2>&1 | Out-Null
Start-Sleep -Milliseconds 400

# The cells the frame says are selected, rendered exactly the way the client
# renders them (src/client.rs: the in_selection predicate).
function Painted-Text($leaf) {
    if ($null -eq $leaf.sel_start_row -or $null -eq $leaf.sel_end_row) { return $null }
    $sr = [int]$leaf.sel_start_row; $sc = [int]$leaf.sel_start_col
    $er = [int]$leaf.sel_end_row;   $ec = [int]$leaf.sel_end_col
    $mode = if ($leaf.sel_mode) { $leaf.sel_mode } else { "char" }
    $lines = @()
    for ($r = $sr; $r -le $er; $r++) {
        $row = $leaf.content[$r]
        if ($mode -eq "line") { $c0 = 0; $c1 = $row.Count - 1 }
        elseif ($mode -eq "rect") { $c0 = [Math]::Min($sc,$ec); $c1 = [Math]::Max($sc,$ec) }
        elseif ($sr -eq $er) { $c0 = [Math]::Min($sc,$ec); $c1 = [Math]::Max($sc,$ec) }
        elseif ($r -eq $sr) { $c0 = $sc; $c1 = $row.Count - 1 }
        elseif ($r -eq $er) { $c0 = 0;   $c1 = $ec }
        else { $c0 = 0; $c1 = $row.Count - 1 }
        $sb = New-Object System.Text.StringBuilder
        for ($c = $c0; $c -le $c1 -and $c -lt $row.Count; $c++) { [void]$sb.Append($row[$c].text) }
        $lines += ($sb.ToString() -replace '[ \t]+$','')
    }
    return ($lines -join "`n")
}

function Normalize($s) {
    if ($null -eq $s) { return "" }
    $s = $s -replace "`r`n", "`n"
    $s = $s.TrimEnd("`n")
    return (($s -split "`n") | ForEach-Object { $_ -replace '[ \t]+$','' }) -join "`n"
}

# One gesture.  Waypoints are (col,row) pairs delivered as held moves.  When
# -Slip is given the slip motion and the button up are written in ONE
# WriteConsoleInput batch, the way a terminal coalesces a fast flick.
function Invoke-Drag {
    param(
        [string]$Name,
        [int]$PressX, [int]$PressY,
        [int[][]]$Waypoints,
        [int[]]$Slip = $null
    )
    & $PSMUX -L $NS delete-buffer 2>&1 | Out-Null
    & $PSMUX -L $NS delete-buffer 2>&1 | Out-Null
    & $PSMUX -L $NS copy-mode -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600

    & $probeExe $client.Id press $PressX $PressY 2>&1 | Out-Null
    Start-Sleep -Milliseconds 180
    foreach ($w in $Waypoints) {
        & $probeExe $client.Id move $w[0] $w[1] 2>&1 | Out-Null
        Start-Sleep -Milliseconds 180
    }
    Start-Sleep -Milliseconds 700

    # What is on screen right now: the frame the client is painting from.
    $stDrag = Get-State
    $leafDrag = Get-Leaf $stDrag
    $painted = Painted-Text $leafDrag
    $coords = "sel=($($leafDrag.sel_start_row),$($leafDrag.sel_start_col))..($($leafDrag.sel_end_row),$($leafDrag.sel_end_col))"

    $last = $Waypoints[$Waypoints.Count - 1]
    if ($Slip) {
        & $probeExe $client.Id slipup $Slip[0] $Slip[1] $Slip[0] $Slip[1] 2>&1 | Out-Null
    } else {
        & $probeExe $client.Id up $last[0] $last[1] 2>&1 | Out-Null
    }
    Start-Sleep -Milliseconds 900

    $yank = (& $PSMUX -L $NS show-buffer 2>&1 | Out-String)
    $inMode = (& $PSMUX -L $NS display-message -t $SESSION -p '#{pane_in_mode}' 2>&1 | Out-String).Trim()
    if ($inMode -ne "0") {
        & $PSMUX -L $NS send-keys -t $SESSION -X cancel 2>&1 | Out-Null
        Start-Sleep -Milliseconds 300
    }

    return [pscustomobject]@{
        Name    = $Name
        Coords  = $coords
        Painted = Normalize $painted
        Yank    = Normalize $yank
    }
}

function Check($res) {
    Write-Info ("$($res.Name): $($res.Coords)")
    Write-Info ("  painted [" + $res.Painted.Replace("`n","\n") + "] len=" + $res.Painted.Length)
    Write-Info ("  yanked  [" + $res.Yank.Replace("`n","\n")    + "] len=" + $res.Yank.Length)
    if ($res.Painted.Length -eq 0) {
        Write-Fail "$($res.Name): nothing was painted, the gesture never produced a selection"
        return
    }
    if ($res.Painted -ceq $res.Yank) {
        Write-Pass "$($res.Name): yank equals the highlight"
    } else {
        Write-Fail "$($res.Name): yank differs from the highlight"
    }
}

for ($iter = 1; $iter -le $Repeat; $iter++) {
    if ($Repeat -gt 1) { Write-Host "`n--- pass $iter of $Repeat ---" -ForegroundColor Yellow }

    Write-Host "`n--- CASE 1: left to right on one row ---" -ForegroundColor Yellow
    Check (Invoke-Drag -Name "case1-ltr" -PressX 0 -PressY $ROW_L1 -Waypoints @(@(4,$ROW_L1),@(8,$ROW_L1)))

    Write-Host "`n--- CASE 2: right to left on one row ---" -ForegroundColor Yellow
    Check (Invoke-Drag -Name "case2-rtl" -PressX 16 -PressY $ROW_L1 -Waypoints @(@(10,$ROW_L1),@(4,$ROW_L1)))

    Write-Host "`n--- CASE 3: upward multi row drag ---" -ForegroundColor Yellow
    Check (Invoke-Drag -Name "case3-up" -PressX 9 -PressY $ROW_L3 -Waypoints @(@(6,$ROW_L2),@(3,$ROW_L1)))

    Write-Host "`n--- CASE 4: downward multi row drag ---" -ForegroundColor Yellow
    Check (Invoke-Drag -Name "case4-down" -PressX 3 -PressY $ROW_L1 -Waypoints @(@(6,$ROW_L2),@(9,$ROW_L3)))

    Write-Host "`n--- CASE 5: ending on the last column ---" -ForegroundColor Yellow
    $lastCol = [int]$leaf.cols - 1
    Check (Invoke-Drag -Name "case5-lastcol" -PressX 0 -PressY $ROW_L1 -Waypoints @(@(40,$ROW_L1),@($lastCol,$ROW_L1)))

    Write-Host "`n--- CASE 6: release one cell past the last drag (one batch) ---" -ForegroundColor Yellow
    Check (Invoke-Drag -Name "case6-slip" -PressX 0 -PressY $ROW_L1 -Waypoints @(@(4,$ROW_L1),@(8,$ROW_L1)) -Slip @(9,$ROW_L1))

    Write-Host "`n--- CASE 7: CJK, ending on the second half of a wide cell ---" -ForegroundColor Yellow
    # "W5 CJK 另外别忘了那 end": the wide run starts at column 7, so column 8 is
    # the second half of the first wide character.
    Check (Invoke-Drag -Name "case7-cjk" -PressX 0 -PressY $ROW_CJK -Waypoints @(@(4,$ROW_CJK),@(8,$ROW_CJK)))
}

Write-Host "`n================================================================" -ForegroundColor Cyan
Write-Host " PASSED: $($script:Passed)   FAILED: $($script:Failed)" -ForegroundColor Cyan
Write-Host "================================================================`n" -ForegroundColor Cyan

Stop-Everything
$leftover = Get-Process -Id $script:OpenedPids -EA SilentlyContinue
if ($leftover) { Write-Host "  [warn] leftover pids: $($leftover.Id -join ',')" -ForegroundColor Yellow }
else { Write-Host "  [info] every process this test opened is closed" -ForegroundColor DarkGray }

if ($script:Failed -gt 0) { exit 1 } else { exit 0 }
