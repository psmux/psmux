# test_creation_latency_gate.ps1 - the regression gate on how long it takes to
# get a usable pane.
#
# WHAT THIS PINS
#
# Creating a window or a split used to be bimodal: about 15ms when a spare shell
# was claimed and 470 to 630ms when the spare pool had been drained and not yet
# refilled. Measured on master (8615957), ten back to back `new-window` calls
# came out
#
#   15, 470, 15, 476, 31, 488, 15, 520, 31, 504 ms
#
# which averages to a forgettable 257ms and feels like a stutter every other
# time you press the key. An average hides that completely, so this gate asserts
# the SHAPE of the distribution, not its centre: HOW MANY of the ten creations
# are slow (see $SlowBudget), with p90 and max as backstops.
#
# WHY IT IS MEASURED THIS WAY
#
#   - "Ready" means a PROMPT IS VISIBLE in the new pane, not that the command
#     returned. `new-window` returns in well under a millisecond while the pane
#     behind it can still be blank for half a second; timing the command would
#     score the defect as perfect.
#   - Readiness also requires THE ACTIVE PANE ID TO HAVE CHANGED. A split that
#     is refused for lack of room leaves the old pane active with its prompt
#     already on screen, so a prompt match on its own would record a refusal as
#     a 16ms creation. Every sample here is proof a pane was really created.
#   - Commands and polls go over one short lived TCP connection each (~1ms round
#     trip) rather than through the CLI (~30ms of process spawn per poll), so
#     the numbers are the server's behaviour and not the client's.
#   - Splits kill the pane they just created before the next one, because a
#     30 row window has room for only three vertical splits and every later one
#     would be refused.
#
# WHAT ELSE IS TIMED HERE
#
#   variants     the split flags the deep cells do not cover: -f, -b, -bh, -bv,
#                -l in cells and in percent (each checked to have produced the
#                size it asked for), a split issued in a ZOOMED window (checked
#                to really be zoomed first) and a split that carries its own
#                command. Five samples each
#                rather than ten, because these are shape checks on flag
#                handling and not the distribution study the three deep cells
#                are. Verified on 2026-09-22 against the installed 0bcc421 that
#                every one of them really creates a pane and really moves the
#                active pane id, so none of these samples can be a refusal
#                recorded as a fast creation.
#   cold launch  five new-window at a human pace (2 s after the first prompt,
#                then 300 ms apart) on a FRESH server whose pool is still
#                filling. Gated on creation 4: master d19e9905 (depth 2) gave
#                [790, 724, 852] ms here, depth 3 gives 15 to 16 ms.
#   sessions     new-session to a visible prompt, twice: once with the warm
#                server pool allowed to serve the claim and once with
#                PSMUX_NO_WARM=1 so the client has to cold spawn a server. The
#                difference between the two rows IS the warm pool's value, and
#                it is the number that disappears first when the claim breaks.
#   kill         how long it takes to get RID of things: kill-pane and
#                kill-window until the pane or window is really gone from
#                list-panes / list-windows, and kill-session until the port
#                anchor file is gone, which is the point at which the server
#                process has actually exited rather than merely promised to.
#
# THRESHOLDS THAT DO NOT FLAP
#
# This gate used to assert only the tail: how many of ten creations were slow,
# p90, and max. Measured on 2026-09-22 against the SAME installed binary
# (0bcc421) on the same machine, once quiet and once with five sibling agents
# building:
#
#   quiet   new-window  p50  25 ms  p90 110 ms  max  474 ms   slow 3/30  PASS
#   loaded  new-window  p50  35 ms  p90 794 ms  max 1781 ms   slow 8/30  FAIL
#
# Nothing about psmux changed between those two runs. The p50 moved by 10 ms
# because it is the warm pool's own path; the tail moved by a factor of four
# because a creation that has to wait out a cold pwsh start waits out whatever a
# cold pwsh start costs on a busy machine, and no multiplexer can be faster than
# the shell it is starting. So:
#
#   p50 per cell is the HARD gate, always. It is the statistic the pool
#   controls, and the defect this file exists for (no pool at all) puts it at
#   400 ms and over, four times the budget.
#
#   the tail assertions (slow count, p90, max) are hard when the machine was
#   QUIET during the run and are recorded as warnings when it was not. The load
#   is sampled with \Processor(_Total)\% Processor Time around every cell and
#   written into the JSON, so a warning is auditable: a reader can see whether
#   the tail was the product or the afternoon.
#
# Runs in its own `-L` namespace and kills only that namespace, so it cannot
# disturb sessions the developer is using.
param(
    # Defaults to this checkout's target\release build, then PSMUX_TEST_BIN,
    # then the psmux on PATH. Point it at a build to compare two of them. The
    # name must stay one the server recognises as its own image
    # (psmux / pmux / tmux): session.rs gates the warm server claim on it, so a
    # differently named copy silently loses the fast path and the run would be
    # measuring the rename.
    [string]$Binary = "",
    [int]$Count = 10,
    [int]$SettleMs = 2000,
    # What counts as a creation the user notices, and how many of ten are allowed
    # to be one.
    #
    # Two, not one, and counted rather than read off p90. A run of ten rapid
    # creations cannot all be instant: the pool holds `warm-pool-size` settled
    # spares, and everything past them is served by shells that were all started
    # at about the same moment and therefore mature in a staircase. Measured here,
    # that is one creation waiting out most of a shell startup (~470 ms) and one
    # waiting out part of another (~150 ms), with the remaining eight at 14 to
    # 30 ms.
    #
    # p90 over ten samples is the SECOND WORST, so it fails on that second,
    # partial wait and says nothing about the eight instant ones: it read 132 to
    # 159 ms against a 150 ms budget on identical behaviour, passing or failing on
    # noise. The count is the statistic that matches what a user feels, and it
    # still rejects the defect this gate exists for: before the spare pool, ten
    # new-windows were [27, 416, 15, 396, 14, 412, 15, 428, 15, 457], five slow of
    # ten, and split-window was eight of ten.
    [int]$SlowMs = 150,
    [int]$SlowBudget = 2,
    # THE HARD GATE. p50 is what the warm pool controls and what a user feels on
    # the eight creations out of ten that the pool serves. Measured on this
    # machine it is 16 to 35 ms for every cell, quiet or loaded; the defect this
    # file exists for (no pool, every creation a cold shell start) puts it at
    # 400 ms and over. 150 ms is a factor of four above the measurement and a
    # factor of three below the defect, so it cannot flap and cannot be passed
    # by a build that lost the pool.
    [int]$P50LimitMs = 150,
    # The variant cells (see -SkipVariants) share this one: they are the same
    # split path with different flags and measure the same 16 to 40 ms.
    [int]$VariantP50LimitMs = 200,
    # A split that carries its own command cannot be served by a spare shell,
    # because the spare is running the default shell and not that command, so
    # this one pays a whole cold start and is budgeted like a launch.
    [int]$CommandSplitP50LimitMs = 2000,
    # new-session spawns a server process as well as a shell. The warm row is
    # the claim path, the no-warm row is the cold spawn; the gap between them is
    # the pool's value.
    [int]$WarmSessionP50LimitMs = 2000,
    [int]$ColdSessionP50LimitMs = 3000,
    # Teardown. kill-pane and kill-window only have to unhook a pane from a
    # live server; kill-session has to wait for the whole process tree to go,
    # which is the 3 s that issue #22 argued about.
    [int]$KillPaneP50LimitMs = 400,
    [int]$KillWindowP50LimitMs = 400,
    [int]$KillSessionP50LimitMs = 3000,
    # At or under this percentage of TOTAL cpu the machine counts as quiet and
    # the tail assertions are hard failures. Above it they are warnings. 25 on
    # this 32 core box is eight cores busy, which one sibling cargo build
    # already exceeds.
    [double]$QuietLoadPct = 25.0,
    # Kept as a backstop on how bad that second wait may get. It is NOT a
    # partial wait any more: traced on 2026-09-25 (PSMUX_WARM_TRACE), the
    # surge that claim 2 opens spawns its batch four at a time, and those
    # boots lengthen the neighbouring spare's READY time from 427 to 666 ms,
    # so the 4th creation of a 150 ms cadence can pay a whole boot remainder
    # just as the 3rd does. The gate read that as p90 = 302 and 333 against
    # 300 on a quiet machine, with p50 at 16 ms and the slow count inside its
    # budget, so the backstop sat on the measurement. A full pwsh boot
    # remainder is the honest bound; 600 ms is one, and anything past it is
    # the blow-up the max limit below describes. Measured with the same
    # cadence, to the prompt: pool 2 = [91,72,228,170,94,126,73,145,77,81],
    # pool 3 = [92,71,84,135,86,72,82,68,142,73], pool 4 = [92,80,69,71,70,
    # 69,80,70,83,70]. A deeper default pool is what makes every creation
    # flat, at ~45 MB idle per extra spare; `warm-pool-size` exists for it.
    [int]$P90LimitMs = 600,
    # max only catches a blow-up. The floor for one creation in a run of ten is
    # a whole shell startup, because no pool can produce a booted shell faster
    # than a shell boots, and a pwsh cold start measures 600 to 900ms on this
    # machine. A budget at 800ms flaked on exactly that; 1500ms was above the
    # floor with headroom and still far below anything pathological.
    #
    # 2026-10-04: the floor moved. The cold creation in a run is the one that
    # drains the pool and then boots through the surge it opened, and the Store
    # pwsh it waits on costs 1.6x its solo time while the surge's other spawns
    # go through package activation beside it (the no psmux probe in
    # docs/warm-sessions.md, "The third quick creation at depth two"). Measured
    # on master 8c9ef128 to b8c9777f with the bare pwsh start at 324 to 371 ms:
    # 943 to 1070 ms in six interleaved runs and 1434, 1534 and 1547 ms in three
    # of about twelve runs under the sweep runner. 1500 sat inside that
    # envelope, so it failed on the floor it was meant to sit above. 2000 is
    # above the measured envelope and still half of anything pathological; the
    # real per creation accounting is tests/test_pane_startup_perf.ps1, which
    # writes cold_creation_ratio against the bare shell for the trend.
    #
    # 2026-10-08: the envelope moved again and 2000 now sits inside it. The
    # draining creation of the back to back new-window run (the 3rd at depth
    # two, the 4th at depth three since 1e5c2021; every other creation 13 to
    # 30 ms) measured 1409 to 2279 ms in 15 runs at depth two on 10-07 and 1634
    # to 2113 ms in 12 runs at depth three on 10-08, failing two sweeps out of
    # three on max alone with p50 at 15 ms. Serialising the spawns cut it to
    # 750 ms but made the typical creation 150 ms (9054da96, reverted
    # 51aea044), so the floor stays and the backstop moves above it.
    [int]$MaxLimitMs = 2500,
    [int]$PollMs = 10,
    # The resource cell: how many windows and splits are stacked up before the
    # second memory sample is taken, and how long the quiet windows are.
    [int]$ResourceWindows = 20,
    [int]$ResourceSplits = 3,
    [int]$IdleSeconds = 3,
    # Samples per cell for the sections added on 2026-09-22.
    [int]$VariantCount = 5,
    [int]$SessionCount = 4,
    [int]$KillCount = 5,
    [switch]$SkipResources,
    [switch]$SkipVariants,
    [switch]$SkipSessions,
    [switch]$SkipKill,
    # The cold launch cell: a fresh server, ColdLaunchGapMs for the user to see
    # the prompt, then five new-window ColdLaunchBetweenMs apart. Its hard gate
    # is the p50 over the reps of creation FOUR, the one the depth two default
    # stalled (p50 791 ms, max 918 ms on 2026-10-08) and depth three does not
    # (3 ms). See docs/warm-sessions.md, "Why the default is three".
    [int]$ColdLaunchReps = 3,
    [int]$ColdLaunchGapMs = 2000,
    [int]$ColdLaunchBetweenMs = 300,
    [int]$ColdLaunchC4P50LimitMs = 150,
    [switch]$SkipColdLaunch,
    [string]$MetricsDir = ""
)

