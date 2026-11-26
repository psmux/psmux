# Issue #673: the copy mode snapshot PR #671 installed had two measured limits.
#
# 1. MEMORY. `Parser::snapshot` deep copied the pane's grid, so entering copy
#    mode paid for the whole scrollback a second time. Measured on the merged
#    tree, one pane at 200 columns, history-limit 60000, filled to
#    history_size 49953:
#
#      BEFORE copy mode : working set 197.1 MB  private 191.9 MB
#      IN     copy mode : working set 378.0 MB  private 378.4 MB
#      AFTER  copy mode : working set 199.6 MB  private 192.0 MB
#
#    a 186.5 MB spike, and 30 enter and leave cycles plateaued 61 MB above the
#    baseline. A retained row is immutable (compacted once on the way into
#    history, issue #641, never written to again), so the snapshot now SHARES
#    those rows with the live grid and copies only the visible screen. Same
#    pane after the fix: 191.8 MB to 192.7 MB, a 0.9 MB snapshot, and 30 cycles
#    flat at 192.7 MB.
#
# 2. SCOPE. `sync_copy_snapshot` gated on the active pane of the active window,
#    while psmux allows several panes in copy mode at once (#607) and
#    `copy-mode -t` puts a pane that is not focused into it. Such a pane took a
#    snapshot and lost it on the very next frame, so its own output kept
#    pushing the view it was meant to freeze. Two panes both printing, the non
#    active one in copy mode, its last visible line:
#
#      [NON ACTIVE PANE] before: 'P LINE 93'   after 4s: 'P LINE 152'   <- moved
#      [ACTIVE PANE]     before: 'P LINE 32'   after 4s: 'P LINE 32'    <- frozen
#
#    tmux has no such gate: `window_copy_init` (window-copy.c) copies the grid
#    of whichever pane enters the mode, and the copy lives on that pane until
#    the mode is dismissed, focused or not.
#
# WHAT IS ASSERTED
#   1. Entering copy mode on a 50000 line, 200 column history costs less than
#      MAX_SNAPSHOT_MB of private bytes. The bound is 15 MB: comfortably above
#      the 0.9 MB the fix produces and far below the 186.5 MB the old build
#      pays, so it cannot pass on an unfixed binary.
#   2. 30 enter and leave cycles retain less than MAX_RETAINED_MB (20 MB):
#      measured 0 MB after, 61 MB before.
#   3. The history is intact after all that snapshotting: capture-pane -S still
#      returns the lines, at full length, contiguously numbered.
#   4. capture-pane still answers with the LIVE screen while copy mode is up.
#      That is ratified tmux parity from PR #671 and the sharing must not
#      quietly turn it into a read of the frozen view.
#   5. A pane in copy mode that is NOT the focused one holds its own snapshot:
#      its rendered rows do not move while it keeps printing.
#   6. Control: the focused pane in copy mode is frozen too, and a pane that is
#      not in copy mode at all keeps moving.

$ErrorActionPreference = "Continue"

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
Write-Host "exe: $PSMUX"

$DATA = $env:PSMUX_DATA_DIR
if (-not $DATA) { $DATA = Join-Path $env:USERPROFILE ".psmux" }

function Pmux { & $PSMUX -L $SOCK @args 2>&1 }
function Kill-Sess([string]$n) { Pmux kill-session -t $n | Out-Null }

$script:Metrics = [ordered]@{
    timestamp = (Get-Date).ToString("o")
    exe       = $PSMUX
    issue     = 673
}

# Server processes started from THIS executable, never matched by image name
# alone: other psmux builds and the developer's own sessions must stay out of
# the measurement.
function Get-ServerPid {
    $procs = @(Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.ExecutablePath -eq $PSMUX -and $_.CommandLine -match 'server' })
    if ($procs.Count -eq 0) { return $null }
    # The flooded session's server is the one holding the history; the idle
    # `-s __warm__` standby next to it holds nothing.
    ($procs | Sort-Object -Property @{Expression = { $_.PrivatePageCount }} -Descending)[0].ProcessId
}
function Get-PrivMb([int]$ProcId) {
    try { [Math]::Round((Get-Process -Id $ProcId -EA Stop).PrivateMemorySize64 / 1MB, 1) } catch { $null }
}
function Get-WsMb([int]$ProcId) {
    try { [Math]::Round((Get-Process -Id $ProcId -EA Stop).WorkingSet64 / 1MB, 1) } catch { $null }
}

