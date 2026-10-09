# Issue #686: the warm pool's surge spawns were serialised, and kill-server
# leaked the spares that were still in flight.
#
# WHAT WAS REPRODUCED (worktree build of master d18b389, ten `new-window` back
# to back on one session, PSMUX_WARM_TRACE=1):
#
#   pool: spawned spare pane=4  in  44.8ms
#   pool: spawned spare pane=5  in  69.7ms
#   pool: spawned spare pane=6  in 124.8ms
#   pool: spawned spare pane=9  in 180.6ms
#   pool: spawned spare pane=10 in 239.9ms
#   pool: spawned spare pane=7  in 302.3ms
#   pool: spawned spare pane=8  in 345.0ms
#   pool: spawned spare pane=11 in 397.9ms
#
#   Eight spawns started together and each cost about 55ms more than the one
#   before it, because every ConPTY spawn held the process console state lock
#   for the whole of its CreateProcessW. A claim arriving while the queue had
#   landed nothing waited for the next landing, which is where the stalls came
#   from: per call 31 18 19 41 453 535 47 83 546 50.
#
#   And: three rounds of "new-session -d, six new-window back to back,
#   kill-server immediately" left orphan `pwsh` pane shells whose parent psmux
#   was gone, each holding a conhost, idle forever. Six over ten rounds.
#
# WHAT THIS SUITE PINS
#
#   1. CONCURRENCY. Spawns whose lifetimes overlap must cost about the same.
#      The serialised build is not visible as "slow", it is visible as a
#      staircase: eight spawns that all START together and finish one whole
#      CreateProcessW apart. So the assertion is on the SPREAD of the costs of
#      spawns that overlap in time, which was ~322ms serialised and is under
#      30ms concurrent, and does not depend on how fast this machine is.
#
#      The spread is taken over what psmux owns of each spawn: its whole cost
#      minus the time inside CreateProcessW, which the trace line reports
#      separately. A psmux lock that serialises spawns makes the waiting ones
#      wait OUTSIDE their own CreateProcessW, so it still shows in full. What
#      no longer counts is Windows: the Store pwsh is created through package
#      activation, and measured with no psmux and no job object at all (an
#      unpackaged C# probe, eight threads, Windows 11 26200), eight concurrent
#      CreateProcessW calls of it took ~300ms each and returned in batches at
#      the same instant, one batch 86 to 250ms after another, where cmd.exe
#      took 2 to 17ms. That batching failed this gate on builds with and
#      without the pane job object (spread 154.6ms on 10/02 before the job
#      existed, 185.9ms after: three spares landing together and a fourth
#      one batch later, its whole delay inside CreateProcessW). The whole cost
#      spread is still printed.
#
#   2. The user visible number that follows from it: the p50 of a ten call
#      burst. This one is a guard rather than the discriminator, because the
#      serialised build's p50 was 57ms with the damage in its tail. It is here
#      because the first attempt at the fix, unserialising the spawns with no
#      cap on how many run at once, took the p50 to 251ms: eight concurrent
#      spawns are slow enough that every claim cold spawns and retires the
#      batch in flight. Under 110ms means neither failure is present.
#
#   3. ZERO orphan pane shells after a kill-server with a surge in flight.
#      Orphans are counted only as pwsh children of the server pid THIS suite
#      started, so a sibling psmux on the machine is never miscounted and never
#      touched.
#
# Set PSMUX_TEST_BIN to test a non-installed binary.
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue686_pool_surge_and_reap.ps1
param(
    [string]$Binary = "",
    [int]$Burst = 10,
    # How far apart two overlapping spawns may cost. Serialised: ~322ms.
    # Concurrent, on a machine also running a sweep: under 60ms.
    [int]$SpreadLimitMs = 90,
    # The burst p50. Serialised it was 57 to 60ms with 450 to 550ms outliers;
    # the point of the gate is that a MAJORITY of the calls are not stalls.
    [int]$P50LimitMs = 110,
    [int]$LeakRounds = 3
)