$ErrorActionPreference = "Continue"
. "$PSScriptRoot\perf_metrics_common.ps1"
$script:TestsPassed = 0
$script:TestsFailed = 0

$script:Warnings = New-Object System.Collections.Generic.List[string]

function Write-Pass { param($msg) Write-Host "[PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail { param($msg) Write-Host "[FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
# A tail assertion on a machine that was not quiet. Recorded, printed, kept in
# the JSON, and deliberately NOT counted as a failure: see the header.
function Write-Warn { param($msg) Write-Host "[WARN] $msg" -ForegroundColor Yellow; $script:Warnings.Add($msg) | Out-Null }
# One place decides whether a tail assertion is hard or soft, so the two can
# never drift apart.
function Assert-Tail {
    param([bool]$Ok, [string]$PassMsg, [string]$FailMsg)
    if ($Ok) { Write-Pass $PassMsg; return }
    if (Test-PerfMachineQuiet $QuietLoadPct) { Write-Fail $FailMsg }
    else { Write-Warn ("$FailMsg  (machine load p50 {0}% of total cpu, over the {1}% quiet mark, so this is a warning and not a failure)" -f (Get-PerfLoadSummary).p50_pct, $QuietLoadPct) }
}
function Write-Info { param($msg) Write-Host "[INFO] $msg" -ForegroundColor Cyan }
function Write-Test { param($msg) Write-Host "[TEST] $msg" -ForegroundColor White }
function Write-Perf { param($msg) Write-Host "[PERF] $msg" -ForegroundColor Magenta }

# ── binary ────────────────────────────────────────────────────────────────
# The BUILD IN THE TREE comes first, ahead of the installed copy on PATH.
# run_all_tests.ps1 announces target\release\psmux.exe as the binary under test,
# and a gate that quietly timed the installed psmux instead would report a green
# sweep for a build nobody measured. -Binary or PSMUX_TEST_BINARY override,
# which is how two builds are compared against each other.
if (-not $Binary -and $env:PSMUX_EXE) { $Binary = $env:PSMUX_EXE }
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
if (-not $Binary -or -not (Test-Path $Binary)) {
    Write-Fail "no psmux binary found (not on PATH, nothing in target\release)"
    Write-Host "`nTests passed: 0, failed: 1"
    exit 1
}
$Binary = (Resolve-Path $Binary).Path
$imgName = [IO.Path]::GetFileNameWithoutExtension($Binary).ToLower()
if ($imgName -notin @("psmux", "pmux", "tmux")) {
    Write-Fail "'$imgName' is not a recognised server image name; the warm server claim would be off and every timing here would be wrong"
    Write-Host "`nTests passed: 0, failed: 1"
    exit 1
}
Write-Info "Using: $Binary"

$DataDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR.TrimEnd('\', '/') } else { "$env:USERPROFILE\.psmux" }
$MetricsDir = Get-PerfMetricsDir $MetricsDir
# Routing env vars would retarget every command at whatever session happens to
# host the shell this suite was started from.
$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION = $null

$Ns = "clg$PID"
$Sess = "gate"
$PromptRe = 'PS [A-Z]:\\'

function Remove-Namespace {
    try { & $Binary -L $Ns kill-server 2>&1 | Out-Null } catch {}
    Start-Sleep -Milliseconds 400
    Get-ChildItem "$DataDir\$($Ns)__*" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
}

# One authenticated round trip. Returns @{ ok; lines }: a hashtable and never a
# bare collection, because PowerShell unrolls an empty collection to $null and
# `new-window` answers with no output at all.
function Invoke-Psmux {
    param([int]$Port, [string]$Key, [string]$Cmd, [string]$Target = "")
    if (-not $Target) { $Target = $Sess }
    $tcp = New-Object System.Net.Sockets.TcpClient
    $tcp.NoDelay = $true
    try {
        $tcp.Connect("127.0.0.1", $Port)
        $st = $tcp.GetStream(); $st.ReadTimeout = 20000
        $wr = New-Object System.IO.StreamWriter($st); $wr.AutoFlush = $false
        $rd = New-Object System.IO.StreamReader($st)
        $wr.WriteLine("AUTH $Key"); $wr.Flush()
        if ($rd.ReadLine() -ne "OK") { return @{ ok = $false; lines = @() } }
        $wr.WriteLine("TARGET $Target")
        $wr.WriteLine($Cmd)
        $wr.Flush()
        $acc = New-Object System.Collections.Generic.List[string]
        while ($true) {
            $l = $rd.ReadLine()
            if ($null -eq $l -or $l -eq "") { break }
            $acc.Add($l)
        }
        return @{ ok = $true; lines = $acc.ToArray() }
    } catch {
        return @{ ok = $false; lines = @() }
    } finally { $tcp.Close() }
}

function Get-Text { param($r) if ($null -eq $r -or -not $r.ok) { return "" } return ($r.lines -join "`n") }

function Wait-Registered {
    param([int]$TimeoutMs = 20000, [string]$Session = "")
    if (-not $Session) { $Session = $Sess }
    $pf = "$DataDir\$($Ns)__$Session.port"
    $kf = "$DataDir\$($Ns)__$Session.key"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        if ((Test-Path $pf) -and (Test-Path $kf)) {
            try {
                $p = [int](Get-Content $pf -Raw).Trim()
                $k = (Get-Content $kf -Raw).Trim()
                if ($p -gt 0 -and $k.Length -gt 0) { return @{ Port = $p; Key = $k } }
            } catch {}
        }
        Start-Sleep -Milliseconds 10
    }
    return $null
}

function Get-ActivePaneId {
    param([int]$Port, [string]$Key)
    (Get-Text (Invoke-Psmux $Port $Key "display-message -p '#{pane_id}'")).Trim().Trim("'")
}

function Wait-FirstPrompt {
    param([int]$Port, [string]$Key, [int]$TimeoutMs = 25000)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        if ((Get-Text (Invoke-Psmux $Port $Key "capture-pane -p")) -match $PromptRe) { return $true }
        Start-Sleep -Milliseconds $PollMs
    }
    return $false
}

# Issue one creation and return ms until the NEW pane shows a prompt, or -1.
function Measure-Creation {
    param([int]$Port, [string]$Key, [string]$Cmd, [string]$OldId, [int]$TimeoutMs = 25000)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $r = Invoke-Psmux $Port $Key $Cmd
    if (-not $r.ok) { return -1 }
    $msg = ($r.lines -join " ")
    if ($msg -match 'too small|no space|error|unknown command') {
        Write-Info "  creation refused: $msg"
        return -1
    }
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        $id = Get-ActivePaneId $Port $Key
        if ($id -and $id -ne $OldId) {
            if ((Get-Text (Invoke-Psmux $Port $Key "capture-pane -p -t $id")) -match $PromptRe) {
                return $sw.Elapsed.TotalMilliseconds
            }
        }
        Start-Sleep -Milliseconds $PollMs
    }
    return -1
}

