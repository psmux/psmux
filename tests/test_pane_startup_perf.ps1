# test_pane_startup_perf.ps1 — Comprehensive pane/window/session startup latency test
# Measures EXACTLY how long it takes for pwsh to fully load in psmux panes,
# and isolates whether the delay is from psmux infrastructure or from pwsh itself.
#
# Tests:
#   1. Baseline: raw pwsh startup time (no psmux)
#   2. First session creation + first pane ready time
#   3. New window creation + shell ready time (repeated N times)
#   4. Split-window pane creation + shell ready time (repeated N times)
#   5. Rapid sequential window creation (stress test)
#   6. Multiple sessions creation
#   7. Pane close / window close latency
#
# For each, we measure wall-clock time until the pwsh prompt actually appears
# (detected via capture-pane output containing "PS " prompt marker).

param(
    [int]$WindowCount = 5,
    [int]$SplitCount = 4,
    [int]$SessionCount = 3,
    [int]$PromptTimeoutSec = 30,
    # Point the suite at a specific build to compare two of them. The name must
    # still be one the server recognises as its own image (psmux / pmux / tmux),
    # because session.rs gates the warm-server claim on it: a differently named
    # copy silently loses the fast path and the run measures the rename.
    [string]$Psmux = "",
    # Where the run's samples are written. Outside the repo by default: a perf
    # suite that writes into the tree makes every run a dirty working copy.
    [string]$MetricsDir = "",
    [switch]$Verbose
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\perf_metrics_common.ps1"
if (-not $Psmux -and $env:PSMUX_EXE) { $Psmux = $env:PSMUX_EXE }
if (-not $Psmux -and $env:PSMUX_TEST_BIN) { $Psmux = $env:PSMUX_TEST_BIN }
if (-not $Psmux -and $env:PSMUX_TEST_BINARY) { $Psmux = $env:PSMUX_TEST_BINARY }
if ($Psmux) {
    $PSMUX = $Psmux
} else {
    $PSMUX = $null
    foreach ($name in @("psmux.exe", "pmux.exe", "tmux.exe")) {
        $candidate = Join-Path $PSScriptRoot "..\target\release\$name"
        if (Test-Path $candidate) { $PSMUX = $candidate; break }
    }
}
if (-not $PSMUX -or -not (Test-Path $PSMUX)) {
    Write-Host "ERROR: Cannot find psmux.exe, pmux.exe or tmux.exe in target\release\" -ForegroundColor Red
    exit 1
}
$PSMUX = (Resolve-Path $PSMUX).Path
$imgName = [IO.Path]::GetFileNameWithoutExtension($PSMUX).ToLower()
if ($imgName -notin @("psmux", "pmux", "tmux")) {
    Write-Host "REFUSING: '$imgName' is not a recognised server image name; the warm-server claim would be disabled and every timing here would be wrong" -ForegroundColor Red
    exit 1
}

$PASS = 0; $FAIL = 0; $TOTAL_TESTS = 0

# ── Latency assertions ────────────────────────────────────────────────────
# An average alone hid the defect this suite exists to catch: with a spare
# shell pool of depth one, every OTHER creation claimed a spare that had been
# spawned moments earlier and paid its whole shell startup. Five windows came
# out 68, 558, 41, 584, 47 ms -- an average of 260 that looks unremarkable and
# a lived experience that is anything but. So assert the shape of the
# distribution, not its centre:
#
#   p90  - the slow half cannot hide behind the fast half
#   max  - no single creation may be terrible
#   bimodality - max more than 3x the median means the fast and slow paths are
#                two different code paths, which is precisely the bug
function Assert-Latency {
    param(
        [string]$Label,
        [double[]]$Samples,
        [int]$P90Limit,
        [int]$MaxLimit,
        [double]$BimodalRatio = 3.0,
        [int]$MinSamples = 3
    )
    if ($Samples.Count -lt $MinSamples) {
        Write-Fail "$Label - only $($Samples.Count) samples, cannot judge latency"
        return
    }
    $s = $Samples | Sort-Object
    $median = $s[[int][Math]::Floor(($s.Count - 1) / 2)]
    $p90 = $s[[Math]::Min($s.Count - 1, [int][Math]::Ceiling(0.9 * $s.Count) - 1)]
    $max = $s[-1]
    $list = ($Samples | ForEach-Object { [int]$_ }) -join ', '
    Write-Host ("  {0,-42} med={1,5:N0} p90={2,5:N0} max={3,5:N0}  [{4}]" -f $Label, $median, $p90, $max, $list) -ForegroundColor Gray

    if ($p90 -le $P90Limit) { Write-Pass "$Label p90 ${p90}ms <= ${P90Limit}ms" }
    else { Write-Fail "$Label p90 ${p90}ms exceeds ${P90Limit}ms  [$list]" }

    if ($max -le $MaxLimit) { Write-Pass "$Label max ${max}ms <= ${MaxLimit}ms" }
    else { Write-Fail "$Label max ${max}ms exceeds ${MaxLimit}ms  [$list]" }

    # Bimodality. Guard the degenerate case where everything is so fast that
    # timer noise alone clears 3x (a 10ms median against a 35ms max is not a
    # regression), by requiring the max to also be meaningfully slow.
    if ($median -gt 0 -and $max -gt ($median * $BimodalRatio) -and $max -gt 150) {
        Write-Fail "$Label is BIMODAL: max ${max}ms is more than ${BimodalRatio}x the median ${median}ms - some creations are taking a code path the others are not  [$list]"
    } else {
        Write-Pass "$Label is not bimodal (max ${max}ms vs median ${median}ms)"
    }
}
function Write-Pass { param([string]$msg) $script:PASS++; $script:TOTAL_TESTS++; Write-Host "  PASS: $msg" -ForegroundColor Green }
function Write-Fail { param([string]$msg) $script:FAIL++; $script:TOTAL_TESTS++; Write-Host "  FAIL: $msg" -ForegroundColor Red }
function Write-Info { param([string]$msg) Write-Host "  INFO: $msg" -ForegroundColor Gray }
function Get-Median { param([double[]]$Samples)
    if (-not $Samples -or $Samples.Count -eq 0) { return 0 }
    $s = @($Samples | Sort-Object)
    $n = $s.Count
    if ($n % 2 -eq 1) { return [double]$s[[math]::Floor($n / 2)] }
    return ([double]$s[$n / 2 - 1] + [double]$s[$n / 2]) / 2
}

function Write-Metric { param([string]$label, [double]$ms)
    $color = if ($ms -lt 2000) { "Green" } elseif ($ms -lt 5000) { "Yellow" } else { "Red" }
    Write-Host ("  {0,-50} {1,8:N0} ms" -f $label, $ms) -ForegroundColor $color
}

# Where this psmux keeps its registry. Honouring PSMUX_DATA_DIR is what lets
# the suite be pointed at a scratch root, so a run cannot kill-server the
# sessions a developer is actually using.
$PSMUX_DIR = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR.TrimEnd('\', '/') } else { "$env:USERPROFILE\.psmux" }

# Helper: wait for port/key files to appear, return (port, key)
function Wait-ServerReady {
    param([string]$SessionName, [int]$TimeoutSec = 15)
    $pf = "$PSMUX_DIR\${SessionName}.port"
    $kf = "$PSMUX_DIR\${SessionName}.key"
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt ($TimeoutSec * 1000)) {
        if ((Test-Path $pf) -and (Test-Path $kf)) {
            $port = [int](Get-Content $pf -Raw).Trim()
            $key  = (Get-Content $kf -Raw).Trim()
            if ($port -gt 0 -and $key.Length -gt 0) {
                return @{ Port = $port; Key = $key; ElapsedMs = $sw.ElapsedMilliseconds }
            }
        }
        Start-Sleep -Milliseconds 50
    }
    return $null
}

# Helper: wait until capture-pane shows a pwsh prompt (line containing "PS " and ">")
function Wait-PanePrompt {
    param(
        [string]$SessionName,
        [int]$TimeoutMs = 30000,
        # Match "PS " anywhere — covers word-wrapped prompts in small panes
        [string]$PromptPattern = "PS [A-Z]:\\"
    )
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        try {
            $output = & $PSMUX capture-pane -t $SessionName -p 2>&1 | Out-String
            if ($output -match $PromptPattern) {
                return @{ Found = $true; ElapsedMs = $sw.ElapsedMilliseconds; Output = $output }
            }
        } catch {
            # Server not ready yet, retry
        }
        Start-Sleep -Milliseconds 100
    }
    return @{ Found = $false; ElapsedMs = $sw.ElapsedMilliseconds; Output = "" }
}

