#!/usr/bin/env pwsh
# A pane that runs a command must not throw the warm pool away.
#
# `new-window <cmd>` and `split-window <cmd>` cannot use a spare shell, so they
# cold spawn. They used to take a fresh pane id above every id the pooled
# spares had reserved (a spare's id is its shell's TMUX_PANE, fixed when it is
# spawned), so the id floor rule killed ALL the spares and the refill booted
# three new pwsh at the very moment the user's command was starting. On a quiet
# machine `new-window cmd /c <pwsh marker>` reached its marker ~560 ms after
# the CLI started with the pool on, ~360 ms with the pool off, behind Windows
# Terminal (~396 ms) and WezTerm (~440 ms) in tests\test_perf_vs_terminals.ps1
# section 3b.
#
# Now the cold pane takes the lowest reserved id, only the spare that held it
# is retired, and its single replacement waits until the new pane has started.
#
# Checked through the server's warm trace (PSMUX_WARM_TRACE) and the pane's
# own TMUX_PANE. Runs in its own -L namespace and data dir.

param([string]$Exe = $env:PSMUX_TEST_BIN)

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$script:Passed = 0
$script:Failed = 0
function Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }
function Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }

if (-not $Exe) { $Exe = (Get-Command psmux -ErrorAction SilentlyContinue).Source }
if (-not $Exe -or -not (Test-Path $Exe)) { Write-Host "[FATAL] psmux not found" -ForegroundColor Red; exit 1 }
Write-Host "  Binary: $Exe"

$ns = "tcoldpool" + [guid]::NewGuid().ToString("N").Substring(0, 8)
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux-" + $ns)
New-Item -ItemType Directory -Force $work | Out-Null
$saved = @{}
foreach ($k in "PSMUX_DATA_DIR", "PSMUX_WARM_TRACE", "PSMUX_WARM_TRACE_FILE", "PSMUX_NO_WARM", "PSMUX_WARM_POOL_SIZE") { $saved[$k] = [Environment]::GetEnvironmentVariable($k) }
$env:PSMUX_DATA_DIR = Join-Path $work "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$trace = Join-Path $work "warm_trace.log"
$env:PSMUX_WARM_TRACE = "1"
$env:PSMUX_WARM_TRACE_FILE = $trace
$env:PSMUX_NO_WARM = $null
$env:PSMUX_WARM_POOL_SIZE = $null

function Mux { & $Exe -L $ns @args 2>&1 }
# The standby `__warm__` server writes to the same trace file; only the
# session's own server matters, the one whose pool has a target of three.
$script:SrvPid = ""
function Trace-Lines {
    $all = if (Test-Path $trace) { @(Get-Content $trace) } else { @() }
    if (-not $script:SrvPid) {
        $hit = $all | Where-Object { $_ -match 'pid=(\d+)\] pool: sample depth=\d+ ready=\d+ inflight=\d+ target=3 ' } | Select-Object -First 1
        if ($hit -and $hit -match 'pid=(\d+)\]') { $script:SrvPid = $Matches[1] }
    }
    if ($script:SrvPid) { @($all | Where-Object { $_ -match "pid=$($script:SrvPid)\]" }) } else { $all }
}

