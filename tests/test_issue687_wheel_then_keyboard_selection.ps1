#!/usr/bin/env pwsh
###############################################################################
# test_issue687_wheel_then_keyboard_selection.ps1
#
# Issue #687, the routes the PR's own suite does not drive: a REAL mouse.
#
# tests/test_issue687_copy_mode_keyboard_selection.ps1 moves the view with
# `send-keys -X scroll-up` and with the cursor walking off the top row.  The
# route most people actually take is the wheel, and the wheel is a Windows
# MOUSE_EVENT record, not a key, so it reaches copy mode through a different
# door.  This drives a real attached client and writes real wheel records into
# its console input buffer with WriteConsoleInput, then selects three lines
# with the vi keys themselves and checks what lands in the buffer.
#
# Part A  wheel up N notches, then V j j Enter by real keys.
# Part B  a left CLICK first (which is what pins `copy_pos_scroll_offset`),
#         then scroll, then V j j Enter.  A click is not a drag: it leaves copy
#         mode open with the endpoint pinned to the view the click happened in,
#         and every later keyboard selection is resolved against that old view.
#
# Usage:
#   test_issue687_wheel_then_keyboard_selection.ps1 [-Binary <path to psmux.exe>]
###############################################################################
param(
    [string]$Binary = ""
)

$ErrorActionPreference = "Continue"

$PSMUX = if ($Binary) { (Resolve-Path $Binary).Path }
         elseif ($env:PSMUX_EXE) { $env:PSMUX_EXE }
         else { (Get-Command psmux -EA Stop).Source }
$NS = "i687w"
$SESSION = "i687w_s"
if (-not $env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR = "$env:TEMP\i687w-data" }
$env:PSMUX_NO_WARM = "1"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null

$script:Passed = 0
$script:Failed = 0
$script:Skipped = 0
$script:OpenedPids = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:Passed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red;  $script:Failed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:Skipped++ }
function Write-Info($msg) { Write-Host "  [info] $msg" -ForegroundColor DarkGray }

Write-Host "`n=== Issue #687: the wheel and the click reach it too ===" -ForegroundColor Cyan
Write-Info "binary: $PSMUX"

# --- probes -----------------------------------------------------------------
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }

$wheelSrc = Join-Path $PSScriptRoot "pr688_wheel_probe.cs"
$wheelExe = Join-Path $env:TEMP "pr688_wheel_probe.exe"
$mouseSrc = Join-Path $PSScriptRoot "pr678_mouse_probe.cs"
$mouseExe = Join-Path $env:TEMP "pr678_mouse_probe.exe"
$keySrc   = Join-Path $PSScriptRoot "injector.cs"
$keyExe   = Join-Path $env:TEMP "psmux_injector_687w.exe"

foreach ($pair in @(@($wheelSrc, $wheelExe), @($mouseSrc, $mouseExe), @($keySrc, $keyExe))) {
    if (-not (Test-Path $pair[0])) { Write-Fail "missing $($pair[0])"; exit 1 }
    & $csc /nologo /optimize /out:$($pair[1]) $pair[0] 2>&1 | Out-Null
    if (-not (Test-Path $pair[1])) { Write-Fail "csc failed to build $($pair[0])"; exit 1 }
}