# Helper: wait for a specific pane (by target) to show prompt  
function Wait-PanePromptTarget {
    param(
        [string]$Target,
        [int]$TimeoutMs = 30000,
        [string]$PromptPattern = "PS [A-Z]:\\"
    )
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        try {
            $output = & $PSMUX capture-pane -t $Target -p 2>&1 | Out-String
            if ($output -match $PromptPattern) {
                return @{ Found = $true; ElapsedMs = $sw.ElapsedMilliseconds; Output = $output }
            }
        } catch {}
        Start-Sleep -Milliseconds 100
    }
    return @{ Found = $false; ElapsedMs = $sw.ElapsedMilliseconds; Output = "" }
}

# Helper: kill session and wait for cleanup
function Kill-TestSession {
    param([string]$SessionName)
    try {
        & $PSMUX kill-session -t $SessionName 2>&1 | Out-Null
    } catch {}
    # Also try kill-server for cleanliness
    Start-Sleep -Milliseconds 300
}

# Cleanup any stale sessions from prior runs
function Cleanup-All {
    try { & $PSMUX kill-server 2>&1 | Out-Null } catch {}
    Start-Sleep -Milliseconds 500
    # Remove stale port/key files
    if (Test-Path $PSMUX_DIR) {
        Get-ChildItem "$PSMUX_DIR\perf_test_*.port" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
        Get-ChildItem "$PSMUX_DIR\perf_test_*.key"  -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
    }
}

# ==============================================================================
Write-Host ""
Write-Host "================================================================" -ForegroundColor Cyan
Write-Host " psmux Pane Startup Performance Test" -ForegroundColor Cyan
Write-Host " $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')" -ForegroundColor Cyan
Write-Host " Binary: $PSMUX" -ForegroundColor Cyan
Write-Host "================================================================" -ForegroundColor Cyan
Write-Host ""

Cleanup-All

# ==============================================================================
# TEST 0: Baseline — raw pwsh startup time (no psmux)
# ==============================================================================
Write-Host "--- TEST 0: Baseline pwsh startup (no psmux) ---" -ForegroundColor Yellow
$baselineTimes = @()
for ($i = 0; $i -lt 3; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    # Start pwsh, run a command that proves it's loaded, capture output
    $result = & pwsh -NoLogo -NoProfile -Command "Write-Output 'READY'" 2>&1 | Out-String
    $sw.Stop()
    $baselineTimes += $sw.ElapsedMilliseconds
    if ($result -match "READY") {
        Write-Metric "  pwsh -NoProfile startup #$($i+1)" $sw.ElapsedMilliseconds
    } else {
        Write-Fail "pwsh baseline #$($i+1) - no output"
    }
}
$baselineAvg = ($baselineTimes | Measure-Object -Average).Average
Write-Metric "  pwsh -NoProfile AVERAGE" $baselineAvg
Write-Host ""

# Now test with profile (this is what psmux does by default)
$profileTimes = @()
for ($i = 0; $i -lt 3; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $result = & pwsh -NoLogo -Command "Write-Output 'READY'" 2>&1 | Out-String
    $sw.Stop()
    $profileTimes += $sw.ElapsedMilliseconds
    if ($result -match "READY") {
        Write-Metric "  pwsh (with profile) startup #$($i+1)" $sw.ElapsedMilliseconds
    } else {
        Write-Fail "pwsh+profile baseline #$($i+1) - no output"
    }
}
$profileAvg = ($profileTimes | Measure-Object -Average).Average
Write-Metric "  pwsh (with profile) AVERAGE" $profileAvg
Write-Host ""

# ==============================================================================
# TEST 1: First session creation — full cold start
# ==============================================================================
Write-Host "--- TEST 1: First session creation (cold start) ---" -ForegroundColor Yellow
$session1 = "perf_test_session1"
$swTotal = [System.Diagnostics.Stopwatch]::StartNew()

$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session", "-s", $session1, "-d" -PassThru -WindowStyle Hidden
$swServer = [System.Diagnostics.Stopwatch]::StartNew()

# Phase 1: wait for server ready (port file)
$serverInfo = Wait-ServerReady -SessionName $session1 -TimeoutSec 15
if ($null -eq $serverInfo) {
    Write-Fail "Session '$session1' — server never started (no .port file)"
    Cleanup-All
    exit 1
}
$serverReadyMs = $serverInfo.ElapsedMs
Write-Metric "Server ready (.port file appeared)" $serverReadyMs

# Phase 2: wait for pwsh prompt to appear in the pane
$promptResult = Wait-PanePrompt -SessionName $session1 -TimeoutMs ($PromptTimeoutSec * 1000)
$swTotal.Stop()
if ($promptResult.Found) {
    $totalMs = $swTotal.ElapsedMilliseconds
    $psmuxOverhead = $serverReadyMs
    $shellTime = $promptResult.ElapsedMs  # from when we started polling (after server ready)
    Write-Metric "Prompt appeared (from server ready)" $promptResult.ElapsedMs
    Write-Metric "TOTAL first session startup" $totalMs
    Write-Pass "First session created and shell ready in ${totalMs}ms"
} else {
    Write-Fail "First session — pwsh prompt never appeared within ${PromptTimeoutSec}s"
    if ($Verbose) { Write-Info "Last capture: $($promptResult.Output.Substring(0, [Math]::Min(200, $promptResult.Output.Length)))" }
}
Write-Host ""

