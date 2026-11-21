#!/usr/bin/env pwsh
# test_long_paragraph_benchmark.ps1 - long paragraphs typed fast (15 ms per key,
# about 66 keys a second) through psmux, timed per character at the client screen.
#
# WHY IT IS A WRAPPER NOW (2026-11-22)
# ------------------------------------
# This suite used to watch the pane by starting a new psmux.exe capture-pane every
# 15 ms from a PowerShell job and called any 200 ms gap between two polls that saw
# growth a "freeze". That is the probe test_typing_benchmark dropped on 2026-11-07:
# a single capture-pane launch from a job takes 200 to 575 ms now and then, before
# psmux's main() even runs. Sweep 2026-11-21_15-34-11 failed it on one 266 ms gap;
# eleven idle reruns showed gaps of 200 to 1580 ms, every one followed by 14 to 20
# characters arriving at once (the keys were never late, the poll was), and the
# "direct PowerShell" baseline it compared against reported 5 s gaps and threw away
# half its runs. The same ten paragraphs at the same 15 ms per key, timed in the
# client's console buffer by tests\typing_latency_probe.cs, gave 2619 of 2619 keys
# in each of three runs with p99 18 ms and the slowest key 30 ms.
#
# So it now runs test_typing_benchmark with these paragraphs: own -L namespace and
# PSMUX_DATA_DIR, a real direct pwsh baseline, the same gates (p99, slowest key,
# no lost characters, first key after Ctrl+C), and metrics in
# %USERPROFILE%\.psmux-test-data\metrics\long-paragraph-benchmark-*.json.

param([string]$Binary = "")

$paragraphs = @(
    "the quick brown fox jumps over the lazy dog and then it runs all the way back across the entire field because it realized it forgot something very important at home and now it needs to hurry before the sun goes down completely over the hills in the distance tonight"
    "pack my box with five dozen liquor jugs and make sure you stack them carefully on the shelf near the back wall of the warehouse so they do not fall over when the delivery truck arrives early tomorrow morning before anyone else gets to the loading dock area"
    "how vexingly quick daft zebras jump across the wide open fields while the farmers watch from their porches drinking coffee and wondering why these animals keep showing up every single morning without fail regardless of the weather or the season of the year"
    "the five boxing wizards jump quickly through the dark misty forest path that winds around the old abandoned castle where nobody has lived for hundreds of years and the walls are covered with thick green ivy that grows taller every single summer without stopping"
    "a large fawn jumped quickly over white zinc boxes left near the highway rest stop where truckers often park their vehicles overnight to get some sleep before continuing on their long journey across the country to deliver goods to stores and warehouses everywhere"
    "crazy frederick bought many very exquisite opal jewels from the old antique shop downtown near the river and he paid with cash because he did not trust the card reader that looked like it had been sitting there since the early nineteen eighties without being updated"
    "we promptly judged antique ivory buckles for the next prize competition at the county fair where hundreds of people gather every autumn to show off their crafts and compete for ribbons and trophies that they display proudly on their mantles at home all year long"
    "sixty zippers were quickly picked from the woven jute bag on the warehouse floor by the new employee who was trying very hard to impress the supervisor on her first day at work because she really needed this job to pay for her college tuition and rent this month"
    "back in june we delivered oxygen equipment of the same size and weight to the city hospital emergency room on the third floor and the nurses were so grateful because they had been waiting for weeks and the patients really needed those supplies right away urgently"
    "playing a quiet game of chess with the king requires very careful strategic moves and a deep understanding of all the possible outcomes that could arise from each decision you make on the board because one wrong move and the entire game could be lost in seconds flat"
)

$textFile = Join-Path $env:TEMP "psmux_long_paragraphs_$PID.txt"
[IO.File]::WriteAllLines($textFile, [string[]]$paragraphs, (New-Object Text.UTF8Encoding $false))
$benchArgs = @{
    TextFile    = $textFile
    IntervalMs  = 15
    SuiteName   = "test_long_paragraph_benchmark"
    MetricsStem = "long-paragraph-benchmark"
}
if ($Binary) { $benchArgs.Binary = $Binary }
try {
    & (Join-Path $PSScriptRoot "test_typing_benchmark.ps1") @benchArgs
    $code = $LASTEXITCODE
} finally {
    Remove-Item -LiteralPath $textFile -Force -ErrorAction SilentlyContinue
}
exit $code