function Stop-Everything {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    foreach ($procId in $script:OpenedPids) {
        if (Get-Process -Id $procId -EA SilentlyContinue) {
            Stop-Process -Id $procId -Force -EA SilentlyContinue
        }
    }
    $script:OpenedPids = @()
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
& $PSMUX -L $NS set -g mode-keys vi 2>&1 | Out-Null
Start-Sleep -Milliseconds 300

# 200 numbered lines so every assertion names a line the reader can find.
& $PSMUX -L $NS send-keys -t $SESSION "1..200 | % { 'LINE{0:d3}' -f `$_ }" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 4

function Get-State {
    $raw = (& $PSMUX -L $NS dump-state -t $SESSION 2>&1 | Out-String)
    try { return ($raw | ConvertFrom-Json) } catch { return $null }
}

function Get-Leaf {
    $s = Get-State
    if (-not $s) { return $null }
    return $s.layout
}

function Get-Buffer {
    Start-Sleep -Milliseconds 300
    ((& $PSMUX -L $NS show-buffer 2>&1 | Out-String).TrimEnd()) -replace "`r?`n", ","
}

# Copy mode parks the cursor on the last row; five cursor-ups put it on LINE196
# with the view still at the live bottom, clear of both edges.
function Enter-Parked {
    & $PSMUX -L $NS send-keys -t $SESSION -X cancel 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    & $PSMUX -L $NS copy-mode -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    1..5 | ForEach-Object { & $PSMUX -L $NS send-keys -t $SESSION -X cursor-up 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 300
}

function Expected($scroll) {
    $first = 196 - $scroll
    (($first..($first + 2)) | ForEach-Object { "LINE{0:D3}" -f $_ }) -join ","
}

function Select-Three-With-Keys {
    & $keyExe $client.Id "V" | Out-Null; Start-Sleep -Milliseconds 300
    & $keyExe $client.Id "j" | Out-Null; Start-Sleep -Milliseconds 250
    & $keyExe $client.Id "j" | Out-Null; Start-Sleep -Milliseconds 250
    & $keyExe $client.Id "{ENTER}" | Out-Null; Start-Sleep -Milliseconds 500
}

# ── Part A: the wheel moves the view, the keys make the selection ───────────
Write-Host "`n[Part A] real wheel records, then V j j Enter by real keys" -ForegroundColor Yellow
foreach ($notches in @(1, 3)) {
    Enter-Parked
    $before = Get-Leaf
    if (-not $before -or -not $before.copy_mode) { Write-Skip "copy mode did not open"; continue }
    & $wheelExe $client.Id up 10 8 $notches | Out-Null
    Start-Sleep -Milliseconds 700
    $after = Get-Leaf
    $scroll = [int]$after.scroll_offset
    if ($scroll -le 0) {
        Write-Skip "$notches wheel notch(es) did not move the view, nothing to prove"
        continue
    }
    Write-Info "$notches notch(es) scrolled the view to offset $scroll"
    Select-Three-With-Keys
    $got = Get-Buffer
    $want = Expected $scroll
    if ($got -eq $want) { Write-Pass "wheel x$notches (offset $scroll) then V j j Enter copied [$got]" }
    else { Write-Fail "wheel x$notches (offset $scroll) copied [$got], want [$want]" }
}

# ── Part B: a click pins the endpoint, a later keyboard selection must not ──
#            be resolved against the view that click happened in.
Write-Host "`n[Part B] a left click, then scroll, then V j j Enter" -ForegroundColor Yellow
Enter-Parked
$l = Get-Leaf
if (-not $l -or -not $l.copy_mode) {
    Write-Skip "copy mode did not open"
} else {
    # A press and a release in the same cell is a click, not a drag: it
    # positions the copy cursor and leaves copy mode open.
    & $mouseExe $client.Id press 10 18 | Out-Null
    Start-Sleep -Milliseconds 250
    & $mouseExe $client.Id up 10 18 | Out-Null
    Start-Sleep -Milliseconds 400
    $l = Get-Leaf
    if (-not $l -or -not $l.copy_mode) {
        Write-Skip "the click closed copy mode on this host, nothing to prove"
    } else {
        $row = [int]$l.copy_cursor_row
        Write-Info "the click left the copy cursor on row $row"
        & $PSMUX -L $NS send-keys -t $SESSION -X scroll-up 2>&1 | Out-Null
        & $PSMUX -L $NS send-keys -t $SESSION -X scroll-up 2>&1 | Out-Null
        & $PSMUX -L $NS send-keys -t $SESSION -X scroll-up 2>&1 | Out-Null
        Start-Sleep -Milliseconds 400
        $l = Get-Leaf
        $scroll = [int]$l.scroll_offset
        # The click parked the cursor on a known screen row, so the three lines
        # under it follow from the row, the offset and the pane height: the
        # bottom row holds the shell prompt, the one above it holds LINE200, so
        # screen row r at offset s holds LINE(202 - height + r - s).
        $height = [int]$l.rows
        $first = 202 - $height + $row - $scroll
        $want = (($first..($first + 2)) | ForEach-Object { "LINE{0:D3}" -f $_ }) -join ","
        Select-Three-With-Keys
        $got = Get-Buffer
        if ($got -eq $want) { Write-Pass "click then scroll-up x3 then V j j Enter copied [$got]" }
        else { Write-Fail "click then scroll-up x3 then V j j Enter copied [$got], want [$want]" }
    }
}

Stop-Everything
Write-Host "`n=== Summary ===" -ForegroundColor Cyan
Write-Host "  Passed:  $script:Passed"
Write-Host "  Failed:  $script:Failed"
Write-Host "  Skipped: $script:Skipped"
if ($script:Failed -gt 0) { exit 1 } else { exit 0 }
