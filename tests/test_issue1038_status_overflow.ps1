# PR #1038: an overflowing window list keeps the active window in view.
#
# A real client is attached in its own 60 column console window and the
# status row is read back from that console (conread.exe), so this checks
# what the user actually sees. Every expected row below was drawn by a real
# tmux client with the same options, windows and width: psmux must match it
# character for character (viewport centred on the active window, "<" and
# ">" markers drawn over the ends, status-left and status-right reserved,
# absolute-centre drawn over both sides). Clicks on a scrolled list must
# select the window drawn under the pointer and a marker cell selects nothing.
#
# Isolation: own -L namespace and PSMUX_DATA_DIR; only PIDs started here are
# stopped.
#
# Run: pwsh -NoProfile -File tests\test_issue1038_status_overflow.ps1 [-Exe path]
param([string]$Exe = $env:PSMUX_EXE)
$ErrorActionPreference = 'Continue'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$script:Passed = 0
$script:Failed = 0
function Pass($m) { Write-Host "[PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Fail($m) { Write-Host "[FAIL] $m" -ForegroundColor Red; $script:Failed++ }

if (-not $Exe) { $Exe = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -ErrorAction SilentlyContinue).Path }
if (-not $Exe -or -not (Test-Path $Exe)) { Write-Error 'psmux binary not found. Build first or pass -Exe.'; exit 1 }
Write-Host "Using: $Exe"

$rand = Get-Random -Maximum 999999
$ns = "t1038_$rand"
$root = Join-Path $env:TEMP "psmux_t1038_$rand"
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root 'data'
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
Remove-Item Env:PSMUX_CONFIG_FILE -ErrorAction SilentlyContinue

$csc = 'C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe'
$reader = Join-Path $root 'conread.exe'
$clicker = Join-Path $root 'click.exe'
foreach ($pair in @(@($reader, 'conread.cs'), @($clicker, 'click_injector.cs'))) {
    & $csc /nologo "/out:$($pair[0])" (Join-Path $PSScriptRoot $pair[1]) | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Error "Cannot compile $($pair[1])"; exit 1 }
}

function Mux { (& $Exe -L $ns @args 2>&1 | Out-String).Trim() }
function Status {
    $rows = @(& $reader $script:clientId -u -g 2>$null)
    if ($rows.Count -eq 0) { return '' }
    return ([string]$rows[-1]).TrimEnd("`r")
}
function Wait-Status([string]$Expected, [int]$Seconds = 10) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        $line = Status
        if ($line -eq $Expected) { return $line }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    return $line
}

$config = Join-Path $root 'fixture.conf'
@(
    'set -g mouse on',
    'set -g status-left "[s] "', 'set -g status-right "RIGHT"', 'set -g status-style default',
    'set -g window-status-format "#I:#W "', 'set -g window-status-current-format "#I:#W*"',
    'set -g automatic-rename off'
) | Set-Content $config -Encoding utf8
$launch = Join-Path $root 'launch.cmd'
@"
@echo off
set TMUX=
set TMUX_PANE=
set PSMUX_SESSION=
set PSMUX_TARGET_SESSION=
set PSMUX_DATA_DIR=$($env:PSMUX_DATA_DIR)
set PSMUX_NO_WARM=1
mode con: cols=60 lines=12
"$Exe" -L $ns -f "$config" new-session -s s -n pwsh
"@ | Set-Content $launch -Encoding ascii

