# test_extreme_perf.ps1 — Extreme-scale pane/window performance benchmark
# Opens hundreds of panes and windows and measures time to PS prompt.
#
# Metrics collected:
#   1. Baseline: raw pwsh startup time (no psmux)
#   2. Server cold start latency
#   3. Sequential window creation (100 windows) — per-window time + cumulative
#   4. Burst window creation (50 at once) — throughput
#   5. Split pane scaling (max splits in one window)
#   6. Mixed: 20 windows x 5 splits = 100 panes — total time
#   7. Prompt-ready latency percentiles (p50, p90, p99)
#   8. Command round-trip latency under load
#   9. Memory/handle overhead per pane (via process inspection)

param(
    [int]$SequentialWindows = 100,
    [int]$BurstWindows      = 50,
    [int]$MixedWindows      = 20,
    [int]$MixedSplits       = 5,
    [int]$PromptTimeoutMs   = 45000,
    [int]$BurstDelayMs      = 0,
    [switch]$SkipPromptCheck,
    [switch]$Verbose
)

$ErrorActionPreference = "Continue"
. "$PSScriptRoot\perf_metrics_common.ps1"
# The build in this tree first, then PSMUX_TEST_BIN, then the psmux on PATH.
$PSMUX = Join-Path $PSScriptRoot "..\target\release\psmux.exe"
if (-not (Test-Path $PSMUX)) {
    $PSMUX = Join-Path $PSScriptRoot "..\target\release\tmux.exe"
}
if (-not (Test-Path $PSMUX) -and $env:PSMUX_TEST_BIN) { $PSMUX = $env:PSMUX_TEST_BIN }
if (-not (Test-Path $PSMUX)) { $c = Get-Command psmux -ErrorAction SilentlyContinue; if ($c) { $PSMUX = $c.Source } }
if (-not (Test-Path $PSMUX)) {
    Write-Host "ERROR: no psmux binary (target\release, PSMUX_TEST_BIN or PATH)" -ForegroundColor Red
    exit 1
}
$PSMUX = (Resolve-Path $PSMUX).Path

# ISOLATION. Everything this suite starts lives in its own -L namespace AND its
# own data directory, so it can run on a machine with live sessions. It used to
# run a bare kill-server (the default namespace), Stop-Process every psmux on
# the machine by image name, and delete every *.port and *.key in ~/.psmux.
# Cleanup now ends only this namespace, and then only processes running THIS
# binary with this namespace on their command line, by PID.
$NS = "xperf$PID"
$DataDir = Join-Path $env:TEMP "psmux_xperf_$PID"
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
$prevDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = $DataDir
$env:PSMUX_SESSION_NAME = $null

$PASS = 0; $FAIL = 0; $TOTAL = 0
function Pass($msg) { $script:PASS++; $script:TOTAL++; Write-Host "  [PASS] $msg" -ForegroundColor Green }
function Fail($msg) { $script:FAIL++; $script:TOTAL++; Write-Host "  [FAIL] $msg" -ForegroundColor Red }
function Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor Gray }
function Metric($label, $ms) {
    $c = if ($ms -lt 2000) { "Green" } elseif ($ms -lt 5000) { "Yellow" } else { "Red" }
    Write-Host ("  {0,-55} {1,8:N0} ms" -f $label, $ms) -ForegroundColor $c
}
function Cleanup {
    & $PSMUX -L $NS -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    # Stragglers of THIS namespace only (exact binary path and -L $NS on the
    # command line), by PID. Never by image name.
    foreach ($p in (Get-PerfNamespacePids -Binary $PSMUX -Ns $NS)) {
        try { Stop-Process -Id $p -Force -ErrorAction SilentlyContinue } catch {}
    }
    Start-Sleep -Milliseconds 500
    # Only this suite's own data directory is cleared.
    Get-ChildItem "$DataDir\*.port" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
    Get-ChildItem "$DataDir\*.key"  -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
}

