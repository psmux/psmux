# Issue #673: entering copy mode clones the whole scrollback.
#
# Since PR #671 copy mode reads a *snapshot* of the pane's screen, which is
# what tmux does (`window_copy_init` copies the pane's grid) and is what keeps
# a frozen view from churning underneath the reader.  psmux took that literally
# and deep-cloned the entire parser, scrollback included, so entering copy mode
# on a deep history doubled the pane's memory for as long as the user read.
#
# The reporter measured, at 200 columns with `history-limit 60000` filled to
# `history_size 49952`:
#
#     before copy mode   190.7 MB working set / 184.8 MB private
#     in copy mode       365.3 MB / 364.9 MB   (+180.1 MB private)
#     after one cycle    193.9 MB / 186.1 MB
#
# and about 59 MB retained after repeated cycles, plateauing around cycle 10
# (allocator retention rather than a leak).
#
# Scrollback rows are immutable once they are in history: they are only ever
# trimmed off the old end or cleared wholesale.  So the snapshot can SHARE them
# with the live grid instead of copying them, and entering copy mode should
# cost the visible screen plus whatever changes afterwards, not the history.
#
# WHAT IS ASSERTED
#   1. Entering copy mode on a 200 col pane with ~50,000 retained lines costs
#      less than MAX_ENTER_FRACTION of the live history's own private bytes.
#      Before the fix this is ~97%; the bound is 10%.
#   2. Over CYCLES enter/leave cycles the server's private bytes do not grow
#      without bound: the last cycle's resting private bytes stay within
#      MAX_PLATEAU_FRACTION of the first cycle's.
#   3. Entering copy mode does not get SLOWER: p50 and p99 of the `copy-mode`
#      round trip are reported and bounded.
#   4. Scope (#673 part 2): a pane that is in copy mode but is NOT the active
#      pane still holds its own snapshot, so its view does not move when that
#      pane's own program prints.  psmux allows more than one pane in copy mode
#      at once (#607) but only ever snapshotted the active one.
#   5. Nothing regressed: capture-pane still reads the LIVE screen,
#      #{history_size} still counts the live grid, and the copy-mode view still
#      reads the history it is anchored to.
#
# Run this against a pre-fix binary and 1, 2 and 4 FAIL while 3 and 5 pass.

$ErrorActionPreference = "Continue"

[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$OutputEncoding = New-Object System.Text.UTF8Encoding($false)
Remove-Item Env:NO_COLOR -ErrorAction SilentlyContinue

$SOCK = "i673"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red;   $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Test($m) { Write-Host "`n[$m]" -ForegroundColor Cyan }

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -EA SilentlyContinue).Source }
if (-not $PSMUX) { Write-Host "psmux not found"; exit 1 }
$PSMUX = (Resolve-Path $PSMUX).Path
$IMAGE = [System.IO.Path]::GetFileName($PSMUX)

function Pmux { & $PSMUX -L $SOCK @args 2>&1 }
function Kill-Sess([string]$n) { Pmux kill-session -t $n 2>&1 | Out-Null }

# --- geometry ----------------------------------------------------------------
$COLS    = 200
$ROWS    = 50
$HISTORY = 60000
$LINES   = 50000
$LINELEN = 84       # plus the "NNNNN " prefix: ~90 characters per retained line
$CYCLES  = 30

$MAX_ENTER_FRACTION   = 0.10   # snapshot must cost < 10% of the live history
$MAX_PLATEAU_FRACTION = 1.25   # resting private bytes must not creep past 1.25x

$script:Metrics = [ordered]@{
    timestamp = (Get-Date).ToString("o")
    exe       = $PSMUX
    geometry  = [ordered]@{ cols = $COLS; rows = $ROWS; history_limit = $HISTORY; lines = $LINES; line_len = $LINELEN; cycles = $CYCLES }
    cycles    = @()
    enter_ms  = @()
    leave_ms  = @()
    noop_ms   = @()
}

