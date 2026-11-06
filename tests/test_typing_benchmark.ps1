# test_typing_benchmark.ps1 - per character typing latency, psmux vs a plain shell
#
# WHAT IT MEASURES
# ----------------
# For every typed character: the time from WriteConsoleInput into the window
# that owns the keyboard (the psmux CLIENT) to that character being visible in
# the same window's console buffer. Both stamps are taken by ONE process
# (tests\typing_latency_probe.cs) on the same QueryPerformanceCounter clock, and
# the screen is read every 1 ms, so the number is the latency a user sees with
# nothing of the harness folded into it.
#
# WHY IT WAS REWRITTEN (2026-11-07)
# ---------------------------------
# The old suite counted a "stall" when two capture-pane polls that saw growth
# were more than 200 ms apart, and every poll was a new psmux.exe started from a
# PowerShell Start-Job. Its "P50 gap" was the polling period, and every stall it
# reported (11 over 80 sentences in an isolated reproduction) was ONE capture-pane
# call that took 211 to 575 ms, nearly all of it Windows launching the process
# from the job before psmux's main() ran (PSMUX_STARTUP_TRACE cli.entry). Over the
# same runs the client screen never took more than 35 ms for any character.
# Its direct PowerShell baseline also never measured anything: it summed every
# non-empty cell in five rows and stopped as soon as that reached the text length,
# so text already on the screen ended it at the first sample (render=0 for 9 of 10
# sentences in every log). Both are fixed here: no process spawn per sample, and
# both phases match the typed text from the cursor cell recorded before typing.
#
# THE CTRL+C CASE
# ---------------
# Every Ctrl+C used to take nine full CreateToolhelp32Snapshot walks (about 10 ms
# each) on the server thread that writes keystrokes into ConPTY before the 0x03
# reached the pane, so the first key typed after Ctrl+C was held 114 to 131 ms
# (p50). With one pinned walk per decision it is 36 to 37 ms. The third phase
# presses Ctrl+C in the client and types at once, and gates the median latency of
# that first key.
#
# KNOWN WINDOWS BEHAVIOUR, TOLERATED ONCE
# ---------------------------------------
# conhost occasionally holds the LAST input written to a ConPTY until the next
# input arrives (about 1 in 6000 to 25000 writes, reproduced with no psmux). A
# sentence whose only unseen character is its final one is reported as "held"
# and one such sentence per run is tolerated. Any other missing character fails.
#
# ISOLATION
# ---------
# Own namespace (-L typbench<pid>) and a private PSMUX_DATA_DIR; it never touches
# the default namespace or ~/.psmux, kills nothing by name, and closes the two
# windows it opens by PID. Binary: -Binary, else PSMUX_TEST_EXE, else
# PSMUX_TEST_BIN, else psmux on PATH.
#
# Metrics JSON: %USERPROFILE%\.psmux-test-data\metrics\typing-benchmark-*.json
# (tests\perf_metrics_common.ps1 envelope).

param(
    [string]$Binary = "",
    [int]$IntervalMs = 40,
    # Generous so a loaded sweep does not trip them. Measured on a quiet box:
    # per char p99 26 to 28 ms and max 35 ms; a real stall is over 200 ms.
    [double]$MaxP99Ms = 150,
    [double]$MaxCharMs = 400,
    # First key after Ctrl+C, median over the repeats. 114 to 131 ms with the
    # nine walk router, 36 to 37 ms with one walk.
    [double]$MaxCtrlCFirstMs = 80,
    [int]$CtrlCRepeats = 6,
    [switch]$SkipDirect,
    # Set PSMUX_PTY_TRACE=<TraceDir>\trace for the client and the server it
    # starts, so a slow key can be followed hop by hop (src/pty_trace.rs).
    [string]$TraceDir = ""
)

$ErrorActionPreference = "Continue"
. "$PSScriptRoot\perf_metrics_common.ps1"
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

if (-not $Binary) {
    $Binary = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA SilentlyContinue).Source }
}
if (-not $Binary -or -not (Test-Path $Binary)) { Write-Fail "psmux binary not found ($Binary)"; exit 1 }
Write-Info "binary under test: $Binary"

