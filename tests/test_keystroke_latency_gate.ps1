# Keystroke to screen latency gate.
#
# WHAT IT MEASURES
# ----------------
# The time from a key record landing in an attached psmux client's console input
# buffer to the echoed character appearing in that console's screen buffer. Both
# timestamps come from one QueryPerformanceCounter inside one process
# (tests/keylat.cs), so there is no cross process clock skew and no polling
# granularity in the number.
#
# The pane runs tests/echo_load_child.cs, not a shell. A shell means PSReadLine,
# which redraws the whole edited line per keystroke and adds about 15ms that
# psmux cannot touch; including it would bury the thing this gate exists to
# protect. The echo child writes every byte it reads at a fixed screen position,
# so the oracle watches one cell and the whole number is psmux's own path:
#
#   console input -> client -> socket -> server loop -> ConPTY write ->
#   echo read -> parser -> frame push -> socket -> client parse -> render
#
# SECOND CELL: THE SAME PATH WITH A REAL SHELL IN THE PANE
# -------------------------------------------------------
# The echo child cannot catch a regression that only appears when the pane runs a
# shell, and users run shells. So a second cell puts `pwsh -NoLogo -NoProfile` in
# the pane and measures the identical oracle.
#
# That cell CANNOT be gated on an absolute number. With pwsh in the pane the
# dominant term is not psmux at all: conhost serialises PSReadLine's redraw to
# the pseudoconsole output pipe in two frames, the first carrying only the
# cursor-hide and the second, ~15ms later, carrying the character. Measured four
# independent ways on one machine, commit 203ee90:
#
#   tests/conpty_echolat.cs, a pseudoconsole host with NO psmux in it at all,
#   pwsh in the pty                                  15.78ms median
#       (char in the first read chunk: 0 of 40 trials; the 6 byte cursor-hide
#        chunk arrives at 0.61ms, the character 15.2ms later)
#   keylat against the PANE's own conhost screen buffer under psmux, ie
#   PSReadLine's own handling with nothing downstream of it
#                                                     0.55ms median
#   keylat against the attached psmux client, pwsh pane
#                                                    16.45ms median
#   psmux's own pty_trace, same run: pty write to the read that carries the
#   character 15.2ms, and every hop after it (parse, frame build, socket,
#   client parse, client pick up) 0.34ms in total
#
# So the shell cell is gated on psmux's OWN HOPS, the last row above, split out
# of every keystroke with the server's pty trace in the same run (see
# Get-PerfShellSplit in perf_metrics_common.ps1). That is the part psmux owns,
# and it is the part a regression would move. Until 2026-09-30 the gate took
# end to end minus the conpty_echolat floor instead, which was the same thing
# while both sat at ~15.7ms; that day the standalone floor fell to ~11.1ms
# while pwsh in a psmux pane stayed at ~15 for an old and a new build alike,
# and the subtraction started charging psmux ~4.5ms of timer state. The floor
# is still measured and still judged, against the pane shell's own echo time,
# so psmux making the SHELL wait (a reply it answers late) cannot hide inside
# the split.
#
# WHY A GATE AND NOT A BENCHMARK
# ------------------------------
# Every hop on that path has at some point been a poll rather than an event, and
# each time the symptom was the same: a median that looked fine and a tail that
# was a multiple of some timer interval. A threshold on the median alone would
# have passed all of them. So this asserts both:
#
#   median < 10ms   the path is event driven, not waiting on a tick
#   p99    < 25ms   no hop falls back to a timer under any sample
#
# WHAT THESE DEFAULTS DO AND DO NOT CATCH
# ---------------------------------------
# Measured on the development machine, 3 runs of 40 keys pooled, against the
# commit that made the server loop and the client wake on events and took the
# process-table walk off the event loop:
#
#   before   median 3.86ms   p90 5.48ms   p99 8.66ms
#   after    median 1.85ms   p90 2.68ms   p99 3.79ms
#
# Both of those are inside 10 and 25. So these defaults are a product floor, not
# a guard on that specific change: reverting it would still pass them. They are
# deliberately loose because this number is measured on whatever machine CI or a
# developer happens to be using, and a tight threshold on a loaded box reports a
# product failure that is not there.
#
# To pin the change instead of the floor, run it with thresholds between the two
# rows above, which was verified to fail "before" on both assertions and pass
# "after" on both:
#
#   pwsh -File tests\test_keystroke_latency_gate.ps1 -MedianMaxMs 3 -P99MaxMs 8
#
# WHICH OF THE TWO SURVIVES A BUSY MACHINE
# ----------------------------------------
# Measured on 2026-09-22 against the SAME installed binary (0bcc421) on this
# machine, against the five quiet runs already on disk:
#
#   quiet   (5 runs)          median 1.64 to 1.71   p99 2.23 to 2.77
#   loaded  (5 agents busy)   median 3.20           p99 4.45
#   the defect, quiet         median 3.86           p99 8.66
#
# Under load the healthy median lands on top of the defect's median and the
# assertion stops discriminating; the healthy p99 is still half the defect's.
# So the P99 ASSERTION IS HARD ALWAYS, and the median assertion is hard when
# the machine was quiet and a recorded warning when it was not. The machine
# load is sampled from \Processor(_Total)\% Processor Time and goes into the
# JSON envelope, so a warning can always be checked against what the machine
# was doing. Dropping the median assertion instead was rejected: on a quiet
# machine it is the cheaper of the two signals and it reads 1.7 against a 3.0
# budget, which is not a close thing.
#
# MEMORY AND CPU RIDE ALONG
# -------------------------
# The first echo run also samples the server and the attached client: working
# set and private bytes at the prompt and again after the key burst, the CPU
# those keystrokes cost normalised to ms per 100 keys, and the CPU burnt over a
# quiet window with nothing typed. The keystroke run is the only part of this
# suite that holds a real server and a real client alive with a prompt up, so it
# is the cheapest place to take the sample. The numbers are RECORDED, not gated:
# the thresholds on them live in test_perf_vs_terminals (T6, T7, T8), and the
# only assertion here is that the section produced data at all, because a JSON
# full of nulls that still says PASS is worse than a failure.
#
# SAMPLES ARE KEPT
# ----------------
# Every sample is written to %USERPROFILE%\.psmux-test-data\metrics as JSON, so a
# regression can be compared against the run that last passed instead of against
# a number in a comment. Never inside the repo. Each file carries the shared
# envelope from tests/perf_metrics_common.ps1: the git sha of the tree the
# binary was built in (or "installed"), the binary path, the machine name and
# the CPU model, so two files can be compared without guessing what produced
# them. tests/perf_summary.ps1 prints the trend across them.

