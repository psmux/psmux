# perf_summary.ps1 - what the perf gates have been recording, as a trend.
#
# The gates each write one JSON per run into %USERPROFILE%\.psmux-test-data\
# metrics. After a few hundred runs that folder answers every question worth
# asking about a regression, and nothing was reading it. This does.
#
#   pwsh -File tests\perf_summary.ps1              last 8 runs of every metric
#   pwsh -File tests\perf_summary.ps1 -Last 20     a longer window
#   pwsh -File tests\perf_summary.ps1 -Metric B    one section (A, B, C, D, T or all)
#   pwsh -File tests\perf_summary.ps1 -Metric T    the trend table on its own
#   pwsh -File tests\perf_summary.ps1 -Csv out.csv the same rows, for a chart
#
# The five sections are the owner's four questions, plus the one that reads the
# answers back over time:
#
#   A  launch to a usable shell prompt, psmux against a bare pwsh, and against
#      Windows Terminal, WezTerm and Alacritty where a head to head run exists
#   B  keystroke to screen, p50 / p90 / p99, with the pwsh cell judged against
#      the ConPTY floor measured in the same run
#   C  creation latency for new-session, new-window and both splits, p50 / p90
#   D  memory (working set and private bytes) and CPU (per creation, per 100
#      keystrokes, at the first prompt of a fresh session, over an idle window,
#      and idle as a percentage of one core) for the server and the attached
#      client, from all five perf gates
#   T  the trend: every headline number as min / median / max over the window,
#      with the newest run compared against the MEDIAN OF THE PREVIOUS FIVE and
#      flagged when it is worse by more than -RegressPct (20 by default)
#
# WHY THE COMPARISON IS AGAINST A MEDIAN OF FIVE AND NOT AGAINST LAST TIME
#
# Any single previous run can be the one that ran while a sibling agent was
# linking. Comparing against it produces a regression alert every other day and
# the alert stops being read. The median of the previous five is unmoved by one
# bad afternoon and still moves immediately when a build genuinely gets slower,
# because a real regression is present in every run after it lands. Twenty
# percent is above this machine's run to run spread on every metric in the
# table: keystroke p50 sat between 1.64 and 1.71 ms over five runs (4 percent),
# launch delta between 224 and 235 ms (5 percent), and creation p50 between 15
# and 28 ms, which is the widest at about 30 percent and is exactly why the
# creation rows are read with the gate's own p50 budget beside them.
#
# Every row carries the git sha of the tree the measured binary was built in, or
# "installed" when it was a cargo install copy, so two rows can be compared
# without guessing which build produced them. Files written before that envelope
# landed show a blank sha; they are still listed, because a number with an
# unknown provenance is still a number, and the point of this script is the
# shape of the line.

param(
    [int]$Last = 8,
    [ValidateSet("all", "A", "B", "C", "D", "T")]
    [string]$Metric = "all",
    [string]$MetricsDir = "",
    [string]$Csv = "",
    # How much worse than the median of the previous five counts as a
    # regression, in percent. See the header for why 20.
    [double]$RegressPct = 20.0,
    # Exit 1 when any tracked metric is flagged. Off by default: this script is
    # a reading tool and the gates are the thing that fails a sweep. CI that
    # wants the trend to be a gate turns it on.
    [switch]$FailOnRegression
)

$ErrorActionPreference = "Continue"
if (-not $MetricsDir) { $MetricsDir = Join-Path $env:USERPROFILE ".psmux-test-data\metrics" }
if (-not (Test-Path $MetricsDir)) {
    Write-Host "no metrics folder at $MetricsDir; run a perf gate first" -ForegroundColor Yellow
    exit 0
}

$script:CsvRows = New-Object System.Collections.Generic.List[object]

function Head($t) {
    Write-Host ""
    Write-Host ("=" * 108) -ForegroundColor DarkGray
    Write-Host "  $t" -ForegroundColor White
    Write-Host ("=" * 108) -ForegroundColor DarkGray
}

function Note($t) { Write-Host "  $t" -ForegroundColor DarkGray }

# Newest first, capped at $Last. A pattern with no files is not an error: a
# machine that has never run test_perf_vs_terminals simply has no row for it.
function Get-Runs {
    param([string]$Pattern, [string[]]$Exclude = @())
    $files = @(Get-ChildItem -LiteralPath $MetricsDir -Filter $Pattern -File -ErrorAction SilentlyContinue |
        Where-Object { $n = $_.Name; -not ($Exclude | Where-Object { $n -like $_ }) } |
        Sort-Object LastWriteTime -Descending | Select-Object -First $Last)
    $out = @()
    foreach ($f in $files) {
        try {
            $j = Get-Content -LiteralPath $f.FullName -Raw | ConvertFrom-Json
            $out += [pscustomobject]@{ File = $f.Name; When = $f.LastWriteTime; Json = $j }
        } catch { }
    }
    return $out
}

function Prop {
    param($Obj, [string]$Name, $Default = $null)
    if ($null -eq $Obj) { return $Default }
    $p = $Obj.PSObject.Properties[$Name]
    if ($null -eq $p -or $null -eq $p.Value) { return $Default }
    return $p.Value
}

function Sha($j) {
    $s = Prop $j "git_sha" ""
    if (-not $s) { return "" }
    if ($s.Length -gt 7 -and $s -notmatch '[^0-9a-f]') { return $s.Substring(0, 7) }
    if ($s.Length -gt 11) { return $s.Substring(0, 11) }
    return $s
}