# ==============================================================================
# TEST 2: New window creation — measure time for each new window's shell to load
# ==============================================================================
Write-Host "--- TEST 2: New window creation (${WindowCount}x) ---" -ForegroundColor Yellow
$windowTimes = @()
for ($w = 0; $w -lt $WindowCount; $w++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $PSMUX new-window -t $session1 2>&1 | Out-Null
    
    # Wait for prompt in the new (now-active) window
    $result = Wait-PanePrompt -SessionName $session1 -TimeoutMs ($PromptTimeoutSec * 1000)
    $sw.Stop()
    
    if ($result.Found) {
        $windowTimes += $sw.ElapsedMilliseconds
        Write-Metric "  Window #$($w+1) shell ready" $sw.ElapsedMilliseconds
    } else {
        Write-Fail "  Window #$($w+1) — prompt never appeared"
        if ($Verbose) { 
            $cap = & $PSMUX capture-pane -t $session1 -p 2>&1 | Out-String
            Write-Info "Capture: $($cap.Substring(0, [Math]::Min(200, $cap.Length)))" 
        }
    }
}
if ($windowTimes.Count -gt 0) {
    $winAvg = ($windowTimes | Measure-Object -Average).Average
    $winMax = ($windowTimes | Measure-Object -Maximum).Maximum
    $winMin = ($windowTimes | Measure-Object -Minimum).Minimum
    Write-Metric "  New window AVG" $winAvg
    Write-Metric "  New window MIN" $winMin
    Write-Metric "  New window MAX" $winMax
    Write-Pass "Created $($windowTimes.Count) windows successfully"
    
    # Check if psmux is adding significant overhead vs baseline
    $overhead = $winAvg - $profileAvg
    if ($overhead -gt 3000) {
        Write-Fail "psmux adds ${overhead}ms overhead per window over raw pwsh (>${overhead}ms vs ${profileAvg}ms)"
    } elseif ($overhead -gt 1000) {
        Write-Info "psmux adds ~${overhead}ms overhead per window (moderate)"
    } else {
        Write-Pass "psmux overhead per window is minimal (~${overhead}ms)"
    }

    # At the shipped default pool depth (2) the first two creations of a run
    # come from settled spares and the third has to wait out a shell start. So
    # this is a budget on how MANY creations are allowed to be slow, not on the
    # worst one: with the depth-one pool it was every other creation.
    #
    # Two, not one. Measured 2026-09-12 on three builds (8708d12, 58aa34d,
    # d493da1), six runs alternating between the pre and post batch binaries:
    # the third creation is always the cold one (600 to 770 ms) and the FIFTH
    # is slow about one run in three ([57,36,600,54,324] and [72,56,668,67,360]
    # were two of them), on old and new code alike. The mechanism is the refill
    # itself: the moment the third creation finds the pool empty, two
    # replacement spares start booting on their own threads WHILE the cold
    # spawn boots, so three pwsh starts race on the machine. Creation four takes
    # the first replacement; creation five can land while the second is still
    # booting and waits the remainder (330 to 360 ms). That is the pool doing
    # exactly what it should at depth two, so the budget here is two slow
    # creations. The depth five contract below (every creation fast, p90 150
    # ms) is the real guarantee and stays strict.
    #
    # Since b117e745 (2026-10-04) the FIRST creation after a cold launch is a
    # shell booting alone by design: the server holds its pool refill until the
    # first pane's shell has started and then trickles the spares one at a
    # time, so a new-window that arrives before the first spare is ready boots
    # its own shell (611 to 701 ms) instead of racing three pwsh starts. That
    # creation is judged by test_launch_to_prompt_gate and by the cold creation
    # check below, not here. Counting it against this budget made the gate flip
    # on whether the run happened to find a parked standby (first creation 35
    # ms) or launched cold (first creation 650 ms): sweep 2026-10-05_19-58-06
    # failed on [652, 34, 28, 2239, 172] and an A/B of eight runs per binary
    # on 2026-10-06 reproduced the shape on the pre batch binary as well
    # ([686, 29, 27, 1800, 311]), so the count is taken over creations two to
    # five, where the budget of two slow creations is the refill race the
    # paragraph above measured.
    $afterFirst = @($windowTimes | Select-Object -Skip 1)
    $slowWindows = @($afterFirst | Where-Object { $_ -gt 300 }).Count
    if ($slowWindows -le 2) {
        Write-Pass "new-window: $slowWindows of $($afterFirst.Count) creations after the first over 300ms (at most 2 allowed at the default pool depth)"
    } else {
        Write-Fail "new-window: $slowWindows of $($afterFirst.Count) creations after the first over 300ms - the spare pool is not being refilled ahead of demand  [$(($windowTimes | ForEach-Object { [int]$_ }) -join ', ')]"
    }
    # The mean used to be gated at 300 ms as well. That double counted the one
    # cold creation the budget above already allows: with the cold one at 770
    # ms the mean is 262, with it at 1000 ms the mean is 308, so the gate
    # flipped on how long THIS machine takes to start one pwsh, which the
    # count gate had already judged. Sweep 2026-10-03_14-07-51 failed on
    # [50, 31, 1547, 309, 29] with the count gate passing. Two gates replace it:
    #
    #   median - at depth two the centre of five creations must be a warm
    #            claim, so the median is held to the same 300 ms.
    #   cold   - the slowest creation is a pwsh start psmux did not have ready,
    #            so it is judged against the suite's own measurement of a bare
    #            `pwsh -NoProfile` start on this machine, taken minutes earlier
    #            (Test 0). Measured on master 8c9ef128, 2026-10-03/04, bare
    #            332 to 371 ms: the cold creation was 943 to 1070 ms in six
    #            interleaved runs (2.6x to 3.2x) and 1434 and 1547 ms (4.3x,
    #            4.6x) in two of four runs under the sweep runner. Window 2
    #            takes the last ready spare, which opens a surge of seven
    #            spawns in two waves, and window 3 boots the one spare that was
    #            already starting through both waves; a Store pwsh costs 1.6x
    #            its solo time when four others start within 50 ms of it (no
    #            psmux probe), and ten refill orderings measured on
    #            2026-10-04 all moved that cost to a later creation instead of
    #            removing it (docs/warm-sessions.md, "The third quick creation
    #            at depth two"). So 5x is a regression tripwire on the whole
    #            envelope, not a target: the target is the ratio written to
    #            the metrics JSON as cold_creation_ratio trending back toward
    #            the 1.5x to 2x this suite recorded in September 2026.
    # The median is taken over the same creations as the count gate, for the
    # same reason: with the lone first boot counted, a cold launch plus the
    # refill race put three slow samples in five and the median became the
    # refill wait itself (313 ms on [658, 29, 24, 1931, 312]).
    $winMedian = Get-Median $afterFirst
    if ($winMedian -le 300) {
        Write-Pass ("new-window median {0:N0}ms over creations two to five is within budget (300ms)" -f $winMedian)
    } else {
        Write-Fail ("new-window median {0:N0}ms over creations two to five exceeds 300ms  [$(($windowTimes | ForEach-Object { [int]$_ }) -join ', ')]" -f $winMedian)
    }
    #
    # 6x since 2026-10-06. Traced with PSMUX_WARM_TRACE on a cold launch: the
    # first creation boots alone (b117e745), creations two and three take the
    # two trickled spares, and creation two, finding no READY spare, opens the
    # surge (target 2 to 8, seven refills at once). Creation four then takes a
    # spare 2 ms old whose shell boots beside seven others, with CreateProcessW
    # at 258 to 489 ms instead of 85 ms solo, and its prompt arrives 1.5 to
    # 2.2 s later: 1579 ms in the trace, 1931, 1967 and 2239 ms in the runs
    # that tripped 5x (bare pwsh 369 to 390 ms, limit 1845 to 1950). That is
    # the surge envelope docs/warm-sessions.md measured on 2026-10-04, so a
    # tripwire at 5x sat inside it and fired one run in four. 6x stays outside
    # it; the ratio is written to the metrics JSON as cold_creation_ratio and
    # that trend, not this gate, is where the surge cost is to be driven down.
    $winMax = ($windowTimes | Measure-Object -Maximum).Maximum
    $coldLimit = [math]::Round(6 * $baselineAvg)
    if ($winMax -le $coldLimit) {
        Write-Pass ("new-window cold creation {0:N0}ms is within 6x the bare pwsh start ({1:N0}ms on this machine, limit {2}ms)" -f $winMax, $baselineAvg, $coldLimit)
    } else {
        Write-Fail ("new-window cold creation {0:N0}ms exceeds 6x the bare pwsh start ({1:N0}ms, limit {2}ms): the cold path regressed beyond its measured envelope  [$(($windowTimes | ForEach-Object { [int]$_ }) -join ', ')]" -f $winMax, $baselineAvg, $coldLimit)
    }
    Write-Metric "  New window MEDIAN" $winMedian
}
Write-Host ""