# === MEMORY: flood one deep pane ============================================
$MEM = "i673_mem"
$LINES = 50000
Kill-Sess $MEM
Start-Sleep -Milliseconds 400
Pmux new-session -d -s $MEM -x 200 -y 50 | Out-Null
Start-Sleep -Milliseconds 1200
Pmux set-option -t $MEM history-limit 60000 | Out-Null

Write-Host "`nflooding $LINES lines into a 200 column pane..." -ForegroundColor DarkGray
$sentinel = "ZZ673" + "DONEZZ"
$cmd = 'for ($i=0;$i -lt ' + $LINES + ';$i++){ "$i " + (''x''*78) }; "ZZ673" + "DONEZZ"'
Pmux send-keys -t $MEM $cmd Enter | Out-Null
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$flooded = $false
while ($sw.Elapsed.TotalSeconds -lt 420) {
    Start-Sleep -Milliseconds 700
    $view = (Pmux capture-pane -p -t $MEM) -join "`n"
    if ($view -match [regex]::Escape($sentinel)) { $flooded = $true; break }
}
Start-Sleep -Seconds 2

$serverPid = Get-ServerPid
$histSize = ((Pmux display-message -p -t $MEM '#{history_size}') -join '').Trim()

# === TEST 1: the snapshot costs the screen, not the history =================
$MAX_SNAPSHOT_MB = 15
Write-Test "TEST 1: entering copy mode on a $LINES line history is cheap"
if (-not $flooded) {
    Write-Skip "the flood did not finish in time, memory readings are not comparable"
} elseif ($null -eq $serverPid) {
    Write-Fail "could not locate the server process for $PSMUX"
} else {
    $beforeP = Get-PrivMb $serverPid
    $beforeW = Get-WsMb $serverPid
    Pmux copy-mode -t $MEM | Out-Null
    Start-Sleep -Seconds 2
    $inP = Get-PrivMb $serverPid
    $inW = Get-WsMb $serverPid
    $inMode = ((Pmux display-message -p -t $MEM '#{pane_in_mode}') -join '').Trim()
    Pmux send-keys -t $MEM -X cancel | Out-Null
    Start-Sleep -Seconds 2
    $afterP = Get-PrivMb $serverPid
    $cost = [Math]::Round($inP - $beforeP, 1)

    $script:Metrics.history_size      = $histSize
    $script:Metrics.before_private_mb = $beforeP
    $script:Metrics.before_ws_mb      = $beforeW
    $script:Metrics.in_private_mb     = $inP
    $script:Metrics.in_ws_mb          = $inW
    $script:Metrics.after_private_mb  = $afterP
    $script:Metrics.snapshot_cost_mb  = $cost

    Write-Host ("       history_size $histSize, private {0} MB -> {1} MB in copy mode -> {2} MB after" -f $beforeP, $inP, $afterP)
    if ($inMode -ne "1") {
        Write-Fail "copy-mode did not put the pane in copy mode (pane_in_mode='$inMode')"
    } elseif ($cost -lt $MAX_SNAPSHOT_MB) {
        Write-Pass ("the snapshot cost {0} MB of private bytes, under the {1} MB bound (was 186.5 MB before the fix)" -f $cost, $MAX_SNAPSHOT_MB)
    } else {
        Write-Fail ("the snapshot cost {0} MB of private bytes: the scrollback is still being copied, not shared" -f $cost)
    }
}