# ── the gate ──────────────────────────────────────────────────────────────
$allSamples = [ordered]@{}

function Test-Cell {
    param([string]$Label, [string]$Cmd, [switch]$KillAfter)
    Write-Test "$Label x$Count back to back after a ${SettleMs}ms settle"
    Remove-Namespace
    Start-Process -FilePath $Binary -ArgumentList "-L", $Ns, "new-session", "-d", "-s", $Sess -WindowStyle Hidden | Out-Null
    $inf = Wait-Registered
    if ($null -eq $inf) {
        Write-Fail "$Label - the test session never registered"
        $allSamples[$Label] = @()
        return
    }
    if (-not (Wait-FirstPrompt $inf.Port $inf.Key)) {
        Write-Fail "$Label - the test session's first pane never reached a prompt"
        $allSamples[$Label] = @()
        Remove-Namespace
        return
    }
    # Settle: let the spare pool fill AND let those shells finish starting. A
    # spare is worth nothing until its prompt is up.
    Start-Sleep -Milliseconds $SettleMs

    $t = @()
    for ($i = 0; $i -lt $Count; $i++) {
        $old = Get-ActivePaneId $inf.Port $inf.Key
        $ms = Measure-Creation $inf.Port $inf.Key $Cmd $old
        if ($ms -ge 0) { $t += $ms } else { Write-Info "  creation $($i + 1) produced no pane" }
        if ($KillAfter) {
            # Keep room in the window: a 30 row pane allows only three vertical
            # splits, and every later one would be refused rather than slow.
            Invoke-Psmux $inf.Port $inf.Key "kill-pane" | Out-Null
            Start-Sleep -Milliseconds 120
        }
    }
    $allSamples[$Label] = @($t | ForEach-Object { [math]::Round($_, 1) })
    Remove-Namespace
    Add-PerfLoadSample "after $Label" | Out-Null

    if ($t.Count -lt $Count) {
        Write-Fail "$Label - only $($t.Count) of $Count creations produced a pane"
        return
    }
    $s = @($t | Sort-Object)
    $median = $s[[int][Math]::Floor(($s.Count - 1) / 2)]
    $p90 = $s[[Math]::Min($s.Count - 1, [int][Math]::Ceiling(0.9 * $s.Count) - 1)]
    $max = $s[-1]
    $slow = @($t | Where-Object { $_ -gt $SlowMs }).Count
    $list = (($t | ForEach-Object { [int]$_ }) -join ', ')
    Write-Perf ("{0,-18} med={1,6:N0} p90={2,6:N0} max={3,6:N0} ms  slow={4}/{5}  [{6}]" -f $Label, $median, $p90, $max, $slow, $t.Count, $list)

    # THE HARD GATE: p50, the statistic the warm pool controls. It moved 25 to
    # 35 ms between a quiet machine and one at 60 percent load, and it sits at
    # 400 ms and over on a build with no pool. See the header.
    if ($median -le $P50LimitMs) {
        Write-Pass ("$Label p50 {0:N0}ms is within {1}ms" -f $median, $P50LimitMs)
    } else {
        Write-Fail ("$Label p50 {0:N0}ms exceeds {1}ms - the warm pool is not serving these creations  [{2}]" -f $median, $P50LimitMs, $list)
    }

    # HOW MANY creations are slow, which is the thing the user feels, and not the
    # 2nd worst of ten. See $SlowBudget for why p90 alone was the wrong statistic.
    # These three are the TAIL, and the tail belongs as much to the machine as to
    # psmux, so Assert-Tail downgrades them to warnings on a loaded box.
    Assert-Tail ($slow -le $SlowBudget) `
        ("$Label {0} of {1} creations over {2}ms (budget {3})" -f $slow, $t.Count, $SlowMs, $SlowBudget) `
        ("$Label {0} of {1} creations over {2}ms, budget {3} - creations are waiting out a shell startup  [{4}]" -f $slow, $t.Count, $SlowMs, $SlowBudget, $list)
    Assert-Tail ($p90 -le $P90LimitMs) `
        ("$Label p90 {0:N0}ms is within {1}ms" -f $p90, $P90LimitMs) `
        ("$Label p90 {0:N0}ms exceeds {1}ms  [{2}]" -f $p90, $P90LimitMs, $list)
    Assert-Tail ($max -le $MaxLimitMs) `
        ("$Label max {0:N0}ms is within {1}ms" -f $max, $MaxLimitMs) `
        ("$Label max {0:N0}ms exceeds {1}ms  [{2}]" -f $max, $MaxLimitMs, $list)
}