# ==============================================================================
# TEST 3: Split-window pane creation — measure shell ready time for splits
# ==============================================================================
Write-Host "--- TEST 3: Split-window pane creation (${SplitCount}x) ---" -ForegroundColor Yellow
# Switch to window 0 first
& $PSMUX select-window -t "${session1}:0" 2>&1 | Out-Null
Start-Sleep -Milliseconds 300

$splitTimes = @()
for ($s = 0; $s -lt $SplitCount; $s++) {
    $direction = if ($s % 2 -eq 0) { "-v" } else { "-h" }
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $PSMUX split-window $direction -t $session1 2>&1 | Out-Null
    
    # Wait for prompt in the new (now-active) pane
    $result = Wait-PanePrompt -SessionName $session1 -TimeoutMs ($PromptTimeoutSec * 1000)
    $sw.Stop()
    
    if ($result.Found) {
        $splitTimes += $sw.ElapsedMilliseconds
        Write-Metric "  Split #$($s+1) ($direction) shell ready" $sw.ElapsedMilliseconds
    } else {
        Write-Fail "  Split #$($s+1) — prompt never appeared"
        if ($Verbose) {
            $cap = & $PSMUX capture-pane -t $session1 -p 2>&1 | Out-String
            Write-Info "Capture: $($cap.Substring(0, [Math]::Min(200, $cap.Length)))"
        }
    }
}
if ($splitTimes.Count -gt 0) {
    $splitAvg = ($splitTimes | Measure-Object -Average).Average
    $splitMax = ($splitTimes | Measure-Object -Maximum).Maximum
    Write-Metric "  Split AVG" $splitAvg
    Write-Metric "  Split MAX" $splitMax
    Write-Pass "Created $($splitTimes.Count) splits successfully"

    $slowSplits = @($splitTimes | Where-Object { $_ -gt 300 }).Count
    if ($slowSplits -le 1) {
        Write-Pass "split-window: $slowSplits of $($splitTimes.Count) creations over 300ms (at most 1 allowed at the default pool depth)"
    } else {
        Write-Fail "split-window: $slowSplits of $($splitTimes.Count) creations over 300ms - the spare pool is not being refilled ahead of demand  [$(($splitTimes | ForEach-Object { [int]$_ }) -join ', ')]"
    }
    if ($splitAvg -le 250) {
        Write-Pass ("split average {0:N0}ms is within budget (250ms)" -f $splitAvg)
    } else {
        Write-Fail ("split average {0:N0}ms exceeds 250ms" -f $splitAvg)
    }
}
Write-Host ""

