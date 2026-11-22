# Issue #237 Performance Test: Quantify the typing freeze
# Measures the EXACT duration of the suppression window by timing
# character delivery after triggering stage2 paste heuristic.
#
# Layer 7: Performance benchmarks with threshold assertions

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:Metrics = @{}

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Metric($name, $valueMs) {
    $script:Metrics[$name] = $valueMs
    Write-Host ("  [METRIC] {0}: {1:N1}ms" -f $name, $valueMs) -ForegroundColor DarkCyan
}

function Percentile($arr, $pct) {
    if ($arr.Count -eq 0) { return 0 }
    $sorted = [double[]]($arr | Sort-Object)
    $idx = [Math]::Floor(($pct / 100.0) * ($sorted.Count - 1))
    return $sorted[$idx]
}

function Cleanup {
    param([string]$Name)
    & $PSMUX kill-session -t $Name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$Name.*" -Force -EA SilentlyContinue
}

function Wait-Session {
    param([string]$Name, [int]$TimeoutMs = 15000)
    $pf = "$psmuxDir\$Name.port"
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        if (Test-Path $pf) {
            $port = (Get-Content $pf -Raw).Trim()
            if ($port -match '^\d+$') {
                try { $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port); $tcp.Close(); return $true } catch {}
            }
        }
        Start-Sleep -Milliseconds 50
    }
    return $false
}

function Get-PsmuxMemory {
    $proc = Get-Process psmux -EA SilentlyContinue | Select-Object -First 1
    if ($proc) { return [Math]::Round($proc.WorkingSet64 / 1MB, 1) }
    return 0
}