# ── a few windows at a human pace right after a cold launch ───────────────
#
# The deep cells above start after a settle, with the pool full. This one does
# not: a fresh server, the prompt, ColdLaunchGapMs for the user to read it, then
# five new-window ColdLaunchBetweenMs apart, each timed to its own prompt. The
# pool is still filling behind the boot hold when they arrive, which is where a
# shallow default pool stalled the fourth creation for most of a second.
$script:ColdLaunchBlock = $null
function Test-ColdLaunchRun {
    Write-Test ("cold launch, {0} ms gap, then 5 new-window {1} ms apart, x{2}" -f $ColdLaunchGapMs, $ColdLaunchBetweenMs, $ColdLaunchReps)
    $runs = @(); $colds = @()
    for ($r = 0; $r -lt $ColdLaunchReps; $r++) {
        Remove-Namespace
        $sw = [Diagnostics.Stopwatch]::StartNew()
        Start-Process -FilePath $Binary -ArgumentList "-L", $Ns, "new-session", "-d", "-s", $Sess -WindowStyle Hidden | Out-Null
        $inf = Wait-Registered
        if ($null -eq $inf -or -not (Wait-FirstPrompt $inf.Port $inf.Key)) {
            Write-Info "  cold launch rep $($r + 1): the session never reached a prompt"
            continue
        }
        $colds += [math]::Round($sw.Elapsed.TotalMilliseconds, 1)
        Start-Sleep -Milliseconds $ColdLaunchGapMs
        $v = @()
        for ($i = 0; $i -lt 5; $i++) {
            if ($i -gt 0) { Start-Sleep -Milliseconds $ColdLaunchBetweenMs }
            $old = Get-ActivePaneId $inf.Port $inf.Key
            $v += [math]::Round((Measure-Creation $inf.Port $inf.Key "new-window" $old), 1)
        }
        $runs += , $v
        Write-Info ("  rep {0}: cold {1:N0} ms, creations [{2}]" -f ($r + 1), $colds[-1], (($v | ForEach-Object { [int]$_ }) -join ', '))
    }
    Remove-Namespace
    Add-PerfLoadSample "after cold launch run" | Out-Null
    $c4 = @($runs | ForEach-Object { $_[3] } | Where-Object { $_ -ge 0 })
    $allSamples["cold launch creation 4"] = $c4
    $allSamples["cold launch to prompt"] = $colds
    $script:ColdLaunchBlock = [ordered]@{
        reps = $ColdLaunchReps; gap_ms = $ColdLaunchGapMs; between_ms = $ColdLaunchBetweenMs
        c4_p50_limit_ms = $ColdLaunchC4P50LimitMs
        cold_to_prompt_ms = $colds
        creations_ms = @($runs | ForEach-Object { , @($_) })
    }
    if ($c4.Count -lt $ColdLaunchReps) {
        Write-Fail "cold launch run - only $($c4.Count) of $ColdLaunchReps reps produced a 4th window"
        return
    }
    $s = @($c4 | Sort-Object)
    $median = $s[[int][Math]::Floor(($s.Count - 1) / 2)]
    $list = (($c4 | ForEach-Object { [int]$_ }) -join ', ')
    Write-Perf ("{0,-18} med={1,6:N0} max={2,6:N0} ms  [{3}]" -f "cold c4 (human)", $median, $s[-1], $list)
    if ($median -le $ColdLaunchC4P50LimitMs) {
        Write-Pass ("cold launch creation 4 p50 {0:N0}ms is within {1}ms" -f $median, $ColdLaunchC4P50LimitMs)
    } else {
        Write-Fail ("cold launch creation 4 p50 {0:N0}ms exceeds {1}ms - the pool is too shallow for a few windows opened after launch  [{2}]" -f $median, $ColdLaunchC4P50LimitMs, $list)
    }
    $all = @($runs | ForEach-Object { $_ } | Where-Object { $_ -ge 0 })
    $mx = ($all | Measure-Object -Maximum).Maximum
    Assert-Tail ($mx -le $MaxLimitMs) `
        ("cold launch run max {0:N0}ms is within {1}ms" -f $mx, $MaxLimitMs) `
        ("cold launch run max {0:N0}ms exceeds {1}ms" -f $mx, $MaxLimitMs)
}

