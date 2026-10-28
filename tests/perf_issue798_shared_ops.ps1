# Issue #798: what the everyday operations cost in a shared server against one
# server per session. Not a gate: it measures and writes the numbers.
#
# For each mode, in a fresh -L namespace and PSMUX_DATA_DIR, with session A alive:
#   * new-session -d (a new session each run, killed again after the sample;
#     a pause between runs lets the per session warm standby refill, so the
#     default mode is measured as users see it);
#   * new-window -d -t A;
#   * split-window -d into a fresh window of A (so panes never get too small);
#   * send-keys echo: send-keys 'echo MARK' Enter into A's first pane, then
#     capture-pane until MARK is on screen (the poll itself is a CLI call, so
#     this is CLI to screen, with a granularity of one capture-pane call).
#
# Metrics JSON: $env:USERPROFILE\.psmux-test-data\metrics\ (perf_metrics_common).
# Cleans up only with `-L <ns> kill-server`. PSMUX_TEST_EXE picks the binary.

param([int]$Runs = 24)
$ErrorActionPreference = "Continue"
$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $c = Get-Command psmux -EA SilentlyContinue; if ($c) { $PSMUX = $c.Source } }
$PSMUX = (Resolve-Path $PSMUX).Path
foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_TARGET_SESSION', 'TMUX', 'TMUX_PANE', 'PSMUX_SERVER_MODE') { Remove-Item "Env:$v" -EA SilentlyContinue }
$CMD = Join-Path $env:SystemRoot "System32\cmd.exe"
. "$PSScriptRoot\isolated_config.ps1" -Lines @("set -g default-shell $CMD")
. "$PSScriptRoot\perf_metrics_common.ps1"

$ROOT = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_798o_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $ROOT | Out-Null
$namespaces = @()

function Ms([scriptblock]$b) { $sw = [Diagnostics.Stopwatch]::StartNew(); & $b | Out-Null; [math]::Round($sw.Elapsed.TotalMilliseconds, 1) }

$raw = [ordered]@{}
try {
    foreach ($mode in 'per-session', 'shared') {
        $ns = "p798o" + [guid]::NewGuid().ToString('N').Substring(0, 6)
        $dir = Join-Path $ROOT $ns; New-Item -ItemType Directory -Force $dir | Out-Null
        $namespaces += , @($ns, $dir)
        $env:PSMUX_DATA_DIR = $dir
        if ($mode -eq 'shared') { $env:PSMUX_SERVER_MODE = 'shared' } else { Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue }
        & $PSMUX -L $ns new-session -d -s A -x 160 -y 50 2>&1 | Out-Null
        Start-Sleep -Seconds 2
        $r = [ordered]@{ new_session = @(); new_window = @(); split_window = @(); send_keys_echo = @() }
        for ($i = 0; $i -lt $Runs; $i++) {
            $r.new_session += Ms { & $PSMUX -L $ns new-session -d -s "N$i" -x 120 -y 30 2>&1 }
            Start-Sleep -Milliseconds 300
            & $PSMUX -L $ns kill-session -t "N$i" 2>&1 | Out-Null
            Start-Sleep -Milliseconds 1200
        }
        for ($i = 0; $i -lt $Runs; $i++) {
            $r.new_window += Ms { & $PSMUX -L $ns new-window -d -t A: 2>&1 }
        }
        for ($i = 0; $i -lt $Runs; $i++) {
            & $PSMUX -L $ns new-window -d -t A: -n "S$i" 2>&1 | Out-Null
            $r.split_window += Ms { & $PSMUX -L $ns split-window -d -t "A:S$i" 2>&1 }
        }
        Start-Sleep -Seconds 1
        for ($i = 0; $i -lt $Runs; $i++) {
            & $PSMUX -L $ns send-keys -t A:0.0 'cls' Enter 2>&1 | Out-Null
            Start-Sleep -Milliseconds 300
            $mark = "MK$i" + "Z"
            $sw = [Diagnostics.Stopwatch]::StartNew()
            & $PSMUX -L $ns send-keys -t A:0.0 "echo $mark" Enter 2>&1 | Out-Null
            $seen = $false
            while ($sw.ElapsedMilliseconds -lt 5000) {
                $cap = (& $PSMUX -L $ns capture-pane -p -t A:0.0 2>&1) -join "`n"
                # the echoed line, not the typed command line
                if ($cap -match "(?m)^$mark\s*$") { $seen = $true; break }
            }
            if ($seen) { $r.send_keys_echo += [math]::Round($sw.Elapsed.TotalMilliseconds, 1) }
        }
        $raw[$mode] = $r
        $line = foreach ($k in $r.Keys) { $s = Get-PerfStats @($r[$k]); "{0} n={1} p50={2} p90={3} p99={4}" -f $k, $s.n, $s.p50, $s.p90, $s.p99 }
        Write-Host ("{0,-11}: {1}" -f $mode, ($line -join '; ')) -ForegroundColor Cyan
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
        Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue
        Start-Sleep -Milliseconds 500
    }
} finally {
    Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue
    foreach ($pair in $namespaces) { $env:PSMUX_DATA_DIR = $pair[1]; & $PSMUX -L $pair[0] kill-server 2>&1 | Out-Null }
    Remove-Item -Recurse -Force $ROOT -EA SilentlyContinue
    if (Get-Command Remove-PsmuxIsolatedConfig -EA SilentlyContinue) { Remove-PsmuxIsolatedConfig }
}

$summary = [ordered]@{}
foreach ($mode in $raw.Keys) {
    $summary[$mode] = [ordered]@{}
    foreach ($k in $raw[$mode].Keys) { $summary[$mode][$k] = Get-PerfStats @($raw[$mode][$k]) }
}
$path = Write-PerfMetrics -Suite "perf_issue798_shared_ops" -Binary $PSMUX -Data ([ordered]@{ runs = $Runs; summary = $summary; raw = $raw })
Write-Host "metrics: $path"
