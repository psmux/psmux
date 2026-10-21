# test_launch_visible_prompt_gate.ps1: launch to a VISIBLE prompt, psmux cold
# and warm against a bare pwsh, timed in the console the user is looking at.
#
# WHY A SECOND LAUNCH GATE
#   test_launch_to_prompt_gate and test_perf_vs_terminals time launch with a
#   marker script the pane shell runs as its first act, which needs the pane to
#   run THAT command. A psmux new-session with a command never claims the warm
#   standby (the standby's pane runs the default shell), so the cell those
#   suites call "warm" is a cold server spawn every time, and the path a user
#   actually takes when a psmux server is already running, `psmux` with the
#   default shell already at its prompt in a standby, was not measured by any
#   suite. Measured 2026-10-22 on base 0145958f: that path took 125 to 210 ms
#   to a visible prompt, against 491 to 536 ms for a bare pwsh, and nothing would
#   have noticed it doubling.
#
# WHAT IT MEASURES
#   tests/launchprobe.cs starts the command in a NEW console (as Explorer, a
#   shortcut or `start` does) and polls that console's screen until a pwsh
#   prompt ("PS X:\") is visible, both instants from one QPC clock. For psmux
#   that console is the attached client's, so the number holds the client, the
#   server, the pane shell, the first frame and the draw. Four cells, rounds
#   interleaved so machine drift lands on all of them:
#     bare          pwsh -NoLogo -NoProfile
#     cold_cmd      psmux -L <fresh ns> new-session -s x pwsh -NoLogo -NoProfile
#     cold_default  psmux -L <fresh ns> new-session -s x        (default shell)
#     warm_default  psmux -L <ns with a settled standby> new-session -s x
#   The default shell is whatever the user's default-shell resolves to, with its
#   profile, so cold_default is recorded and not gated against bare.
#
# GATES
#   warm_default p50 <= $WarmMaxMs, hard: measured 80 to 84 ms after the
#     2026-10-22 launch work and 125 to 213 ms before it (base 0145958f, this
#     gate, quiet machine: p50 213 ms), so 150 fails the claim, rename and pool
#     regressions that work fixed and leaves room for a loaded machine.
#   warm_default p50 < bare p50, hard when the machine was quiet: with a server
#     running psmux must beat a bare shell to a usable prompt.
#   cold_cmd p50 / bare p50 <= $ColdRatioMax, hard: a ratio, like the other
#     launch gate, because both arms stretch together under load. Measured 1.10
#     to 1.28 after the work and 1.16 to 1.31 before, depending on how fast the
#     bare pwsh came up that run; 1.40 is a floor against a cold path that
#     regresses by a whole extra process start, not a guard on the last 30 ms.
#
# Isolation: a private PSMUX_DATA_DIR and per run -L namespaces, cleaned with
# `kill-server -L` only. Every process it starts is closed by PID.
param(
    [string]$Binary = "",
    [int]$Rounds = 8,
    [double]$WarmMaxMs = 150,
    [double]$ColdRatioMax = 1.40,
    [double]$QuietLoadPct = 25.0
)
$ErrorActionPreference = "Continue"
. "$PSScriptRoot\perf_metrics_common.ps1"
$script:TestsPassed = 0; $script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Warn($m) { Write-Host "  [WARN] $m" -ForegroundColor Yellow }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

if (-not $Binary -and $env:PSMUX_TEST_BIN) { $Binary = $env:PSMUX_TEST_BIN }
if (-not $Binary) {
    $local = Join-Path $PSScriptRoot "..\target\release\psmux.exe"
    if (Test-Path $local) { $Binary = (Resolve-Path $local).Path }
}
if (-not $Binary) { $c = Get-Command psmux -EA SilentlyContinue; if ($c) { $Binary = $c.Source } }
if (-not $Binary -or -not (Test-Path $Binary)) { Write-Host "[SKIP] no psmux binary"; exit 0 }
$Binary = (Resolve-Path $Binary).Path
if (-not (Get-Command pwsh -EA SilentlyContinue)) { Write-Host "[SKIP] pwsh not installed"; exit 0 }
Write-Info "binary under test: $Binary"