# ── the split flags the three deep cells do not cover ─────────────────────
#
# One session for the whole section rather than one per flag: each Test-Cell
# setup costs a session start plus a 2 s settle, and these cells are five
# samples each. The created pane is killed after every sample, which is what
# keeps room in a 30 row window for the next one.
#
# Verified live against the installed 0bcc421 on 2026-09-22 that -f, -b, -bh,
# -bv and a split carrying a command each really add a pane and really move the
# active pane id, so Measure-Creation's "the active pane changed AND the new
# pane shows a prompt" rule cannot record a refusal as a fast creation here.
# (psmux currently accepts -b and -f and ignores the placement they ask for;
# that is a behaviour question, not a timing one, and this cell times the
# creation either way.)
function Test-Variants {
    Write-Test "split flag variants x$VariantCount each, one session, pane killed between samples"
    Remove-Namespace
    Start-Process -FilePath $Binary -ArgumentList "-L", $Ns, "new-session", "-d", "-s", $Sess -WindowStyle Hidden | Out-Null
    $inf = Wait-Registered
    if ($null -eq $inf) { Write-Fail "variants - the test session never registered"; return }
    if (-not (Wait-FirstPrompt $inf.Port $inf.Key)) {
        Write-Fail "variants - the test session's first pane never reached a prompt"
        Remove-Namespace; return
    }
    Start-Sleep -Milliseconds $SettleMs

    $cells = [ordered]@{
        "split -f"       = @{ cmd = "split-window -f";  limit = $VariantP50LimitMs }
        "split -b"       = @{ cmd = "split-window -b";  limit = $VariantP50LimitMs }
        "split -bh"      = @{ cmd = "split-window -bh"; limit = $VariantP50LimitMs }
        "split -bv"      = @{ cmd = "split-window -bv"; limit = $VariantP50LimitMs }
        # Sizes, both forms tmux accepts: a cell count and a percentage. The
        # size is applied by the layout after the pane exists, so these go
        # through the same claim as a plain split and are budgeted like one.
        "split -l 10"    = @{ cmd = "split-window -v -l 10";  limit = $VariantP50LimitMs }
        "split -l 30%"   = @{ cmd = "split-window -h -l 30%"; limit = $VariantP50LimitMs }
        # A split issued while the window is ZOOMED. tmux unzooms first
        # (cmd-split-window.c, window_unzoom) and then splits, so this is the
        # unzoom plus resize of every pane plus the claim. It needs a window
        # with two panes to zoom at all (tmux refuses to zoom a lone pane), so
        # the cell keeps one extra pane for its whole run and zooms the active
        # pane before every sample; Test-Variants checks the zoom engaged, so
        # an unzoomed split can never be recorded as the zoomed one.
        "split zoomed"   = @{ cmd = "split-window -v"; limit = $VariantP50LimitMs; zoom = $true }
        "split with cmd" = @{ cmd = "split-window -h pwsh -NoLogo -NoProfile"; limit = $CommandSplitP50LimitMs }
    }
    foreach ($label in @($cells.Keys)) {
        $t = @()
        $zoom = [bool]$cells[$label].zoom
        $basePane = ""
        if ($zoom) {
            $o0 = Get-ActivePaneId $inf.Port $inf.Key
            if ((Measure-Creation $inf.Port $inf.Key "split-window -h" $o0) -lt 0) {
                Write-Fail "$label - could not make the second pane the zoom needs"
                continue
            }
            $basePane = Get-ActivePaneId $inf.Port $inf.Key
        }
        for ($i = 0; $i -lt $VariantCount; $i++) {
            if ($zoom) {
                Invoke-Psmux $inf.Port $inf.Key "resize-pane -Z" | Out-Null
                $zf = (Get-Text (Invoke-Psmux $inf.Port $inf.Key "display-message -p '#{window_zoomed_flag}'")).Trim().Trim("'")
                if ($zf -ne "1") {
                    Write-Info "  $label sample $($i + 1): the window did not zoom (window_zoomed_flag '$zf'), sample dropped"
                    continue
                }
            }
            $old = Get-ActivePaneId $inf.Port $inf.Key
            $ms = Measure-Creation $inf.Port $inf.Key $cells[$label].cmd $old
            if ($ms -ge 0) { $t += $ms } else { Write-Info "  $label sample $($i + 1) produced no pane" }
            # A size flag that is accepted and ignored would time an ordinary
            # split under the wrong name, so the first sample of a sized cell
            # checks the new pane really got the size it asked for (within two
            # cells, for the border and percentage rounding).
            if ($ms -ge 0 -and $i -eq 0 -and $cells[$label].cmd -match '-l (\d+)(%?)') {
                $want = [int]$Matches[1]; $pct = ($Matches[2] -eq '%')
                $dim = if ($cells[$label].cmd -match '-h') { 'width' } else { 'height' }
                $got = (Get-Text (Invoke-Psmux $inf.Port $inf.Key "display-message -p '#{pane_$dim} #{window_$dim}'")).Trim().Trim("'") -split '\s+'
                if ($got.Count -ge 2) {
                    $exp = if ($pct) { [math]::Round([int]$got[1] * $want / 100.0) } else { $want }
                    if ([math]::Abs([int]$got[0] - $exp) -le 2) {
                        Write-Pass ("$label made a pane {0} {1} against {2} asked for" -f $got[0], $dim, $exp)
                    } else {
                        Write-Fail ("$label made a pane {0} {1}, but {2} was asked for (window {3}), so its timing is not a sized split" -f $got[0], $dim, $exp, $got[1])
                    }
                }
            }
            Invoke-Psmux $inf.Port $inf.Key "kill-pane" | Out-Null
            Start-Sleep -Milliseconds 150
        }
        if ($basePane) { Invoke-Psmux $inf.Port $inf.Key "kill-pane -t $basePane" | Out-Null; Start-Sleep -Milliseconds 150 }
        $allSamples[$label] = @($t | ForEach-Object { [math]::Round($_, 1) })
        if ($t.Count -lt $VariantCount) {
            Write-Fail "$label - only $($t.Count) of $VariantCount creations produced a pane"
            continue
        }
        $srt = @($t | Sort-Object)
        $median = $srt[[int][Math]::Floor(($srt.Count - 1) / 2)]
        $max = $srt[-1]
        $list = (($t | ForEach-Object { [int]$_ }) -join ', ')
        Write-Perf ("{0,-18} med={1,6:N0} max={2,6:N0} ms  [{3}]" -f $label, $median, $max, $list)
        $lim = $cells[$label].limit
        if ($median -le $lim) {
            Write-Pass ("$label p50 {0:N0}ms is within {1}ms" -f $median, $lim)
        } else {
            Write-Fail ("$label p50 {0:N0}ms exceeds {1}ms  [{2}]" -f $median, $lim, $list)
        }
    }
    Remove-Namespace
    Add-PerfLoadSample "after variants" | Out-Null
}