$launcher = $null
$script:clientId = $null
try {
    $launcher = Start-Process -FilePath $launch -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while (-not $script:clientId -and [DateTime]::UtcNow -lt $deadline) {
        $c = Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
            Where-Object { $_.CommandLine -like "*-L $ns *new-session*" } | Select-Object -First 1
        if ($c) { $script:clientId = [int]$c.ProcessId } else { Start-Sleep -Milliseconds 200 }
    }
    if (-not $script:clientId) { Fail 'attached client did not start'; throw 'no client' }
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while (-not (Status).Contains('0:pwsh*') -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 200 }
    for ($n = 1; $n -le 14; $n++) { Mux new-window -d -t s -n pwsh | Out-Null }
    $count = @((Mux list-windows -t s -F '#{window_index}') -split "`n").Count
    if ($count -ne 15) { Fail "fixture needs 15 windows, got $count" }

    # Rows drawn by tmux for exactly these options at 60 columns.
    $cases = @(
        @('left', 14, '[s] <pwsh  10:pwsh  11:pwsh  12:pwsh  13:pwsh  14:pwsh*RIGHT'),
        @('left', 7, '[s] <wsh  5:pwsh  6:pwsh  7:pwsh* 8:pwsh  9:pwsh  10:p>RIGHT'),
        @('left', 0, '[s] 0:pwsh* 1:pwsh  2:pwsh  3:pwsh  4:pwsh  5:pwsh  6:>RIGHT'),
        @('left', 10, '[s] <sh  8:pwsh  9:pwsh  10:pwsh* 11:pwsh  12:pwsh  13>RIGHT'),
        @('centre', 7, '[s] <wsh  5:pwsh  6:pwsh  7:pwsh* 8:pwsh  9:pwsh  10:p>RIGHT'),
        @('right', 7, '[s] <wsh  5:pwsh  6:pwsh  7:pwsh* 8:pwsh  9:pwsh  10:p>RIGHT'),
        @('absolute-centre', 7, '<  4:pwsh  5:pwsh  6:pwsh  7:pwsh* 8:pwsh  9:pwsh  10:pwsh >')
    )
    foreach ($c in $cases) {
        Mux set -g status-justify $c[0] | Out-Null
        Mux select-window -t "s:$($c[1])" | Out-Null
        $line = Wait-Status $c[2]
        if ($line -eq $c[2]) { Pass "$($c[0]) active $($c[1]) matches tmux: |$line|" }
        else { Fail "$($c[0]) active $($c[1])`n  expected |$($c[2])|`n  drawn    |$line|" }
    }

    # Clicks on the scrolled list (left justify, window 7 active).
    Mux set -g status-justify left | Out-Null
    Mux select-window -t s:7 | Out-Null
    $line = Wait-Status '[s] <wsh  5:pwsh  6:pwsh  7:pwsh* 8:pwsh  9:pwsh  10:p>RIGHT'
    $row = @(& $reader $script:clientId -u -g).Count - 1
    $col = $line.IndexOf('9:pwsh')
    & $clicker $script:clientId ($col + 1) $row 80 | Out-Null
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ((Mux display-message -p -t s '#{window_index}') -ne '9' -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
    $sel = Mux display-message -p -t s '#{window_index}'
    if ($sel -eq '9') { Pass "click at column $($col + 1) on the drawn 9:pwsh selects window 9" }
    else { Fail "click on 9:pwsh selected window $sel" }

    # Window 9 active: the clipped "10:p" tab shown before ">" is still window 10.
    $expected9 = '[s] <wsh  7:pwsh  8:pwsh  9:pwsh* 10:pwsh  11:pwsh  12>RIGHT'
    $line = Wait-Status $expected9
    if ($line -eq $expected9) { Pass "after the click the list scrolls like tmux: |$line|" }
    else { Fail "after the click`n  expected |$expected9|`n  drawn    |$line|" }
    $col = $line.IndexOf('12>')
    & $clicker $script:clientId $col $row 80 | Out-Null
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ((Mux display-message -p -t s '#{window_index}') -ne '12' -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
    $sel = Mux display-message -p -t s '#{window_index}'
    if ($sel -eq '12') { Pass "click on the clipped tab 12 at column $col selects window 12" }
    else { Fail "click on the clipped tab 12 selected window $sel" }

    # The "<" marker cell belongs to no window: clicking it changes nothing.
    $line = Status
    $col = $line.IndexOf('<')
    & $clicker $script:clientId $col $row 80 | Out-Null
    Start-Sleep -Milliseconds 800
    $sel = Mux display-message -p -t s '#{window_index}'
    if ($sel -eq '12') { Pass "click on the < marker at column $col keeps window 12" }
    else { Fail "click on the < marker selected window $sel" }

    $alive = Mux list-clients
    if ($alive -match 'attached') { Pass 'client still attached after all redraws' } else { Fail "client gone: $alive" }
} finally {
    & $Exe -L $ns kill-server 2>&1 | Out-Null
    if ($launcher -and -not $launcher.HasExited) { Stop-Process -Id $launcher.Id -Force -ErrorAction SilentlyContinue }
    Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
}

Write-Host "`nResults: $script:Passed passed, $script:Failed failed"
if ($script:Failed -gt 0) { exit 1 }
exit 0