foreach ($v in @('PSMUX_SESSION', 'PSMUX_SESSION_NAME', 'PSMUX_SOCKET', 'PSMUX_PANE_ID', 'PSMUX_PTY_TRACE', 'TMUX', 'TMUX_PANE')) {
    Remove-Item "Env:\$v" -EA SilentlyContinue
}

# ── probe ────────────────────────────────────────────────────────────────
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$probeSrc = Join-Path $PSScriptRoot "typing_latency_probe.cs"
$probe = Join-Path $env:TEMP "psmux_typing_latency_probe.exe"
if ((-not (Test-Path $probe)) -or ((Get-Item $probeSrc).LastWriteTime -gt (Get-Item $probe).LastWriteTime)) {
    & $csc /nologo /optimize "/out:$probe" $probeSrc | Out-Null
}
if (-not (Test-Path $probe)) { Write-Fail "could not build typing_latency_probe.exe"; exit 1 }

function Invoke-Probe {
    param([string[]]$ProbeArgs)
    $out = @(& $probe @ProbeArgs 2>&1 | ForEach-Object { $_.ToString() })
    $lat = [System.Collections.Generic.List[double]]::new()
    $miss = [System.Collections.Generic.List[int]]::new()
    foreach ($l in $out) {
        if ($l -match '^LAT (\d+) ([\d.]+)$') { $lat.Add([double]::Parse($Matches[2], [Globalization.CultureInfo]::InvariantCulture)) }
        elseif ($l -match '^MISS (\d+)$') { $miss.Add([int]$Matches[1]) }
    }
    return [pscustomobject]@{ Lat = $lat.ToArray(); Miss = $miss.ToArray(); Raw = $out }
}

$sentences = @(
    "the quick brown fox jumps over the lazy dog and runs back home again"
    "pack my box with five dozen liquor jugs before the party starts tonight"
    "how vexingly quick daft zebras jump across the wide open fields today"
    "the five boxing wizards jump quickly through the dark misty forest path"
    "a large fawn jumped quickly over white zinc boxes left near the highway"
    "crazy frederick bought many very exquisite opal jewels from the old shop"
    "we promptly judged antique ivory buckles for the next prize competition"
    "sixty zippers were quickly picked from the woven jute bag on the floor"
    "back in june we delivered oxygen equipment of the same size to the city"
    "playing a quiet game of chess with the king requires very careful moves"
)

$NS = "typbench$PID"
$S = "bench"
$DATA = Join-Path $env:TEMP "psmux_typbench_$PID"
New-Item -ItemType Directory -Force -Path $DATA | Out-Null
$prevDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = $DATA

$psmuxRows = @(); $ctrlcFirst = @(); $directRows = @()
$psmuxAll = [System.Collections.Generic.List[double]]::new()
$directAll = [System.Collections.Generic.List[double]]::new()
$held = 0; $lost = 0
$client = $null; $direct = $null