function Wait-Prompt {
    param([string]$Target, [int]$Timeout = $PromptTimeoutMs)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $Timeout) {
        try {
            $cap = & $PSMUX -L $NS capture-pane -t $Target -p 2>&1 | Out-String
            # Detect PS prompt: standard "PS C:\", oh-my-posh "❯", or starship "$"
            #
            # TEST 4 (split-pane scaling) drives panes down to 3 rows tall
            # by width ~14-15 cols. At that size the prompt line
            # "PS C:\Users\...\...>" line-wraps to more rows than the pane
            # is tall, so the TOP wrapped segment (containing the literal
            # "PS C:\" prefix every other check here looks for) scrolls out
            # of the visible capture -- confirmed live: `capture-pane -p` on
            # a 14x3 pane showed only the tail, e.g. "dwin\Documents /
            # \workspace\psm / ux>", with "PS C:\" nowhere in the visible
            # text even though the shell is fully alive and correctly
            # rendering its prompt. That is real terminal line-wrap
            # behavior (any shell/terminal would do the same at that size),
            # not a psmux defect, so also accept a trailing '>' on the last
            # non-blank line -- the one fragment of a wrapped PS/cmd prompt
            # that survives regardless of how many rows scrolled away.
            $lastLine = ($cap -split "`r?`n" | Where-Object { $_.Trim() -ne "" } | Select-Object -Last 1)
            if ($cap -match "PS [A-Z]:\\" -or $cap -match "\xE2\x9D\xAF" -or $cap -match "❯" -or ($cap -match "@" -and $cap.Trim().Length -gt 5) -or ($lastLine -and $lastLine.TrimEnd().EndsWith(">"))) {
                return @{ Found = $true; ElapsedMs = $sw.ElapsedMilliseconds }
            }
        } catch {}
        Start-Sleep -Milliseconds 100
    }
    return @{ Found = $false; ElapsedMs = $sw.ElapsedMilliseconds }
}

function Wait-ServerReady {
    param([string]$Session, [int]$TimeoutMs = 15000)
    $pf = Join-Path $DataDir "${NS}__${Session}.port"
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        if (Test-Path $pf) {
            $port = [int](Get-Content $pf -Raw).Trim()
            if ($port -gt 0) { return @{ Port = $port; ElapsedMs = $sw.ElapsedMilliseconds } }
        }
        Start-Sleep -Milliseconds 25
    }
    return $null
}

function Percentile($arr, $pct) {
    if ($arr.Count -eq 0) { return 0 }
    $sorted = $arr | Sort-Object
    $idx = [Math]::Floor(($pct / 100.0) * ($sorted.Count - 1))
    return $sorted[$idx]
}

# Working set of this namespace's session server, found by its pid anchor. One
# session runs at a time here, and the warm standby has no anchor of that kind.
function Get-PsmuxMemory {
    foreach ($f in @(Get-ChildItem (Join-Path $DataDir "${NS}__*.pid") -ErrorAction SilentlyContinue)) {
        $sess = $f.BaseName.Substring($NS.Length + 2)
        if ($sess -like "__warm__*") { continue }
        $sp = Get-PerfServerPid -Ns $NS -Session $sess -DataDir $DataDir
        if ($sp -gt 0) {
            $proc = Get-Process -Id $sp -ErrorAction SilentlyContinue
            if ($proc) { return [Math]::Round($proc.WorkingSet64 / 1MB, 1) }
        }
    }
    return 0
}

# ═══════════════════════════════════════════════════════════════════════════
Write-Host ""
Write-Host ("=" * 78) -ForegroundColor Cyan
Write-Host "  PSMUX EXTREME PERFORMANCE BENCHMARK" -ForegroundColor Cyan
Write-Host "  $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX" -ForegroundColor Cyan
Write-Host ("=" * 78) -ForegroundColor Cyan
Write-Host ""

$results = @{}
Cleanup

# ═══════════════════════════════════════════════════════════════════════════
# TEST 0: BASELINE — raw pwsh startup time
# ═══════════════════════════════════════════════════════════════════════════
Write-Host ("─" * 78) -ForegroundColor DarkGray
Write-Host "  TEST 0: BASELINE — raw pwsh startup" -ForegroundColor Yellow
Write-Host ("─" * 78) -ForegroundColor DarkGray