param(
    # Binary under test. Defaults to this checkout's target\release build, which
    # is the binary run_all_tests.ps1 announces, then PSMUX_TEST_BIN, then the
    # psmux on PATH; pass -Binary to A/B two builds against each other.
    [string]$Binary = "",
    [int]$Runs = 3,
    [int]$N = 40,
    [double]$MedianMaxMs = 3.0,
    [double]$P99MaxMs = 8.0,
    # Shell cell: how far above the measured ConPTY floor psmux may sit. The
    # measured overhead is 0.5 to 1.0ms across runs (16.46 against a 15.72 floor,
    # 16.45 against 15.78, 16.29 against 15.84), and the floor's own run to run
    # spread is about 0.7ms.
    #
    # This cell is deliberately LOOSER than the echo cell, because the shell's own
    # 15.6ms wait partly absorbs anything psmux adds: measured with
    # PSMUX_NO_FRAME_WAKE=1, which makes the client notice a frame on its poll
    # instead of on the event, the echo cell moves 1.77 -> 3.09ms (+1.32) but this
    # cell's overhead moves only 0.74 -> 1.32ms (+0.58), about 45 percent of it. So
    # do NOT treat this cell as a tight guard on psmux's hops - that is the echo
    # cell's job. This one catches a regression that only appears with a shell in
    # the pane, which is the kind that costs whole 15.6ms ticks: a coalescing tick
    # per chunk, a cursor position report answered on a poll, a second 15ms wait
    # per keystroke.
    #
    # Since 2026-09-30 these two budgets are applied to psmux's OWN hops per
    # shell keystroke, split out of the end to end number with the server's
    # pty trace in the same run (Get-PerfShellSplit in perf_metrics_common),
    # not to end to end minus a floor taken in another pseudoconsole host. The
    # standalone floor dropped from ~15.7 to ~11.1ms on this machine while pwsh
    # in a psmux pane stayed at ~15, for both an old and a new build, so the old
    # subtraction charged psmux ~4.5ms it never spent. The hops read 1.5 to
    # 1.8ms median, the same as the 0.7ms over floor era plus the echo cell's
    # own path, so the budgets keep their meaning.
    [double]$PwshMedianDeltaMaxMs = 2.5,
    [double]$PwshP99DeltaMaxMs = 6.0,
    # The pane shell's own echo time (pty write to the read carrying the char)
    # against the standalone floor. Wider than the ~4.5ms the two timer states
    # sit apart, narrower than the one extra 15.6ms tick a late reply to a
    # shell query costs.
    [double]$PwshShellOverFloorMaxMs = 7.0,
    # Sanity ceiling in case the floor measurement itself fails or the machine is
    # pathological: a shell keystroke must land inside this no matter what.
    [double]$PwshAbsMedianMaxMs = 30.0,
    # Quiet window for the idle CPU sample, in seconds. Nothing is typed during
    # it, so whatever the server and the client burn is work nobody asked for.
    [int]$IdleSeconds = 3,
    # At or under this percentage of TOTAL cpu the machine counts as quiet and
    # the echo median is a hard failure; above it, a warning. The p99 assertion
    # is hard either way. See the header for the measurements behind that split.
    [double]$QuietLoadPct = 25.0,
    [switch]$SkipPwsh,
    [switch]$SkipResources
)

