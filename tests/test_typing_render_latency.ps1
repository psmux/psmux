#!/usr/bin/env pwsh
# test_typing_render_latency.ps1 - does typed text keep appearing while you type?
#
# Keys are written into the psmux client's console input one at a time at a
# fixed rate (tests\typing_bench.cs), and the same tool samples the client's
# cursor every 2 ms in that console, so a character counts as shown the moment
# the client drew it. A "freeze" is a gap of more than 300 ms between two
# visible changes while keys are still going in.
#
# HISTORY
# -------
# Until 2026-11-22 this suite watched the pane by starting psmux.exe
# capture-pane every 30 ms from a PowerShell job. One launch from a job can
# take 200 to 575 ms before psmux's main() runs (test_typing_benchmark dropped
# that probe on 2026-11-07), so sweep 2026-11-22_04-42-19 failed it on a single
# 346 ms gap while the input log showed every key going straight through.
#
# The same sweep found a real freeze with the accurate probe in
# test_realistic_typing: when a busy machine leaves keys queued, the client
# reads several at once, the paste heuristic (3 or more characters in 20 ms)
# takes them for a paste, and it held every later key until input went quiet
# for 100 ms, which steady typing never does. The line appeared all at once
# when the typist stopped: 4.3 s late at 12 ms per key. The burst cases below
# make that bunch on purpose and gate on it.

param([string]$Binary = "")

$ErrorActionPreference = "Continue"
$PSMUX = if ($Binary) { $Binary } else { (Get-Command psmux -EA Stop).Source }
. "$PSScriptRoot\perf_metrics_common.ps1"

$script:Passed = 0
$script:Failed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$benchExe = Join-Path $env:TEMP "psmux_typing_bench_trl_$PID.exe"
& $csc /nologo /optimize /out:$benchExe "$PSScriptRoot\typing_bench.cs" 2>&1 | Where-Object { $_ -notmatch "CS0420" } | Out-Host
if (-not (Test-Path $benchExe)) { Write-Host "typing_bench.cs did not compile" -ForegroundColor Red; exit 1 }

$NS = "trl$PID"
$SESSION = "trl"
$dataDir = Join-Path $env:TEMP "psmux-trl-$PID"
New-Item -ItemType Directory -Force $dataDir | Out-Null
$env:PSMUX_DATA_DIR = $dataDir

$line43 = "the quick brown fox jumps over the lazy dog"
$long = "the quick brown fox jumps over the lazy dog and then it runs all the way back across the entire field because it realized it forgot something very important at home and now it needs to hurry before the sun goes down completely over"

# Name, text, ms per key, ms after a space, burst start, burst length
$cases = @(
    @{ Name = "Normal fast typing (80 ms, ~75 WPM)";        Text = $line43; Intra = 80; Inter = 80; BurstAt = -1; BurstLen = 0 }
    @{ Name = "Very fast typing (40 ms, ~150 WPM)";         Text = $line43; Intra = 40; Inter = 40; BurstAt = -1; BurstLen = 0 }
    @{ Name = "Extreme speed (15 ms, key repeat rate)";     Text = $line43; Intra = 15; Inter = 15; BurstAt = -1; BurstLen = 0 }
    @{ Name = "Long line at 12 ms per key";                 Text = $long;   Intra = 12; Inter = 30; BurstAt = -1; BurstLen = 0 }
    @{ Name = "5 queued keys, then 12 ms per key";          Text = $long;   Intra = 12; Inter = 30; BurstAt = 0;  BurstLen = 5 }
    @{ Name = "5 queued keys mid line, then 12 ms per key"; Text = $long;   Intra = 12; Inter = 30; BurstAt = 60; BurstLen = 5 }
)

function Parse-Bench([string[]]$Lines) {
    $h = @{}
    foreach ($l in $Lines) {
        if ($l -match '^FIRST_RENDER_AFTER_INJECT_MS=(-?\d+)') { $h["first"] = [int]$Matches[1] }
        if ($l -match '^SUMMARY ') {
            ($l -replace '^SUMMARY ', '' -split ' ') | ForEach-Object { $kv = $_ -split '='; if ($kv.Length -eq 2) { $h[$kv[0]] = $kv[1] } }
        }
    }
    return $h
}

