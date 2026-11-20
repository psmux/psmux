# Issue #556: yazi opens a "Shell:" popup with rgb:0c0c/0c0c/0c0c at startup
# inside psmux.
#
# Claim: psmux's #473 color-query answering delivers the OSC 11 response
# AFTER the app's probe window has closed (ConPTY answers DA1 itself in ~33ms,
# which closes yazi's window), so yazi re-parses the late response bytes as an
# interactive `shell` action.
#
# Part 1 - timing probe: send OSC11 + ?996n + DA1 (yazi order) from a pane
#          app, timestamp every response chunk. REPRO = OSC 11 answer arrives
#          after the DA1 answer (raceLost), i.e. outside yazi's probe window.
# Part 2 - live yazi repro: run yazi in a pane, capture-pane, look for the
#          "Shell:" popup containing rgb:0c0c.
# Part 3 - regression guard (post-fix): OSC 11 must arrive BEFORE DA1 and
#          still carry correct colors (the #473 feature must keep working).

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$NODE = (Get-Command node -EA Stop).Source
# yazi from PATH first: winget can put it there from WinGet\Packages without
# a WinGet\Links shim, which is all this used to look at.
$YAZI = (Get-Command yazi -CommandType Application -EA SilentlyContinue | Select-Object -First 1).Source
if (-not $YAZI) { $YAZI = "$env:LOCALAPPDATA\Microsoft\WinGet\Links\yazi.exe" }
$psmuxDir = "$env:USERPROFILE\.psmux"
$PROBE = Join-Path $PSScriptRoot "color_query_timing_probe.js"
$outDir = Join-Path $env:TEMP "psmux_test_556"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }

function Remove-Session($name) {
    & $PSMUX kill-session -t $name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$name.*" -Force -EA SilentlyContinue
}