$ErrorActionPreference = "Continue"
. "$PSScriptRoot\perf_metrics_common.ps1"
$script:TestsPassed = 0
$script:TestsFailed = 0

$script:Warnings = New-Object System.Collections.Generic.List[string]
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Warn($msg) { Write-Host "  [WARN] $msg" -ForegroundColor Yellow; $script:Warnings.Add($msg) | Out-Null }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }

# Binary selection, in the order the suite runner needs it. The BUILD IN THE
# TREE comes before the installed copy on PATH: run_all_tests.ps1 reports
# target\release\psmux.exe as the binary under test and a gate that quietly
# measured the installed psmux instead would report a green sweep for a build
# nobody timed. -Binary or PSMUX_TEST_BINARY still override, which is how two
# builds are compared against each other.
if (-not $Binary -and $env:PSMUX_EXE) { $Binary = $env:PSMUX_EXE }
if (-not $Binary -and $env:PSMUX_TEST_BIN) { $Binary = $env:PSMUX_TEST_BIN }
if (-not $Binary -and $env:PSMUX_TEST_BINARY) { $Binary = $env:PSMUX_TEST_BINARY }
if (-not $Binary) {
    $local = Join-Path $PSScriptRoot "..\target\release\psmux.exe"
    if (Test-Path $local) { $Binary = (Resolve-Path $local).Path }
}
if (-not $Binary) {
    $cmd = Get-Command psmux -EA SilentlyContinue
    if (-not $cmd) { Write-Fail "psmux not found on PATH and no -Binary given"; exit 1 }
    $Binary = $cmd.Source
}
if (-not (Test-Path $Binary)) { Write-Fail "binary not found: $Binary"; exit 1 }
$Binary = (Resolve-Path $Binary).Path
Write-Info "binary under test: $Binary"

# This shell's own psmux routing must not reach the client we launch, or the
# client attaches to the wrong server and measures nothing.
foreach ($v in @('PSMUX_SESSION','PSMUX_SESSION_NAME','PSMUX_SOCKET','PSMUX_PANE_ID','PSMUX_PTY_TRACE')) {
    Remove-Item "Env:\$v" -EA SilentlyContinue
}

$root = Split-Path -Parent $PSScriptRoot
$build = Join-Path $root "target\release"
New-Item -ItemType Directory -Force -Path $build | Out-Null
$KeyLat = Join-Path $build "keylat.exe"
$EchoChild = Join-Path $build "echo_load_child.exe"
$EchoLat = Join-Path $build "conpty_echolat.exe"
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { Write-Fail "csc.exe not found at $csc"; exit 1 }
foreach ($pair in @(@("keylat", $KeyLat), @("echo_load_child", $EchoChild), @("conpty_echolat", $EchoLat))) {
    $src = Join-Path $PSScriptRoot "$($pair[0]).cs"
    if (-not (Test-Path $src)) { Write-Fail "missing harness source $src"; exit 1 }
    if ((-not (Test-Path $pair[1])) -or ((Get-Item $src).LastWriteTime -gt (Get-Item $pair[1]).LastWriteTime)) {
        & $csc /nologo /optimize "/out:$($pair[1])" $src | Out-Null
    }
    if (-not (Test-Path $pair[1])) { Write-Fail "failed to build $($pair[0]).exe"; exit 1 }
}

$OutDir = Join-Path $env:TEMP "psmux_keylat_gate"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$MetricsDir = Get-PerfMetricsDir

function Pct($arr, $p) {
    if ($arr.Count -eq 0) { return -1 }
    $s = [double[]]($arr | Sort-Object)
    return $s[[Math]::Floor(($p / 100.0) * ($s.Count - 1))]
}

function Get-OwnPids {
    @(Get-CimInstance Win32_Process -Filter "Name='$([IO.Path]::GetFileName($Binary))'" -EA SilentlyContinue |
        Where-Object { $_.ExecutablePath -eq $Binary } | Select-Object -ExpandProperty ProcessId)
}