# The pool is full and settled: three ready spares and nothing in flight.
function Wait-PoolSettled([int]$TimeoutS = 30) {
    $deadline = (Get-Date).AddSeconds($TimeoutS)
    while ((Get-Date) -lt $deadline) {
        $last = Trace-Lines | Where-Object { $_ -match 'pool: sample depth=' } | Select-Object -Last 1
        if ($last -match 'depth=3 ready=3 inflight=0') { return $true }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

# A pane command that writes its TMUX_PANE to a file and then waits.
$markerPs = Join-Path $work "marker.ps1"
@'
param([string]$Out)
[IO.File]::WriteAllText($Out, "$env:TMUX_PANE")
Start-Sleep -Seconds 600
'@ | Set-Content -Path $markerPs -Encoding UTF8
function New-Marker([string]$Tag) {
    $m = Join-Path $work "m_$Tag.txt"
    $w = Join-Path $work "w_$Tag.cmd"
    "@echo off`r`npwsh -NoLogo -NoProfile -File `"$markerPs`" `"$m`"`r`n" | Set-Content -Path $w -Encoding ASCII
    return @{ Marker = $m; Wrapper = $w }
}
function Wait-File([string]$f, [int]$TimeoutMs = 20000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        if (Test-Path $f) { $t = (Get-Content $f -Raw -ErrorAction SilentlyContinue); if ($t) { return $t.Trim() } }
        Start-Sleep -Milliseconds 20
    }
    return $null
}

try {
    Mux new-session -d -s cold -x 120 -y 30 | Out-Null
    if (-not (Wait-PoolSettled 40)) { Fail "the spare pool never settled at three ready spares"; throw "setup" }
    Pass "pool settled at three ready spares"

    foreach ($kind in @("new-window", "split-window")) {
        if (-not (Wait-PoolSettled 30)) { Fail "$kind : pool did not settle before the creation"; continue }
        $before = (Trace-Lines).Count
        $spec = New-Marker ($kind -replace '\W', '')
        $sw = [Diagnostics.Stopwatch]::StartNew()
        if ($kind -eq "new-window") { Mux new-window -d -t cold cmd /c $spec.Wrapper | Out-Null }
        else { Mux split-window -d -h -t cold:0 cmd /c $spec.Wrapper | Out-Null }
        $tmuxPane = Wait-File $spec.Marker
        $ms = $sw.Elapsed.TotalMilliseconds
        if (-not $tmuxPane) { Fail "$kind : the command never ran"; continue }
        Info ("$kind : command running {0:F0} ms after the CLI started, TMUX_PANE={1}" -f $ms, $tmuxPane)
        Start-Sleep -Milliseconds 2500
        $new = @(Trace-Lines | Select-Object -Skip $before)

        $discard = @($new | Where-Object { $_ -match 'discarded (\d+) spare' })
        if ($discard.Count -eq 0) { Pass "$kind : no spare discarded by an id floor raise" }
        else { Fail "$kind : spares discarded: $($discard -join ' | ')" }

        $took = @($new | Where-Object { $_ -match 'cold spawn takes reserved id (\d+)' })
        if ($took.Count -eq 1 -and $tmuxPane -eq ('%' + ([regex]::Match($took[0], 'reserved id (\d+)').Groups[1].Value))) {
            Pass "$kind : the cold pane took the pool front's id and its shell sees it as TMUX_PANE ($tmuxPane)"
        } else { Fail "$kind : expected one reserved id hand over matching TMUX_PANE $tmuxPane, got: $($took -join ' | ')" }

        $refills = @($new | Where-Object { $_ -match 'refill scheduled' })
        if ($refills.Count -le 1) { Pass "$kind : at most one replacement spare spawned ($($refills.Count))" }
        else { Fail "$kind : $($refills.Count) replacement spares spawned: $($refills -join ' | ')" }

        # The replacement must not boot beside the command: it is scheduled
        # only after the hold on the new pane let go.
        $iRel = [array]::FindIndex([string[]]$new, [Predicate[string]]{ param($l) $l -match 'boot hold released' })
        $iRef = [array]::FindIndex([string[]]$new, [Predicate[string]]{ param($l) $l -match 'refill scheduled' })
        if ($iRef -lt 0 -or ($iRel -ge 0 -and $iRel -lt $iRef)) { Pass "$kind : the replacement waited for the new pane to start" }
        else { Fail "$kind : the replacement was scheduled while the new pane was still starting" }

        # The pane ids still come out in creation order: the next default shell
        # gets an id above the command pane's.
        $ids = @(Mux list-panes -a -F '#{pane_id}' | ForEach-Object { [int]("$_".TrimStart('%')) })
        Mux new-window -d -t cold | Out-Null
        Start-Sleep -Milliseconds 400
        $ids2 = @(Mux list-panes -a -F '#{pane_id}' | ForEach-Object { [int]("$_".TrimStart('%')) })
        $added = @($ids2 | Where-Object { $ids -notcontains $_ })
        $cmdId = [int]$tmuxPane.TrimStart('%')
        if ($added.Count -eq 1 -and $added[0] -gt $cmdId) { Pass "$kind : the next pane is %$($added[0]), after the command pane's $tmuxPane" }
        else { Fail "$kind : the next pane id went backwards or is missing (command $tmuxPane, added $($added -join ','))" }
    }
} catch {
    if ("$_" -ne "setup") { Fail "unexpected error: $_" }
} finally {
    Mux kill-server | Out-Null
    foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "  Passed: $script:Passed  Failed: $script:Failed"
if ($script:Failed -gt 0) { exit 1 }
exit 0