# ── new-session, with the warm server pool and without it ─────────────────
#
# Every psmux session is its own server process, so new-session pays a process
# spawn that new-window and split-window never pay. The warm pool exists to
# take that spawn off the critical path by having a server already booted and
# waiting to be claimed. PSMUX_NO_WARM=1 turns the claim off, so the two rows
# here are the same work with and without the pool and the GAP BETWEEN THEM is
# what the pool is worth. A regression that breaks the claim closes that gap,
# and closing it is visible in the trend long before either row crosses a
# budget.
#
# Each sample is a fresh session in this suite's own namespace, killed straight
# after it is measured so the next one does not inherit its state.
function Test-Sessions {
    Write-Test "new-session to a visible prompt x$SessionCount, warm pool allowed and PSMUX_NO_WARM=1"
    foreach ($mode in @("warm", "nowarm")) {
        $label = if ($mode -eq "warm") { "new-session (warm)" } else { "new-session (no warm)" }
        $limit = if ($mode -eq "warm") { $WarmSessionP50LimitMs } else { $ColdSessionP50LimitMs }
        Remove-Namespace
        $prev = $env:PSMUX_NO_WARM
        if ($mode -eq "nowarm") { $env:PSMUX_NO_WARM = "1" } else { Remove-Item Env:\PSMUX_NO_WARM -ErrorAction SilentlyContinue }
        $t = @()
        try {
            for ($i = 0; $i -lt $SessionCount; $i++) {
                $name = "s$mode$i"
                $sw = [Diagnostics.Stopwatch]::StartNew()
                Start-Process -FilePath $Binary -ArgumentList "-L", $Ns, "new-session", "-d", "-s", $name -WindowStyle Hidden | Out-Null
                $si = Wait-Registered -Session $name
                $ms = -1
                if ($null -ne $si) {
                    while ($sw.ElapsedMilliseconds -lt 30000) {
                        if ((Get-Text (Invoke-Psmux $si.Port $si.Key "capture-pane -p" $name)) -match $PromptRe) {
                            $ms = $sw.Elapsed.TotalMilliseconds
                            break
                        }
                        Start-Sleep -Milliseconds $PollMs
                    }
                }
                if ($ms -ge 0) { $t += $ms } else { Write-Info "  $label sample $($i + 1) never reached a prompt" }
                try { & $Binary -L $Ns kill-session -t $name 2>&1 | Out-Null } catch {}
                # The warm row needs the pool a moment to put a fresh standby
                # back; measuring the refill race instead of the claim would be
                # measuring the wrong thing.
                Start-Sleep -Milliseconds $(if ($mode -eq "warm") { 1500 } else { 400 })
            }
        } finally {
            if ($null -ne $prev) { $env:PSMUX_NO_WARM = $prev } else { Remove-Item Env:\PSMUX_NO_WARM -ErrorAction SilentlyContinue }
            Remove-Namespace
        }
        $allSamples[$label] = @($t | ForEach-Object { [math]::Round($_, 1) })
        if ($t.Count -eq 0) { Write-Fail "$label - no session reached a prompt"; continue }
        $srt = @($t | Sort-Object)
        $median = $srt[[int][Math]::Floor(($srt.Count - 1) / 2)]
        $list = (($t | ForEach-Object { [int]$_ }) -join ', ')
        Write-Perf ("{0,-18} med={1,6:N0} max={2,6:N0} ms  [{3}]" -f $label, $median, $srt[-1], $list)
        if ($median -le $limit) {
            Write-Pass ("$label p50 {0:N0}ms is within {1}ms" -f $median, $limit)
        } else {
            Write-Fail ("$label p50 {0:N0}ms exceeds {1}ms  [{2}]" -f $median, $limit, $list)
        }
    }
    Add-PerfLoadSample "after sessions" | Out-Null
}