function Num($v, [int]$d = 1) {
    if ($null -eq $v) { return "-" }
    try { return ([math]::Round([double]$v, $d)).ToString() } catch { return "-" }
}

function Emit($section, $row) {
    $row | Add-Member -NotePropertyName section -NotePropertyValue $section -Force
    $script:CsvRows.Add($row)
}

# ── A: launch to prompt ───────────────────────────────────────────────────
function Show-A {
    Head "A  LAUNCH TO A USABLE PROMPT (ms)"
    $runs = Get-Runs "launch-to-prompt-*.json"
    if ($runs.Count -eq 0) { Note "no test_launch_to_prompt_gate runs recorded" }
    else {
        Write-Host ("  {0,-17} {1,-11} {2,7} {3,7} {4,7} {5,7} {6,7}  {7}" -f "when", "sha", "bareP50", "muxP50", "muxP90", "delta", "gate", "binary") -ForegroundColor DarkCyan
        foreach ($r in $runs) {
            $j = $r.Json
            $bs = Prop $j "bare_stats"; $ps = Prop $j "psmux_stats"
            $row = [pscustomobject]@{
                when   = $r.When.ToString("MM-dd HH:mm")
                sha    = (Sha $j)
                bare_p50 = (Prop $j "bare_median")
                psmux_p50 = (Prop $j "psmux_median")
                psmux_p90 = (Prop $ps "p90")
                delta_ms = (Prop $j "delta_ms")
                gate_ms  = (Prop $j "gate_ms")
                binary   = (Prop $j "binary" "")
            }
            $over = ($null -ne $row.delta_ms -and $null -ne $row.gate_ms -and [double]$row.delta_ms -gt [double]$row.gate_ms)
            Write-Host ("  {0,-17} {1,-11} {2,7} {3,7} {4,7} {5,7} {6,7}  {7}" -f `
                $row.when, $row.sha, (Num $row.bare_p50), (Num $row.psmux_p50), (Num $row.psmux_p90),
                (Num $row.delta_ms), (Num $row.gate_ms), (Split-Path -Leaf $row.binary)) `
                -ForegroundColor $(if ($over) { "Red" } else { "Gray" })
            Emit "A_launch_gate" $row
        }
        Note "delta is psmux median minus bare pwsh median; a red row is over the gate"
    }

    $vt = Get-Runs "perf_vs_terminals-*.json"
    if ($vt.Count -eq 0) { Note "no test_perf_vs_terminals runs recorded, so there is no head to head against WT / WezTerm / Alacritty" ; return }
    Write-Host ""
    Write-Host ("  head to head, launch median per host (ms)") -ForegroundColor DarkCyan
    Write-Host ("  {0,-17} {1,-11} {2,9} {3,9} {4,9} {5,10} {6,10} {7,10}" -f "when", "sha", "bare", "wt", "wezterm", "alacritty", "psmux", "psmux+wt") -ForegroundColor DarkCyan
    foreach ($r in $vt) {
        $t = Prop $r.Json "summary_table" @()
        $get = { param($h) ($t | Where-Object { $_.host -eq $h } | Select-Object -First 1) }
        $row = [pscustomobject]@{
            when      = $r.When.ToString("MM-dd HH:mm")
            sha       = (Sha $r.Json)
            bare      = (Prop (& $get "bare_pwsh") "launch_median")
            wt        = (Prop (& $get "wt_pwsh") "launch_median")
            wezterm   = (Prop (& $get "wezterm_pwsh") "launch_median")
            alacritty = (Prop (& $get "alacritty_pwsh") "launch_median")
            psmux     = (Prop (& $get "psmux_attached") "launch_median")
            psmux_wt  = (Prop (& $get "psmux_in_wt") "launch_median")
        }
        Write-Host ("  {0,-17} {1,-11} {2,9} {3,9} {4,9} {5,10} {6,10} {7,10}" -f `
            $row.when, $row.sha, (Num $row.bare), (Num $row.wt), (Num $row.wezterm),
            (Num $row.alacritty), (Num $row.psmux), (Num $row.psmux_wt))
        Emit "A_vs_terminals" $row
    }
}

# ── B: keystroke to screen ────────────────────────────────────────────────
function Show-B {
    Head "B  KEYSTROKE TO SCREEN (ms)"
    $runs = Get-Runs "keystroke-latency-*.json" @("keystroke-latency-pwsh-*")
    if ($runs.Count -eq 0) { Note "no test_keystroke_latency_gate runs recorded" }
    else {
        Write-Host ("  {0,-17} {1,-11} {2,6} {3,7} {4,7} {5,7} {6,7} {7,7}  {8}" -f "when", "sha", "n", "p50", "p90", "p99", "gateP50", "gateP99", "binary") -ForegroundColor DarkCyan
        foreach ($r in $runs) {
            $j = $r.Json; $p = Prop $j "pooled"
            $row = [pscustomobject]@{
                when = $r.When.ToString("MM-dd HH:mm"); sha = (Sha $j)
                n = (Prop $p "n"); p50 = (Prop $p "median"); p90 = (Prop $p "p90"); p99 = (Prop $p "p99")
                gate_p50 = (Prop $j "medianMaxMs"); gate_p99 = (Prop $j "p99MaxMs")
                binary = (Prop $j "binary" "")
            }
            $over = ($null -ne $row.p50 -and $null -ne $row.gate_p50 -and [double]$row.p50 -ge [double]$row.gate_p50) -or
                    ($null -ne $row.p99 -and $null -ne $row.gate_p99 -and [double]$row.p99 -ge [double]$row.gate_p99)
            Write-Host ("  {0,-17} {1,-11} {2,6} {3,7} {4,7} {5,7} {6,7} {7,7}  {8}" -f `
                $row.when, $row.sha, $row.n, (Num $row.p50 2), (Num $row.p90 2), (Num $row.p99 2),
                (Num $row.gate_p50 1), (Num $row.gate_p99 1), (Split-Path -Leaf $row.binary)) `
                -ForegroundColor $(if ($over) { "Red" } else { "Gray" })
            Emit "B_echo_cell" $row
        }
        Note "echo child in the pane: this is psmux's own path, no shell redraw in it"
    }

    $pw = Get-Runs "keystroke-latency-pwsh-*.json"
    if ($pw.Count -eq 0) { return }
    Write-Host ""
    Write-Host ("  pwsh in the pane, against the ConPTY floor measured in the same run") -ForegroundColor DarkCyan
    Write-Host ("  {0,-17} {1,-11} {2,7} {3,7} {4,7} {5,9} {6,9}" -f "when", "sha", "p50", "p90", "p99", "floorP50", "overFloor") -ForegroundColor DarkCyan
    foreach ($r in $pw) {
        $p = Prop $r.Json "pwsh"
        $row = [pscustomobject]@{
            when = $r.When.ToString("MM-dd HH:mm"); sha = (Sha $r.Json)
            p50 = (Prop $p "median"); p90 = (Prop $p "p90"); p99 = (Prop $p "p99")
            floor_p50 = (Prop $p "floorMedian"); over_floor = (Prop $p "medianDelta")
        }
        Write-Host ("  {0,-17} {1,-11} {2,7} {3,7} {4,7} {5,9} {6,9}" -f `
            $row.when, $row.sha, (Num $row.p50 2), (Num $row.p90 2), (Num $row.p99 2), (Num $row.floor_p50 2), (Num $row.over_floor 2))
        Emit "B_pwsh_cell" $row
    }
    Note "overFloor is the part psmux owns; the floor is conhost's pseudoconsole serializer and every ConPTY consumer pays it"
}