# One measurement run: isolated -L namespace, attached client in its own
# console, the pane program of this cell, then n single keystrokes.
#
# `$cell` is "echo" (tests/echo_load_child.cs, one fixed cell oracle, no erase)
# or "pwsh" (a real shell, cursor oracle, erase on). Everything else about the
# measurement is identical between the two, which is the point: the difference
# between the two numbers is what the shell adds, not what the harness adds.
function Invoke-Run([int]$idx, [string]$cell = "echo", [bool]$withResources = $false) {
    $ns = "klgate$cell$idx$PID"
    $before = Get-OwnPids
    $client = $null
    $resAtPrompt = $null; $resAfterKeys = $null; $resIdle = $null; $keyCpu = $null
    $paneCmd = if ($cell -eq "pwsh") { @("pwsh", "-NoLogo", "-NoProfile") } else { @($EchoChild) }
    # The shell cell traces the keystroke path so every key can be split into
    # the pane shell's own echo time and psmux's hops (Get-PerfShellSplit). The
    # variable is set only around this launch: the client inherits it and the
    # server it spawns inherits it from the client. The echo cell stays
    # untraced, so its number is exactly the shipped path.
    $traceBase = Join-Path $OutDir "ptrace_${cell}_$idx"
    Get-ChildItem -Path $OutDir -Filter "ptrace_${cell}_$idx.*" -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue
    try {
        if ($cell -eq "pwsh") { $env:PSMUX_PTY_TRACE = $traceBase }
        $client = Start-Process -FilePath $Binary `
            -ArgumentList (@("-L", $ns, "new-session", "-s", "g") + $paneCmd) -PassThru
    } catch {
        Write-Info "run $idx could not launch the client: $_"
        return $null
    } finally {
        Remove-Item Env:\PSMUX_PTY_TRACE -ErrorAction SilentlyContinue
    }

    $deadline = (Get-Date).AddSeconds(25)
    $up = $false
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
        if ((& $Binary -L $ns ls 2>&1 | Out-String) -match 'g:') { $up = $true; break }
    }
    $samples = @()
    if ($up) {
        # let the pane program finish its first paint; a shell needs longer than
        # the echo child because PSReadLine's prompt and history load first
        if ($cell -eq "pwsh") { Start-Sleep -Seconds 4 } else { Start-Sleep -Seconds 2 }
        $out = Join-Path $OutDir "gate_${cell}_$idx.txt"
        Remove-Item $out -EA SilentlyContinue
        # MEMORY AND CPU, sampled around the same keystroke run, because this is
        # the only place in the suite that holds a real server and a real
        # attached client alive with a prompt up. The server is found by its
        # <ns>__g.pid anchor, never by image name: the machine routinely has a
        # dozen psmux servers belonging to other sessions and a warm standby
        # shares this binary and this namespace.
        $roles = $null
        if ($withResources) {
            $srv = Get-PerfServerPid -Ns $ns -Session "g"
            $roles = [ordered]@{}
            if ($srv -gt 0) { $roles["server"] = $srv }
            if ($client -and -not $client.HasExited) { $roles["client"] = $client.Id }
            $resAtPrompt = Get-PerfResourceSnapshot $roles
        }
        if ($cell -eq "pwsh") {
            & $KeyLat --pid $client.Id --label "gate$cell$idx" --out $out `
                --mode single --n $N --warmup 5 --gap 120 --oracle "cursor" | Out-Null
        } else {
            & $KeyLat --pid $client.Id --label "gate$idx" --out $out `
                --mode single --n $N --warmup 5 --gap 120 --oracle "cell:0,0" --noerase | Out-Null
        }
        if ($withResources -and $resAtPrompt) {
            $resAfterKeys = Get-PerfResourceSnapshot $roles
            # ms of CPU per 100 keystrokes, so cells with different key counts
            # stay comparable.
            $keyCpu = Get-PerfCpuDelta $resAtPrompt $resAfterKeys ([double]$N) 100.0
            # Then sit still. Nothing is typed in this window, so anything the
            # server or the client burns in it is a poll loop, and no latency
            # number in this file can see it.
            $resIdle = Measure-PerfIdleCpu $roles $IdleSeconds
            # Kept at script scope rather than returned, so a run that produces
            # no keystroke samples still contributes its resource numbers.
            $script:ResourceBlock = [ordered]@{
                cell                  = $cell
                run                   = $idx
                keys                  = $N
                idle_window_seconds   = $IdleSeconds
                at_prompt             = (Get-PerfMemorySummary $resAtPrompt)
                after_key_burst       = (Get-PerfMemorySummary $resAfterKeys)
                cpu_ms_per_100_keys   = $keyCpu
                idle_cpu_pct_of_core  = $resIdle.pct_of_one_core
                idle_measured_over_ms = $resIdle.window_ms
            }
            Write-Info (Format-PerfResourceLine $resAtPrompt "at prompt  ")
            Write-Info (Format-PerfResourceLine $resAfterKeys "after keys ")
            Write-Info ("cpu ms per 100 keys   : " + (@($keyCpu.Keys | ForEach-Object { "{0} {1:F1}" -f $_, $keyCpu[$_] }) -join "  "))
            Write-Info ("idle cpu, % of a core : " + (@($resIdle.pct_of_one_core.Keys | ForEach-Object { "{0} {1:F2}%" -f $_, $resIdle.pct_of_one_core[$_] }) -join "  "))
        }
        if (Test-Path $out) {
            # keylat writes every per keystroke measurement on one RAW line, as
            # comma separated milliseconds. Percentiles are computed here from
            # the pooled samples rather than read off the per run SUMMARY, so
            # the p99 is a p99 of 120 keystrokes and not a median of three p99s.
            $m = [regex]::Match((Get-Content $out -Raw), '(?m)^RAW \S+ (.+)$')
            if ($m.Success) {
                foreach ($v in ($m.Groups[1].Value -split ',')) {
                    $v = $v.Trim()
                    if ($v) { $samples += [double]::Parse($v, [Globalization.CultureInfo]::InvariantCulture) }
                }
            }
            $miss = [regex]::Match((Get-Content $out -Raw), 'MISSING \S+ (\d+) of (\d+)')
            if ($miss.Success -and [int]$miss.Groups[1].Value -gt 0) {
                Write-Info ("run {0}: {1} of {2} keystrokes never appeared" -f $idx, $miss.Groups[1].Value, $miss.Groups[2].Value)
            }
            if ($samples.Count -eq 0) {
                Write-Info ("run {0} produced no RAW samples: {1}" -f $idx, ((Get-Content $out -Raw).Trim() -replace "`r?`n", ' | '))
            }
        }
    } else {
        Write-Info "run $idx session never came up"
    }

    & $Binary -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    try { if (-not $client.HasExited) { Stop-Process -Id $client.Id -Force -EA SilentlyContinue } } catch {}
    foreach ($p in (Get-OwnPids)) {
        if ($before -notcontains $p) { try { Stop-Process -Id $p -Force -EA SilentlyContinue } catch {} }
    }
    Start-Sleep -Milliseconds 400

    if ($samples.Count -eq 0) { return $null }
    $split = $null
    if ($cell -eq "pwsh") {
        $split = Get-PerfShellSplit -KeylatOut (Join-Path $OutDir "gate_${cell}_$idx.txt") -TraceBase $traceBase
    }
    return [pscustomobject]@{
        Split = $split
        Run = $idx
        N = $samples.Count
        Min = (Pct $samples 0)
        Median = (Pct $samples 50)
        P90 = (Pct $samples 90)
        P99 = (Pct $samples 99)
        Max = (Pct $samples 100)
        Samples = $samples
    }
}

Write-Host "=== Keystroke to screen latency gate ===" -ForegroundColor Cyan
Write-Host "--- cell 1: raw echo child in the pane (psmux's own path) ---" -ForegroundColor DarkCyan
$runStats = @()
$all = @()
$script:ResourceBlock = $null
Add-PerfLoadSample "start" | Out-Null
Write-Info ("machine load at the start: {0}% of total cpu" -f (Get-PerfLoadSummary).p50_pct)
for ($i = 1; $i -le $Runs; $i++) {
    # Run 1 also carries the memory and CPU sample: one is enough for a trend
    # line and each one costs an extra $IdleSeconds of sitting still.
    $r = Invoke-Run $i "echo" ($i -eq 1 -and -not $SkipResources)
    if ($null -eq $r) { Write-Info "run $i produced no samples"; continue }
    $runStats += $r
    $all += $r.Samples
    Write-Info ("run {0}  n={1}  min={2:N2}  median={3:N2}  p90={4:N2}  p99={5:N2}  max={6:N2} ms" -f `
        $r.Run, $r.N, $r.Min, $r.Median, $r.P90, $r.P99, $r.Max)
}