$baseTimes = @()
for ($i = 0; $i -lt 5; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $r = & pwsh -NoLogo -NoProfile -Command "Write-Output 'RDY'" 2>&1 | Out-String
    $sw.Stop()
    if ($r -match "RDY") { $baseTimes += $sw.ElapsedMilliseconds }
}
$baseAvg = [Math]::Round(($baseTimes | Measure-Object -Average).Average, 0)
Metric "pwsh -NoProfile avg (5 runs)" $baseAvg
$results["baseline_noprofile_ms"] = $baseAvg

$profileTimes = @()
for ($i = 0; $i -lt 3; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $r = & pwsh -NoLogo -Command "Write-Output 'RDY'" 2>&1 | Out-String
    $sw.Stop()
    if ($r -match "RDY") { $profileTimes += $sw.ElapsedMilliseconds }
}
$profileAvg = [Math]::Round(($profileTimes | Measure-Object -Average).Average, 0)
Metric "pwsh (with profile) avg (3 runs)" $profileAvg
$results["baseline_profile_ms"] = $profileAvg
Write-Host ""

# ═══════════════════════════════════════════════════════════════════════════
# TEST 1: SERVER COLD START
# ═══════════════════════════════════════════════════════════════════════════
Write-Host ("─" * 78) -ForegroundColor DarkGray
Write-Host "  TEST 1: SERVER COLD START" -ForegroundColor Yellow
Write-Host ("─" * 78) -ForegroundColor DarkGray

Cleanup
$coldSw = [System.Diagnostics.Stopwatch]::StartNew()
Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -s xperf -d" -WindowStyle Hidden
$srv = Wait-ServerReady "xperf"
if ($srv) {
    Metric "Server ready (port file)" $srv.ElapsedMs
    $r = Wait-Prompt "xperf:0"
    $coldSw.Stop()
    if ($r.Found) {
        Metric "First pane PS prompt" $r.ElapsedMs
        Metric "Total cold start → prompt" $coldSw.ElapsedMilliseconds
        Pass "Cold start: server + first prompt in $($coldSw.ElapsedMilliseconds)ms"
        $results["cold_start_ms"] = $coldSw.ElapsedMilliseconds
    } else {
        Fail "First pane never got PS prompt"
    }
} else {
    Fail "Server never started"
}
$mem0 = Get-PsmuxMemory
Info "Memory after 1 pane: ${mem0} MB"
Write-Host ""

# ═══════════════════════════════════════════════════════════════════════════
# TEST 2: SEQUENTIAL WINDOW CREATION — $SequentialWindows windows
# ═══════════════════════════════════════════════════════════════════════════
Write-Host ("─" * 78) -ForegroundColor DarkGray
Write-Host "  TEST 2: SEQUENTIAL $SequentialWindows WINDOWS (command + prompt check)" -ForegroundColor Yellow
Write-Host ("─" * 78) -ForegroundColor DarkGray

Cleanup
Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -s seq -d" -WindowStyle Hidden
Start-Sleep -Seconds 3
$seqPrompt = Wait-Prompt "seq:0"
if (-not $seqPrompt.Found) { Fail "Initial prompt for seq session"; }

$cmdTimes = @()
$promptTimes = @()
$failedWindows = @()

$totalSw = [System.Diagnostics.Stopwatch]::StartNew()
for ($i = 1; $i -le $SequentialWindows; $i++) {
    $cmdSw = [System.Diagnostics.Stopwatch]::StartNew()
    & $PSMUX -L $NS new-window -t seq 2>&1 | Out-Null
    $cmdSw.Stop()
    $cmdTimes += $cmdSw.ElapsedMilliseconds

    if (-not $SkipPromptCheck) {
        $r = Wait-Prompt "seq:$i"
        if ($r.Found) {
            $promptTimes += $r.ElapsedMs
        } else {
            $failedWindows += $i
        }
    }

    if ($i % 25 -eq 0) {
        $mem = Get-PsmuxMemory
        $cmdAvg = [Math]::Round(($cmdTimes | Select-Object -Last 25 | Measure-Object -Average).Average, 0)
        $pAvg = if ($promptTimes.Count -gt 0) { [Math]::Round(($promptTimes | Select-Object -Last 25 | Measure-Object -Average).Average, 0) } else { "N/A" }
        Info "Window $i/$SequentialWindows — cmd avg: ${cmdAvg}ms, prompt avg: ${pAvg}ms, mem: ${mem}MB"
    }
}
$totalSw.Stop()