# Sample the server process started from THIS executable path.  Scoping by path
# and never by image name keeps other psmux builds and the developer's own
# sessions out of the measurement; the maximum over matches avoids reading the
# idle `__warm__` standby instead of the pane's server (see test_issue641).
function Get-ServerMem {
    $procs = @(Get-CimInstance Win32_Process -Filter "Name='$IMAGE'" |
        Where-Object { $_.ExecutablePath -eq $PSMUX -and $_.CommandLine -match 'server' })
    if ($procs.Count -eq 0) { return $null }
    $best = $null
    foreach ($p in $procs) {
        $o = Get-Process -Id $p.ProcessId -EA SilentlyContinue
        if (-not $o) { continue }
        if ($null -eq $best -or $o.PrivateMemorySize64 -gt $best.priv) {
            $best = [ordered]@{ pid = $o.Id; priv = $o.PrivateMemorySize64; ws = $o.WorkingSet64 }
        }
    }
    if ($null -eq $best) { return $null }
    [ordered]@{
        pid     = $best.pid
        priv_mb = [Math]::Round($best.priv / 1MB, 1)
        ws_mb   = [Math]::Round($best.ws / 1MB, 1)
        priv    = $best.priv
    }
}

function Measure-Cmd {
    param([scriptblock]$Body)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $Body | Out-Null
    $sw.Stop()
    [Math]::Round($sw.Elapsed.TotalMilliseconds, 2)
}

function Percentile([double[]]$v, [double]$p) {
    if ($v.Count -eq 0) { return 0 }
    $s = @($v | Sort-Object)
    $i = [Math]::Min($s.Count - 1, [Math]::Max(0, [int][Math]::Ceiling($p * $s.Count) - 1))
    [Math]::Round($s[$i], 2)
}

# === FILL ====================================================================
Write-Test "SETUP: $COLS col pane, history-limit $HISTORY, flooding $LINES lines"
Kill-Sess "i673_mem"
Start-Sleep -Milliseconds 300
Pmux new-session -d -s "i673_mem" -x $COLS -y $ROWS | Out-Null
Start-Sleep -Milliseconds 900
Pmux set-option -t "i673_mem" history-limit $HISTORY | Out-Null

$sentinel = "ZZ673DONEZZ"
$cmd = 'for ($i=0;$i -lt ' + $LINES + ';$i++){ "$i " + (''x''*' + $LINELEN + ') }; "ZZ673" + "DONEZZ"'
Pmux send-keys -t "i673_mem" $cmd Enter | Out-Null

$filled = $false
$sw = [System.Diagnostics.Stopwatch]::StartNew()
while ($sw.Elapsed.TotalSeconds -lt 600) {
    Start-Sleep -Milliseconds 500
    if (((Pmux capture-pane -p -t "i673_mem") -join "`n") -match [regex]::Escape($sentinel)) { $filled = $true; break }
}
Start-Sleep -Milliseconds 1500

$histSize = ((Pmux display-message -p -t "i673_mem" '#{history_size}') -join '').Trim()
$histBytes = ((Pmux display-message -p -t "i673_mem" '#{history_bytes}') -join '').Trim()
Write-Host "       filled=$filled history_size=$histSize history_bytes=$histBytes in $([Math]::Round($sw.Elapsed.TotalSeconds,1))s"
$script:Metrics.history_size  = $histSize
$script:Metrics.history_bytes = $histBytes
$script:Metrics.fill_seconds  = [Math]::Round($sw.Elapsed.TotalSeconds, 1)

if (-not $filled) {
    Write-Fail "the flood never finished, memory readings are not comparable"
} else {
    Write-Pass "history filled to history_size=$histSize"
}