if ($runStats.Count -eq 0) {
    Write-Fail "no run produced a measurement, so nothing was verified"
    Write-Host "`n=== Results ===" -ForegroundColor Cyan
    Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
    Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor Red
    exit 1
}

# Percentiles over every keystroke from every run pooled together. Pooling
# rather than averaging per run statistics is the point: a tail that shows up in
# one run out of three is still a tail the user feels, and averaging three p99s
# hides it.
$median = Pct $all 50
$p90 = Pct $all 90
$p99 = Pct $all 99
$min = Pct $all 0
$max = Pct $all 100
$n = $all.Count

Write-Info ("pooled n={0}  min={1:N2}  median={2:N2}  p90={3:N2}  p99={4:N2}  max={5:N2} ms" -f `
    $n, $min, $median, $p90, $p99, $max)

Add-PerfLoadSample "after echo cell" | Out-Null
$loadSummary = Get-PerfLoadSummary
$wasQuiet = Test-PerfMachineQuiet $QuietLoadPct
$topTxt = Get-PerfLoadTopText; if ($topTxt) { Write-Host "[INFO] $topTxt" -ForegroundColor Cyan }
Write-Info ("machine load: n={0} min={1}% p50={2}% max={3}%  -> {4}" -f `
    $loadSummary.n, $loadSummary.min_pct, $loadSummary.p50_pct, $loadSummary.max_pct, `
    $(if ($wasQuiet) { "quiet, the median assertion is a hard failure" } else { "loaded, the median assertion is a warning" }))

$stamp = (Get-Date).ToString("yyyyMMdd-HHmmss")
$jsonPath = Write-PerfMetrics -Suite "test_keystroke_latency_gate" -Binary $Binary `
    -FileStem "keystroke-latency" -MetricsDir $MetricsDir -Stamp $stamp -Data ([ordered]@{
    runs         = $Runs
    keysPerRun   = $N
    medianMaxMs  = $MedianMaxMs
    p99MaxMs     = $P99MaxMs
    quiet_load_pct    = $QuietLoadPct
    machine_was_quiet = $wasQuiet
    pooled       = [pscustomobject]@{ n = $n; min = $min; median = $median; p90 = $p90; p99 = $p99; max = $max }
    perRun       = @($runStats | ForEach-Object {
        [pscustomobject]@{ run = $_.Run; n = $_.N; min = $_.Min; median = $_.Median; p90 = $_.P90; p99 = $_.P99; max = $_.Max }
    })
    resources    = $script:ResourceBlock
    samplesMs    = $all
})
if ($jsonPath) { Write-Info "samples written to $jsonPath" }

if ($median -lt $MedianMaxMs) {
    Write-Pass ("keystroke to screen median {0:N2}ms is under the {1:N1}ms gate" -f $median, $MedianMaxMs)
} elseif ($wasQuiet) {
    Write-Fail ("keystroke to screen median {0:N2}ms exceeds the {1:N1}ms gate - a hop on the input path is waiting on a poll interval rather than an event" -f $median, $MedianMaxMs)
} else {
    # See the header: under load a healthy median lands on top of the defect's
    # median and stops discriminating, while the p99 below still does. The p99
    # assertion is the one that stays hard.
    Write-Warn ("keystroke to screen median {0:N2}ms exceeds the {1:N1}ms gate, but the machine was at {2}% of total cpu, over the {3}% quiet mark; the p99 assertion below is the one that still discriminates under load" -f $median, $MedianMaxMs, $loadSummary.p50_pct, $QuietLoadPct)
}

if ($p99 -lt $P99MaxMs) {
    Write-Pass ("keystroke to screen p99 {0:N2}ms is under the {1:N1}ms gate" -f $p99, $P99MaxMs)
} else {
    Write-Fail ("keystroke to screen p99 {0:N2}ms exceeds the {1:N1}ms gate - some samples are waiting out a timer; a median inside the gate does not clear this" -f $p99, $P99MaxMs)
}

# A resource section that silently produced nothing is worse than one that is
# missing, because the JSON still looks complete. This asserts the data is
# there; it deliberately does NOT put a threshold on the numbers, which is
# test_perf_vs_terminals' job (T6, T7, T8).
if (-not $SkipResources) {
    if ($script:ResourceBlock -and $script:ResourceBlock.at_prompt -and $script:ResourceBlock.at_prompt.Contains("server")) {
        Write-Pass ("memory and CPU collected: server ws {0} MB private {1} MB, client ws {2} MB, idle window {3} ms" -f `
            $script:ResourceBlock.at_prompt.server.ws_mb, $script:ResourceBlock.at_prompt.server.private_mb,
            $(if ($script:ResourceBlock.at_prompt.Contains("client")) { $script:ResourceBlock.at_prompt.client.ws_mb } else { "n/a" }),
            $script:ResourceBlock.idle_measured_over_ms)
    } else {
        Write-Fail "memory and CPU were not collected: the server process could not be found from its pid anchor, so this run has no resource data"
    }
}

# ── cell 2: a real shell in the pane, gated on psmux's overhead over the floor ──
$pwshStats = $null
if (-not $SkipPwsh) {
    Write-Host "--- cell 2: pwsh in the pane, against the measured ConPTY floor ---" -ForegroundColor DarkCyan
    # The floor: the same keystroke, the same shell, a pseudoconsole host with no
    # psmux in it. Whatever this reports, no ConPTY consumer can beat it, psmux
    # and Windows Terminal alike, because the character is not on the output pipe
    # any earlier than this.
    $floorOut = Join-Path $OutDir "floor.txt"
    Remove-Item $floorOut -EA SilentlyContinue
    $floorMedian = -1.0
    & $EchoLat --cmd "pwsh -NoLogo -NoProfile" --n $N --gap 120 --settle 4000 `
        --label floor --out $floorOut 2>&1 | Out-Null
    if (Test-Path $floorOut) {
        $fm = [regex]::Match((Get-Content $floorOut -Raw), 'SUMMARY floor .*median=([0-9.]+)')
        if ($fm.Success) { $floorMedian = [double]::Parse($fm.Groups[1].Value, [Globalization.CultureInfo]::InvariantCulture) }
        $split = [regex]::Match((Get-Content $floorOut -Raw), 'trials_with_char_in_first_chunk=(\d+) of (\d+)')
        if ($split.Success) {
            Write-Info ("ConPTY floor: char in the FIRST read chunk in {0} of {1} trials" -f $split.Groups[1].Value, $split.Groups[2].Value)
        }
    }
    if ($floorMedian -lt 0) {
        Write-Info "the ConPTY floor could not be measured; the shell cell falls back to the absolute ceiling only"
    } else {
        Write-Info ("ConPTY floor with pwsh, no psmux in the path: {0:N2}ms median" -f $floorMedian)
    }

    $pwshAll = @()
    $pwshRuns = @()
    for ($i = 1; $i -le $Runs; $i++) {
        $r = Invoke-Run $i "pwsh"
        if ($null -eq $r) { Write-Info "pwsh run $i produced no samples"; continue }
        $pwshRuns += $r
        $pwshAll += $r.Samples
        Write-Info ("pwsh run {0}  n={1}  min={2:N2}  median={3:N2}  p90={4:N2}  p99={5:N2}  max={6:N2} ms" -f `
            $r.Run, $r.N, $r.Min, $r.Median, $r.P90, $r.P99, $r.Max)
    }
    if ($pwshAll.Count -eq 0) {
        Write-Fail "the pwsh cell produced no measurement, so the shell path was not verified"
    } else {
        $pMedian = Pct $pwshAll 50
        $pP90 = Pct $pwshAll 90
        $pP99 = Pct $pwshAll 99
        Write-Info ("pwsh pooled n={0}  median={1:N2}  p90={2:N2}  p99={3:N2} ms" -f $pwshAll.Count, $pMedian, $pP90, $pP99)
        $pwshStats = [pscustomobject]@{
            n = $pwshAll.Count; median = $pMedian; p90 = $pP90; p99 = $pP99
            floorMedian = $floorMedian
            medianDelta = $(if ($floorMedian -ge 0) { $pMedian - $floorMedian } else { $null })
            p99Delta    = $(if ($floorMedian -ge 0) { $pP99 - $floorMedian } else { $null })
            samplesMs   = $pwshAll
        }
        if ($pMedian -lt $PwshAbsMedianMaxMs) {
            Write-Pass ("pwsh keystroke to screen median {0:N2}ms is under the {1:N1}ms ceiling" -f $pMedian, $PwshAbsMedianMaxMs)
        } else {
            Write-Fail ("pwsh keystroke to screen median {0:N2}ms exceeds the {1:N1}ms ceiling" -f $pMedian, $PwshAbsMedianMaxMs)
        }
        if ($floorMedian -ge 0) {
            # Kept as information: this is the number the gate used to judge,
            # and it is only right when the pane and the standalone host happen
            # to share a timer state (see Get-PerfShellSplit).
            Write-Info ("end to end minus the standalone floor: median {0:N2}ms, p99 {1:N2}ms (not judged, see the split below)" -f `
                ($pMedian - $floorMedian), ($pP99 - $floorMedian))
        }

        # The judged numbers: every keystroke split, from the server's own trace
        # in the same run, into the pane shell's echo time and psmux's hops.
        $shellAll = @(); $ownAll = @()
        foreach ($r in $pwshRuns) {
            if ($r.Split) { $shellAll += $r.Split.ShellMs; $ownAll += $r.Split.PsmuxMs }
        }
        $coverage = if ($pwshAll.Count -gt 0) { $ownAll.Count / $pwshAll.Count } else { 0 }
        if ($ownAll.Count -eq 0 -or $coverage -lt 0.8) {
            Write-Fail ("the keystroke trace split only {0} of {1} shell keystrokes into shell and psmux time, so psmux's share of the shell path was not verified" -f $ownAll.Count, $pwshAll.Count)
        } else {
            $oMed = Pct $ownAll 50; $oP90 = Pct $ownAll 90; $oP99 = Pct $ownAll 99
            $sMed = Pct $shellAll 50; $sP99 = Pct $shellAll 99
            Write-Info ("split n={0}: pane shell echo median {1:N2} p99 {2:N2} ms | psmux hops median {3:N2} p90 {4:N2} p99 {5:N2} ms" -f `
                $ownAll.Count, $sMed, $sP99, $oMed, $oP90, $oP99)
            $pwshStats | Add-Member -NotePropertyName split -NotePropertyValue ([pscustomobject]@{
                n = $ownAll.Count
                shellMedian = $sMed; shellP99 = $sP99
                psmuxMedian = $oMed; psmuxP90 = $oP90; psmuxP99 = $oP99
                shellOverFloor = $(if ($floorMedian -ge 0) { $sMed - $floorMedian } else { $null })
                psmuxSamplesMs = $ownAll
                shellSamplesMs = $shellAll
            })
            # Same load policy as the echo cell, for the same reason: under load
            # a healthy median rises onto the budget (2.25ms measured with other
            # agents busy, against 1.75 quiet) while the p99 still discriminates.
            if ($oMed -lt $PwshMedianDeltaMaxMs) {
                Write-Pass ("psmux's own hops cost {0:N2}ms median per shell keystroke, under the {1:N1}ms budget" -f $oMed, $PwshMedianDeltaMaxMs)
            } elseif (-not $wasQuiet) {
                Write-Warn ("psmux's own hops cost {0:N2}ms median per shell keystroke, over the {1:N1}ms budget, but the machine was at {2}% of total cpu, over the {3}% quiet mark; the p99 assertion below still holds" -f $oMed, $PwshMedianDeltaMaxMs, $loadSummary.p50_pct, $QuietLoadPct)
            } else {
                Write-Fail ("psmux's own hops cost {0:N2}ms median per shell keystroke, over the {1:N1}ms budget; the shell path has picked up a hop the echo cell does not exercise" -f $oMed, $PwshMedianDeltaMaxMs)
            }
            if ($oP99 -lt $PwshP99DeltaMaxMs) {
                Write-Pass ("psmux's own hops cost {0:N2}ms p99 per shell keystroke, under the {1:N1}ms budget" -f $oP99, $PwshP99DeltaMaxMs)
            } else {
                Write-Fail ("psmux's own hops cost {0:N2}ms p99 per shell keystroke, over the {1:N1}ms budget; some shell keystrokes are waiting out a timer inside psmux" -f $oP99, $PwshP99DeltaMaxMs)
            }
            # What the split cannot see by itself: psmux making the SHELL slower,
            # for instance a query the shell blocks on (a cursor position report,
            # a device attributes reply) that psmux answers on a poll. That lands
            # inside r - w. So the pane's echo time is held against the standalone
            # floor with a margin wider than the two timer states apart (about
            # 4.5ms, see Get-PerfShellSplit) and narrower than the one extra
            # 15.6ms tick such a defect costs.
            if ($floorMedian -ge 0) {
                $over = $sMed - $floorMedian
                if ($over -lt $PwshShellOverFloorMaxMs) {
                    Write-Pass ("the pane shell echoes in {0:N2}ms median under psmux, {1:N2}ms from the {2:N2}ms standalone floor, inside the {3:N1}ms margin" -f $sMed, $over, $floorMedian, $PwshShellOverFloorMaxMs)
                } else {
                    Write-Fail ("the pane shell echoes in {0:N2}ms median under psmux, {1:N2}ms over the {2:N2}ms standalone floor, past the {3:N1}ms margin; psmux is making the shell itself wait, most likely a reply it answers late" -f $sMed, $over, $floorMedian, $PwshShellOverFloorMaxMs)
                }
            }
        }
    }
    if ($pwshStats) {
        $pwshJson = Write-PerfMetrics -Suite "test_keystroke_latency_gate (pwsh cell)" -Binary $Binary `
            -FileStem "keystroke-latency-pwsh" -MetricsDir $MetricsDir -Stamp $stamp -Data ([ordered]@{
            runs      = $Runs
            keysPerRun = $N
            medianDeltaMaxMs = $PwshMedianDeltaMaxMs
            p99DeltaMaxMs    = $PwshP99DeltaMaxMs
            absMedianMaxMs   = $PwshAbsMedianMaxMs
            shellOverFloorMaxMs = $PwshShellOverFloorMaxMs
            pwsh      = $pwshStats
        })
        if ($pwshJson) { Write-Info "pwsh cell samples written to $pwshJson" }
    }
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Warnings: $($script:Warnings.Count)" -ForegroundColor $(if ($script:Warnings.Count -gt 0) { "Yellow" } else { "Green" })
exit $script:TestsFailed