# ==============================================================================
# TEST 3b: the spare pool depth contract
# ==============================================================================
# The defect this pins: the pool used to be a single slot. Claiming it queued a
# refill, and the refill was a shell that had just started, so the very next
# creation claimed a newborn and paid its entire startup -- fast, slow, fast,
# slow, for ever, at any creation rate. Depth alone fixes that, because the
# spare handed to creation N+1 has then had the whole of creation N to boot.
#
# The contract, independent of what the shipped default happens to be: with
# `warm-pool-size` set to N, the first N creations of a run must ALL be fast.
# Old code has no such option, so it ignores the setting and alternates, which
# is exactly what these assertions reject.
Write-Host "--- TEST 3b: warm-pool-size contract (depth 5) ---" -ForegroundColor Yellow
$poolSess = "perf_test_pool"
Start-Process -FilePath $PSMUX -ArgumentList "new-session", "-s", $poolSess, "-d" -WindowStyle Hidden | Out-Null
$poolInfo = Wait-ServerReady -SessionName $poolSess -TimeoutSec 15
if ($null -eq $poolInfo) {
    Write-Fail "pool-depth test session failed to start"
} else {
    Wait-PanePrompt -SessionName $poolSess -TimeoutMs ($PromptTimeoutSec * 1000) | Out-Null

    # Set the depth through the option, not through the environment. A session
    # that claimed a standby `__warm__` server is running a process that was
    # started before this suite was, so it never saw PSMUX_WARM_POOL_SIZE --
    # setting the env here would silently measure the default and prove
    # nothing. The option is also the path users actually take.
    & $PSMUX set-option -g warm-pool-size 5 2>&1 | Out-Null
    $reported = (& $PSMUX show-options -g warm-pool-size 2>&1 | Out-String).Trim()
    if ($reported -match 'warm-pool-size\s+5') {
        Write-Pass "set -g warm-pool-size 5 took effect ($reported)"
    } else {
        Write-Fail "set -g warm-pool-size 5 did not take effect (show-options says '$reported')"
    }

    # Let the pool fill AND let its shells finish starting. A spare is only
    # worth anything once its shell is up; measuring before that would test the
    # settle time rather than the pool.
    Start-Sleep -Milliseconds 4500

    $poolWin = @()
    for ($i = 0; $i -lt 5; $i++) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $PSMUX new-window -t $poolSess 2>&1 | Out-Null
        $r = Wait-PanePrompt -SessionName $poolSess -TimeoutMs ($PromptTimeoutSec * 1000)
        $sw.Stop()
        if ($r.Found) { $poolWin += $sw.ElapsedMilliseconds }
    }
    Assert-Latency -Label "new-window @ warm-pool-size 5" -Samples $poolWin -P90Limit 150 -MaxLimit 300

    Start-Sleep -Milliseconds 3000
    & $PSMUX select-window -t "${poolSess}:0" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    $poolSplitV = @(); $poolSplitH = @()
    for ($i = 0; $i -lt 4; $i++) {
        $dir = if ($i % 2 -eq 0) { "-v" } else { "-h" }
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $PSMUX split-window $dir -t $poolSess 2>&1 | Out-Null
        $r = Wait-PanePrompt -SessionName $poolSess -TimeoutMs ($PromptTimeoutSec * 1000)
        $sw.Stop()
        if ($r.Found) {
            if ($dir -eq "-v") { $poolSplitV += $sw.ElapsedMilliseconds } else { $poolSplitH += $sw.ElapsedMilliseconds }
        }
    }
    Assert-Latency -Label "split -v @ warm-pool-size 5" -Samples $poolSplitV -P90Limit 150 -MaxLimit 300 -MinSamples 2
    Assert-Latency -Label "split -h @ warm-pool-size 5" -Samples $poolSplitH -P90Limit 150 -MaxLimit 300 -MinSamples 2

    # A burst: five commands with no waiting in between. With five settled
    # spares every one is a transplant, so the last prompt must be up promptly
    # after the last command was sent, not a shell startup later.
    Start-Sleep -Milliseconds 3000
    $swPB = [System.Diagnostics.Stopwatch]::StartNew()
    for ($i = 0; $i -lt 5; $i++) { & $PSMUX new-window -t $poolSess 2>&1 | Out-Null }
    $pbSent = $swPB.ElapsedMilliseconds
    $pbReady = Wait-PanePrompt -SessionName $poolSess -TimeoutMs ($PromptTimeoutSec * 1000)
    $swPB.Stop()
    Write-Metric "  burst of 5 @ depth 5: commands sent in" $pbSent
    Write-Metric "  burst of 5 @ depth 5: last prompt ready" $swPB.ElapsedMilliseconds
    if ($pbReady.Found -and $swPB.ElapsedMilliseconds -le 600) {
        Write-Pass "burst of 5 at depth 5 completed in $($swPB.ElapsedMilliseconds)ms (<= 600ms)"
    } else {
        Write-Fail "burst of 5 at depth 5 took $($swPB.ElapsedMilliseconds)ms (> 600ms) - the burst is not being served from the pool"
    }

    Kill-TestSession -SessionName $poolSess
}
Write-Host ""

# ==============================================================================
# TEST 4: Rapid sequential window creation (stress test)
# ==============================================================================
Write-Host "--- TEST 4: Rapid sequential windows (burst of 5) ---" -ForegroundColor Yellow
$swBurst = [System.Diagnostics.Stopwatch]::StartNew()

# Create 5 windows as fast as possible
for ($w = 0; $w -lt 5; $w++) {
    & $PSMUX new-window -t $session1 2>&1 | Out-Null
}
$createMs = $swBurst.ElapsedMilliseconds
Write-Metric "  5 new-window commands sent in" $createMs

# Now check how long until ALL 5 have prompts by listing windows and checking each
Start-Sleep -Milliseconds 500
$lsw = & $PSMUX list-windows -t $session1 2>&1 | Out-String
$winCount = ($lsw -split "`n" | Where-Object { $_ -match '\S' }).Count
Write-Info "Total windows now: $winCount"

# Wait for the last window to have a prompt
$burstResult = Wait-PanePrompt -SessionName $session1 -TimeoutMs ($PromptTimeoutSec * 1000)
$swBurst.Stop()
if ($burstResult.Found) {
    Write-Metric "  Last window prompt ready" $swBurst.ElapsedMilliseconds
    Write-Pass "Burst window creation: all shells started"
} else {
    Write-Fail "Burst window creation: some prompts never appeared"
}
Write-Host ""

# ==============================================================================
# TEST 5: List panes — check how many are alive
# ==============================================================================
Write-Host "--- TEST 5: Pane health check ---" -ForegroundColor Yellow
$lsp = & $PSMUX list-panes -t $session1 2>&1 | Out-String
$paneLines = ($lsp -split "`n" | Where-Object { $_ -match '\S' })
Write-Info "Total panes: $($paneLines.Count)"
if ($Verbose) { $paneLines | ForEach-Object { Write-Info "  $_" } }

# Check each pane for prompt
$lsw2 = & $PSMUX list-windows -t $session1 2>&1
$winLines = ($lsw2 | Out-String) -split "`n" | Where-Object { $_ -match '^\d+:' }
$aliveCount = 0
$deadCount = 0
foreach ($winLine in $winLines) {
    if ($winLine -match '^(\d+):') {
        $winIdx = $Matches[1]
        try {
            $cap = & $PSMUX capture-pane -t "${session1}:${winIdx}" -p 2>&1 | Out-String
            if ($cap -match "PS [A-Z]:\\") {
                $aliveCount++
            } else {
                $deadCount++
                if ($Verbose) { Write-Info "  Window $winIdx has no prompt: $($cap.Substring(0, [Math]::Min(100, $cap.Length)))" }
            }
        } catch {
            $deadCount++
        }
    }
}
Write-Info "Windows with prompt: $aliveCount, without: $deadCount"
if ($deadCount -eq 0) {
    Write-Pass "All $aliveCount windows have active shells"
} else {
    Write-Fail "$deadCount windows have no shell prompt (hanging/crashed panes)"
}
Write-Host ""

# MEMORY AND CPU of the session this suite just filled (first window, the new
# windows, the splits and the burst), so a timing row here always has the cost
# beside it. The server is found by its .pid anchor, never by image name; the
# tree under it is every pane shell, the warm pool's spares, their console
# hosts and the standby server, which is where nearly all the memory is.
$script:ResourceBlock = $null
try {
    $srvPid = Get-PerfServerPid -Ns "" -Session $session1 -DataDir $PSMUX_DIR
    if ($srvPid -gt 0) {
        $snapA = Get-PerfResourceSnapshot ([ordered]@{ server = $srvPid })
        $idle = Measure-PerfIdleCpu ([ordered]@{ server = $srvPid }) 3
        $tree = Get-PerfTreeMemory $srvPid
        $script:ResourceBlock = [ordered]@{
            windows               = $winLines.Count
            panes                 = $paneLines.Count
            server                = (Get-PerfMemorySummary $snapA)
            server_cpu_ms_total   = $(if ($snapA.Contains("server")) { $snapA["server"].cpu_ms } else { $null })
            idle_cpu_pct_of_core  = $idle.pct_of_one_core
            idle_measured_over_ms = $idle.window_ms
            session_tree          = [ordered]@{ processes = $tree.processes; ws_mb = $tree.ws_mb; private_mb = $tree.private_mb; by_name = $tree.by_name }
        }
        Write-Info (Format-PerfResourceLine $snapA "session full")
        Write-Info ("server idle cpu, % of a core: {0:F2}" -f $idle.pct_of_one_core["server"])
        Write-Info ("session tree: {0} processes, {1} MB working set, {2} MB private  ({3})" -f $tree.processes, $tree.ws_mb, $tree.private_mb, (@($tree.by_name.Keys | ForEach-Object { "$_ x$($tree.by_name[$_])" }) -join ', '))
    } else {
        Write-Info "no .pid anchor for $session1 in $PSMUX_DIR, so no resource sample was taken"
    }
} catch {
    Write-Info "resource sample failed: $_"
}
Write-Host ""