$ErrorActionPreference = "Continue"
$script:Pass = 0
$script:Fail = 0
function Write-Pass { param($m) Write-Host "[PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail { param($m) Write-Host "[FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info { param($m) Write-Host "[INFO] $m" -ForegroundColor Cyan }
function Write-Test { param($m) Write-Host "[TEST] $m" -ForegroundColor White }

if (-not $Binary -and $env:PSMUX_TEST_BIN) { $Binary = $env:PSMUX_TEST_BIN }
if (-not $Binary -and $env:PSMUX_TEST_BINARY) { $Binary = $env:PSMUX_TEST_BINARY }
if (-not $Binary) {
    foreach ($n in @("psmux.exe", "pmux.exe", "tmux.exe")) {
        $c = Join-Path $PSScriptRoot "..\target\release\$n"
        if (Test-Path $c) { $Binary = $c; break }
    }
}
if (-not $Binary) {
    $cmd = Get-Command psmux -ErrorAction SilentlyContinue
    if ($cmd) { $Binary = $cmd.Source }
}
if (-not $Binary -or -not (Test-Path $Binary)) { Write-Fail "no psmux binary found"; exit 1 }
$Binary = (Resolve-Path $Binary).Path
$imgName = [IO.Path]::GetFileNameWithoutExtension($Binary).ToLower()
if ($imgName -notin @("psmux", "pmux", "tmux")) {
    Write-Fail "'$imgName' is not a recognised server image name; the warm pool would be off"
    exit 1
}
Write-Info "Using: $Binary"

# The pool IS the subject, so an opted-out environment would test nothing.
if ($env:PSMUX_NO_WARM -eq "1" -or $env:PSMUX_NO_WARM -eq "true") {
    Write-Info "PSMUX_NO_WARM is set: the pool is off, nothing to measure"
    exit 0
}

$Ns = "i686$PID"
$DataDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR.TrimEnd('\', '/') } else { "$env:USERPROFILE\.psmux" }
$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION = $null

function Cleanup {
    try { & $Binary -L $Ns kill-server 2>&1 | Out-Null } catch {}
    Start-Sleep -Milliseconds 500
    Get-ChildItem "$DataDir\$($Ns)__*" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue
}

function Get-ServerPid {
    # By command line, never by image name alone: other psmux servers belong to
    # other people and must be neither counted nor killed.
    $p = Get-CimInstance Win32_Process -Filter "Name='$imgName.exe'" -EA SilentlyContinue |
         Where-Object { $_.CommandLine -match [regex]::Escape("-L $Ns") -and $_.CommandLine -match 'server' }
    if ($p) { return @($p)[0].ProcessId }
    return 0
}

Cleanup

# ---------------------------------------------------------------- 1. the burst
Write-Test "ten new-window back to back: spawn cost is flat across concurrent spawns"

$TraceFile = Join-Path ([IO.Path]::GetTempPath()) "psmux_i686_$PID.log"
if (Test-Path $TraceFile) { Remove-Item $TraceFile -Force -EA SilentlyContinue }
$env:PSMUX_WARM_TRACE = "1"
$env:PSMUX_WARM_TRACE_FILE = $TraceFile

& $Binary -L $Ns new-session -d -s burst 2>&1 | Out-Null
Start-Sleep -Milliseconds 1500
$srvPid = Get-ServerPid
if ($srvPid -eq 0) { Write-Fail "no server started for -L $Ns"; Cleanup; exit 1 }

$per = @()
for ($i = 1; $i -le $Burst; $i++) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $Binary -L $Ns new-window -t burst: 2>&1 | Out-Null
    $sw.Stop()
    $per += [int]$sw.ElapsedMilliseconds
}
Start-Sleep -Milliseconds 1200

$sorted = $per | Sort-Object
$p50 = $sorted[[int]([math]::Floor($sorted.Count / 2))]
$mean = [math]::Round((($per | Measure-Object -Sum).Sum / $per.Count), 1)
Write-Info "per call: $($per -join ' ')"
Write-Info "p50=$p50 mean=$mean max=$($sorted[-1])"

if (-not (Test-Path $TraceFile)) {
    Write-Fail "the warm trace was never written ($TraceFile); PSMUX_WARM_TRACE is how this suite sees the pool"
} else {
    # Only THIS server's lines: a `__warm__` standby writes to the same file.
    $lines = Get-Content $TraceFile | Where-Object { $_ -match "pid=$srvPid\]" }
    $spawns = @()
    foreach ($l in $lines) {
        if ($l -match '^\[\s*([\d\.]+)\s+pid=\d+\]\s+pool: spawned spare pane=(\d+).* in ([\d\.]+)ms') {
            $end = [double]$Matches[1]
            $cost = [double]$Matches[3]
            $pane = [int]$Matches[2]
            # The OS share; absent from builds older than the field.
            $os = if ($l -match 'in CreateProcessW ([\d\.]+)ms') { [double]$Matches[1] } else { 0.0 }
            $spawns += [pscustomobject]@{ Pane = $pane; End = $end; Start = $end - $cost; Cost = $cost; Os = $os; Own = $cost - $os }
        }
    }
    Write-Info "$($spawns.Count) spare spawns traced"
    if ($spawns.Count -lt 4) {
        Write-Fail "only $($spawns.Count) spawns traced; the burst never surged, so there is nothing to measure"
    } else {
        # Group by START, not by overlap: the staircase is precisely "several
        # spawns begin together and finish one CreateProcessW apart", so the
        # spawns that began within a few milliseconds of each other are the
        # ones whose costs must agree. Grouping by overlap instead would mix
        # consecutive waves, whose costs legitimately differ with machine load.
        $worstSpread = 0.0
        $worstGroup = $null
        $worstWhole = 0.0
        $worstWholeGroup = $null
        foreach ($a in $spawns) {
            $group = @($spawns | Where-Object { [math]::Abs($_.Start - $a.Start) -le 30 })
            if ($group.Count -lt 4) { continue }
            $own = $group | ForEach-Object { $_.Own }
            $spread = (($own | Measure-Object -Maximum).Maximum) - (($own | Measure-Object -Minimum).Minimum)
            if ($spread -gt $worstSpread) { $worstSpread = $spread; $worstGroup = $group }
            $costs = $group | ForEach-Object { $_.Cost }
            $whole = (($costs | Measure-Object -Maximum).Maximum) - (($costs | Measure-Object -Minimum).Minimum)
            if ($whole -gt $worstWhole) { $worstWhole = $whole; $worstWholeGroup = $group }
        }
        if ($null -eq $worstGroup -and $null -eq $worstWholeGroup) {
            Write-Fail "no four spare spawns ever started together; the surge did not run concurrently at all"
        } else {
            if ($null -eq $worstGroup) { $worstGroup = $worstWholeGroup }
            $fmt = { param($g) ($g | Sort-Object Pane | ForEach-Object { "pane=$($_.Pane):$([math]::Round($_.Cost,1))ms(os $([math]::Round($_.Os,1)))" }) -join ' ' }
            Write-Info "whole cost spread $([math]::Round($worstWhole,1))ms, Windows included: $(& $fmt $worstWholeGroup)"
            Write-Info "worst group by psmux's own share ($($worstGroup.Count) spawns): $(& $fmt $worstGroup)"
            if ($worstSpread -le $SpreadLimitMs) {
                Write-Pass "spawns that started together spent within $([math]::Round($worstSpread,1))ms of each other outside CreateProcessW (limit ${SpreadLimitMs}ms): the surge is not serialised"
            } else {
                Write-Fail "spawns that started together are a staircase: psmux's own share spreads $([math]::Round($worstSpread,1))ms over $($worstGroup.Count) of them (limit ${SpreadLimitMs}ms) -- each is queued behind the previous one's CreateProcessW"
            }
        }
    }
}

# The p50 is judged on what psmux owns of it, like the spread above. Once the
# burst drains the pool a call waits for the spare already in flight to land,
# so its cost is one CreateProcessW of the Store pwsh: 40 to 60 ms when package
# activation is warm, 158 to 185 ms when several activations run beside it
# (run 2026-10-04_02-20-44: whole cost 164 to 186 ms of which 158 to 180 inside
# CreateProcessW, burst p50 166 against the 110 limit, with the spread gate
# passing at 21.8 ms). The 110 was calibrated on 2026-09-16 with the warm OS
# cost inside it, so it read Windows' activation tiers as psmux stalling.
#
# Two gates replace the bare number. The regression the old gate was set
# against (unserialised spawns with no cap: "every claim cold spawns and
# retires the batch in flight") shows in the trace as claims with
# got_warming=false, so that is counted directly: at most one claim in the
# burst may cold spawn. Then the p50 is bounded by one CreateProcessW of this
# run's own spawns (their median) plus 60 ms for the pool's bookkeeping, the
# landing, the claim and the CLI round trip, or by the old 110 when the
# machine is quick enough for that to hold. Without a trace the whole p50 is
# judged as before.
$osMedian = 0.0
$coldClaims = -1
if ($spawns -and $spawns.Count -ge 4) {
    $osSorted = @($spawns | ForEach-Object { $_.Os } | Sort-Object)
    $osMedian = [double]$osSorted[[int]([math]::Floor($osSorted.Count / 2))]
    $misses = @($lines | Where-Object { $_ -match 'claim\(new-window\): NO READY SPARE \(depth=\d+ got_warming=(true|false)\)' })
    $coldClaims = @($misses | Where-Object { $_ -match 'got_warming=false' }).Count
    Write-Info "burst p50 ${p50}ms whole; $($misses.Count) of $Burst calls found no ready spare, $coldClaims cold spawned; median CreateProcessW of this run's spawns $([math]::Round($osMedian,1))ms"
}
if ($coldClaims -ge 0) {
    if ($coldClaims -le 1) {
        Write-Pass "at most one call in the burst cold spawned ($coldClaims): the surge served the rest from spares already in flight"
    } else {
        Write-Fail "$coldClaims calls in the burst cold spawned: the surge is not keeping spares in flight ahead of the claims"
    }
    $p50Limit = [math]::Max($P50LimitMs, [int][math]::Round($osMedian + 60))
    if ($p50 -le $p50Limit) {
        Write-Pass "burst p50 ${p50}ms is within one CreateProcessW plus 60ms (limit ${p50Limit}ms)"
    } else {
        Write-Fail "burst p50 ${p50}ms exceeds one CreateProcessW plus 60ms (limit ${p50Limit}ms): the majority of a burst is stalling on the pool"
    }
} elseif ($p50 -le $P50LimitMs) {
    Write-Pass "burst p50 ${p50}ms is within ${P50LimitMs}ms"
} else {
    Write-Fail "burst p50 ${p50}ms exceeds ${P50LimitMs}ms: the majority of a burst is stalling on the pool"
}

Cleanup

# ------------------------------------------------------- 2. the teardown leak
Write-Test "kill-server with a surge in flight leaves no orphan pane shells"

$leaked = 0
for ($r = 1; $r -le $LeakRounds; $r++) {
    & $Binary -L $Ns new-session -d -s "leak$r" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900
    $srvPid = Get-ServerPid
    if ($srvPid -eq 0) { Write-Fail "round ${r}: no server"; continue }
    # Six creations back to back open a surge, then the kill lands in the
    # middle of it: several spares exist whose spawn has not reached the
    # server loop yet.
    for ($i = 1; $i -le 6; $i++) { & $Binary -L $Ns new-window -t "leak${r}:" 2>&1 | Out-Null }
    & $Binary -L $Ns kill-server 2>&1 | Out-Null
    Start-Sleep -Seconds 4

    $stillAlive = @(Get-CimInstance Win32_Process -Filter "ProcessId=$srvPid" -EA SilentlyContinue)
    if ($stillAlive.Count -gt 0) { Write-Fail "round ${r}: server $srvPid survived kill-server" }
    # Children of a pid that is gone: only this suite's own pane shells can
    # match, because the parent pid is the server we just started.
    $orph = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$srvPid" -EA SilentlyContinue)
    if ($orph.Count -gt 0) {
        Write-Info "round ${r}: $($orph.Count) orphan(s): $(($orph | ForEach-Object { "$($_.Name)/$($_.ProcessId)" }) -join ' ')"
        $leaked += $orph.Count
        foreach ($o in $orph) { try { Stop-Process -Id $o.ProcessId -Force -EA Stop } catch {} }
    }
    Start-Sleep -Milliseconds 300
}
if ($leaked -eq 0) {
    Write-Pass "no orphan pane shells over $LeakRounds rounds of kill-server during a surge"
} else {
    Write-Fail "$leaked orphan pane shell(s) over $LeakRounds rounds: a spare whose spawn was in flight outlived the server"
}

Cleanup

# ------------------------------------------- 3. the teardown leak, made certain
# The rounds above only catch the leak when Windows happens to be slow. A sweep
# whose median Store pwsh CreateProcessW was 736 ms left 3 orphans in one round
# (2026-10-09): the reaper waited 400 ms twice, and a spawn still inside
# CreateProcessW after that was ended by process::exit with its shell created
# SUSPENDED and not yet in its job (the Store pwsh refuses the job list, so it
# is assigned after creation). PSMUX_TEST_SLOW_CREATE_MS holds every created
# shell for that long before the assign, which reproduces the window every
# time: 9 orphans over 3 rounds before the fix, all suspended pwsh.
Write-Test "kill-server while every spawn is held 1500 ms inside its create leaves no orphan pane shells"
$env:PSMUX_TEST_SLOW_CREATE_MS = "1500"
$slowLeaked = 0
for ($r = 1; $r -le $LeakRounds; $r++) {
    & $Binary -L $Ns new-session -d -s "slow$r" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900
    $srvPid = Get-ServerPid
    if ($srvPid -eq 0) { Write-Fail "slow round ${r}: no server"; continue }
    for ($i = 1; $i -le 6; $i++) { & $Binary -L $Ns new-window -t "slow${r}:" 2>&1 | Out-Null }
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $Binary -L $Ns kill-server 2>&1 | Out-Null
    $killMs = $sw.ElapsedMilliseconds
    Start-Sleep -Seconds 4
    $stillAlive = @(Get-CimInstance Win32_Process -Filter "ProcessId=$srvPid" -EA SilentlyContinue)
    if ($stillAlive.Count -gt 0) { Write-Fail "slow round ${r}: server $srvPid survived kill-server" }
    $orph = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$srvPid" -EA SilentlyContinue)
    Write-Info "slow round ${r}: kill-server took ${killMs}ms, $($orph.Count) orphan(s) $(($orph | ForEach-Object { "$($_.Name)/$($_.ProcessId)" }) -join ' ')"
    if ($orph.Count -gt 0) {
        $slowLeaked += $orph.Count
        foreach ($o in $orph) { try { Stop-Process -Id $o.ProcessId -Force -EA Stop } catch {} }
    }
    Start-Sleep -Milliseconds 300
}
Remove-Item Env:PSMUX_TEST_SLOW_CREATE_MS -ErrorAction SilentlyContinue
if ($slowLeaked -eq 0) {
    Write-Pass "no orphan pane shells over $LeakRounds rounds with every create held 1500 ms"
} else {
    Write-Fail "$slowLeaked orphan pane shell(s) over $LeakRounds rounds with every create held 1500 ms: teardown gave up on a spawn still inside CreateProcessW"
}

Cleanup
Write-Host ""
Write-Host "Passed: $script:Pass  Failed: $script:Fail"
if ($script:Fail -gt 0) { exit 1 } else { exit 0 }
