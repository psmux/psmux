# Real client status-bar rendering and mouse clicks. Run only in a disposable VM:
# $env:PSMUX_ISOLATED_VM = '1'; $env:PSMUX_EXE = <build>; ./tests/test_status_window_overflow.ps1
$ErrorActionPreference = 'Stop'
if ($env:PSMUX_ISOLATED_VM -ne '1') { throw 'This integration test requires a disposable VM (PSMUX_ISOLATED_VM=1).' }
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -ErrorAction Stop).Source }
$root = Join-Path $env:TEMP ('status-overflow-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $root | Out-Null
$namespace = Split-Path $root -Leaf
$csc = 'C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe'
$reader = Join-Path $root 'conread.exe'
$clicker = Join-Path $root 'click.exe'
foreach ($pair in @(@($reader, 'conread.cs'), @($clicker, 'click_injector.cs'))) {
    & $csc /nologo "/out:$($pair[0])" (Join-Path $PSScriptRoot $pair[1])
    if ($LASTEXITCODE -ne 0) { throw "Cannot compile $($pair[1])" }
}
function Mux {
    $output = & $PSMUX -L $namespace @args 2>&1
    if ($LASTEXITCODE -ne 0) { throw "psmux $args failed: $output" }
    ($output -join "`n").Trim()
}
function Wait-For([scriptblock] $Condition, [string] $Label) {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        if (& $Condition) { return }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Timed out: $Label. Evidence: $root"
}
function Screen {
    $rows = @(& $reader $script:clientId -u -g)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read the attached client screen' }
    return ,$rows
}
function Status {
    $rows = Screen
    if ($script:position -eq 'top') { return [string]$rows[0] }
    return [string]$rows[-1]
}
$launcher = $null
$script:clientId = $null
try {
    foreach ($width in @(80, 120)) {
        $script:position = 'bottom'
        $config = Join-Path $root 'fixture.conf'
        @(
            'set -g default-shell pwsh', 'set -g mouse on', 'set -g base-index 1',
            'set -g status-left "LEFT "', 'set -g status-right " RIGHT"',
            'set -g window-status-format "#I:#W "',
            'set -g window-status-current-format "#[reverse][#I:#W] #[default]"'
        ) | Set-Content $config -Encoding utf8
        $launch = Join-Path $root 'launch.cmd'
        @"
@echo off
set TMUX=
set TMUX_PANE=
set PSMUX_SESSION=
set PSMUX_TARGET_SESSION=
set SSH_CONNECTION=
set SSH_CLIENT=
set SSH_TTY=
set PSMUX_VT_INPUT=
set PSMUX_NO_WARM=1
mode con: cols=$width lines=24
"$PSMUX" -L $namespace -f "$config" new-session -s validation -n W01-long-name -- pwsh -NoProfile -Command "Start-Sleep 3600"
"@ | Set-Content $launch -Encoding ascii
        $launcher = Start-Process -FilePath $launch -PassThru
        Wait-For {
            $candidate = Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
                Where-Object { $_.CommandLine -like "*-L $namespace *new-session*" } |
                Select-Object -First 1
            if ($candidate) { $script:clientId = [int]$candidate.ProcessId; return $true }
            return $false
        } 'attached client starts'
        Wait-For { (Status).Contains('W01-long-name') } 'first tab drawn'
        if ((Status).Contains('<') -or (Status).Contains('>')) { throw 'A fitting list must not have overflow markers' }
        for ($n = 2; $n -le 12; $n++) {
            Mux new-window -n ('W{0:00}-long-name' -f $n) -- pwsh -NoProfile -Command 'Start-Sleep 3600' | Out-Null
        }
        if ((Mux list-windows -F '#{window_index}').Split("`n").Count -ne 12) { throw 'Fixture requires 12 windows' }
        if ((Mux show-options -g status-format) -ne 'status-format') { throw 'Test must exercise the default renderer' }
        foreach ($script:position in @('bottom', 'top')) {
            Mux set-option -g status-position $script:position | Out-Null
            foreach ($justify in @('left', 'centre', 'right', 'absolute-centre')) {
                Mux set-option -g status-justify $justify | Out-Null
                foreach ($active in @(1, 6, 12)) {
                    Mux select-window -t ":$active" | Out-Null
                    $name = 'W{0:00}-long-name' -f $active
                    Wait-For {
                        $line = Status
                        $markersCorrect = switch ($active) {
                            1 { -not $line.Contains('<') -and $line.Contains('>') }
                            12 { $line.Contains('<') -and -not $line.Contains('>') }
                            default { $line.Contains('<') -or $line.Contains('>') }
                        }
                        $line.StartsWith('LEFT ') -and $line.EndsWith(' RIGHT') -and
                            $line.Contains("[${active}:$name]") -and $markersCorrect
                    } "$width/$position/$justify active $active visible with markers and both sides"
                    $line = Status
                    $line | Set-Content (Join-Path $root "$width-$position-$justify-$active.txt")
                    Write-Host "PASS $width/$position/$justify active $active : $line"
                }
                # Locate an inactive visible tab on the real screen; its cell must
                # select that window even though the list is scrolled away from 1.
                Mux select-window -t ':6' | Out-Null
                Wait-For { (Status).Contains('[6:W06-long-name]') } 'middle tab drawn before click'
                $line = Status
                $tab = [regex]::Matches($line, '(\d+):W(\d{2})-long-name') |
                    Where-Object {
                        [int]$_.Groups[1].Value -ne 6 -and
                        [int]$_.Groups[1].Value -eq [int]$_.Groups[2].Value
                    } | Select-Object -First 1
                if (-not $tab) { throw "No inactive visible tab to click: $line" }
                $row = if ($script:position -eq 'top') { 0 } else { (Screen).Count - 1 }
                & $clicker $script:clientId $tab.Index $row 80
                if ($LASTEXITCODE -ne 0) { throw 'Mouse injection failed' }
                $target = $tab.Groups[1].Value
                Wait-For { (Mux display-message -p '#{window_index}') -eq $target } "click selects visible window $target"
                Write-Host "PASS $width/$position/$justify scrolled-tab click selects $target"
            }
        }
        Mux kill-server | Out-Null
        if ($launcher -and -not $launcher.HasExited) { Stop-Process -Id $launcher.Id -Force }
        $launcher = $null
        $script:clientId = $null
    }
    Write-Host "PASS status overflow integration. Screen evidence: $root"
} finally {
    # Startup can create the server before client-process detection succeeds.
    $PSNativeCommandUseErrorActionPreference = $false
    $cleanupOutput = & $PSMUX -L $namespace kill-server 2>&1
    $cleanupFailed = $LASTEXITCODE -ne 0 -and "$cleanupOutput" -notmatch 'no server running'
    if ($launcher -and -not $launcher.HasExited) { Stop-Process -Id $launcher.Id -Force }
    if ($cleanupFailed) { throw "Namespace cleanup failed: $cleanupOutput" }
}