try {
    # ── phase 1: through psmux ──────────────────────────────────────────
    Write-Host "`n=== PHASE 1: typing through psmux (client screen) ===" -ForegroundColor Yellow
    Add-PerfLoadSample "start" | Out-Null
    if ($TraceDir) { New-Item -ItemType Directory -Force -Path $TraceDir | Out-Null; $env:PSMUX_PTY_TRACE = Join-Path $TraceDir "trace" }
    $client = Start-Process -FilePath $Binary -ArgumentList @("-L", $NS, "new-session", "-s", $S) -PassThru
    Remove-Item Env:\PSMUX_PTY_TRACE -EA SilentlyContinue
    Write-Info "client pid $($client.Id), namespace $NS"
    $ready = $false
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt 30) {
        Start-Sleep -Milliseconds 500
        $cap = & $Binary -L $NS capture-pane -t $S -p 2>&1 | Out-String
        if ($cap -match "PS [A-Z]:\\|>\s*$") { $ready = $true; break }
    }
    if (-not $ready) { Write-Fail "the psmux pane never showed a prompt"; throw "no prompt" }
    Start-Sleep -Seconds 1

    $n = 0
    foreach ($sentence in $sentences) {
        $n++
        & $Binary -L $NS send-keys -t $S C-c 2>&1 | Out-Null
        Start-Sleep -Milliseconds 200
        & $Binary -L $NS send-keys -t $S "clear" Enter 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1000
        $r = Invoke-Probe @("type", "$($client.Id)", $sentence, "$IntervalMs")
        foreach ($x in $r.Lat) { $psmuxAll.Add($x) }
        $note = ""
        if ($r.Miss.Count -eq 1 -and $r.Miss[0] -eq $sentence.Length) { $held++; $note = " (final char held)" }
        elseif ($r.Miss.Count -gt 0) { $lost += $r.Miss.Count; $note = " MISSING $($r.Miss.Count) chars from #$($r.Miss[0])" }
        $st = Get-PerfStats $r.Lat 1
        $row = [ordered]@{ sentence = $n; chars = $sentence.Length; seen = $r.Lat.Count; p50 = $st.p50; p99 = $st.p99; max = $st.max; missing = @($r.Miss) }
        # Keep every key of a sentence that had a slow one, so the shape (one
        # held burst, or a slow drift) is in the record, not just its max.
        if ($null -ne $st.max -and $st.max -gt 200) { $row["per_char_ms"] = $r.Lat }
        $psmuxRows += $row
        Write-Host ("  sentence {0,2}: seen {1}/{2} p50={3}ms p99={4}ms max={5}ms{6}" -f $n, $r.Lat.Count, $sentence.Length, $st.p50, $st.p99, $st.max, $note) -ForegroundColor $(if ($note -match "MISSING") { "Red" } else { "Gray" })
    }
    Add-PerfLoadSample "after_typing" | Out-Null

    # ── phase 2: Ctrl+C then type at once ───────────────────────────────
    Write-Host "`n=== PHASE 2: Ctrl+C in the client, then type at once ===" -ForegroundColor Yellow
    for ($i = 1; $i -le $CtrlCRepeats; $i++) {
        & $Binary -L $NS send-keys -t $S C-c 2>&1 | Out-Null
        Start-Sleep -Milliseconds 300
        & $Binary -L $NS send-keys -t $S "clear" Enter 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1200
        $text = "~$($i)qzj~ typed after ctrl c"
        $r = Invoke-Probe @("ctrlc", "$($client.Id)", $text, "$IntervalMs", "0")
        if ($r.Lat.Count -ge 1) {
            $ctrlcFirst += $r.Lat[0]
            Write-Host ("  repeat {0}: first key after Ctrl+C {1}ms, keys 2-4 {2}" -f $i, $r.Lat[0], (($r.Lat | Select-Object -Skip 1 -First 3) -join "/")) -ForegroundColor Gray
        } else {
            Write-Host "  repeat ${i}: the typed text never appeared" -ForegroundColor Red
        }
    }
    Add-PerfLoadSample "after_ctrlc" | Out-Null
} finally {
    & $Binary -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    if ($client) { try { if (-not $client.HasExited) { Stop-Process -Id $client.Id -Force -EA SilentlyContinue } } catch { } }
    if ($null -eq $prevDataDir) { Remove-Item Env:\PSMUX_DATA_DIR -EA SilentlyContinue } else { $env:PSMUX_DATA_DIR = $prevDataDir }
    Remove-Item -Recurse -Force $DATA -EA SilentlyContinue
}

# ── phase 3: the same sentences straight into a shell ───────────────────
if (-not $SkipDirect) {
    Write-Host "`n=== PHASE 3: baseline, the same typing into a plain shell ===" -ForegroundColor Yellow
    $shell = (Get-Command pwsh -EA SilentlyContinue).Source
    if (-not $shell) { $shell = "powershell.exe" }
    try {
        $direct = Start-Process -FilePath $shell -ArgumentList "-NoProfile", "-NoExit", "-Command", "function prompt { 'BENCH> ' }" -PassThru
        Start-Sleep -Seconds 4
        $n = 0
        foreach ($sentence in $sentences) {
            $n++
            # Escape reverts the line PSReadLine is holding, then clear runs.
            & $probe line "$($direct.Id)" ([string][char]27 + "clear") | Out-Null
            Start-Sleep -Milliseconds 1000
            $r = Invoke-Probe @("type", "$($direct.Id)", $sentence, "$IntervalMs")
            foreach ($x in $r.Lat) { $directAll.Add($x) }
            $st = Get-PerfStats $r.Lat 1
            $directRows += [ordered]@{ sentence = $n; chars = $sentence.Length; seen = $r.Lat.Count; p50 = $st.p50; p99 = $st.p99; max = $st.max }
            Write-Host ("  sentence {0,2}: seen {1}/{2} p50={3}ms p99={4}ms max={5}ms" -f $n, $r.Lat.Count, $sentence.Length, $st.p50, $st.p99, $st.max) -ForegroundColor Gray
        }
    } finally {
        if ($direct) { try { if (-not $direct.HasExited) { Stop-Process -Id $direct.Id -Force -EA SilentlyContinue } } catch { } }
    }
}

