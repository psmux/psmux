# probe_client_cpu_per_key.ps1 - how much CPU does ONE keystroke cost the
# psmux client and server, with the idle cost taken out?
#
# WHY
# The trend row "cpu per 100 keystrokes, srv+cli" (perf_summary -Metric T)
# reads 820 to 2110 ms, and the typing benchmark on 2026-11-10 put the client at
# 875 ms and the server at 426 ms per 100 typed characters. Those numbers divide
# ALL the CPU spent during a typing window by the keys typed, so they also carry
# whatever the client and server burn while nothing is typed at all (timer
# ticks, status line redraws, polling), plus the send-keys clears between
# sentences. This probe separates the two:
#
#   idle rate    CPU per second over -IdleSeconds with nothing typed
#   typing       CPU over a window in which -Keys characters are typed at
#                -IntervalMs apart into the attached client's own console
#   per key      (typing CPU - idle rate x typing window) / keys
#
# measured -Rounds times, with an idle window before EVERY typing window so a
# drifting idle rate is subtracted where it was measured. The keys go in with
# WriteConsoleInput through tests\typing_latency_probe.cs (type mode), the same
# injector the typing benchmark uses, so the path is the real one: client
# console input, client, socket, server, pty, shell echo, frame, client draw.
#
# RUN IT ON A QUIET MACHINE. CPU time is charged per process, so other load
# does not add to these numbers directly, but a loaded machine stretches every
# wait and changes how many frames the client draws per key. The probe samples
# the machine load and says so in its output and JSON when it was not quiet.
#
# ISOLATION: its own -L namespace and its own PSMUX_DATA_DIR; it kills only the
# client it started, by PID, and only its namespace with -L kill-server.
#
#   pwsh -NoProfile -File tests\probe_client_cpu_per_key.ps1
#   pwsh -NoProfile -File tests\probe_client_cpu_per_key.ps1 -Binary C:\scratch\pmux.exe -Rounds 5
#
# Writes client-cpu-per-key-<stamp>.json to the metrics folder (or
# PSMUX_METRICS_DIR). Not a test: it never fails, it measures.
param(
    [string]$Binary = "",
    [int]$Rounds = 3,
    [int]$Keys = 100,
    [int]$IntervalMs = 40,
    [int]$IdleSeconds = 10,
    [int]$SettleSeconds = 8,
    [double]$QuietLoadPct = 25.0
)

$ErrorActionPreference = "Continue"
. "$PSScriptRoot\perf_metrics_common.ps1"
function Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

if (-not $Binary) {
    $Binary = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA SilentlyContinue).Source }
}
if (-not $Binary -or -not (Test-Path $Binary)) { Write-Host "psmux binary not found ($Binary)" -ForegroundColor Red; exit 1 }
$Binary = (Resolve-Path $Binary).Path
Info "binary: $Binary"
foreach ($v in @('PSMUX_SESSION', 'PSMUX_SESSION_NAME', 'PSMUX_SOCKET', 'PSMUX_PANE_ID', 'PSMUX_PTY_TRACE', 'TMUX', 'TMUX_PANE', 'SHELL')) {
    Remove-Item "Env:\$v" -EA SilentlyContinue
}

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$probeSrc = Join-Path $PSScriptRoot "typing_latency_probe.cs"
$probe = Join-Path $env:TEMP "psmux_typing_latency_probe.exe"
if ((-not (Test-Path $probe)) -or ((Get-Item $probeSrc).LastWriteTime -gt (Get-Item $probe).LastWriteTime)) {
    & $csc /nologo /optimize "/out:$probe" $probeSrc | Out-Null
}
if (-not (Test-Path $probe)) { Write-Host "could not build typing_latency_probe.exe" -ForegroundColor Red; exit 1 }

# Lower case letters and spaces only, no word PSReadLine would colour or
# predict differently from round to round.
$alpha = "abcdefghijklmnopqrstuvwxyz"
$text = -join (0..($Keys - 1) | ForEach-Object { if ($_ % 8 -eq 7) { " " } else { $alpha[$_ % 26] } })

$NS = "cpukey$PID"
$S = "cpu"
$DATA = Join-Path $env:TEMP "psmux_cpukey_$PID"
New-Item -ItemType Directory -Force -Path $DATA | Out-Null
$prevDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = $DATA