# === TEST 2: repeated entry does not accumulate =============================
$MAX_RETAINED_MB = 20
Write-Test "TEST 2: 30 enter and leave cycles retain nothing"
if (-not $flooded -or $null -eq $serverPid) {
    Write-Skip "no comparable baseline"
} else {
    $base = Get-PrivMb $serverPid
    for ($c = 0; $c -lt 30; $c++) {
        Pmux copy-mode -t $MEM | Out-Null
        Start-Sleep -Milliseconds 120
        Pmux send-keys -t $MEM -X cancel | Out-Null
        Start-Sleep -Milliseconds 120
    }
    Start-Sleep -Seconds 2
    $end = Get-PrivMb $serverPid
    $retained = [Math]::Round($end - $base, 1)
    $script:Metrics.cycles_baseline_mb = $base
    $script:Metrics.cycles_end_mb      = $end
    $script:Metrics.cycles_retained_mb = $retained
    Write-Host ("       private {0} MB before the cycles, {1} MB after" -f $base, $end)
    if ($retained -lt $MAX_RETAINED_MB) {
        Write-Pass ("30 cycles retained {0} MB, under the {1} MB bound (was ~61 MB before the fix)" -f $retained, $MAX_RETAINED_MB)
    } else {
        Write-Fail ("30 cycles retained {0} MB: every entry is still allocating a copy of the history" -f $retained)
    }
}

# === TEST 3: the shared history is still complete and correct ===============
Write-Test "TEST 3: the history survives being shared with a snapshot"
if (-not $flooded) {
    Write-Skip "the flood did not finish in time"
} else {
    # Timed as well: reading the history one row at a time must stay a direct
    # index into it. Measured 363 ms on the pre fix binary, and a walk per row
    # made this time out entirely while the sharing was being built.
    $capSw = [System.Diagnostics.Stopwatch]::StartNew()
    $cap = @(Pmux capture-pane -p -t $MEM -S -49000)
    $capSw.Stop()
    $script:Metrics.capture_49000_ms = [int]$capSw.Elapsed.TotalMilliseconds
    Write-Host ("       capture-pane -S -49000 took {0} ms" -f $script:Metrics.capture_49000_ms)
    $full = @($cap | Where-Object { $_ -match '^\d+ x{78}$' })
    if ($cap.Count -ge 48000) {
        Write-Pass "capture-pane -S -49000 returned $($cap.Count) lines"
    } else {
        Write-Fail "capture-pane -S -49000 returned only $($cap.Count) lines, history was lost"
    }
    if ($full.Count -ge 40000) {
        Write-Pass "$($full.Count) history lines still carry all 78 payload characters"
    } else {
        Write-Fail "only $($full.Count) history lines are intact, the shared rows were damaged"
    }
    $nums = @($full | ForEach-Object { [int]($_ -replace ' x+$', '') })
    if ($nums.Count -gt 100) {
        $gaps = 0
        for ($i = 1; $i -lt $nums.Count; $i++) { if ($nums[$i] -ne $nums[$i - 1] + 1) { $gaps++ } }
        if ($gaps -eq 0) {
            Write-Pass "the $($nums.Count) captured line numbers are contiguous, no row was dropped"
        } else {
            Write-Fail "$gaps discontinuities in the captured line numbers"
        }
    } else {
        Write-Skip "too few numbered lines captured to check contiguity"
    }
}

# === TEST 4: capture-pane still reads the LIVE screen in copy mode ==========
# The counter has to be started BEFORE copy mode opens: once a pane is in copy
# mode, send-keys goes to the mode, not to the shell (as in tmux).
Write-Test "TEST 4: capture-pane reads the live screen while copy mode is up"
$LIVE = "i673_live"
Kill-Sess $LIVE
Start-Sleep -Milliseconds 300
Pmux new-session -d -s $LIVE -x 80 -y 24 | Out-Null
Start-Sleep -Seconds 2
Pmux send-keys -t $LIVE '1..100000 | ForEach-Object { Write-Host "L673 $_"; Start-Sleep -Milliseconds 150 }' Enter | Out-Null
Start-Sleep -Seconds 3
Pmux copy-mode -t $LIVE | Out-Null
Start-Sleep -Milliseconds 700
$atEntry = @((Pmux capture-pane -p -t $LIVE) | Where-Object { $_ -match 'L673 (\d+)' } | Select-Object -Last 1)
$nEntry = if ($atEntry) { [int](($atEntry[0] -replace '.*L673 (\d+).*', '$1')) } else { -1 }
Start-Sleep -Seconds 4
$later = @((Pmux capture-pane -p -t $LIVE) | Where-Object { $_ -match 'L673 (\d+)' } | Select-Object -Last 1)
$nLater = if ($later) { [int](($later[0] -replace '.*L673 (\d+).*', '$1')) } else { -1 }
$inMode = ((Pmux display-message -p -t $LIVE '#{pane_in_mode}') -join '').Trim()
if ($inMode -ne "1") {
    Write-Fail "the pane left copy mode during the check (pane_in_mode='$inMode')"
} elseif ($nEntry -lt 0 -or $nLater -lt 0) {
    Write-Skip "the counter did not produce readable output (saw '$($atEntry -join '')' / '$($later -join '')')"
} elseif ($nLater -gt $nEntry) {
    Write-Pass "capture-pane followed the live screen while copy mode was up (L673 $nEntry then $nLater), as tmux does"
} else {
    Write-Fail "capture-pane stayed at L673 ${nEntry}: it is reading the frozen copy view, not the live screen"
}
Pmux send-keys -t $LIVE -X cancel | Out-Null
Kill-Sess $LIVE