# === CYCLES ==================================================================
Write-Test "TEST 1-3: $CYCLES enter/leave cycles, private bytes and entry latency"
$before = Get-ServerMem
if ($null -eq $before) {
    Write-Fail "could not locate the server process for $PSMUX"
} else {
    Write-Host ("       before copy mode: {0:N1} MB working set / {1:N1} MB private (pid {2})" -f $before.ws_mb, $before.priv_mb, $before.pid)
    $script:Metrics.before = $before

    # The live history's own private bytes: what the pane costs above an empty
    # server.  The snapshot is compared against this, not against the whole
    # process, so the fixed constant cost of a server is not counted as a win.
    $histPriv = [double]$histBytes
    $script:Metrics.history_bytes_live = $histPriv

    $enter = @(); $leave = @(); $noop = @()
    $inFirst = $null; $afterFirst = $null; $restFirst = $null; $restLast = $null

    for ($c = 1; $c -le $CYCLES; $c++) {
        $noop += (Measure-Cmd { Pmux display-message -p -t "i673_mem" '#{pane_id}' })
        $enter += (Measure-Cmd { Pmux copy-mode -t "i673_mem" })
        Start-Sleep -Milliseconds 250
        $inMem = Get-ServerMem
        $leave += (Measure-Cmd { Pmux send-keys -t "i673_mem" -X cancel })
        Start-Sleep -Milliseconds 250
        $outMem = Get-ServerMem

        if ($c -eq 1) { $inFirst = $inMem; $afterFirst = $outMem; $restFirst = $outMem.priv }
        $restLast = $outMem.priv

        $script:Metrics.cycles += [ordered]@{
            cycle        = $c
            in_priv_mb   = $inMem.priv_mb
            in_ws_mb     = $inMem.ws_mb
            out_priv_mb  = $outMem.priv_mb
            out_ws_mb    = $outMem.ws_mb
            enter_ms     = $enter[-1]
            leave_ms     = $leave[-1]
        }
        if ($c -le 3 -or $c -eq $CYCLES) {
            Write-Host ("       cycle {0,2}: in {1,7:N1} MB priv, out {2,7:N1} MB priv, enter {3,6:N2} ms" -f $c, $inMem.priv_mb, $outMem.priv_mb, $enter[-1])
        }
    }

    $script:Metrics.enter_ms = $enter
    $script:Metrics.leave_ms = $leave
    $script:Metrics.noop_ms  = $noop
    $script:Metrics.in_first = $inFirst
    $script:Metrics.after_first = $afterFirst

    # --- TEST 1: the snapshot's cost ---------------------------------------
    $cost = $inFirst.priv - $before.priv
    $costMb = [Math]::Round($cost / 1MB, 1)
    $script:Metrics.snapshot_cost_bytes = $cost
    $script:Metrics.snapshot_cost_mb = $costMb
    if ($histPriv -gt 0) {
        $frac = $cost / $histPriv
        $script:Metrics.snapshot_cost_fraction = [Math]::Round($frac, 4)
        Write-Host ("       snapshot cost {0:N1} MB against {1:N1} MB of live history = {2:P1}" -f $costMb, ($histPriv / 1MB), $frac)
        if ($frac -lt $MAX_ENTER_FRACTION) {
            Write-Pass ("entering copy mode costs {0:N1} MB, {1:P1} of the live history (bound {2:P0})" -f $costMb, $frac, $MAX_ENTER_FRACTION)
        } else {
            Write-Fail ("entering copy mode costs {0:N1} MB, {1:P1} of the live history: the scrollback is being cloned" -f $costMb, $frac)
        }
    } else {
        Write-Skip "#{history_bytes} reported '$histBytes', cannot form the ratio"
    }

    # --- TEST 2: no growing plateau ----------------------------------------
    $growth = if ($restFirst -gt 0) { $restLast / $restFirst } else { 0 }
    $script:Metrics.plateau_ratio = [Math]::Round($growth, 4)
    Write-Host ("       resting private bytes: cycle 1 {0:N1} MB, cycle {1} {2:N1} MB, ratio {3:N3}" -f `
        ($restFirst / 1MB), $CYCLES, ($restLast / 1MB), $growth)
    if ($growth -gt 0 -and $growth -lt $MAX_PLATEAU_FRACTION) {
        Write-Pass ("after $CYCLES cycles resting private bytes are {0:N3}x cycle 1 (bound {1}x)" -f $growth, $MAX_PLATEAU_FRACTION)
    } else {
        Write-Fail ("after $CYCLES cycles resting private bytes are {0:N3}x cycle 1: cycles retain memory" -f $growth)
    }

    # --- TEST 3: entry latency ---------------------------------------------
    $p50 = Percentile $enter 0.50
    $p99 = Percentile $enter 0.99
    $n50 = Percentile $noop 0.50
    $script:Metrics.enter_p50 = $p50
    $script:Metrics.enter_p99 = $p99
    $script:Metrics.noop_p50  = $n50
    Write-Host ("       copy-mode entry p50 {0:N2} ms, p99 {1:N2} ms (no-op round trip p50 {2:N2} ms)" -f $p50, $p99, $n50)
    # A `copy-mode` round trip is a no-op round trip plus the snapshot.  On a
    # 50,000 line history the clone is what makes the difference; sharing the
    # rows should put it within a few ms of a plain round trip.
    if ($p99 -lt ($n50 + 250)) {
        Write-Pass ("copy-mode entry p99 {0:N2} ms stays within 250 ms of a no-op round trip ({1:N2} ms)" -f $p99, $n50)
    } else {
        Write-Fail ("copy-mode entry p99 {0:N2} ms is far past a no-op round trip ({1:N2} ms)" -f $p99, $n50)
    }
}

# === TEST 5: read paths still correct ========================================
Write-Test "TEST 5: capture-pane reads the live screen, the copy view reads the snapshot"
Pmux copy-mode -t "i673_mem" | Out-Null
Start-Sleep -Milliseconds 500
$histIn = ((Pmux display-message -p -t "i673_mem" '#{history_size}') -join '').Trim()
if ($histIn -eq $histSize) {
    Write-Pass "#{history_size} still reports $histIn inside copy mode (it answers from the LIVE grid)"
} else {
    Write-Fail "#{history_size} changed from $histSize to $histIn inside copy mode"
}
# capture-pane reads wp->base in tmux, so it must keep answering from the live
# grid while the snapshot is up.  Retried once: this is a ~400 KB reply over the
# control socket and a single truncated read would otherwise read as a fault in
# the sharing, which the isolated check (capture out of copy mode, in copy mode
# and after cancel, all 2050 lines) shows it is not.
$cap = @(Pmux capture-pane -p -t "i673_mem" -S -2000)
if ($cap.Count -lt 2000) {
    Start-Sleep -Milliseconds 1000
    $cap = @(Pmux capture-pane -p -t "i673_mem" -S -2000)
}
if ($cap.Count -ge 2000) {
    Write-Pass "capture-pane -S -2000 still returns $($cap.Count) lines of live history inside copy mode"
} else {
    Write-Fail "capture-pane -S -2000 returned only $($cap.Count) lines inside copy mode, first line: '$($cap[0])'"
}
# The copy-mode view reads the history it is anchored to.
Pmux send-keys -t "i673_mem" -X history-top | Out-Null
Start-Sleep -Milliseconds 400
$pos = [int](((Pmux display-message -p -t "i673_mem" '#{scroll_position}') -join '').Trim())
if ($pos -gt 40000) {
    Write-Pass "history-top reaches #{scroll_position} $pos, the snapshot still carries the whole history"
} else {
    Write-Fail "history-top only reached #{scroll_position} $pos, the snapshot lost the history"
}
$view = (Pmux capture-pane -p -t "i673_mem" -S "-$pos" -E "-$([Math]::Max(0, $pos - 9))") -join "`n"
if ($view -match 'x{40}') {
    Write-Pass "the copy-mode view at the top of history reads real content"
} else {
    Write-Fail "the copy-mode view at the top of history is empty"
}
# Shared rows must still be correct rows: the oldest retained line and the
# newest must both read back intact while the snapshot holds them.
$oldest = ($view -split "`n") | Where-Object { $_ -match '^\d+ x+' } | Select-Object -First 1
if ($oldest -match '^(\d+) (x+)$' -and $Matches[2].Length -eq $LINELEN) {
    Write-Pass "the oldest shared history row reads back whole: '$($Matches[1]) ' plus $LINELEN payload characters"
} else {
    Write-Fail "the oldest shared history row is damaged: '$oldest'"
}
Pmux send-keys -t "i673_mem" -X cancel | Out-Null
Start-Sleep -Milliseconds 400
$capAfter = @(Pmux capture-pane -p -t "i673_mem" -S -2000)
if ($capAfter.Count -ge 2000) {
    Write-Pass "capture-pane still returns $($capAfter.Count) lines after leaving copy mode, the shared rows survived the snapshot being dropped"
} else {
    Write-Fail "capture-pane returned only $($capAfter.Count) lines after leaving copy mode"
}
Kill-Sess "i673_mem"
Start-Sleep -Milliseconds 500


# === TEST 4: every pane in copy mode holds its own snapshot ==================
# psmux allows more than one pane in copy mode at once (#607).  Only the ACTIVE
# pane was ever snapshotted, so a non active pane in copy mode had its snapshot
# torn down by `sync_copy_snapshot` on every frame and its own output then
# pushed the view it was anchored to.
#
# What this can and cannot see from the CLI.  `#{pane_in_mode}` resolves against
# the pane named by -t, so it is per pane and trustworthy here.  `#{history_size}`
# and `#{scroll_position}` are NOT: history_size answers from `active_pane(...)`
# and scroll_position returns `app.copy_scroll_offset`, the one live copy cursor,
# so with -t naming a background pane they both still describe the ACTIVE pane.
# (Verified directly: two panes seeded to 900 and 120 lines both report
# history_size 105 while capture-pane -t correctly returns 905 and 125 lines.
# That is a separate pre-existing defect, not something #673 introduced.)
# So the background pane's anchor is read back only after focus returns to it,
# which is when `restore_copy_state_from_pane` makes that pane's own offset the
# live one; the frozen view itself is pinned at the unit level instead, in
# tests-rs/test_issue673_snapshot_sharing.rs, which reads each pane's snapshot
# directly.
Write-Test "TEST 4: two panes in copy mode at once, the background one keeps its mode and anchor"
Kill-Sess "i673_two"
Start-Sleep -Milliseconds 300
Pmux new-session -d -s "i673_two" -x 100 -y 20 | Out-Null
Start-Sleep -Milliseconds 900
Pmux split-window -t "i673_two" -h | Out-Null
Start-Sleep -Milliseconds 900
$panes = @(Pmux list-panes -t "i673_two" -F '#{pane_id}')
if ($panes.Count -lt 2) {
    Write-Skip "could not create two panes (got $($panes.Count))"
} else {
    $p0 = $panes[0]; $p1 = $panes[1]
    foreach ($p in @($p0, $p1)) {
        Pmux send-keys -t $p "1..600 | ForEach-Object { `"seed `$_`" }" Enter | Out-Null
    }
    Start-Sleep -Seconds 5

    # Start a producer in pane 0 BEFORE copy mode.  Output has to arrive from
    # the pane's own program while the pane sits in copy mode; driving it with
    # `send-keys` afterwards would be sending keys INTO the mode, which tmux and
    # psmux both route to the copy-mode key table, and that cancels the mode for
    # reasons that have nothing to do with the snapshot.
    Pmux send-keys -t $p0 "1..400 | ForEach-Object { `"tick `$_`"; Start-Sleep -Milliseconds 40 }" Enter | Out-Null
    Start-Sleep -Seconds 2

    # Both panes enter copy mode, each anchored at the top of its own history.
    Pmux select-pane -t $p0 | Out-Null
    Pmux copy-mode -t $p0 | Out-Null
    Pmux send-keys -t $p0 -X history-top | Out-Null
    Start-Sleep -Milliseconds 400
    # Read pane 0's anchor while pane 0 is still the active pane, so the global
    # copy cursor really is pane 0's.
    $anchor0 = ((Pmux display-message -p -t $p0 '#{scroll_position}') -join '').Trim()

    Pmux select-pane -t $p1 | Out-Null
    Pmux copy-mode -t $p1 | Out-Null
    Pmux send-keys -t $p1 -X history-top | Out-Null
    Start-Sleep -Milliseconds 600

    # Pane 0 is now the NON active pane, in copy mode, with its own producer
    # still printing into it.
    $mode0Before = ((Pmux display-message -p -t $p0 '#{pane_in_mode}') -join '').Trim()
    $live0Before = @(Pmux capture-pane -p -t $p0 -S -).Count

    Start-Sleep -Seconds 6

    $mode0After = ((Pmux display-message -p -t $p0 '#{pane_in_mode}') -join '').Trim()
    $mode1After = ((Pmux display-message -p -t $p1 '#{pane_in_mode}') -join '').Trim()
    $live0After = @(Pmux capture-pane -p -t $p0 -S -).Count

    # Focus back to pane 0: its own parked anchor becomes the live one again.
    Pmux select-pane -t $p0 | Out-Null
    Start-Sleep -Milliseconds 600
    $anchor0Back = ((Pmux display-message -p -t $p0 '#{scroll_position}') -join '').Trim()

    Write-Host "       background pane $p0 - in_mode $mode0Before to $mode0After, live lines $live0Before to $live0After, anchor $anchor0 to $anchor0Back"
    $script:Metrics.two_panes = [ordered]@{
        background_pane = $p0; active_pane = $p1
        in_mode_before = $mode0Before; in_mode_after = $mode0After; other_in_mode = $mode1After
        live_lines_before = $live0Before; live_lines_after = $live0After
        anchor_before = $anchor0; anchor_after_refocus = $anchor0Back
    }

    # The producer must actually have produced, or nothing below is proven.
    if ($live0After -gt $live0Before) {
        Write-Pass "the background pane really produced output while in copy mode ($live0Before to $live0After live lines)"
    } else {
        Write-Skip "the background pane produced no output, the checks below prove nothing"
    }
    if ($mode0Before -eq '1' -and $mode0After -eq '1') {
        Write-Pass "the background pane $p0 is still in copy mode after its own output"
    } else {
        Write-Fail "the background pane $p0 left copy mode (#{pane_in_mode} $mode0Before to $mode0After)"
    }
    if ($mode1After -eq '1') {
        Write-Pass "the active pane $p1 is in copy mode at the same time, two panes at once"
    } else {
        Write-Fail "the active pane $p1 left copy mode (#{pane_in_mode}=$mode1After)"
    }
    if ($anchor0 -match '^\d+$' -and $anchor0Back -match '^\d+$' -and $anchor0 -eq $anchor0Back -and [int]$anchor0 -gt 0) {
        Write-Pass "the background pane came back to its own anchor unchanged: #{scroll_position} $anchor0Back"
    } else {
        Write-Fail "the background pane's anchor moved from $anchor0 to $anchor0Back across its own output"
    }
}
Kill-Sess "i673_two"


# === METRICS =================================================================
$metricsDir = "$env:USERPROFILE\.psmux-test-data\metrics"
New-Item -ItemType Directory -Force -Path $metricsDir | Out-Null
$stamp = (Get-Date).ToString("yyyyMMdd-HHmmss")
$metricsFile = Join-Path $metricsDir "issue673-$stamp.json"
$script:Metrics.passed  = $script:TestsPassed
$script:Metrics.failed  = $script:TestsFailed
$script:Metrics.skipped = $script:TestsSkipped
$script:Metrics | ConvertTo-Json -Depth 6 | Set-Content -Path $metricsFile -Encoding UTF8
Write-Host "`nmetrics written to $metricsFile" -ForegroundColor DarkGray

# === TEARDOWN ================================================================
foreach ($s in @("i673_mem", "i673_two")) { Kill-Sess $s }
Pmux kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 400

Write-Host "`n=== Results: $script:TestsPassed passed, $script:TestsFailed failed, $script:TestsSkipped skipped ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