# ── verdicts ────────────────────────────────────────────────────────────
Write-Host "`n=== VERDICT ===" -ForegroundColor Cyan
$pStats = Get-PerfStats $psmuxAll.ToArray() 1
$dStats = Get-PerfStats $directAll.ToArray() 1
$cStats = Get-PerfStats $ctrlcFirst 1
$totalChars = ($sentences | Measure-Object -Property Length -Sum).Sum
Write-Info ("psmux per char: n={0} p50={1}ms p90={2}ms p99={3}ms max={4}ms" -f $pStats.n, $pStats.p50, $pStats.p90, $pStats.p99, $pStats.max)
if (-not $SkipDirect) {
    Write-Info ("direct per char: n={0} p50={1}ms p90={2}ms p99={3}ms max={4}ms" -f $dStats.n, $dStats.p50, $dStats.p90, $dStats.p99, $dStats.max)
    if ($dStats.n -lt $totalChars) { Write-Host "  [WARN] the direct baseline saw $($dStats.n) of $totalChars chars; its numbers are partial" -ForegroundColor Yellow }
    elseif ($null -ne $pStats.p50) { Write-Info ("psmux overhead: p50 +{0}ms, p99 +{1}ms" -f [math]::Round($pStats.p50 - $dStats.p50, 1), [math]::Round($pStats.p99 - $dStats.p99, 1)) }
}

if ($lost -eq 0 -and $held -le 1) {
    Write-Pass "every typed character reached the client screen ($($pStats.n) of $totalChars, $held sentence(s) with the final char held by conhost)"
} else {
    Write-Fail "characters never reached the client screen: $lost lost, $held sentence(s) with the final char held (one is tolerated)"
}
if ($null -ne $pStats.p99 -and $pStats.p99 -le $MaxP99Ms) { Write-Pass "per char p99 $($pStats.p99)ms <= ${MaxP99Ms}ms" }
else { Write-Fail "per char p99 $($pStats.p99)ms > ${MaxP99Ms}ms" }
if ($null -ne $pStats.max -and $pStats.max -le $MaxCharMs) { Write-Pass "slowest character $($pStats.max)ms <= ${MaxCharMs}ms" }
else { Write-Fail "slowest character $($pStats.max)ms > ${MaxCharMs}ms: a key took that long to appear in the client" }
if ($cStats.n -ge [math]::Ceiling($CtrlCRepeats / 2) -and $cStats.p50 -le $MaxCtrlCFirstMs) {
    Write-Pass "first key after Ctrl+C: median $($cStats.p50)ms <= ${MaxCtrlCFirstMs}ms over $($cStats.n) presses"
} else {
    Write-Fail "first key after Ctrl+C: median $($cStats.p50)ms over $($cStats.n) presses > ${MaxCtrlCFirstMs}ms (the Ctrl+C router holds the server's input thread)"
}

$jsonPath = Write-PerfMetrics -Suite "test_typing_benchmark" -Binary $Binary -FileStem "typing-benchmark" -Data ([ordered]@{
    interval_ms           = $IntervalMs
    thresholds            = [ordered]@{ max_p99_ms = $MaxP99Ms; max_char_ms = $MaxCharMs; max_ctrlc_first_ms = $MaxCtrlCFirstMs }
    psmux_per_char        = $pStats
    psmux_sentences       = $psmuxRows
    psmux_held_final_char = $held
    psmux_lost_chars      = $lost
    ctrlc_first_key       = $cStats
    ctrlc_first_key_ms    = $ctrlcFirst
    direct_per_char       = $dStats
    direct_sentences      = $directRows
    failed                = $script:TestsFailed
})
if ($jsonPath) { Write-Info "metrics: $jsonPath" }

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