foreach ($v in 'PSMUX_SESSION','PSMUX_SESSION_NAME','PSMUX_TARGET_SESSION','TMUX','TMUX_PANE','PSMUX_STARTUP_TRACE','PSMUX_NO_WARM') {
    Remove-Item "Env:\$v" -EA SilentlyContinue
}
$run = Join-Path ([IO.Path]::GetTempPath()) "psmux_lvp_$PID"
Remove-Item -Recurse -Force $run -EA SilentlyContinue
New-Item -ItemType Directory -Force $run | Out-Null
$savedDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = Join-Path $run "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$probe = Join-Path $run "launchprobe.exe"
& $csc /nologo /optimize "/out:$probe" (Join-Path $PSScriptRoot "launchprobe.cs") 2>&1 | Out-Null
if (-not (Test-Path $probe)) { Write-Fail "could not compile tests/launchprobe.cs"; exit 1 }

$freq = [double][Diagnostics.Stopwatch]::Frequency
$warmNs = "lvw$PID"
$cold = 0
function Wait-Standby {
    $f = Join-Path $env:PSMUX_DATA_DIR "${warmNs}____warm__.port"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path $f) -and $sw.ElapsedMilliseconds -lt 15000) { Start-Sleep -Milliseconds 50 }
    # the standby's own shell has to reach its prompt too
    Start-Sleep -Milliseconds 2000
    return (Test-Path $f)
}