$cmdAvgAll = [Math]::Round(($cmdTimes | Measure-Object -Average).Average, 0)
$cmdMax = ($cmdTimes | Measure-Object -Maximum).Maximum
$cmdMin = ($cmdTimes | Measure-Object -Minimum).Minimum

Metric "new-window command avg" $cmdAvgAll
Metric "new-window command min" $cmdMin
Metric "new-window command max" $cmdMax
Metric "Total elapsed for $SequentialWindows windows" $totalSw.ElapsedMilliseconds

if ($promptTimes.Count -gt 0) {
    $pAvg = [Math]::Round(($promptTimes | Measure-Object -Average).Average, 0)
    $p50 = Percentile $promptTimes 50
    $p90 = Percentile $promptTimes 90
    $p99 = Percentile $promptTimes 99
    Metric "Prompt latency avg" $pAvg
    Metric "Prompt latency p50" $p50
    Metric "Prompt latency p90" $p90
    Metric "Prompt latency p99" $p99
    $results["seq_prompt_avg"] = $pAvg
    $results["seq_prompt_p50"] = $p50
    $results["seq_prompt_p90"] = $p90
    $results["seq_prompt_p99"] = $p99
}

$mem1 = Get-PsmuxMemory
Info "Memory after $SequentialWindows windows: ${mem1} MB"
$memPerPane = if ($SequentialWindows -gt 0 -and $mem1 -gt $mem0) { [Math]::Round(($mem1 - $mem0) / $SequentialWindows, 2) } else { 0 }
Info "Approx memory per window: ${memPerPane} MB"

if ($failedWindows.Count -eq 0) {
    Pass "All $SequentialWindows windows got PS prompts"
} else {
    Fail "$($failedWindows.Count) of $SequentialWindows windows failed to show prompt: $($failedWindows[0..([Math]::Min(9,$failedWindows.Count-1))] -join ',')"
}

$results["seq_cmd_avg"] = $cmdAvgAll
$results["seq_total_ms"] = $totalSw.ElapsedMilliseconds
$results["seq_mem_mb"] = $mem1
Write-Host ""

# ═══════════════════════════════════════════════════════════════════════════
# TEST 3: BURST WINDOW CREATION — fire $BurstWindows new-window commands, then check all
# ═══════════════════════════════════════════════════════════════════════════
Write-Host ("─" * 78) -ForegroundColor DarkGray
Write-Host "  TEST 3: BURST $BurstWindows WINDOWS (fire all, then verify)" -ForegroundColor Yellow
Write-Host ("─" * 78) -ForegroundColor DarkGray

Cleanup
Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -s burst -d" -WindowStyle Hidden
Start-Sleep -Seconds 3
Wait-Prompt "burst:0" | Out-Null

$burstSw = [System.Diagnostics.Stopwatch]::StartNew()
$burstCmdTimes = @()
for ($i = 1; $i -le $BurstWindows; $i++) {
    $sw2 = [System.Diagnostics.Stopwatch]::StartNew()
    & $PSMUX -L $NS new-window -t burst 2>&1 | Out-Null
    $sw2.Stop()
    $burstCmdTimes += $sw2.ElapsedMilliseconds
    if ($BurstDelayMs -gt 0) { Start-Sleep -Milliseconds $BurstDelayMs }
}
$burstCmdTotal = $burstSw.ElapsedMilliseconds
Info "All $BurstWindows new-window commands sent in ${burstCmdTotal}ms (avg: $([Math]::Round(($burstCmdTimes | Measure-Object -Average).Average, 0))ms)"

# Now verify all have prompts
$burstAlive = 0; $burstDead = 0; $burstPromptTimes = @()
for ($i = 0; $i -le $BurstWindows; $i++) {
    if (-not $SkipPromptCheck) {
        $r = Wait-Prompt "burst:$i"
        if ($r.Found) { $burstAlive++; $burstPromptTimes += $r.ElapsedMs }
        else { $burstDead++ }
    } else {
        $burstAlive++
    }
}
$burstSw.Stop()