$proc = $null
$results = @()
try {
    $env:PSMUX_INPUT_DEBUG = "1"
    $proc = Start-Process -FilePath $PSMUX -ArgumentList "-L", $NS, "new-session", "-s", $SESSION -PassThru
    $env:PSMUX_INPUT_DEBUG = $null
    $ready = $false
    for ($i = 0; $i -lt 60; $i++) {
        Start-Sleep -Milliseconds 500
        $cap = & $PSMUX -L $NS capture-pane -t $SESSION -p 2>&1 | Out-String
        if ($cap -match "PS [A-Z]:\\") { $ready = $true; break }
    }
    if (-not $ready) { Write-Fail "the psmux pane never showed a prompt"; throw "no prompt" }
    Start-Sleep -Seconds 1
    Write-Host "Client PID $($proc.Id), namespace $NS" -ForegroundColor Cyan

    foreach ($c in $cases) {
        & $benchExe $proc.Id "" 0 0 "clear" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 800
        $benchArgs = @($proc.Id, $c.Text, $c.Intra, $c.Inter)
        if ($c.BurstAt -ge 0) { $benchArgs += @($c.BurstAt, $c.BurstLen) }
        $out = & $benchExe @benchArgs 2>&1 | ForEach-Object { "$_" }
        $s = Parse-Bench $out
        if (-not $s.ContainsKey("rendered")) { Write-Fail "$($c.Name): typing_bench printed no summary"; continue }
        $r = [PSCustomObject]@{
            Case = $c.Name; Chars = [int]$s["chars"]; Rendered = [int]$s["rendered"]
            FirstMs = $s["first"]; InjectMs = [int]$s["inject_ms"]; P50 = [int]$s["p50"]; P90 = [int]$s["p90"]
            P99 = [int]$s["p99"]; MaxGap = [int]$s["max_gap"]; Stalls = [int]$s["stalls"]
        }
        $results += $r
        # typing_bench counts a gap over 150 ms as a stall; the freeze gate here is 300 ms.
        $freeze = $r.MaxGap -gt 300
        $allShown = $r.Rendered -ge $r.Chars
        $firstOk = $r.FirstMs -ge 0 -and $r.FirstMs -le 300
        $detail = "first char after $($r.FirstMs) ms, $($r.Rendered)/$($r.Chars) shown, p50 $($r.P50) ms, p99 $($r.P99) ms, max gap $($r.MaxGap) ms"
        if (-not $freeze -and $allShown -and $firstOk) { Write-Pass "$($c.Name): $detail" }
        else { Write-Fail "$($c.Name): $detail" }
    }
} catch {
    if ("$_" -ne "no prompt") { Write-Fail "error: $_" }
} finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    try { if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } } catch {}
    $env:PSMUX_DATA_DIR = $null
}

# What the paste heuristic did with the typing (information, not a gate: the
# gate is what reached the screen).
$inputLog = @("$dataDir\input_debug.log", "$env:USERPROFILE\.psmux\input_debug.log") | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($inputLog) {
    $lines = @(Get-Content $inputLog -EA SilentlyContinue | Where-Object { $_ -match "^\[" })
    Write-Host "`nPaste heuristic: $(@($lines -match 'stage2: ').Count) stage 2 entries, $(@($lines -match 'stage2 was typing').Count) ended as typing, $(@($lines -match 'stage2 timeout').Count) sent as a paste" -ForegroundColor DarkGray
}
Remove-Item -LiteralPath $benchExe -Force -EA SilentlyContinue

$results | Format-Table -AutoSize | Out-Host
$jsonPath = Write-PerfMetrics -Suite "test_typing_render_latency" -Binary $PSMUX -FileStem "typing-render-latency" -Data ([ordered]@{ cases = $results })
if ($jsonPath) { Write-Host "Metrics: $jsonPath" -ForegroundColor DarkGray }

Write-Host "`nPassed: $script:Passed  Failed: $script:Failed"
if ($script:Failed -gt 0) { exit 1 }
exit 0