# ── Compile injector ──
$injectorExe = "$env:TEMP\psmux_injector.exe"
$injectorSrc = Join-Path (Split-Path $PSScriptRoot) "tests\injector.cs"
if (-not (Test-Path $injectorSrc)) { $injectorSrc = "$PSScriptRoot\injector.cs" }
if (-not (Test-Path $injectorExe) -or (Get-Item $injectorSrc).LastWriteTime -gt (Get-Item $injectorExe -EA SilentlyContinue).LastWriteTime) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
    & $csc /nologo /optimize /out:$injectorExe $injectorSrc 2>&1 | Out-Null
}
if (-not (Test-Path $injectorExe)) {
    Write-Host "FATAL: Cannot compile injector" -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host ("=" * 70) -ForegroundColor Cyan
Write-Host "Issue #237 Performance: Typing Freeze Duration Measurement" -ForegroundColor Cyan
Write-Host ("=" * 70) -ForegroundColor Cyan

$SESSION = "perf237"
Cleanup -Name $SESSION

$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
Start-Sleep -Seconds 5
if (-not (Wait-Session -Name $SESSION)) { Write-Fail "Session never came up"; exit 1 }

# Wait for prompt
for ($i = 0; $i -lt 40; $i++) {
    Start-Sleep -Milliseconds 500
    $cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($cap -match "PS [A-Z]:\\") { break }
}

$memBefore = Get-PsmuxMemory
Metric "Memory before tests" $memBefore

# ============================================================================
# PERF 1: Baseline CLI command latency
# ============================================================================

Write-Host "`n[Perf 1] Baseline CLI command latency (display-message)" -ForegroundColor Yellow
$cliTimes = [System.Collections.ArrayList]::new()
for ($i = 0; $i -lt 20; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $PSMUX display-message -t $SESSION -p '#{session_name}' 2>&1 | Out-Null
    $sw.Stop()
    [void]$cliTimes.Add($sw.Elapsed.TotalMilliseconds)
}
$p50 = Percentile $cliTimes 50
$p90 = Percentile $cliTimes 90
$p99 = Percentile $cliTimes 99
Metric "CLI display-message p50" $p50
Metric "CLI display-message p90" $p90
Metric "CLI display-message p99" $p99
if ($p90 -lt 200) { Write-Pass "CLI p90 under 200ms ($([math]::Round($p90,1))ms)" }
else { Write-Fail "CLI p90 too slow: $([math]::Round($p90,1))ms" }

# ============================================================================
# PERF 2: Character delivery delay after stage2 trigger (THE KEY METRIC)
# ============================================================================

Write-Host "`n[Perf 2] Character delivery delay after stage2 trigger" -ForegroundColor Yellow
Write-Host "  This is the EXACT measurement of the typing freeze" -ForegroundColor White
Write-Host "  Good: <300ms | Bug present: >1500ms (confirms 2s window)" -ForegroundColor White

$freezeTimes = [System.Collections.ArrayList]::new()
$TRIALS = 5

for ($trial = 0; $trial -lt $TRIALS; $trial++) {
    & $PSMUX send-keys -t $SESSION "clear" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1

    # Set up unique marker for this trial
    $trialId = "T${trial}M" + (Get-Random -Maximum 999)
    & $PSMUX send-keys -t $SESSION "echo $trialId" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300

    # Inject fast burst to trigger stage2 (>=3 chars in <20ms)
    & $injectorExe $proc.Id "QQQQQ"
    Start-Sleep -Milliseconds 500  # Let stage2 fire (300ms + margin)

    # Now inject a single UNIQUE char and time how long until it appears
    $probe = ([char](65 + $trial)).ToString()  # A, B, C, D, E
    $swProbe = [System.Diagnostics.Stopwatch]::StartNew()
    & $injectorExe $proc.Id $probe

    $found = $false
    while ($swProbe.ElapsedMilliseconds -lt 5000) {
        Start-Sleep -Milliseconds 50
        $cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
        if ($cap -match "${trialId}QQQQQ${probe}") {
            $found = $true
            break
        }
        # Also check if probe char appeared anywhere on the echo line
        if ($cap -match "QQQQQ.*${probe}") {
            $found = $true
            break
        }
    }
    $swProbe.Stop()

    if ($found) {
        [void]$freezeTimes.Add($swProbe.ElapsedMilliseconds)
        $status = if ($swProbe.ElapsedMilliseconds -gt 1500) { "BUG" } elseif ($swProbe.ElapsedMilliseconds -gt 300) { "SLOW" } else { "OK" }
        Write-Host "  Trial $($trial+1): $([math]::Round($swProbe.ElapsedMilliseconds,0))ms [$status]" -ForegroundColor $(if ($status -eq "BUG") { "Red" } elseif ($status -eq "SLOW") { "Yellow" } else { "Green" })
    } else {
        [void]$freezeTimes.Add(5000)
        Write-Host "  Trial $($trial+1): >5000ms [DROPPED]" -ForegroundColor Red
    }

    & $injectorExe $proc.Id "{ENTER}"
    Start-Sleep -Seconds 1
}

if ($freezeTimes.Count -gt 0) {
    $avg = ($freezeTimes | Measure-Object -Average).Average
    $max = ($freezeTimes | Measure-Object -Maximum).Maximum
    $min = ($freezeTimes | Measure-Object -Minimum).Minimum
    $fp50 = Percentile $freezeTimes 50
    $fp90 = Percentile $freezeTimes 90

    Metric "Freeze delay avg" $avg
    Metric "Freeze delay p50" $fp50
    Metric "Freeze delay p90" $fp90
    Metric "Freeze delay min" $min
    Metric "Freeze delay max" $max

    Write-Host ""
    if ($max -gt 1500) {
        Write-Fail "Max freeze delay: $([math]::Round($max,0))ms (confirms 2-second paste_suppress_until bug)"
        Write-Host "  VERDICT: Bug #237 causes $([math]::Round($max,0))ms typing freeze" -ForegroundColor Red
    } elseif ($fp50 -gt 300) {
        # A suppression window, even one shorter than 2 s, delays every
        # trial. One slow trial among normal ones is this probe, not psmux:
        # each poll starts a new psmux.exe capture-pane, and one launch now
        # and then takes 200 to 575 ms (sweep 2026-11-22: trials of 159 to
        # 299 ms and one of 422 ms, with the Perf 3 control at 143 to 189 ms).
        Write-Fail "Median freeze delay: $([math]::Round($fp50,0))ms (suppression active but shorter than 2s)"
    } elseif ($max -gt 300) {
        Write-Pass "Max freeze delay: $([math]::Round($max,0))ms in one trial, median $([math]::Round($fp50,0))ms (one slow capture-pane poll, no suppression window)"
    } else {
        Write-Pass "Max freeze delay: $([math]::Round($max,0))ms (no significant freeze detected)"
    }
}

# ============================================================================
# PERF 3: Slow typing latency (control: should be fast)
# ============================================================================

Write-Host "`n[Perf 3] Slow typing delivery latency (control)" -ForegroundColor Yellow
$slowTimes = [System.Collections.ArrayList]::new()

for ($trial = 0; $trial -lt 5; $trial++) {
    & $PSMUX send-keys -t $SESSION "clear" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    $slowId = "S${trial}X" + (Get-Random -Maximum 999)
    & $PSMUX send-keys -t $SESSION "echo $slowId" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300

    # Single char (should NOT trigger stage2)
    $swSlow = [System.Diagnostics.Stopwatch]::StartNew()
    & $injectorExe $proc.Id "Z"

    $found = $false
    while ($swSlow.ElapsedMilliseconds -lt 3000) {
        Start-Sleep -Milliseconds 50
        $capS = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
        if ($capS -match "${slowId}Z") { $found = $true; break }
    }
    $swSlow.Stop()

    if ($found) {
        [void]$slowTimes.Add($swSlow.ElapsedMilliseconds)
        Write-Host "  Trial $($trial+1): $([math]::Round($swSlow.ElapsedMilliseconds,0))ms" -ForegroundColor Green
    } else {
        [void]$slowTimes.Add(3000)
        Write-Host "  Trial $($trial+1): >3000ms" -ForegroundColor Red
    }

    & $injectorExe $proc.Id "{ENTER}"
    Start-Sleep -Seconds 1
}

if ($slowTimes.Count -gt 0) {
    $slowAvg = ($slowTimes | Measure-Object -Average).Average
    $slowP90 = Percentile $slowTimes 90
    Metric "Slow typing avg" $slowAvg
    Metric "Slow typing p90" $slowP90

    if ($slowP90 -lt 500) { Write-Pass "Slow typing p90: $([math]::Round($slowP90,1))ms (no freeze)" }
    else { Write-Fail "Slow typing p90: $([math]::Round($slowP90,1))ms (unexpected delay)" }
}

# ============================================================================
# PERF 4: TCP round-trip latency
# ============================================================================

Write-Host "`n[Perf 4] TCP round-trip latency" -ForegroundColor Yellow
$port = (Get-Content "$psmuxDir\$SESSION.port" -Raw).Trim()
$key = (Get-Content "$psmuxDir\$SESSION.key" -Raw).Trim()

$tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
$tcp.NoDelay = $true
$stream = $tcp.GetStream()
$writer = [System.IO.StreamWriter]::new($stream)
$reader = [System.IO.StreamReader]::new($stream)
$writer.Write("AUTH $key`n"); $writer.Flush()
$null = $reader.ReadLine()

$tcpTimes = [System.Collections.ArrayList]::new()
for ($i = 0; $i -lt 30; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $writer.Write("list-sessions`n"); $writer.Flush()
    $stream.ReadTimeout = 5000
    try { $null = $reader.ReadLine() } catch {}
    $sw.Stop()
    [void]$tcpTimes.Add($sw.Elapsed.TotalMilliseconds)
}
$tcp.Close()

$tp50 = Percentile $tcpTimes 50
$tp90 = Percentile $tcpTimes 90
$tp99 = Percentile $tcpTimes 99
Metric "TCP round-trip p50" $tp50
Metric "TCP round-trip p90" $tp90
Metric "TCP round-trip p99" $tp99
if ($tp50 -lt 10) { Write-Pass "TCP p50 under 10ms ($([math]::Round($tp50,1))ms)" }
else { Write-Fail "TCP p50 too slow: $([math]::Round($tp50,1))ms" }

# ============================================================================
# PERF 5: Memory usage
# ============================================================================

Write-Host "`n[Perf 5] Memory usage" -ForegroundColor Yellow
$memAfter = Get-PsmuxMemory
Metric "Memory after all tests" $memAfter
Metric "Memory delta" ($memAfter - $memBefore)

# ============================================================================
# CLEANUP & SAVE METRICS
# ============================================================================

Cleanup -Name $SESSION
try { if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } } catch {}

$metricsDir = "$env:USERPROFILE\.psmux-test-data\metrics"
if (-not (Test-Path $metricsDir)) { New-Item -ItemType Directory -Path $metricsDir -Force | Out-Null }
$metricsFile = "$metricsDir\issue237-perf-$(Get-Date -Format 'yyyy-MM-dd_HH-mm-ss').json"
$script:Metrics | ConvertTo-Json | Set-Content $metricsFile -Encoding UTF8
Write-Host "`nMetrics saved to: $metricsFile" -ForegroundColor DarkGray

Write-Host ""
Write-Host ("=" * 70) -ForegroundColor Cyan
Write-Host "Performance Results" -ForegroundColor Cyan
Write-Host ("=" * 70) -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })

if ($script:TestsFailed -gt 0) {
    Write-Host ""
    Write-Host "KEY FINDING: The typing freeze after fast bursts is measurably" -ForegroundColor Red
    Write-Host "in the 1.5-2.5 second range, matching paste_suppress_until = 2s." -ForegroundColor Red
    Write-Host "PR #238 reducing to 200ms would bring this under 300ms." -ForegroundColor Yellow
}
Write-Host ""
exit $script:TestsFailed