# ── how long it takes to get rid of things ────────────────────────────────
#
# Creation is only half of what a user feels; issue #22 was entirely about the
# other half. "Gone" is defined by observation and never by the command
# returning:
#
#   kill-pane    the pane is no longer in list-panes
#   kill-window  the window is no longer in list-windows
#   kill-session the <ns>__<session>.port anchor file is gone, which is the
#                point at which the server PROCESS has exited rather than
#                merely accepted the request
function Test-Kill {
    Write-Test "teardown: kill-pane, kill-window, kill-session, x$KillCount each"
    Remove-Namespace
    Start-Process -FilePath $Binary -ArgumentList "-L", $Ns, "new-session", "-d", "-s", $Sess -WindowStyle Hidden | Out-Null
    $inf = Wait-Registered
    if ($null -eq $inf) { Write-Fail "kill - the test session never registered"; return }
    if (-not (Wait-FirstPrompt $inf.Port $inf.Key)) {
        Write-Fail "kill - the test session's first pane never reached a prompt"
        Remove-Namespace; return
    }
    Start-Sleep -Milliseconds $SettleMs

    $countOf = {
        param($what)
        $r = Invoke-Psmux $inf.Port $inf.Key $what
        if (-not $r.ok) { return -1 }
        return @($r.lines | Where-Object { $_.Trim() -ne "" }).Count
    }

    foreach ($spec in @(
        @{ label = "kill-pane";   make = "split-window -v"; kill = "kill-pane";   list = "list-panes";   limit = $KillPaneP50LimitMs },
        @{ label = "kill-window"; make = "new-window";      kill = "kill-window"; list = "list-windows"; limit = $KillWindowP50LimitMs }
    )) {
        $t = @()
        for ($i = 0; $i -lt $KillCount; $i++) {
            $old = Get-ActivePaneId $inf.Port $inf.Key
            if ((Measure-Creation $inf.Port $inf.Key $spec.make $old) -lt 0) {
                Write-Info "  $($spec.label) sample $($i + 1): nothing was created to kill"
                continue
            }
            $before = & $countOf $spec.list
            $sw = [Diagnostics.Stopwatch]::StartNew()
            Invoke-Psmux $inf.Port $inf.Key $spec.kill | Out-Null
            $ms = -1
            while ($sw.ElapsedMilliseconds -lt 15000) {
                $now = & $countOf $spec.list
                if ($now -ge 0 -and $now -lt $before) { $ms = $sw.Elapsed.TotalMilliseconds; break }
                Start-Sleep -Milliseconds $PollMs
            }
            if ($ms -ge 0) { $t += $ms } else { Write-Info "  $($spec.label) sample $($i + 1) never went away" }
            Start-Sleep -Milliseconds 120
        }
        $allSamples[$spec.label] = @($t | ForEach-Object { [math]::Round($_, 1) })
        if ($t.Count -eq 0) { Write-Fail "$($spec.label) - nothing was measured"; continue }
        $srt = @($t | Sort-Object)
        $median = $srt[[int][Math]::Floor(($srt.Count - 1) / 2)]
        $list = (($t | ForEach-Object { [int]$_ }) -join ', ')
        Write-Perf ("{0,-18} med={1,6:N0} max={2,6:N0} ms  [{3}]" -f $spec.label, $median, $srt[-1], $list)
        if ($median -le $spec.limit) {
            Write-Pass ("$($spec.label) p50 {0:N0}ms is within {1}ms" -f $median, $spec.limit)
        } else {
            Write-Fail ("$($spec.label) p50 {0:N0}ms exceeds {1}ms  [{2}]" -f $median, $spec.limit, $list)
        }
    }
    Remove-Namespace

    # kill-session is measured on its own sessions, because it takes the whole
    # server with it and there would be nothing left to ask afterwards.
    $t = @()
    for ($i = 0; $i -lt $KillCount; $i++) {
        $name = "k$i"
        Start-Process -FilePath $Binary -ArgumentList "-L", $Ns, "new-session", "-d", "-s", $name -WindowStyle Hidden | Out-Null
        $si = Wait-Registered -Session $name
        if ($null -eq $si) { Write-Info "  kill-session sample $($i + 1): the session never registered"; continue }
        $anchor = "$DataDir\$($Ns)__$name.port"
        Start-Sleep -Milliseconds 400
        $sw = [Diagnostics.Stopwatch]::StartNew()
        & $Binary -L $Ns kill-session -t $name 2>&1 | Out-Null
        $ms = -1
        while ($sw.ElapsedMilliseconds -lt 20000) {
            if (-not (Test-Path $anchor)) { $ms = $sw.Elapsed.TotalMilliseconds; break }
            Start-Sleep -Milliseconds $PollMs
        }
        if ($ms -ge 0) { $t += $ms } else { Write-Info "  kill-session sample $($i + 1): the port anchor never went away" }
        Start-Sleep -Milliseconds 200
    }
    $allSamples["kill-session"] = @($t | ForEach-Object { [math]::Round($_, 1) })
    if ($t.Count -eq 0) {
        Write-Fail "kill-session - nothing was measured"
    } else {
        $srt = @($t | Sort-Object)
        $median = $srt[[int][Math]::Floor(($srt.Count - 1) / 2)]
        $list = (($t | ForEach-Object { [int]$_ }) -join ', ')
        Write-Perf ("{0,-18} med={1,6:N0} max={2,6:N0} ms  [{3}]" -f "kill-session", $median, $srt[-1], $list)
        if ($median -le $KillSessionP50LimitMs) {
            Write-Pass ("kill-session p50 {0:N0}ms is within {1}ms" -f $median, $KillSessionP50LimitMs)
        } else {
            Write-Fail ("kill-session p50 {0:N0}ms exceeds {1}ms - the server process is not exiting promptly  [{2}]" -f $median, $KillSessionP50LimitMs, $list)
        }
    }
    Remove-Namespace
    Add-PerfLoadSample "after kill" | Out-Null
}

# ── memory and CPU, the cost of holding a session open ────────────────────
#
# The latency cells above run DETACHED, so they have a server and no client.
# This cell attaches one, because "what does psmux cost" is a question about
# both processes, and takes four samples:
#
#   at prompt            one window, one pane, prompt up, nothing typed
#   idle after prompt    CPU over a quiet window, as a percentage of ONE core.
#                        This is the busy polling detector: a 1 ms sleep loop
#                        that Windows rounds up to a 15.6 ms timer tick is
#                        invisible in every latency number and obvious here.
#   after N windows      working set and private bytes once $ResourceWindows
#                        windows and $ResourceSplits splits are stacked up, ie
#                        what a real working session costs, plus the CPU those
#                        creations consumed
#   idle after N windows the same quiet window again, with everything open. A
#                        per pane poll shows up as a number that grew with the
#                        pane count while the first idle sample looked fine.
#
# RECORDED, NOT GATED. The thresholds on these numbers live in
# test_perf_vs_terminals (T6 memory, T7 idle CPU, T8 keystroke CPU); inventing a
# second set here would mean two places to argue with. The one assertion is that
# the section produced data, because a JSON full of nulls that still says PASS
# is worse than a failure.
$script:ResourceBlock = $null