Metric "Burst total (send + all prompts)" $burstSw.ElapsedMilliseconds
if ($burstPromptTimes.Count -gt 0) {
    Metric "Burst prompt avg" ([Math]::Round(($burstPromptTimes | Measure-Object -Average).Average, 0))
    Metric "Burst prompt p90" (Percentile $burstPromptTimes 90)
    Metric "Burst prompt max" ($burstPromptTimes | Measure-Object -Maximum).Maximum
}
$burstMem = Get-PsmuxMemory
Info "Memory: ${burstMem} MB | Alive: $burstAlive | Dead: $burstDead"
if ($burstDead -eq 0) { Pass "All $BurstWindows burst windows alive" }
else { Fail "$burstDead of $BurstWindows burst windows dead" }
$results["burst_total_ms"] = $burstSw.ElapsedMilliseconds
Write-Host ""

# ═══════════════════════════════════════════════════════════════════════════
# TEST 4: SPLIT PANE SCALING — max splits in one window
# ═══════════════════════════════════════════════════════════════════════════
Write-Host ("─" * 78) -ForegroundColor DarkGray
Write-Host "  TEST 4: SPLIT PANE SCALING (alternating V/H in one window)" -ForegroundColor Yellow
Write-Host ("─" * 78) -ForegroundColor DarkGray

Cleanup
Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -s splitx -d" -WindowStyle Hidden
Start-Sleep -Seconds 3
Wait-Prompt "splitx:0" | Out-Null

$splitOk = 0; $splitFail = 0; $splitTimes = @()
$maxSplits = 30  # Try up to 30 splits (31 panes total)
for ($i = 1; $i -le $maxSplits; $i++) {
    $flag = if ($i % 2 -eq 0) { "-h" } else { "-v" }
    $sw3 = [System.Diagnostics.Stopwatch]::StartNew()
    $out = & $PSMUX -L $NS split-window $flag -t splitx 2>&1 | Out-String
    $sw3.Stop()
    if ($LASTEXITCODE -ne 0 -or $out -match "too small") {
        Info "Split $i rejected (pane too small): $($out.Trim())"
        break
    }
    $splitTimes += $sw3.ElapsedMilliseconds
    $splitOk++
}

if ($splitTimes.Count -gt 0) {
    Metric "split-window command avg" ([Math]::Round(($splitTimes | Measure-Object -Average).Average, 0))
    Metric "split-window command max" ($splitTimes | Measure-Object -Maximum).Maximum
}
Info "Successful splits: $splitOk (total panes: $($splitOk + 1))"

# Check how many have prompts
$splitAlive = 0
for ($p = 0; $p -le $splitOk; $p++) {
    if (-not $SkipPromptCheck) {
        $r = Wait-Prompt "splitx:0.$p" -Timeout 20000
        if ($r.Found) { $splitAlive++ }
    } else { $splitAlive++ }
}
if ($splitAlive -eq ($splitOk + 1)) { Pass "All $splitAlive split panes have PS prompts" }
else { Fail "Only $splitAlive of $($splitOk + 1) split panes have prompts" }
$results["max_splits"] = $splitOk
Write-Host ""

# ═══════════════════════════════════════════════════════════════════════════
# TEST 5: MIXED WORKLOAD — $MixedWindows windows x $MixedSplits splits each
# ═══════════════════════════════════════════════════════════════════════════
$totalMixed = $MixedWindows * ($MixedSplits + 1)
Write-Host ("─" * 78) -ForegroundColor DarkGray
Write-Host "  TEST 5: MIXED $MixedWindows windows x $MixedSplits splits = $totalMixed panes" -ForegroundColor Yellow
Write-Host ("─" * 78) -ForegroundColor DarkGray

Cleanup
Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -s mixed -d" -WindowStyle Hidden
Start-Sleep -Seconds 3
Wait-Prompt "mixed:0" | Out-Null