function Invoke-Cell([string]$cell, [int]$round) {
    $shell = "pwsh -NoLogo -NoProfile"
    $sess = "s$round"
    $ns = $null
    switch ($cell) {
        "bare"         { $cmd = $shell }
        "cold_cmd"     { $script:cold++; $ns = "lvc$PID$($script:cold)"; $cmd = "`"$Binary`" -L $ns new-session -s $sess $shell" }
        "cold_default" { $script:cold++; $ns = "lvc$PID$($script:cold)"; $cmd = "`"$Binary`" -L $ns new-session -s $sess" }
        "warm_default" { $ns = $warmNs; $cmd = "`"$Binary`" -L $ns new-session -s w$sess" }
    }
    $out = Join-Path $run "p_${cell}_$round.txt"
    & $probe --out $out --cmd $cmd --timeout 20000 | Out-Null
    $kv = @{}
    foreach ($l in (Get-Content $out -EA SilentlyContinue)) { $x = $l.Split(' '); if ($x.Count -ge 2) { $kv[$x[0]] = $x[1] } }
    $ms = -1
    if ($kv["visible"] -and [int64]$kv["visible"] -gt 0) { $ms = ([int64]$kv["visible"] - [int64]$kv["t0"]) * 1000.0 / $freq }
    $launched = if ($kv["pid"]) { [int]$kv["pid"] } else { 0 }
    # Close what this cell opened: the session (its server for a cold cell),
    # then the launched process by PID if it is still there.
    if ($cell -like "cold*") { & $Binary -L $ns kill-server 2>&1 | Out-Null }
    elseif ($cell -eq "warm_default") { & $Binary -L $ns kill-session -t "w$sess" 2>&1 | Out-Null }
    if ($launched -gt 0) {
        $pr = Get-Process -Id $launched -EA SilentlyContinue
        if ($pr) { try { $null = $pr.WaitForExit(2000) } catch {}; if (-not $pr.HasExited) { Stop-Process -Id $launched -Force -EA SilentlyContinue } }
    }
    if ($cell -eq "warm_default") { $null = Wait-Standby } else { Start-Sleep -Milliseconds 800 }
    return $ms
}

Write-Host "=== Launch to a visible prompt: bare pwsh, psmux cold, psmux warm ===" -ForegroundColor Cyan
& $Binary -L $warmNs new-session -d -s anchor 2>&1 | Out-Null
if (-not (Wait-Standby)) { Write-Fail "the warm namespace never got a standby; the warm cell cannot be measured" }
Add-PerfLoadSample "start" | Out-Null
$cells = @("bare", "cold_cmd", "cold_default", "warm_default")
$samples = [ordered]@{}; foreach ($c in $cells) { $samples[$c] = @() }
for ($r = 0; $r -lt $Rounds; $r++) {
    foreach ($c in $cells) {
        $ms = Invoke-Cell $c $r
        if ($ms -ge 0) { $samples[$c] += [math]::Round($ms, 1) } else { Write-Info "$c round $r never showed a prompt" }
    }
    if ($r -eq [int]($Rounds / 2)) { Add-PerfLoadSample "middle" | Out-Null }
}
Add-PerfLoadSample "end" | Out-Null
& $Binary -L $warmNs kill-server 2>&1 | Out-Null

$stats = [ordered]@{}
foreach ($c in $cells) {
    $st = Get-PerfStats $samples[$c] 1
    $stats[$c] = $st
    Write-Info ("{0,-13} n={1,2} p50={2,7} p90={3,7} max={4,7} ms  [{5}]" -f $c, $st.n, $st.p50, $st.p90, $st.max, (($samples[$c] | ForEach-Object { [int]$_ }) -join ', '))
}
$load = Get-PerfLoadSummary
$quiet = Test-PerfMachineQuiet $QuietLoadPct
Write-Info ("machine load: p50={0}% max={1}% -> {2}" -f $load.p50_pct, $load.max_pct, $(if ($quiet) { "quiet" } else { "loaded" }))

$bare = $stats["bare"].p50; $warm = $stats["warm_default"].p50; $coldCmd = $stats["cold_cmd"].p50
if ($null -eq $bare -or $null -eq $warm -or $null -eq $coldCmd) {
    Write-Fail "a cell produced no samples, so nothing was verified"
} else {
    if ($warm -le $WarmMaxMs) { Write-Pass "warm launch p50 $warm ms is within $WarmMaxMs ms" }
    else { Write-Fail "warm launch p50 $warm ms exceeds $WarmMaxMs ms (claim, first frame or pool regression)" }
    if ($warm -lt $bare) { Write-Pass "with a server running psmux reaches a usable prompt before a bare pwsh ($warm < $bare ms)" }
    elseif ($quiet) { Write-Fail "warm psmux p50 $warm ms is not ahead of bare pwsh $bare ms" }
    else { Write-Warn "warm psmux p50 $warm ms is not ahead of bare pwsh $bare ms on a loaded machine" }
    $ratio = [math]::Round($coldCmd / $bare, 3)
    if ($ratio -le $ColdRatioMax) { Write-Pass "cold new-session with a command / bare pwsh = $ratio (budget $ColdRatioMax)" }
    else { Write-Fail "cold new-session with a command / bare pwsh = $ratio exceeds $ColdRatioMax" }
}

$json = Write-PerfMetrics -Suite "test_launch_visible_prompt_gate" -Binary $Binary -FileStem "launch-visible-prompt" -Data ([ordered]@{
    rounds = $Rounds
    warm_max_ms = $WarmMaxMs
    cold_ratio_max = $ColdRatioMax
    machine_was_quiet = $quiet
    stats_ms = $stats
    samples_ms = $samples
    cold_cmd_over_bare = $(if ($bare) { [math]::Round($coldCmd / $bare, 3) } else { $null })
    warm_minus_bare_ms = $(if ($bare -and $warm) { [math]::Round($warm - $bare, 1) } else { $null })
})
if ($json) { Write-Info "metrics: $json" }

$env:PSMUX_DATA_DIR = $savedDataDir
Remove-Item -Recurse -Force $run -EA SilentlyContinue
Write-Host ""
Write-Host "Tests passed: $($script:TestsPassed), failed: $($script:TestsFailed)"
exit $script:TestsFailed