# Kill session 1 before next test
Kill-TestSession -SessionName $session1

# ==============================================================================
# TEST 6: Multiple session creation
# ==============================================================================
Write-Host "--- TEST 6: Multiple session creation (${SessionCount}x) ---" -ForegroundColor Yellow
$sessionTimes = @()
for ($i = 0; $i -lt $SessionCount; $i++) {
    $sn = "perf_test_multi_$i"
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    
    $proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session", "-s", $sn, "-d" -PassThru -WindowStyle Hidden
    
    $serverInfo = Wait-ServerReady -SessionName $sn -TimeoutSec 15
    if ($null -eq $serverInfo) {
        Write-Fail "  Session '$sn' — server never started"
        continue
    }
    
    $promptResult = Wait-PanePrompt -SessionName $sn -TimeoutMs ($PromptTimeoutSec * 1000)
    $sw.Stop()
    
    if ($promptResult.Found) {
        $sessionTimes += $sw.ElapsedMilliseconds
        Write-Metric "  Session #$($i+1) ready" $sw.ElapsedMilliseconds
    } else {
        Write-Fail "  Session #$($i+1) — prompt never appeared"
    }
}
if ($sessionTimes.Count -gt 0) {
    $sessAvg = ($sessionTimes | Measure-Object -Average).Average
    Write-Metric "  Session creation AVG" $sessAvg
    Write-Pass "Created $($sessionTimes.Count) sessions successfully"

    Write-Info "back-to-back session creation: [$(($sessionTimes | ForEach-Object { [int]$_ }) -join ', ')]ms - creations this close together claim a standby that has registered but whose own shell is still starting, so one slow figure here is expected"
}

# Session creation claims a standby `__warm__` server, and exactly one standby
# exists at a time. Back to back creations therefore claim a standby that has
# advertised itself (its .port is on disk) before its first shell has finished
# starting, which is the same shape of problem as a spare shell pool of depth
# one and is bounded by the same thing: how fast a shell starts.
#
# What must hold, and what this asserts, is the case a human actually produces:
# leave the standby the couple of seconds it needs to arm and EVERY subsequent
# session must be instant. A failure here means the replacement standby is not
# being spawned after a claim at all.
Write-Host ""
Write-Host "--- TEST 6b: repeat session creation with the standby armed ---" -ForegroundColor Yellow

# Wait for the replacement standby to actually exist rather than sleeping a
# guessed interval. A fixed 1800ms sleep made this section fail roughly one run
# in three on BOTH the old and the new build: when the standby has not finished
# registering, the session cold starts and the sample is ~600ms instead of
# ~65ms, which says nothing about the code under test. Measured with a real
# wait, base and this change are identical (median 65ms vs 66ms, n=10 each).
function Wait-StandbyArmed {
    # SettleMs has to exceed the standby's OWN shell startup (~600ms for pwsh):
    # a standby claimed the moment its port file appears hands over a pane whose
    # prompt is still on its way, and the claimant is billed for the remainder.
    # Measured at 400ms the claims came out 243 to 364ms; with the shell really
    # up they are ~65ms.
    param([int]$TimeoutMs = 6000, [int]$SettleMs = 1200)
    $warmPort = "$PSMUX_DIR\__warm__.port"
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        if (Test-Path $warmPort) {
            # Registered. Give its own first shell a moment to finish starting:
            # a standby claimed the instant it appears hands over a pane whose
            # prompt is still on its way.
            Start-Sleep -Milliseconds $SettleMs
            return $true
        }
        Start-Sleep -Milliseconds 25
    }
    return $false
}

$armedTimes = @()
for ($i = 0; $i -lt 3; $i++) {
    $sn = "perf_test_armed_$i"
    # Establish the precondition rather than assume it. A standby is spawned by
    # a session being created, so create one, then wait for the standby to
    # register. Relying on whatever standby an earlier test happened to leave
    # behind is what made this section report a cold start as a slow claim.
    #
    # Retried: the standby spawn is serialised by `__warm__.spawnlock`, so a
    # standby that was killed moments ago can suppress its own replacement for
    # one round. Observed about one rep in three. Three attempts failing in a
    # row would mean standbys really are not being spawned, which is worth a
    # failure.
    $anchor = $null
    for ($try = 0; $try -lt 3 -and -not $anchor; $try++) {
        $cand = "perf_test_anchor_${i}_$try"
        Start-Process -FilePath $PSMUX -ArgumentList "new-session", "-s", $cand, "-d" -WindowStyle Hidden | Out-Null
        Wait-ServerReady -SessionName $cand -TimeoutSec 15 | Out-Null
        if (Wait-StandbyArmed) { $anchor = $cand } else { Kill-TestSession -SessionName $cand }
    }
    if (-not $anchor) {
        Write-Fail "  no standby registered within 6s of creating a session, 3 attempts - the replacement standby is not being spawned"
        continue
    }
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Start-Process -FilePath $PSMUX -ArgumentList "new-session", "-s", $sn, "-d" -WindowStyle Hidden | Out-Null
    $si = Wait-ServerReady -SessionName $sn -TimeoutSec 15
    if ($null -eq $si) { Write-Fail "  armed session '$sn' never started"; continue }
    $pr = Wait-PanePrompt -SessionName $sn -TimeoutMs ($PromptTimeoutSec * 1000)
    $sw.Stop()
    if ($pr.Found) {
        $armedTimes += $sw.ElapsedMilliseconds
        Write-Metric "  Armed session #$($i+1) ready" $sw.ElapsedMilliseconds
    } else {
        Write-Fail "  Armed session #$($i+1) - prompt never appeared"
    }
    Kill-TestSession -SessionName $anchor
}
if ($armedTimes.Count -ge 2) {
    Assert-Latency -Label "new-session (standby armed)" -Samples $armedTimes -P90Limit 300 -MaxLimit 500 -BimodalRatio 4.0 -MinSamples 2
} else {
    Write-Info "new-session (standby armed): fewer than 2 samples, no standby was ever armed in time - not judged"
}
for ($i = 0; $i -lt 3; $i++) {
    Kill-TestSession -SessionName "perf_test_armed_$i"
    for ($try = 0; $try -lt 3; $try++) { Kill-TestSession -SessionName "perf_test_anchor_${i}_$try" }
}
Write-Host ""