# === SCOPE: a pane in copy mode that is not the focused one =================
# The copy view is read through the server's own render (dump-state), because
# capture-pane deliberately answers with the live screen (TEST 4).
function Get-DumpJson([string]$Session) {
    $base = Join-Path $DATA ("{0}__{1}" -f $SOCK, $Session)
    if (-not (Test-Path "$base.port")) { $base = Join-Path $DATA $Session }
    if (-not (Test-Path "$base.port")) { return $null }
    $port = (Get-Content "$base.port" -Raw).Trim()
    $key  = (Get-Content "$base.key" -Raw).Trim()
    try {
        $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    } catch { return $null }
    $tcp.NoDelay = $true; $tcp.ReceiveTimeout = 5000
    $stream = $tcp.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush()
    $null = $reader.ReadLine()
    $writer.Write("PERSISTENT`n"); $writer.Flush()
    $writer.Write("dump-state`n"); $writer.Flush()
    $best = $null
    for ($j = 0; $j -lt 60; $j++) {
        try { $line = $reader.ReadLine() } catch { break }
        if ($null -eq $line) { break }
        if ($line -ne "NC" -and $line.Length -gt 100) { $best = $line; $tcp.ReceiveTimeout = 200 }
    }
    $tcp.Close()
    if ($best) { return $best | ConvertFrom-Json }
    return $null
}
function Find-Leaf($node, [int]$id) {
    if ($null -eq $node) { return $null }
    if ($node.type -eq 'leaf') { if ([int]$node.id -eq $id) { return $node } else { return $null } }
    foreach ($c in $node.children) {
        $r = Find-Leaf $c $id
        if ($r) { return $r }
    }
    return $null
}
function Get-PaneView([string]$Session, [string]$PaneId) {
    $n = [int]($PaneId -replace '%', '')
    $j = Get-DumpJson $Session
    if ($null -eq $j) { return $null }
    $leaf = Find-Leaf $j.layout $n
    if ($null -eq $leaf) { return $null }
    $sb = [System.Text.StringBuilder]::new()
    foreach ($row in $leaf.rows_v2) {
        foreach ($run in $row.runs) { [void]$sb.Append($run.text) }
        [void]$sb.Append("`n")
    }
    return $sb.ToString()
}

$SCOPE = "i673_scope"
Kill-Sess $SCOPE
Start-Sleep -Milliseconds 300
Pmux new-session -d -s $SCOPE -x 100 -y 40 | Out-Null
Start-Sleep -Seconds 2
Pmux split-window -v -t $SCOPE | Out-Null
Start-Sleep -Seconds 2
$ids = @(Pmux list-panes -t $SCOPE -F '#{pane_id}')
$activeId = ((Pmux display-message -p -t $SCOPE '#{pane_id}') -join '').Trim()
$inactiveId = @($ids | Where-Object { $_ -ne $activeId })[0]
# Pane ids repeat per session and $MEM still owns the same %N, so a bare id
# from outside a pane is refused as ambiguous since #1078. Commands get the
# pane as session:window.pane; the id stays the key into dump-state.
$script:Tgt = @{}
foreach ($l in @(Pmux list-panes -t $SCOPE -F '#{pane_id} #{session_name}:#{window_index}.#{pane_index}')) {
    $f = "$l".Trim() -split ' ', 2
    if ($f.Count -eq 2) { $script:Tgt[$f[0]] = $f[1] }
}
foreach ($p in $ids) {
    Pmux send-keys -t $script:Tgt[$p] '1..100000 | ForEach-Object { Write-Host "P LINE $_"; Start-Sleep -Milliseconds 120 }' Enter | Out-Null
}
Start-Sleep -Seconds 3