$mixedSw = [System.Diagnostics.Stopwatch]::StartNew()
$windowCmds = @(); $splitCmds = @(); $mixedErrors = 0
for ($w = 1; $w -le $MixedWindows; $w++) {
    $sw4 = [System.Diagnostics.Stopwatch]::StartNew()
    & $PSMUX -L $NS new-window -t mixed 2>&1 | Out-Null
    $sw4.Stop()
    $windowCmds += $sw4.ElapsedMilliseconds

    for ($s = 0; $s -lt $MixedSplits; $s++) {
        $flag = if ($s % 2 -eq 0) { "-v" } else { "-h" }
        $sw5 = [System.Diagnostics.Stopwatch]::StartNew()
        $out = & $PSMUX -L $NS split-window $flag -t mixed 2>&1 | Out-String
        $sw5.Stop()
        if ($LASTEXITCODE -ne 0 -or $out -match "too small") {
            $mixedErrors++
            break
        }
        $splitCmds += $sw5.ElapsedMilliseconds
    }

    if ($w % 5 -eq 0) {
        $mem = Get-PsmuxMemory
        Info "Window $w/$MixedWindows — mem: ${mem}MB"
    }
}
$mixedCmdTotal = $mixedSw.ElapsedMilliseconds

Metric "All create commands sent" $mixedCmdTotal
if ($windowCmds.Count -gt 0) {
    Metric "new-window cmd avg" ([Math]::Round(($windowCmds | Measure-Object -Average).Average, 0))
}
if ($splitCmds.Count -gt 0) {
    Metric "split-window cmd avg" ([Math]::Round(($splitCmds | Measure-Object -Average).Average, 0))
}
if ($mixedErrors -gt 0) { Info "Split failures (too small): $mixedErrors" }

# Verify all panes
$mixedAlive = 0; $mixedDead = 0; $mixedPromptTimes = @()
$totalCreated = 1 + $MixedWindows * ($MixedSplits + 1) - $mixedErrors

# Check a sample (every Nth pane) to avoid waiting forever
$checkCount = [Math]::Min(50, $MixedWindows + 1)
$checkInterval = [Math]::Max(1, [Math]::Floor(($MixedWindows + 1) / $checkCount))
for ($w = 0; $w -le $MixedWindows; $w += $checkInterval) {
    if (-not $SkipPromptCheck) {
        $r = Wait-Prompt "mixed:$w"
        if ($r.Found) { $mixedAlive++; $mixedPromptTimes += $r.ElapsedMs }
        else { $mixedDead++ }
    } else { $mixedAlive++ }
}
$mixedSw.Stop()

Metric "Mixed total (create + sampled prompts)" $mixedSw.ElapsedMilliseconds
if ($mixedPromptTimes.Count -gt 0) {
    Metric "Mixed prompt avg (sampled)" ([Math]::Round(($mixedPromptTimes | Measure-Object -Average).Average, 0))
    Metric "Mixed prompt p90 (sampled)" (Percentile $mixedPromptTimes 90)
}

$mixedMem = Get-PsmuxMemory
Info "Memory: ${mixedMem}MB | Checked: $($mixedAlive + $mixedDead) | Alive: $mixedAlive | Dead: $mixedDead"
if ($mixedDead -eq 0) { Pass "All sampled mixed panes alive" }
else { Fail "$mixedDead of $($mixedAlive + $mixedDead) sampled panes dead" }
$results["mixed_total_ms"] = $mixedSw.ElapsedMilliseconds
$results["mixed_mem_mb"] = $mixedMem
Write-Host ""

# ═══════════════════════════════════════════════════════════════════════════
# TEST 6: COMMAND ROUND-TRIP LATENCY UNDER LOAD (100 panes)
# ═══════════════════════════════════════════════════════════════════════════
Write-Host ("─" * 78) -ForegroundColor DarkGray
Write-Host "  TEST 6: TCP ROUND-TRIP LATENCY UNDER LOAD" -ForegroundColor Yellow
Write-Host ("─" * 78) -ForegroundColor DarkGray