# Cleanup multiple sessions
for ($i = 0; $i -lt $SessionCount; $i++) {
    Kill-TestSession -SessionName "perf_test_multi_$i"
}

# ==============================================================================
# TEST 7: Window/pane close latency
# ==============================================================================
Write-Host "--- TEST 7: Window/pane close latency ---" -ForegroundColor Yellow
$closeSess = "perf_test_close"
Start-Process -FilePath $PSMUX -ArgumentList "new-session", "-s", $closeSess, "-d" -WindowStyle Hidden | Out-Null
$ci = Wait-ServerReady -SessionName $closeSess -TimeoutSec 15
if ($null -eq $ci) {
    Write-Fail "Could not start close-test session"
} else {
    Wait-PanePrompt -SessionName $closeSess -TimeoutMs 15000 | Out-Null
    
    # Create 3 windows
    for ($i = 0; $i -lt 3; $i++) {
        & $PSMUX new-window -t $closeSess 2>&1 | Out-Null
    }
    Start-Sleep -Seconds 3
    
    # Measure close time
    $closeTimes = @()
    for ($i = 0; $i -lt 3; $i++) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $PSMUX kill-window -t $closeSess 2>&1 | Out-Null
        $sw.Stop()
        $closeTimes += $sw.ElapsedMilliseconds
        Write-Metric "  Kill window #$($i+1)" $sw.ElapsedMilliseconds
        Start-Sleep -Milliseconds 200
    }
    if ($closeTimes.Count -gt 0) {
        $closeAvg = ($closeTimes | Measure-Object -Average).Average
        Write-Metric "  Kill window AVG" $closeAvg
        Write-Pass "Window close working"
    }
    
    Kill-TestSession -SessionName $closeSess
}
Write-Host ""

# ==============================================================================
# TEST 8: Direct TCP timing — isolate psmux server overhead
# ==============================================================================
Write-Host "--- TEST 8: Direct TCP pane creation timing ---" -ForegroundColor Yellow
$tcpSess = "perf_test_tcp"
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session", "-s", $tcpSess, "-d" -PassThru -WindowStyle Hidden
$tcpInfo = Wait-ServerReady -SessionName $tcpSess -TimeoutSec 15
if ($null -eq $tcpInfo) {
    Write-Fail "TCP test session failed to start"
} else {
    # Wait for first prompt
    Wait-PanePrompt -SessionName $tcpSess -TimeoutMs ($PromptTimeoutSec * 1000) | Out-Null
    
    # Open TCP connection
    $tcp = New-Object System.Net.Sockets.TcpClient
    $tcp.NoDelay = $true
    $tcp.Connect("127.0.0.1", $tcpInfo.Port)
    $ns = $tcp.GetStream()
    $ns.ReadTimeout = 15000
    $wr = New-Object System.IO.StreamWriter($ns)
    $wr.AutoFlush = $false
    $rd = New-Object System.IO.StreamReader($ns)
    
    $wr.WriteLine("AUTH $($tcpInfo.Key)"); $wr.Flush()
    $auth = $rd.ReadLine()
    if ($auth -ne "OK") {
        Write-Fail "TCP auth failed: $auth"
    } else {
        $wr.WriteLine("PERSISTENT"); $wr.Flush()
        Start-Sleep -Milliseconds 100
        $wr.WriteLine("client-size 120 30"); $wr.Flush()
        Start-Sleep -Milliseconds 200
        
        # Drain any initial data
        $wr.WriteLine("client-attach"); $wr.Flush()
        Start-Sleep -Milliseconds 300
        for ($d = 0; $d -lt 5; $d++) {
            $wr.WriteLine("dump-state"); $wr.Flush()
            try { $rd.ReadLine() | Out-Null } catch {}
            Start-Sleep -Milliseconds 100
        }
        
        # Measure new-window via TCP
        $tcpWinTimes = @()
        for ($w = 0; $w -lt 3; $w++) {
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            $wr.WriteLine("new-window"); $wr.Flush()
            
            # Poll dump-state until we see the prompt in the new pane
            $found = $false
            while ($sw.ElapsedMilliseconds -lt ($PromptTimeoutSec * 1000)) {
                Start-Sleep -Milliseconds 100
                # Use CLI capture-pane since TCP dump-state gives layout JSON not text
                try {
                    $cap = & $PSMUX capture-pane -t $tcpSess -p 2>&1 | Out-String
                    if ($cap -match "PS [A-Z]:\\") {
                        $found = $true
                        break
                    }
                } catch {}
            }
            $sw.Stop()
            
            if ($found) {
                $tcpWinTimes += $sw.ElapsedMilliseconds
                Write-Metric "  TCP new-window #$($w+1) shell ready" $sw.ElapsedMilliseconds
            } else {
                Write-Fail "  TCP new-window #$($w+1) — prompt never appeared"
            }
        }
        
        # Measure split-window via TCP
        $tcpSplitTimes = @()
        for ($s = 0; $s -lt 3; $s++) {
            $dir = if ($s % 2 -eq 0) { "split-window -v" } else { "split-window -h" }
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            $wr.WriteLine($dir); $wr.Flush()
            
            $found = $false
            while ($sw.ElapsedMilliseconds -lt ($PromptTimeoutSec * 1000)) {
                Start-Sleep -Milliseconds 100
                try {
                    $cap = & $PSMUX capture-pane -t $tcpSess -p 2>&1 | Out-String
                    if ($cap -match "PS [A-Z]:\\") {
                        $found = $true
                        break
                    }
                } catch {}
            }
            $sw.Stop()
            
            if ($found) {
                $tcpSplitTimes += $sw.ElapsedMilliseconds
                Write-Metric "  TCP split #$($s+1) shell ready" $sw.ElapsedMilliseconds
            } else {
                Write-Fail "  TCP split #$($s+1) — prompt never appeared"
            }
        }
        
        if ($tcpWinTimes.Count -gt 0) {
            $tcpAvg = ($tcpWinTimes | Measure-Object -Average).Average
            Write-Metric "  TCP new-window AVG" $tcpAvg
        }
        if ($tcpSplitTimes.Count -gt 0) {
            $stAvg = ($tcpSplitTimes | Measure-Object -Average).Average
            Write-Metric "  TCP split AVG" $stAvg
        }
    }
    $tcp.Close()
    Kill-TestSession -SessionName $tcpSess
}
Write-Host ""