function Test-PaneFrozen([string]$PaneId, [string]$Label) {
    Pmux copy-mode -t $script:Tgt[$PaneId] | Out-Null
    Start-Sleep -Seconds 1
    $null = Get-PaneView $SCOPE $PaneId
    Start-Sleep -Seconds 1
    $a = Get-PaneView $SCOPE $PaneId
    Start-Sleep -Seconds 4
    $b = Get-PaneView $SCOPE $PaneId
    $mode = ((Pmux display-message -p -t $script:Tgt[$PaneId] '#{pane_in_mode}') -join '').Trim()
    if ($null -eq $a -or $null -eq $b) {
        Write-Skip "$Label : could not read the rendered view for $PaneId"
    } elseif ($mode -ne "1") {
        Write-Fail "$Label : copy-mode -t $PaneId did not put the pane in copy mode"
    } else {
        $la = (($a -split "`n") | Where-Object { $_ -match 'P LINE' } | Select-Object -Last 1) -replace '\s+$', ''
        $lb = (($b -split "`n") | Where-Object { $_ -match 'P LINE' } | Select-Object -Last 1) -replace '\s+$', ''
        if ($a -eq $b) {
            Write-Pass "$Label : the view is frozen over 4s of its own output (last line '$la')"
        } else {
            Write-Fail "$Label : the view MOVED while in copy mode ('$la' then '$lb'): the pane has no snapshot"
        }
    }
    Pmux send-keys -t $script:Tgt[$PaneId] -X cancel | Out-Null
    Start-Sleep -Milliseconds 600
}

Write-Test "TEST 5: a pane in copy mode that is NOT the focused one holds its own snapshot"
if (-not $inactiveId) {
    Write-Fail "could not identify a non active pane in $SCOPE"
} else {
    Test-PaneFrozen $inactiveId "non active pane $inactiveId"
}

Write-Test "TEST 6: control, the focused pane in copy mode is frozen as well"
Test-PaneFrozen $activeId "focused pane $activeId"

Write-Test "TEST 7: control, a pane that is NOT in copy mode keeps moving"
$c1 = Get-PaneView $SCOPE $activeId
Start-Sleep -Seconds 4
$c2 = Get-PaneView $SCOPE $activeId
if ($null -eq $c1 -or $null -eq $c2) {
    Write-Skip "could not read the rendered view"
} elseif ($c1 -ne $c2) {
    Write-Pass "outside copy mode the pane follows its output, so the freeze above is real"
} else {
    Write-Fail "the pane did not move outside copy mode: the fixture proves nothing"
}

# === METRICS ================================================================
$metricsDir = "$env:USERPROFILE\.psmux-test-data\metrics"
New-Item -ItemType Directory -Force -Path $metricsDir | Out-Null
$stamp = (Get-Date).ToString("yyyyMMdd-HHmmss")
$metricsFile = Join-Path $metricsDir "issue673-$stamp.json"
$script:Metrics.passed  = $script:TestsPassed
$script:Metrics.failed  = $script:TestsFailed
$script:Metrics.skipped = $script:TestsSkipped
$script:Metrics | ConvertTo-Json -Depth 5 | Set-Content -Path $metricsFile -Encoding UTF8
Write-Host "`nmetrics written to $metricsFile" -ForegroundColor DarkGray

# === TEARDOWN ===============================================================
foreach ($s in @($MEM, $LIVE, $SCOPE)) { Kill-Sess $s }
Pmux kill-server | Out-Null
Start-Sleep -Milliseconds 400

Write-Host "`n=== Results: $script:TestsPassed passed, $script:TestsFailed failed, $script:TestsSkipped skipped ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