# Use the mixed session that still has many panes
$port = $null; $key = $null
try {
    $port = [int](Get-Content (Join-Path $DataDir "${NS}__mixed.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $DataDir "${NS}__mixed.key") -Raw).Trim()
} catch { Info "Could not read server port/key" }

if ($port -and $key) {
    $rtTimes = @()
    for ($i = 0; $i -lt 50; $i++) {
        try {
            $sw6 = [System.Diagnostics.Stopwatch]::StartNew()
            $client = New-Object System.Net.Sockets.TcpClient("127.0.0.1", $port)
            $stream = $client.GetStream()
            $stream.ReadTimeout = 5000
            $writer = New-Object System.IO.StreamWriter($stream)
            $reader = New-Object System.IO.StreamReader($stream)
            $writer.WriteLine("AUTH $key")
            $writer.Flush()
            $auth = $reader.ReadLine()
            $writer.WriteLine("list-windows")
            $writer.Flush()
            $resp = $reader.ReadToEnd()
            $client.Close()
            $sw6.Stop()
            $rtTimes += $sw6.ElapsedMilliseconds
        } catch {
            Info "TCP round-trip $i failed: $_"
        }
    }
    if ($rtTimes.Count -gt 0) {
        $rtAvg = [Math]::Round(($rtTimes | Measure-Object -Average).Average, 0)
        $rtP50 = Percentile $rtTimes 50
        $rtP90 = Percentile $rtTimes 90
        $rtMax = ($rtTimes | Measure-Object -Maximum).Maximum
        Metric "list-windows RTT avg (50 calls)" $rtAvg
        Metric "list-windows RTT p50" $rtP50
        Metric "list-windows RTT p90" $rtP90
        Metric "list-windows RTT max" $rtMax
        $results["rtt_avg_ms"] = $rtAvg
        $results["rtt_p90_ms"] = $rtP90
    }

    # dump-state latency
    $dsTimes = @()
    for ($i = 0; $i -lt 20; $i++) {
        try {
            $sw7 = [System.Diagnostics.Stopwatch]::StartNew()
            $client = New-Object System.Net.Sockets.TcpClient("127.0.0.1", $port)
            $stream = $client.GetStream()
            $stream.ReadTimeout = 10000
            $writer = New-Object System.IO.StreamWriter($stream)
            $reader = New-Object System.IO.StreamReader($stream)
            $writer.WriteLine("AUTH $key")
            $writer.Flush()
            $reader.ReadLine() | Out-Null
            $writer.WriteLine("dump-state")
            $writer.Flush()
            $resp = $reader.ReadToEnd()
            $client.Close()
            $sw7.Stop()
            $dsTimes += $sw7.ElapsedMilliseconds
            if ($i -eq 0) { Info "dump-state response size: $($resp.Length) bytes" }
        } catch {
            Info "dump-state $i failed: $_"
        }
    }
    if ($dsTimes.Count -gt 0) {
        Metric "dump-state RTT avg (20 calls)" ([Math]::Round(($dsTimes | Measure-Object -Average).Average, 0))
        Metric "dump-state RTT p90" (Percentile $dsTimes 90)
        Metric "dump-state RTT max" ($dsTimes | Measure-Object -Maximum).Maximum
        $results["dumpstate_avg_ms"] = [Math]::Round(($dsTimes | Measure-Object -Average).Average, 0)
    }
} else {
    Info "Skipping TCP tests (no port/key)"
}
Write-Host ""

# ═══════════════════════════════════════════════════════════════════════════
# TEST 7: RAPID FIRE THROUGHPUT — how many new-window cmds per second?
# ═══════════════════════════════════════════════════════════════════════════
Write-Host ("─" * 78) -ForegroundColor DarkGray
Write-Host "  TEST 7: THROUGHPUT — new-window commands per second" -ForegroundColor Yellow
Write-Host ("─" * 78) -ForegroundColor DarkGray

Cleanup
Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -s tput -d" -WindowStyle Hidden
Start-Sleep -Seconds 3
Wait-Prompt "tput:0" | Out-Null

$tputCount = 200
$tputSw = [System.Diagnostics.Stopwatch]::StartNew()
for ($i = 1; $i -le $tputCount; $i++) {
    & $PSMUX -L $NS new-window -t tput 2>&1 | Out-Null
}
$tputSw.Stop()
$tputRate = [Math]::Round($tputCount / ($tputSw.ElapsedMilliseconds / 1000.0), 1)
Metric "Total time for $tputCount new-window commands" $tputSw.ElapsedMilliseconds
Info "Throughput: $tputRate windows/sec"
$results["throughput_wps"] = $tputRate

# Verify server alive
$alive = & $PSMUX -L $NS list-sessions 2>&1 | Out-String
if ($alive -match "tput") { Pass "Server alive after $tputCount rapid windows" }
else { Fail "Server died after rapid window creation" }
$memFinal = Get-PsmuxMemory
Info "Final memory: ${memFinal}MB"
$results["final_mem_mb"] = $memFinal
Write-Host ""

# ═══════════════════════════════════════════════════════════════════════════
# TEST 8: WINDOW KILL THROUGHPUT
# ═══════════════════════════════════════════════════════════════════════════
Write-Host ("─" * 78) -ForegroundColor DarkGray
Write-Host "  TEST 8: CLEANUP — kill-window throughput" -ForegroundColor Yellow
Write-Host ("─" * 78) -ForegroundColor DarkGray

$killSw = [System.Diagnostics.Stopwatch]::StartNew()
# Kill windows in reverse to avoid index shifting issues  
for ($i = $tputCount; $i -ge 1; $i--) {
    & $PSMUX -L $NS kill-window -t "tput:$i" 2>&1 | Out-Null
}
$killSw.Stop()
Metric "Kill $tputCount windows" $killSw.ElapsedMilliseconds
$killRate = [Math]::Round($tputCount / ($killSw.ElapsedMilliseconds / 1000.0), 1)
Info "Kill throughput: $killRate windows/sec"

$memAfterKill = Get-PsmuxMemory
Info "Memory after killing $tputCount windows: ${memAfterKill}MB"
$results["mem_after_kill_mb"] = $memAfterKill
Write-Host ""

# ═══════════════════════════════════════════════════════════════════════════
# SUMMARY
# ═══════════════════════════════════════════════════════════════════════════
Cleanup

Write-Host ("═" * 78) -ForegroundColor Cyan
Write-Host "  RESULTS SUMMARY" -ForegroundColor Cyan
Write-Host ("═" * 78) -ForegroundColor Cyan
Write-Host ""
Write-Host "  Tests: $TOTAL total | $PASS passed | $FAIL failed" -ForegroundColor $(if ($FAIL -gt 0) { "Red" } else { "Green" })
Write-Host ""
Write-Host "  Key Metrics:" -ForegroundColor White

foreach ($k in ($results.Keys | Sort-Object)) {
    $v = $results[$k]
    $unit = if ($k -match "ms$|_ms") { "ms" } elseif ($k -match "mb$|_mb") { "MB" } elseif ($k -match "wps") { "w/s" } else { "" }
    Write-Host ("    {0,-40} {1,10} {2}" -f $k, $v, $unit)
}
Write-Host ""

# Write results to file
$outFile = Join-Path $PSScriptRoot "..\test_extreme_perf_results.txt"
$results | ConvertTo-Json | Out-File $outFile -Encoding utf8
Info "Results written to $outFile"
# The same numbers with the shared envelope (build, machine, load) in the
# metrics folder perf_summary.ps1 reads, so the scale run has a history too.
$sorted = [ordered]@{}
foreach ($k in ($results.Keys | Sort-Object)) { $sorted[$k] = $results[$k] }
$jsonPath = Write-PerfMetrics -Suite "test_extreme_perf" -Binary $PSMUX -FileStem "extreme_perf" -Data ([ordered]@{
    params  = [ordered]@{ sequential_windows = $SequentialWindows; burst_windows = $BurstWindows; mixed_windows = $MixedWindows; mixed_splits = $MixedSplits }
    results = $sorted
    passed  = $PASS
    failed  = $FAIL
})
if ($jsonPath) { Info "metrics: $jsonPath" }

if ($null -eq $prevDataDir) { Remove-Item Env:\PSMUX_DATA_DIR -ErrorAction SilentlyContinue } else { $env:PSMUX_DATA_DIR = $prevDataDir }
Remove-Item -Recurse -Force $DataDir -ErrorAction SilentlyContinue

if ($FAIL -gt 0) { exit 1 } else { exit 0 }