$roundRows = @()
$client = $null
$srvPid = 0
Add-PerfLoadSample "start" | Out-Null
try {
    $client = Start-Process -FilePath $Binary -ArgumentList @("-L", $NS, "new-session", "-s", $S) -PassThru
    Info "client pid $($client.Id), namespace $NS"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $ready = $false
    while ($sw.Elapsed.TotalSeconds -lt 30) {
        Start-Sleep -Milliseconds 500
        $cap = & $Binary -L $NS capture-pane -t $S -p 2>&1 | Out-String
        if ($cap -match "PS [A-Z]:\\|>\s*$") { $ready = $true; break }
    }
    if (-not $ready) { throw "the pane never showed a prompt" }
    $srvPid = Get-PerfServerPid -Ns $NS -Session $S -DataDir $DATA
    if ($srvPid -le 0) { throw "the server's pid anchor was not found" }
    $roles = [ordered]@{ server = $srvPid; client = $client.Id }
    # The warm pool fills right after the first prompt; that is not idle.
    Info "settling $SettleSeconds s so the warm pool has filled"
    Start-Sleep -Seconds $SettleSeconds

    for ($r = 1; $r -le $Rounds; $r++) {
        & $Binary -L $NS send-keys -t $S C-c 2>&1 | Out-Null
        Start-Sleep -Milliseconds 300
        & $Binary -L $NS send-keys -t $S "clear" Enter 2>&1 | Out-Null
        Start-Sleep -Seconds 2

        $idle = Start-PerfScenario -Name "idle" -Roles $roles
        Start-Sleep -Seconds $IdleSeconds
        $idleRes = Complete-PerfScenario $idle

        $typ = Start-PerfScenario -Name "typing" -Roles $roles
        $out = @(& $probe type "$($client.Id)" $text "$IntervalMs" 2>&1 | ForEach-Object { $_.ToString() })
        $typRes = Complete-PerfScenario $typ -Units $Keys
        $seen = @($out | Where-Object { $_ -match '^LAT ' }).Count

        $row = [ordered]@{ round = $r; key_count = $Keys; seen = $seen; idle_window_ms = $idleRes.wall_ms; typing_window_ms = $typRes.wall_ms }
        foreach ($role in @("server", "client")) {
            if (-not $idleRes.cpu_ms.Contains($role) -or -not $typRes.cpu_ms.Contains($role)) { continue }
            $idleRate = [double]$idleRes.cpu_ms[$role] / [math]::Max(1.0, $idleRes.wall_ms)
            $typCpu = [double]$typRes.cpu_ms[$role]
            $excess = $typCpu - $idleRate * $typRes.wall_ms
            $row["${role}_idle_cpu_pct_of_core"] = [math]::Round($idleRate * 100.0, 2)
            $row["${role}_typing_cpu_ms"] = [math]::Round($typCpu, 1)
            $row["${role}_raw_cpu_ms_per_key"] = [math]::Round($typCpu / $Keys, 3)
            $row["${role}_cpu_ms_per_key"] = [math]::Round($excess / $Keys, 3)
        }
        $row["handles_after"] = [ordered]@{ server = $typRes.after.server.handles; client = $typRes.after.client.handles }
        $roundRows += $row
        Write-Host ("  round {0}: seen {1}/{2}  client idle {3}% of a core, per key {4} ms (raw {5})  |  server idle {6}%, per key {7} ms (raw {8})" -f `
            $r, $seen, $Keys, $row.client_idle_cpu_pct_of_core, $row.client_cpu_ms_per_key, $row.client_raw_cpu_ms_per_key, `
            $row.server_idle_cpu_pct_of_core, $row.server_cpu_ms_per_key, $row.server_raw_cpu_ms_per_key)
        Add-PerfLoadSample "round $r" | Out-Null
    }
} catch {
    Write-Host "  [ERROR] $_" -ForegroundColor Red
} finally {
    & $Binary -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    if ($client) { try { if (-not $client.HasExited) { Stop-Process -Id $client.Id -Force -EA SilentlyContinue } } catch { } }
    if ($null -eq $prevDataDir) { Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue } else { $env:PSMUX_DATA_DIR = $prevDataDir }
    Remove-Item -Recurse -Force $DATA -EA SilentlyContinue
}

$summary = [ordered]@{}
foreach ($k in @("client_cpu_ms_per_key", "server_cpu_ms_per_key", "client_raw_cpu_ms_per_key", "server_raw_cpu_ms_per_key", "client_idle_cpu_pct_of_core", "server_idle_cpu_pct_of_core")) {
    $summary[$k] = Get-PerfStats @($roundRows | ForEach-Object { $_[$k] }) 3
}
$quiet = Test-PerfMachineQuiet $QuietLoadPct
$load = Get-PerfLoadSummary
Write-Host ""
Write-Host ("  per key, idle subtracted (median of {0}): client {1} ms, server {2} ms" -f $roundRows.Count, $summary.client_cpu_ms_per_key.p50, $summary.server_cpu_ms_per_key.p50) -ForegroundColor White
Write-Host ("  idle, % of one core (median): client {0}, server {1}" -f $summary.client_idle_cpu_pct_of_core.p50, $summary.server_idle_cpu_pct_of_core.p50) -ForegroundColor White
if (-not $quiet) {
    Write-Host ("  [WARN] machine load p50 {0}% of total cpu is over {1}%: these numbers are not a quiet box reading" -f $load.p50_pct, $QuietLoadPct) -ForegroundColor Yellow
}
$jsonPath = Write-PerfMetrics -Suite "probe_client_cpu_per_key" -Binary $Binary -FileStem "client-cpu-per-key" -Data ([ordered]@{
    key_count = $Keys; interval_ms = $IntervalMs; idle_seconds = $IdleSeconds; settle_seconds = $SettleSeconds
    machine_was_quiet = $quiet
    rounds = $roundRows
    summary = $summary
})
if ($jsonPath) { Info "metrics: $jsonPath" }
exit 0