# ── C: creation latency ───────────────────────────────────────────────────
function Show-C {
    Head "C  CREATION LATENCY, TIME TO A VISIBLE PROMPT (ms)"
    $runs = Get-Runs "creation_latency_gate-*.json"
    if ($runs.Count -eq 0) { Note "no test_creation_latency_gate runs recorded" }
    else {
        Write-Host ("  {0,-17} {1,-11} {2,17} {3,17} {4,17} {5,6}  {6}" -f "when", "sha", "new-window p50/p90", "split -v p50/p90", "split -h p50/p90", "slow", "binary") -ForegroundColor DarkCyan
        foreach ($r in $runs) {
            $j = $r.Json
            $s = Prop $j "stats_ms"
            $raw = Prop $j "samples_ms"
            $cell = {
                param($name)
                $st = Prop $s $name
                if ($st) { return @((Prop $st "p50"), (Prop $st "p90")) }
                # A file written before stats_ms existed still has the samples.
                $v = @(Prop $raw $name @())
                if ($v.Count -eq 0) { return @($null, $null) }
                $sorted = @($v | ForEach-Object { [double]$_ } | Sort-Object)
                return @($sorted[[math]::Floor(0.5 * ($sorted.Count - 1))], $sorted[[math]::Floor(0.9 * ($sorted.Count - 1))])
            }
            $nw = & $cell "new-window"; $sv = & $cell "split-window -v"; $sh = & $cell "split-window -h"
            $slowMs = Prop $j "slow_ms" 150
            $slow = 0
            foreach ($k in @("new-window", "split-window -v", "split-window -h")) {
                foreach ($v in @(Prop $raw $k @())) { if ([double]$v -gt [double]$slowMs) { $slow++ } }
            }
            $row = [pscustomobject]@{
                when = $r.When.ToString("MM-dd HH:mm"); sha = (Sha $j)
                new_window_p50 = $nw[0]; new_window_p90 = $nw[1]
                split_v_p50 = $sv[0]; split_v_p90 = $sv[1]
                split_h_p50 = $sh[0]; split_h_p90 = $sh[1]
                slow_count = $slow; binary = (Prop $j "binary" "")
            }
            Write-Host ("  {0,-17} {1,-11} {2,17} {3,17} {4,17} {5,6}  {6}" -f `
                $row.when, $row.sha,
                ("{0} / {1}" -f (Num $nw[0] 0), (Num $nw[1] 0)),
                ("{0} / {1}" -f (Num $sv[0] 0), (Num $sv[1] 0)),
                ("{0} / {1}" -f (Num $sh[0] 0), (Num $sh[1] 0)),
                $slow, (Split-Path -Leaf $row.binary))
            Emit "C_creation_gate" $row
        }
        Note "slow is how many of the 30 creations took longer than the gate's slow_ms; the pool cannot beat a cold shell start"
    }

    $ps = Get-Runs "pane_startup_perf-*.json"
    if ($ps.Count -eq 0) { return }
    Write-Host ""
    Write-Host ("  test_pane_startup_perf, warm pool and first session") -ForegroundColor DarkCyan
    Write-Host ("  {0,-17} {1,-11} {2,12} {3,12} {4,12} {5,12}" -f "when", "sha", "newSess p50", "newWin p50", "splitV p50", "splitH p50") -ForegroundColor DarkCyan
    foreach ($r in $ps) {
        $j = $r.Json
        $med = {
            param($name)
            $v = @(Prop $j $name @())
            if ($v.Count -eq 0) { return $null }
            $s = @($v | ForEach-Object { [double]$_ } | Sort-Object)
            return $s[[math]::Floor(0.5 * ($s.Count - 1))]
        }
        $row = [pscustomobject]@{
            when = $r.When.ToString("MM-dd HH:mm"); sha = (Sha $j)
            new_session_p50 = (& $med "new_session_ms")
            new_window_p50  = (& $med "new_window_ms")
            split_v_p50     = (& $med "pool_depth5_split_v_ms")
            split_h_p50     = (& $med "pool_depth5_split_h_ms")
        }
        Write-Host ("  {0,-17} {1,-11} {2,12} {3,12} {4,12} {5,12}" -f `
            $row.when, $row.sha, (Num $row.new_session_p50 0), (Num $row.new_window_p50 0),
            (Num $row.split_v_p50 0), (Num $row.split_h_p50 0))
        Emit "C_pane_startup" $row
    }
}

# ── D: memory and CPU ─────────────────────────────────────────────────────
function Show-D {
    Head "D  MEMORY AND CPU OF THE SERVER AND THE CLIENT"

    $runs = Get-Runs "creation_latency_gate-*.json"
    $any = $false
    $rows = @()
    foreach ($r in $runs) {
        $res = Prop $r.Json "resources"
        if (-not $res) { continue }
        $any = $true
        $ap = Prop $res "at_prompt"; $af = Prop $res "after_panes"
        $i1 = Prop $res "idle_cpu_pct_one_pane"; $i2 = Prop $res "idle_cpu_pct_many_panes"
        $idleSum = { param($m) if (-not $m) { $null } else { ((Prop $m "server" 0) + (Prop $m "client" 0)) } }
        $rows += [pscustomobject]@{
            when = $r.When.ToString("MM-dd HH:mm"); sha = (Sha $r.Json)
            panes = (Prop $res "panes_opened")
            srv_ws_1 = (Prop (Prop $ap "server") "ws_mb"); srv_priv_1 = (Prop (Prop $ap "server") "private_mb")
            cli_ws_1 = (Prop (Prop $ap "client") "ws_mb"); cli_priv_1 = (Prop (Prop $ap "client") "private_mb")
            srv_ws_n = (Prop (Prop $af "server") "ws_mb"); cli_ws_n = (Prop (Prop $af "client") "ws_mb")
            idle_pct_1 = (& $idleSum $i1); idle_pct_n = (& $idleSum $i2)
        }
    }
    if (-not $any) { Note "no creation gate run has a resources block yet (it was added with the metrics envelope)" }
    else {
        Write-Host ("  one pane, then after a session is filled up  (MB, and CPU as % of one core)") -ForegroundColor DarkCyan
        Write-Host ("  {0,-17} {1,-11} {2,6} {3,9} {4,9} {5,9} {6,9} {7,9} {8,9}" -f `
            "when", "sha", "panes", "srvWS/1", "srvPriv", "cliWS/1", "srvWS/n", "idle%/1", "idle%/n") -ForegroundColor DarkCyan
        foreach ($row in $rows) {
            Write-Host ("  {0,-17} {1,-11} {2,6} {3,9} {4,9} {5,9} {6,9} {7,9} {8,9}" -f `
                $row.when, $row.sha, $row.panes, (Num $row.srv_ws_1 1), (Num $row.srv_priv_1 1),
                (Num $row.cli_ws_1 1), (Num $row.srv_ws_n 1), (Num $row.idle_pct_1 2), (Num $row.idle_pct_n 2))
            Emit "D_creation_resources" $row
        }
        Note "srvWS/n is the server's working set once the session is full; idle% is server plus client with nothing typed"
    }

    $k = Get-Runs "keystroke-latency-*.json" @("keystroke-latency-pwsh-*")
    $krows = @()
    foreach ($r in $k) {
        $res = Prop $r.Json "resources"
        if (-not $res) { continue }
        $ap = Prop $res "at_prompt"; $af = Prop $res "after_key_burst"
        $cpu = Prop $res "cpu_ms_per_100_keys"; $idle = Prop $res "idle_cpu_pct_of_core"
        $krows += [pscustomobject]@{
            when = $r.When.ToString("MM-dd HH:mm"); sha = (Sha $r.Json)
            srv_ws = (Prop (Prop $ap "server") "ws_mb"); cli_ws = (Prop (Prop $ap "client") "ws_mb")
            srv_ws_after = (Prop (Prop $af "server") "ws_mb"); cli_ws_after = (Prop (Prop $af "client") "ws_mb")
            cpu_srv_100k = (Prop $cpu "server"); cpu_cli_100k = (Prop $cpu "client")
            idle_srv = (Prop $idle "server"); idle_cli = (Prop $idle "client")
        }
    }
    if ($krows.Count -eq 0) { Note "no keystroke gate run has a resources block yet" }
    else {
        Write-Host ""
        Write-Host ("  around a typing burst  (MB, ms of CPU per 100 keystrokes, idle % of one core)") -ForegroundColor DarkCyan
        Write-Host ("  {0,-17} {1,-11} {2,8} {3,8} {4,9} {5,9} {6,10} {7,10} {8,8} {9,8}" -f `
            "when", "sha", "srvWS", "cliWS", "srvWSaft", "cliWSaft", "srvCPU100", "cliCPU100", "idleSrv", "idleCli") -ForegroundColor DarkCyan
        foreach ($row in $krows) {
            Write-Host ("  {0,-17} {1,-11} {2,8} {3,8} {4,9} {5,9} {6,10} {7,10} {8,8} {9,8}" -f `
                $row.when, $row.sha, (Num $row.srv_ws 1), (Num $row.cli_ws 1), (Num $row.srv_ws_after 1),
                (Num $row.cli_ws_after 1), (Num $row.cpu_srv_100k 0), (Num $row.cpu_cli_100k 0),
                (Num $row.idle_srv 2), (Num $row.idle_cli 2))
            Emit "D_keystroke_resources" $row
        }
    }

    # The launch gate samples one iteration at its prompt, so its row is the
    # cheapest "what does a session cost the moment it is up" number there is.
    $lt = Get-Runs "launch-to-prompt-*.json"
    $lrows = @()
    foreach ($r in $lt) {
        $res = Prop $r.Json "resources"
        if (-not $res) { continue }
        $ap = Prop $res "at_prompt"; $idle = Prop $res "idle_cpu_pct_of_core"
        $lrows += [pscustomobject]@{
            when = $r.When.ToString("MM-dd HH:mm"); sha = (Sha $r.Json)
            srv_ws = (Prop (Prop $ap "server") "ws_mb"); srv_priv = (Prop (Prop $ap "server") "private_mb")
            cli_ws = (Prop (Prop $ap "client") "ws_mb"); cli_priv = (Prop (Prop $ap "client") "private_mb")
            idle_srv = (Prop $idle "server"); idle_cli = (Prop $idle "client")
        }
    }
    if ($lrows.Count -eq 0) { Note "no launch gate run has a resources block yet" }
    else {
        Write-Host ""
        Write-Host ("  at the first prompt of a freshly launched session  (MB, idle % of one core)") -ForegroundColor DarkCyan
        Write-Host ("  {0,-17} {1,-11} {2,9} {3,9} {4,9} {5,9} {6,9} {7,9}" -f `
            "when", "sha", "srvWS", "srvPriv", "cliWS", "cliPriv", "idleSrv", "idleCli") -ForegroundColor DarkCyan
        foreach ($row in $lrows) {
            Write-Host ("  {0,-17} {1,-11} {2,9} {3,9} {4,9} {5,9} {6,9} {7,9}" -f `
                $row.when, $row.sha, (Num $row.srv_ws 1), (Num $row.srv_priv 1), (Num $row.cli_ws 1),
                (Num $row.cli_priv 1), (Num $row.idle_srv 2), (Num $row.idle_cli 2))
            Emit "D_launch_resources" $row
        }
    }

    # An idle attached pair: the line rate it was gated on, and what it cost.
    $idl = Get-Runs "idle-socket-traffic-*.json"
    $irows = @()
    foreach ($r in $idl) {
        $cells = Prop $r.Json "cells"
        if (-not $cells) { continue }
        foreach ($cn in @($cells.PSObject.Properties.Name)) {
            $c = $cells.$cn
            $ie = Prop $c "idle_end"; $ic = Prop $c "idle_cpu_pct_of_core"
            $irows += [pscustomobject]@{
                when = $r.When.ToString("MM-dd HH:mm"); sha = (Sha $r.Json); cell = $cn
                lines_per_sec = (Prop $c "lines_per_sec")
                srv_ws = (Prop (Prop $ie "server") "ws_mb"); cli_ws = (Prop (Prop $ie "client") "ws_mb")
                idle_srv = (Prop $ic "server"); idle_cli = (Prop $ic "client")
            }
        }
    }
    if ($irows.Count -eq 0) { Note "no test_idle_socket_traffic run has a metrics file yet" }
    else {
        Write-Host ""
        Write-Host ("  an idle attached pair, over the window the line count was taken on") -ForegroundColor DarkCyan
        Write-Host ("  {0,-17} {1,-11} {2,-8} {3,10} {4,9} {5,9} {6,9} {7,9}" -f `
            "when", "sha", "cell", "lines/sec", "srvWS", "cliWS", "idleSrv", "idleCli") -ForegroundColor DarkCyan
        foreach ($row in $irows) {
            Write-Host ("  {0,-17} {1,-11} {2,-8} {3,10} {4,9} {5,9} {6,9} {7,9}" -f `
                $row.when, $row.sha, $row.cell, (Num $row.lines_per_sec 1), (Num $row.srv_ws 1),
                (Num $row.cli_ws 1), (Num $row.idle_srv 2), (Num $row.idle_cli 2))
            Emit "D_idle_socket" $row
        }
        Note "lines/sec is the gated number; the CPU beside it says whether a quiet socket was bought by spinning elsewhere"
    }

    $vt = Get-Runs "perf_vs_terminals-*.json"
    if ($vt.Count -eq 0) { return }
    Write-Host ""
    Write-Host ("  test_perf_vs_terminals, psmux attached cell") -ForegroundColor DarkCyan
    # srv+cli is the narrow number every run has; treeWS is the whole session
    # (pane shells, warm pool spares, console hosts, standby server, client),
    # recorded from 2026-10-09 on. Files from before the split carried the
    # narrow number under psmux_ws_mb, hence the fallback.
    Write-Host ("  {0,-17} {1,-11} {2,9} {3,9} {4,9} {5,9} {6,6} {7,11} {8,11}" -f "when", "sha", "srvWS", "cliWS", "srv+cli", "treeWS", "procs", "cpu/100keys", "idle % core") -ForegroundColor DarkCyan
    foreach ($r in $vt) {
        $t = Prop $r.Json "summary_table" @()
        $cell = ($t | Where-Object { $_.host -eq "psmux_attached" } | Select-Object -First 1)
        if (-not $cell) { continue }
        $row = [pscustomobject]@{
            when = $r.When.ToString("MM-dd HH:mm"); sha = (Sha $r.Json)
            srv_ws = (Prop $cell "server_ws_mb"); cli_ws = (Prop $cell "client_ws_mb")
            narrow_ws = (Prop $cell "psmux_ws_mb_server_client" (Prop $cell "psmux_ws_mb"))
            tree_ws = $(if ($null -ne (Prop $cell "psmux_ws_mb_server_client")) { Prop $cell "psmux_ws_mb" } else { $null })
            tree_procs = (Prop $cell "psmux_tree_processes")
            cpu_100k = (Prop $cell "cpu_per_100_keys_psmux"); idle_pct = (Prop $cell "idle_cpu_pct_psmux")
        }
        Write-Host ("  {0,-17} {1,-11} {2,9} {3,9} {4,9} {5,9} {6,6} {7,11} {8,11}" -f `
            $row.when, $row.sha, (Num $row.srv_ws 1), (Num $row.cli_ws 1), (Num $row.narrow_ws 1), (Num $row.tree_ws 0), (Num $row.tree_procs 0), (Num $row.cpu_100k 0), (Num $row.idle_pct 2))
        Emit "D_vs_terminals" $row
    }
}

# ── T: the trend, and what is getting worse ───────────────────────────────
#
# One table instead of five, because the question "did anything get slower this
# week" should not require reading five tables and doing the arithmetic in your
# head. Every row is a metric where LOWER IS BETTER, which is every metric this
# project records: milliseconds, megabytes, percent of a core.
#
# The window is at least six runs whatever -Last says, because the rule needs a
# newest run plus five to take a median of. With fewer than six the row still
# prints its min / median / max and says so in the flag column.
$script:TrendRegressions = 0

function Get-TrendSeries {
    param([string]$Pattern, [string[]]$Exclude = @(), [scriptblock]$Value)
    $files = @(Get-ChildItem -LiteralPath $MetricsDir -Filter $Pattern -File -ErrorAction SilentlyContinue |
        Where-Object { $n = $_.Name; -not ($Exclude | Where-Object { $n -like $_ }) } |
        Sort-Object LastWriteTime -Descending | Select-Object -First ([Math]::Max($Last, 6)))
    $out = @()
    foreach ($f in $files) {
        try {
            $j = Get-Content -LiteralPath $f.FullName -Raw | ConvertFrom-Json
            $v = & $Value $j
            if ($null -eq $v) { continue }
            $d = 0.0
            if (-not [double]::TryParse([string]$v, [ref]$d)) { continue }
            $out += [pscustomobject]@{ When = $f.LastWriteTime; Sha = (Sha $j); Value = $d }
        } catch { }
    }
    # Newest first is how they were read; newest LAST is how a trend is read.
    return @($out | Sort-Object When)
}

function Median-Of {
    param([double[]]$V)
    if ($V.Count -eq 0) { return $null }
    $s = @($V | Sort-Object)
    if ($s.Count % 2 -eq 1) { return [double]$s[[int](($s.Count - 1) / 2)] }
    return (([double]$s[$s.Count / 2 - 1] + [double]$s[$s.Count / 2]) / 2.0)
}

function Show-TrendRow {
    param([string]$Name, [string]$Unit, [int]$Round, $Series)
    if ($null -eq $Series -or @($Series).Count -eq 0) {
        Write-Host ("  {0,-34} {1,-6} {2,5} {3,9} {4,9} {5,9} {6,9} {7,10}  {8}" -f `
            $Name, $Unit, 0, "-", "-", "-", "-", "-", "no runs recorded") -ForegroundColor DarkGray
        return
    }
    $s = @($Series)
    $vals = [double[]]@($s | ForEach-Object { $_.Value })
    $newest = $s[-1]
    $min = ($vals | Measure-Object -Minimum).Minimum
    $max = ($vals | Measure-Object -Maximum).Maximum
    $med = Median-Of $vals

    $flag = "few runs"; $colour = "DarkGray"; $baseline = $null; $pct = $null
    if ($s.Count -ge 6) {
        $prev5 = [double[]]@($s[($s.Count - 6)..($s.Count - 2)] | ForEach-Object { $_.Value })
        $baseline = Median-Of $prev5
        if ($null -ne $baseline -and $baseline -gt 0) {
            $pct = (($newest.Value - $baseline) / $baseline) * 100.0
            if ($pct -gt $RegressPct) {
                $flag = ("REGRESSED +{0:N0}%" -f $pct); $colour = "Red"; $script:TrendRegressions++
            } elseif ($pct -lt (-1 * $RegressPct)) {
                $flag = ("improved {0:N0}%" -f $pct); $colour = "Green"
            } else {
                $flag = ("ok {0:+0;-0;0}%" -f $pct); $colour = "Gray"
            }
        }
    }
    Write-Host ("  {0,-34} {1,-6} {2,5} {3,9} {4,9} {5,9} {6,9} {7,10}  {8}" -f `
        $Name, $Unit, $s.Count, (Num $min $Round), (Num $med $Round), (Num $max $Round),
        (Num $newest.Value $Round), (Num $baseline $Round), $flag) -ForegroundColor $colour
    Emit "T_trend" ([pscustomobject]@{
        metric = $Name; unit = $Unit; runs = $s.Count
        min = $min; median = $med; max = $max
        newest = $newest.Value; newest_when = $newest.When.ToString("MM-dd HH:mm"); newest_sha = $newest.Sha
        baseline_prev5_median = $baseline; delta_pct = $pct; flag = $flag
    })
}

function Show-T {
    Head "T  TREND, LAST $([Math]::Max($Last,6)) RUNS PER METRIC  (lower is better everywhere)"
    Write-Host ("  {0,-34} {1,-6} {2,5} {3,9} {4,9} {5,9} {6,9} {7,10}  {8}" -f `
        "metric", "unit", "runs", "min", "median", "max", "newest", "prev5med", "flag") -ForegroundColor DarkCyan

    $kExcl = @("keystroke-latency-pwsh-*")

    # A: launch
    Show-TrendRow "launch psmux p50" "ms" 0 (Get-TrendSeries "launch-to-prompt-*.json" @() { param($j) Prop $j "psmux_median" })
    Show-TrendRow "launch psmux minus bare pwsh" "ms" 0 (Get-TrendSeries "launch-to-prompt-*.json" @() { param($j) Prop $j "delta_ms" })
    # The load invariant one, and the gate's own hard assertion. A file written
    # before the ratio landed still contributes a row, computed from the two
    # medians it does carry.
    Show-TrendRow "launch psmux / bare pwsh" "x" 3 (Get-TrendSeries "launch-to-prompt-*.json" @() {
        param($j)
        $r = Prop $j "ratio"
        if ($null -ne $r) { return $r }
        $b = Prop $j "bare_median"; $m = Prop $j "psmux_median"
        if ($null -eq $b -or $null -eq $m -or [double]$b -le 0) { return $null }
        return ([double]$m / [double]$b)
    })
    $vtCell = {
        param($j, $host_, $field)
        $t = Prop $j "summary_table" @()
        $c = ($t | Where-Object { $_.host -eq $host_ } | Select-Object -First 1)
        Prop $c $field
    }
    Show-TrendRow "launch psmux attached (vs terms)" "ms" 0 (Get-TrendSeries "perf_vs_terminals-*.json" @() { param($j) & $vtCell $j "psmux_attached" "launch_median" })
    Show-TrendRow "launch Windows Terminal" "ms" 0 (Get-TrendSeries "perf_vs_terminals-*.json" @() { param($j) & $vtCell $j "wt_pwsh" "launch_median" })
    Show-TrendRow "launch WezTerm" "ms" 0 (Get-TrendSeries "perf_vs_terminals-*.json" @() { param($j) & $vtCell $j "wezterm_pwsh" "launch_median" })
    Show-TrendRow "launch bare pwsh" "ms" 0 (Get-TrendSeries "perf_vs_terminals-*.json" @() { param($j) & $vtCell $j "bare_pwsh" "launch_median" })

    # B: keystroke
    Show-TrendRow "keystroke echo p50" "ms" 2 (Get-TrendSeries "keystroke-latency-*.json" $kExcl { param($j) Prop (Prop $j "pooled") "median" })
    Show-TrendRow "keystroke echo p99" "ms" 2 (Get-TrendSeries "keystroke-latency-*.json" $kExcl { param($j) Prop (Prop $j "pooled") "p99" })
    Show-TrendRow "keystroke pwsh over ConPTY floor" "ms" 2 (Get-TrendSeries "keystroke-latency-pwsh-*.json" @() { param($j) Prop (Prop $j "pwsh") "medianDelta" })

    # C: creation and teardown, every cell the gate records
    $cell = {
        param($j, $name, $stat)
        $s = Prop $j "stats_ms"
        $st = Prop $s $name
        if ($st) { return (Prop $st $stat) }
        return $null
    }
    foreach ($c in @(
        @("new-window", "creation new-window p50"),
        @("split-window -v", "creation split -v p50"),
        @("split-window -h", "creation split -h p50"),
        @("split -f", "creation split -f p50"),
        @("split -b", "creation split -b p50"),
        @("split -bh", "creation split -bh p50"),
        @("split -bv", "creation split -bv p50"),
        @("split with cmd", "creation split w/ command p50"),
        @("new-session (warm)", "new-session warm p50"),
        @("new-session (no warm)", "new-session no-warm p50"),
        @("kill-pane", "teardown kill-pane p50"),
        @("kill-window", "teardown kill-window p50"),
        @("kill-session", "teardown kill-session p50")
    )) {
        $key = $c[0]; $label = $c[1]
        Show-TrendRow $label "ms" 0 (Get-TrendSeries "creation_latency_gate-*.json" @() { param($j) & $cell $j $key "p50" }.GetNewClosure())
    }
    Show-TrendRow "creation new-window p90" "ms" 0 (Get-TrendSeries "creation_latency_gate-*.json" @() { param($j) & $cell $j "new-window" "p90" })

    # D: what it costs to hold a session open
    Show-TrendRow "server working set at prompt" "MB" 1 (Get-TrendSeries "launch-to-prompt-*.json" @() { param($j) Prop (Prop (Prop (Prop $j "resources") "at_prompt") "server") "ws_mb" })
    Show-TrendRow "client working set at prompt" "MB" 1 (Get-TrendSeries "launch-to-prompt-*.json" @() { param($j) Prop (Prop (Prop (Prop $j "resources") "at_prompt") "client") "ws_mb" })
    Show-TrendRow "server working set, session full" "MB" 1 (Get-TrendSeries "creation_latency_gate-*.json" @() { param($j) Prop (Prop (Prop (Prop $j "resources") "after_panes") "server") "ws_mb" })
    # The window right at the prompt is where the server spawns the warm pool's
    # spares, so that row moves with warm-pool-size (measured 2026-10-09: 4.2 %
    # at depth two, 7.0 % at three, the same 0.8 % at both once settled). The
    # settled row is the steady state; the fill row is the pool's one time cost.
    Show-TrendRow "idle cpu at prompt (pool fill)" "%core" 2 (Get-TrendSeries "launch-to-prompt-*.json" @() {
        param($j)
        $i = Prop (Prop $j "resources") "idle_cpu_pct_of_core"
        if (-not $i) { return $null }
        return ((Prop $i "server" 0) + (Prop $i "client" 0))
    })
    Show-TrendRow "idle cpu settled, srv+cli" "%core" 2 (Get-TrendSeries "launch-to-prompt-*.json" @() {
        param($j)
        $i = Prop (Prop $j "resources") "idle_cpu_pct_of_core_settled"
        if (-not $i) { return $null }
        return ((Prop $i "server" 0) + (Prop $i "client" 0))
    })
    Show-TrendRow "server cpu, prompt to settled" "ms" 0 (Get-TrendSeries "launch-to-prompt-*.json" @() { param($j) Prop (Prop (Prop $j "resources") "pool_fill_cpu_ms") "server" })
    Show-TrendRow "vs terms: psmux srv+cli ws" "MB" 1 (Get-TrendSeries "perf_vs_terminals-*.json" @() {
        param($j)
        $c = (Prop $j "summary_table" @()) | Where-Object { $_.host -eq "psmux_attached" } | Select-Object -First 1
        if (-not $c) { return $null }
        return (Prop $c "psmux_ws_mb_server_client" (Prop $c "psmux_ws_mb"))
    })
    Show-TrendRow "vs terms: psmux session tree ws" "MB" 0 (Get-TrendSeries "perf_vs_terminals-*.json" @() {
        param($j)
        $c = (Prop $j "summary_table" @()) | Where-Object { $_.host -eq "psmux_attached" } | Select-Object -First 1
        if (-not $c -or $null -eq (Prop $c "psmux_ws_mb_server_client")) { return $null }
        return (Prop $c "psmux_ws_mb")
    })
    Show-TrendRow "pane startup: session tree ws" "MB" 0 (Get-TrendSeries "pane_startup_perf-*.json" @() { param($j) Prop (Prop (Prop $j "resources") "session_tree") "ws_mb" })
    Show-TrendRow "session tree working set, settled" "MB" 0 (Get-TrendSeries "launch-to-prompt-*.json" @() { param($j) Prop (Prop (Prop $j "resources") "session_tree_settled") "ws_mb" })
    Show-TrendRow "cpu per 100 keystrokes, srv+cli" "ms" 0 (Get-TrendSeries "keystroke-latency-*.json" $kExcl {
        param($j)
        $c = Prop (Prop $j "resources") "cpu_ms_per_100_keys"
        if (-not $c) { return $null }
        return ((Prop $c "server" 0) + (Prop $c "client" 0))
    })
    Show-TrendRow "idle socket lines per second" "l/s" 2 (Get-TrendSeries "idle-socket-traffic-*.json" @() {
        param($j)
        $cells = Prop $j "cells"
        if (-not $cells) { return $null }
        $c = Prop $cells "silent"
        if (-not $c) { return $null }
        return (Prop $c "lines_per_sec")
    })

    Note "newest is the most recent run; prev5med is the median of the five before it"
    Note "a row is flagged REGRESSED when newest is more than $RegressPct percent worse than prev5med"
    Note "machine load is in each file's envelope under load; a loaded run explains a tail, never a p50"
    if ($script:TrendRegressions -gt 0) {
        Write-Host ("  {0} metric(s) flagged. Check the machine load in those files before believing any of them." -f $script:TrendRegressions) -ForegroundColor Red
    } else {
        Write-Host "  nothing flagged." -ForegroundColor Green
    }
}

Write-Host ""
Write-Host "psmux performance metrics, last $Last runs per metric" -ForegroundColor Cyan
Write-Host "from $MetricsDir" -ForegroundColor DarkGray

if ($Metric -in @("all", "A")) { Show-A }
if ($Metric -in @("all", "B")) { Show-B }
if ($Metric -in @("all", "C")) { Show-C }
if ($Metric -in @("all", "D")) { Show-D }
if ($Metric -in @("all", "T")) { Show-T }

if ($Csv) {
    try {
        $script:CsvRows | Export-Csv -LiteralPath $Csv -NoTypeInformation -Encoding UTF8
        Write-Host ""
        Write-Host "  $($script:CsvRows.Count) rows written to $Csv" -ForegroundColor Green
    } catch {
        Write-Host "  could not write $Csv : $_" -ForegroundColor Yellow
    }
}
Write-Host ""
if ($FailOnRegression -and $script:TrendRegressions -gt 0) { exit 1 }
exit 0