function Test-Resources {
    Write-Test "memory and CPU of the server and an attached client"
    Remove-Namespace
    $client = $null
    try {
        $client = Start-Process -FilePath $Binary -ArgumentList "-L", $Ns, "new-session", "-s", $Sess -PassThru
    } catch {
        Write-Fail "resources - could not launch an attached client: $_"
        return
    }
    try {
        $inf = Wait-Registered
        if ($null -eq $inf) { Write-Fail "resources - the session never registered"; return }
        if (-not (Wait-FirstPrompt $inf.Port $inf.Key)) { Write-Fail "resources - the first pane never reached a prompt"; return }
        Start-Sleep -Milliseconds $SettleMs

        $srv = Get-PerfServerPid -Ns $Ns -Session $Sess -DataDir $DataDir
        $roles = [ordered]@{}
        if ($srv -gt 0) { $roles["server"] = $srv }
        if ($client -and -not $client.HasExited) { $roles["client"] = $client.Id }
        if ($roles.Count -eq 0) {
            Write-Fail "resources - neither the server nor the client could be identified, so nothing was sampled"
            return
        }

        $atPrompt = Get-PerfResourceSnapshot $roles
        Write-Info (Format-PerfResourceLine $atPrompt "at prompt      ")
        $idle1 = Measure-PerfIdleCpu $roles $IdleSeconds

        # A wall clock budget on the whole fill, so a machine that has gone slow
        # cannot turn this section into a suite timeout. Whatever was opened by
        # the time the budget runs out is what gets measured, and the count is
        # recorded, so the sample is still honest.
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $budgetMs = 120000
        $made = 0
        for ($i = 0; $i -lt $ResourceWindows -and $sw.ElapsedMilliseconds -lt $budgetMs; $i++) {
            $old = Get-ActivePaneId $inf.Port $inf.Key
            if ((Measure-Creation $inf.Port $inf.Key "new-window" $old) -ge 0) { $made++ }
        }
        for ($i = 0; $i -lt $ResourceSplits -and $sw.ElapsedMilliseconds -lt $budgetMs; $i++) {
            $old = Get-ActivePaneId $inf.Port $inf.Key
            if ((Measure-Creation $inf.Port $inf.Key "split-window -v" $old) -ge 0) { $made++ }
        }
        $sw.Stop()
        $afterOpen = Get-PerfResourceSnapshot $roles
        Write-Info (Format-PerfResourceLine $afterOpen ("after {0} panes " -f $made))
        $openCpu = Get-PerfCpuDelta $atPrompt $afterOpen ([double][Math]::Max($made, 1)) 1.0
        $idle2 = Measure-PerfIdleCpu $roles $IdleSeconds
        Write-Info ("idle cpu at one pane  : " + (@($idle1.pct_of_one_core.Keys | ForEach-Object { "{0} {1:F2}%" -f $_, $idle1.pct_of_one_core[$_] }) -join "  "))
        Write-Info ("idle cpu at $made panes: " + (@($idle2.pct_of_one_core.Keys | ForEach-Object { "{0} {1:F2}%" -f $_, $idle2.pct_of_one_core[$_] }) -join "  "))

        $script:ResourceBlock = [ordered]@{
            panes_opened            = $made
            windows_requested       = $ResourceWindows
            splits_requested        = $ResourceSplits
            open_elapsed_ms         = [math]::Round($sw.Elapsed.TotalMilliseconds, 0)
            idle_window_seconds     = $IdleSeconds
            at_prompt               = (Get-PerfMemorySummary $atPrompt)
            after_panes             = (Get-PerfMemorySummary $afterOpen)
            cpu_ms_per_creation     = $openCpu
            idle_cpu_pct_one_pane   = $idle1.pct_of_one_core
            idle_cpu_pct_many_panes = $idle2.pct_of_one_core
        }
        $srvWs = if ($atPrompt.Contains("server")) { $atPrompt.server.ws_mb } else { 0 }
        $srvWs2 = if ($afterOpen.Contains("server")) { $afterOpen.server.ws_mb } else { 0 }
        Write-Perf ("{0,-18} server ws {1} -> {2} MB over {3} panes" -f "resources", $srvWs, $srvWs2, $made)
        Write-Pass ("memory and CPU collected for the server and the client, one pane and $made panes")
    } finally {
        Remove-Namespace
        try { if ($client -and -not $client.HasExited) { Stop-Process -Id $client.Id -Force -ErrorAction SilentlyContinue } } catch { }
        Start-Sleep -Milliseconds 300
    }
}

Write-Host ""
Write-Host ("=" * 76)
Write-Host " Creation latency gate - time to a VISIBLE PROMPT, $Count back to back"
Write-Host (" at most {0} of {1} creations over {2}ms; p90 budget {3}ms, max budget {4}ms" -f $SlowBudget, $Count, $SlowMs, $P90LimitMs, $MaxLimitMs)
Write-Host ("=" * 76)

Add-PerfLoadSample "start" | Out-Null
Write-Info ("machine load at the start: {0}% of total cpu" -f (Get-PerfLoadSummary).p50_pct)

Test-Cell -Label "new-window"      -Cmd "new-window"
Test-Cell -Label "split-window -v" -Cmd "split-window -v" -KillAfter
Test-Cell -Label "split-window -h" -Cmd "split-window -h" -KillAfter
if (-not $SkipColdLaunch) { Test-ColdLaunchRun }
if (-not $SkipVariants) { Test-Variants }
if (-not $SkipSessions) { Test-Sessions }
if (-not $SkipKill)     { Test-Kill }
if (-not $SkipResources) { Test-Resources }

$loadSummary = Get-PerfLoadSummary
$wasQuiet = Test-PerfMachineQuiet $QuietLoadPct
$topTxt = Get-PerfLoadTopText; if ($topTxt) { Write-Host "[INFO] $topTxt" -ForegroundColor Cyan }
Write-Info ("machine load over the run: n={0} min={1}% p50={2}% max={3}%  -> {4}" -f `
    $loadSummary.n, $loadSummary.min_pct, $loadSummary.p50_pct, $loadSummary.max_pct, `
    $(if ($wasQuiet) { "quiet, tail assertions were hard failures" } else { "loaded, tail assertions were warnings" }))
if ($script:Warnings.Count -gt 0) {
    Write-Info ("{0} tail assertion(s) were downgraded to warnings by the machine load; they are in the JSON under tail_warnings" -f $script:Warnings.Count)
}

# ── samples on disk, never in the repo ────────────────────────────────────
# Percentiles are computed here rather than left to the reader: p50 and p90 per
# cell are what tests/perf_summary.ps1 plots, and the raw samples stay alongside
# them so a suspicious percentile can always be checked against the run it came
# from.
$stats = [ordered]@{}
foreach ($k in @($allSamples.Keys)) { $stats[$k] = (Get-PerfStats $allSamples[$k] 1) }
$outFile = Write-PerfMetrics -Suite "test_creation_latency_gate" -Binary $Binary `
    -FileStem "creation_latency_gate" -MetricsDir $MetricsDir -Data ([ordered]@{
    count = $Count
    variant_count = $VariantCount
    session_count = $SessionCount
    kill_count = $KillCount
    settle_ms = $SettleMs
    slow_ms = $SlowMs
    slow_budget = $SlowBudget
    p50_limit_ms = $P50LimitMs
    p90_limit_ms = $P90LimitMs
    max_limit_ms = $MaxLimitMs
    poll_ms = $PollMs
    # Every budget this run judged itself against, in one place, so a reader
    # never has to match a number in the output against a default in the param
    # block of whatever revision happened to produce the file.
    limits_ms = [ordered]@{
        p50                = $P50LimitMs
        variant_p50        = $VariantP50LimitMs
        command_split_p50  = $CommandSplitP50LimitMs
        warm_session_p50   = $WarmSessionP50LimitMs
        cold_session_p50   = $ColdSessionP50LimitMs
        kill_pane_p50      = $KillPaneP50LimitMs
        kill_window_p50    = $KillWindowP50LimitMs
        kill_session_p50   = $KillSessionP50LimitMs
    }
    quiet_load_pct = $QuietLoadPct
    machine_was_quiet = $wasQuiet
    tail_warnings = $script:Warnings.ToArray()
    samples_ms = $allSamples
    stats_ms = $stats
    resources = $script:ResourceBlock
    cold_launch_run = $script:ColdLaunchBlock
    passed = $script:TestsPassed
    failed = $script:TestsFailed
})
if ($outFile) { Write-Info "samples written to $outFile" }

Remove-Namespace
Write-Host ""
Write-Host ("Tests passed: {0}, failed: {1}, warnings: {2}" -f $script:TestsPassed, $script:TestsFailed, $script:Warnings.Count) -ForegroundColor $(if ($script:TestsFailed -eq 0) { "Green" } else { "Red" })
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