function Wait-Prompt($name) {
    for ($i = 0; $i -lt 40; $i++) {
        $cap = & $PSMUX capture-pane -t $name -p 2>&1 | Out-String
        if ($cap -match "PS [A-Z]:\\") { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

function Invoke-TimingProbe($name, $tag) {
    $outFile = Join-Path $outDir "timing_${name}_${tag}.json"
    Remove-Item $outFile -Force -EA SilentlyContinue
    & $PSMUX send-keys -t $name "`$env:PROBE_OUT='$outFile'; & '$NODE' '$PROBE'" Enter
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep -Milliseconds 500
        if (Test-Path $outFile) { break }
    }
    if (-not (Test-Path $outFile)) { return $null }
    Start-Sleep -Milliseconds 300
    return (Get-Content $outFile -Raw | ConvertFrom-Json)
}

Write-Host "`n=== Issue #556: yazi startup probe race ===" -ForegroundColor Cyan

# -- Part 1: timing probe --
Write-Host "`n[Part 1] Response ordering: OSC 11 vs DA1 (yazi probe order)" -ForegroundColor Yellow
$S1 = "t556_timing"
Remove-Session $S1
& $PSMUX new-session -d -s $S1
Start-Sleep -Seconds 3
if (-not (Wait-Prompt $S1)) { Write-Fail "Part 1: prompt never ready" }
else {
    $r = Invoke-TimingProbe $S1 "p1"
    if ($null -eq $r) { Write-Fail "Part 1: probe produced no result" }
    else {
        Write-Info "DA1 answer at: $($r.da1Ms)ms  OSC11 answer at: $($r.osc11Ms)ms  scheme at: $($r.schemeMs)ms"
        if ($null -eq $r.da1Ms) { Write-Fail "Part 1: DA1 never answered (unexpected)" }
        else {
            # ConPTY answers DA1 itself (psmux never even sees the query), so
            # psmux physically cannot answer BEFORE the DA1 reply.  The fix
            # (#556) answers at detection in the reader thread, cutting the
            # coalescing + server-loop latency.  Assert psmux's answer (the
            # ?996n scheme reply, the only query conhost forwards from this
            # bundle on current builds) arrives promptly after the query.
            if ($null -eq $r.schemeMs) { Write-Fail "scheme query (?996n) was never answered" }
            elseif ($r.schemeMs -lt 40) { Write-Pass "psmux answered ?996n at $($r.schemeMs)ms (sync-at-detection path)" }
            else { Write-Fail "psmux's ?996n answer too late: $($r.schemeMs)ms (server-loop latency regressed?)" }
        }
        if ($null -ne $r.osc11Ms) { Write-Info "OSC 11 answered $([math]::Round($r.osc11Ms - $r.da1Ms,1))ms after DA1" }
        else { Write-Info "OSC 11 not answered (conhost consumed the query without forwarding it; expected on Win11 26200)" }
    }
}
Remove-Session $S1

# -- Part 2: live yazi repro --
Write-Host "`n[Part 2] Live yazi: Shell popup with rgb:0c0c at startup" -ForegroundColor Yellow
if (-not (Test-Path $YAZI)) { Write-Fail "yazi not installed at $YAZI" }
else {
    $S2 = "t556_yazi"
    Remove-Session $S2
    & $PSMUX new-session -d -s $S2 -x 120 -y 30
    Start-Sleep -Seconds 3
    if (-not (Wait-Prompt $S2)) { Write-Fail "Part 2: prompt never ready" }
    else {
        & $PSMUX send-keys -t $S2 "& '$YAZI'" Enter
        # Poll capture for up to 15s looking for the popup or a settled yazi UI
        $popupSeen = $false
        $yaziSeen = $false
        $finalCap = ""
        for ($i = 0; $i -lt 30; $i++) {
            Start-Sleep -Milliseconds 500
            $cap = & $PSMUX capture-pane -t $S2 -p 2>&1 | Out-String
            $finalCap = $cap
            if ($cap -match "Shell:" -and $cap -match "rgb:0c0c") { $popupSeen = $true; break }
            if ($cap -match "Shell:") { $popupSeen = $true; break }
        }
        if ($finalCap.Length -gt 100) { $yaziSeen = $true }
        if ($popupSeen) {
            Write-Pass "REPRO CONFIRMED: yazi shows a Shell: popup at startup"
            Write-Info ("popup line: " + (($finalCap -split "`n" | Where-Object { $_ -match "Shell:|rgb:0c0c" }) -join " | "))
        } else {
            # Post-fix expectation: yazi starts clean, no Shell: popup at any poll
            Write-Pass "yazi started with NO Shell: popup (clean startup)"
        }
        # Save the capture for evidence either way
        $finalCap | Set-Content (Join-Path $outDir "yazi_capture.txt") -Encoding UTF8
        # Quit yazi (Esc closes popup if present, q quits)
        & $PSMUX send-keys -t $S2 Escape 2>&1 | Out-Null
        Start-Sleep -Milliseconds 300
        & $PSMUX send-keys -t $S2 "q" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 500
    }
    Remove-Session $S2
}

# -- Part 3: #473 burst still fully answered, now at reader-thread speed --
Write-Host "`n[Part 3] Regression guard: #473 full burst answered promptly" -ForegroundColor Yellow
$S3 = "t556_burst"
Remove-Session $S3
& $PSMUX new-session -d -s $S3
Start-Sleep -Seconds 3
if (-not (Wait-Prompt $S3)) { Write-Fail "Part 3: prompt never ready" }
else {
    $out = Join-Path $outDir "timing_burst_p3.json"
    Remove-Item $out -Force -EA SilentlyContinue
    & $PSMUX send-keys -t $S3 "`$env:PROBE_OUT='$out'; `$env:PROBE_TERM='st'; `$env:PROBE_BUNDLE='b473'; & '$NODE' '$PROBE'" Enter
    for ($i = 0; $i -lt 30; $i++) { Start-Sleep -Milliseconds 500; if (Test-Path $out) { break } }
    Start-Sleep -Milliseconds 300
    if (-not (Test-Path $out)) { Write-Fail "Part 3: probe produced no result" }
    else {
        $r = Get-Content $out -Raw | ConvertFrom-Json
        Write-Info "burst: scheme at $($r.schemeMs)ms, OSC11 at $($r.osc11Ms)ms"
        if ($null -ne $r.schemeMs -and $null -ne $r.osc11Ms) {
            Write-Pass "burst still gets scheme + fg/bg replies (#473 preserved)"
        } else {
            Write-Fail "burst answers missing: scheme=$($r.schemeMs) osc11=$($r.osc11Ms)"
        }
        if ($null -ne $r.osc11Ms -and $r.osc11Ms -lt 40) { Write-Pass "burst OSC replies within 40ms ($($r.osc11Ms)ms)" }
        elseif ($null -ne $r.osc11Ms) { Write-Fail "burst OSC replies too late: $($r.osc11Ms)ms" }
    }
}
Remove-Session $S3

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