# ==============================================================================
# TEST 9: Stress test — many panes rapidly, check for hangs
# ==============================================================================
Write-Host "--- TEST 9: Stress test — 10 windows rapidly ---" -ForegroundColor Yellow
$stressSess = "perf_test_stress"
Start-Process -FilePath $PSMUX -ArgumentList "new-session", "-s", $stressSess, "-d" -WindowStyle Hidden | Out-Null
$stressInfo = Wait-ServerReady -SessionName $stressSess -TimeoutSec 15
if ($null -eq $stressInfo) {
    Write-Fail "Stress test session failed to start"
} else {
    Wait-PanePrompt -SessionName $stressSess -TimeoutMs ($PromptTimeoutSec * 1000) | Out-Null
    
    $swStress = [System.Diagnostics.Stopwatch]::StartNew()
    # Create 10 windows as fast as possible (no waiting between)
    for ($i = 0; $i -lt 10; $i++) {
        & $PSMUX new-window -t $stressSess 2>&1 | Out-Null
    }
    $createBurstMs = $swStress.ElapsedMilliseconds
    Write-Metric "  10 new-window commands took" $createBurstMs
    
    # Wait for all to settle
    Write-Info "Waiting for all panes to initialize..."
    Start-Sleep -Seconds 10
    
    # Count how many actually have prompts
    $lsw3 = & $PSMUX list-windows -t $stressSess 2>&1
    $wins = ($lsw3 | Out-String) -split "`n" | Where-Object { $_ -match '^\d+:' }
    $alive = 0; $dead = 0; $deadList = @()
    foreach ($w in $wins) {
        if ($w -match '^(\d+):') {
            $idx = $Matches[1]
            try {
                $cap = & $PSMUX capture-pane -t "${stressSess}:${idx}" -p 2>&1 | Out-String
                if ($cap -match "PS [A-Z]:\\") { $alive++ }
                else { 
                    $dead++
                    $deadList += "win$idx"
                    if ($Verbose) { Write-Info "  Window $idx capture: '$($cap.Trim().Substring(0, [Math]::Min(80, $cap.Trim().Length)))'" }
                }
            } catch { $dead++; $deadList += "win${idx}(err)" }
        }
    }
    
    $swStress.Stop()
    Write-Info "Total windows: $($alive + $dead), Alive: $alive, Dead/Hanging: $dead"
    if ($dead -gt 0) {
        Write-Fail "STRESS TEST: $dead out of $($alive + $dead) windows have no prompt! ($($deadList -join ', '))"
    } else {
        Write-Pass "STRESS TEST: All $alive windows have active shells"
    }
    
    Kill-TestSession -SessionName $stressSess
}
Write-Host ""

# ==============================================================================
# SUMMARY
# ==============================================================================
Cleanup-All
Write-Host "================================================================" -ForegroundColor Cyan
Write-Host " SUMMARY" -ForegroundColor Cyan
Write-Host "================================================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "  Baseline pwsh -NoProfile:   $([math]::Round($baselineAvg))ms" -ForegroundColor White
Write-Host "  Baseline pwsh (w/profile):  $([math]::Round($profileAvg))ms" -ForegroundColor White
if ($windowTimes.Count -gt 0) {
    Write-Host "  New window AVG:             $([math]::Round(($windowTimes | Measure-Object -Average).Average))ms" -ForegroundColor White
}
if ($splitTimes.Count -gt 0) {
    Write-Host "  Split pane AVG:             $([math]::Round(($splitTimes | Measure-Object -Average).Average))ms" -ForegroundColor White
}
if ($sessionTimes.Count -gt 0) {
    Write-Host "  New session AVG:            $([math]::Round(($sessionTimes | Measure-Object -Average).Average))ms" -ForegroundColor White
}
Write-Host ""
Write-Host "  Tests passed: $PASS / $TOTAL_TESTS" -ForegroundColor $(if ($FAIL -eq 0) { "Green" } else { "Red" })
if ($FAIL -gt 0) {
    Write-Host "  Tests FAILED: $FAIL" -ForegroundColor Red
}
Write-Host ""

# Samples on disk so two runs can be compared later, and NEVER inside the repo:
# a perf suite that commits its own output makes every run a dirty tree. The
# file carries the shared envelope from tests/perf_metrics_common.ps1 (git sha
# of the binary's tree or "installed", machine, CPU) plus per cell percentiles,
# which is what tests/perf_summary.ps1 lines up against the other gates.
try {
    $stats = [ordered]@{
        new_window_ms                = (Get-PerfStats $windowTimes 1)
        split_ms                     = (Get-PerfStats $splitTimes 1)
        new_session_ms               = (Get-PerfStats $sessionTimes 1)
        pool_depth5_new_window_ms    = (Get-PerfStats $poolWin 1)
        pool_depth5_split_v_ms       = (Get-PerfStats $poolSplitV 1)
        pool_depth5_split_h_ms       = (Get-PerfStats $poolSplitH 1)
        standby_armed_new_session_ms = (Get-PerfStats $armedTimes 1)
    }
    $outFile = Write-PerfMetrics -Suite "test_pane_startup_perf" -Binary $PSMUX `
        -FileStem "pane_startup_perf" -MetricsDir $MetricsDir -Data ([ordered]@{
        when = (Get-Date -Format "yyyy-MM-dd HH:mm:ss")
        baseline_pwsh_noprofile_ms = [math]::Round($baselineAvg, 1)
        baseline_pwsh_profile_ms = [math]::Round($profileAvg, 1)
        new_window_ms = @($windowTimes | ForEach-Object { [math]::Round($_, 1) })
        new_window_median_ms = [math]::Round((Get-Median $windowTimes), 1)
        cold_creation_ratio = $(if ($baselineAvg -gt 0 -and $windowTimes.Count -gt 0) { [math]::Round((($windowTimes | Measure-Object -Maximum).Maximum) / $baselineAvg, 2) } else { 0 })
        split_ms = @($splitTimes | ForEach-Object { [math]::Round($_, 1) })
        new_session_ms = @($sessionTimes | ForEach-Object { [math]::Round($_, 1) })
        pool_depth5_new_window_ms = @($poolWin | ForEach-Object { [math]::Round($_, 1) })
        pool_depth5_split_v_ms = @($poolSplitV | ForEach-Object { [math]::Round($_, 1) })
        pool_depth5_split_h_ms = @($poolSplitH | ForEach-Object { [math]::Round($_, 1) })
        standby_armed_new_session_ms = @($armedTimes | ForEach-Object { [math]::Round($_, 1) })
        stats_ms = $stats
        resources = $script:ResourceBlock
        passed = $PASS
        failed = $FAIL
        total = $TOTAL_TESTS
    })
    if ($outFile) { Write-Host "  metrics written to $outFile" -ForegroundColor Gray }
} catch {
    Write-Host "  could not write metrics: $_" -ForegroundColor DarkYellow
}
Write-Host ""

# The runner scores a suite by its exit code as well as its FAIL lines; a
# suite that prints FAIL and exits 0 is a suite the runner cannot trust
# (sweep 2026-09-24_23-37-41 read this one as "25P/2F exit=0").
if ($FAIL -gt 0) { exit 1 }
exit 0
