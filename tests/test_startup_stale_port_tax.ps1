# Startup stale-port tax regression test
#
# Root cause (proven by measurement 2026-07-10): every CLI invocation ran
# cleanup_stale_port_files(), which TCP-probed each .port file SERIALLY
# (3 attempts x 100ms connect timeout). On Windows hosts where a dead
# loopback port never sends RST (stealth firewall), each stale file cost
# ~350-400ms per psmux command AND was classified Inconclusive, so it was
# never reaped: 6 stale files made new-session take ~2.4s, cold start ~4.9s,
# list-sessions ~3.5s, forever.
#
# Fix: consult the .pid sentinel (issue #448) before any network probe.
# A dead PID is a definitive microsecond-cheap stale verdict.
#
# This test plants hard-killed session registries (the real-world stale
# state) and asserts:
#   1. the first psmux invocation reaps them
#   2. new-session / list-sessions stay fast with stales present
#   3. live sessions are NEVER reaped by the pid-anchor path
#   4. TUI attached session works (Layer 2 visual verification)

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

function Make-StaleRegistries($n) {
    # Create real sessions then hard-kill their server processes, leaving
    # genuine stale .port/.key/.sid/.pid files (what a crash leaves behind).
    foreach ($i in 1..$n) {
        & $PSMUX kill-session -t "taxstale$i" 2>&1 | Out-Null
        Remove-Item "$psmuxDir\taxstale$i.*" -Force -EA SilentlyContinue
    }
    Start-Sleep -Milliseconds 400
    foreach ($i in 1..$n) { & $PSMUX new-session -d -s "taxstale$i" 2>&1 | Out-Null }
    Start-Sleep -Seconds 2
    $killed = @()
    foreach ($i in 1..$n) {
        $pidFile = "$psmuxDir\taxstale$i.pid"
        if (Test-Path $pidFile) {
            # .pid body is "pid" or "pid:creation_filetime" (PR #404); keep the pid part
            $p = ((Get-Content $pidFile -Raw).Trim().Split(':'))[0]
            if ($p -match '^\d+$') {
                $proc = Get-Process -Id ([int]$p) -EA SilentlyContinue
                if ($proc -and $proc.ProcessName -eq "psmux") { Stop-Process -Id ([int]$p) -Force; $killed += [int]$p }
            }
        }
    }
    # Wait for the servers to be gone rather than a fixed second: ANY psmux
    # invocation on this machine reaps dead-pid registries (that is the feature),
    # so the longer the gap, the likelier another process does it first.
    if ($killed) { Wait-Process -Id $killed -Timeout 5 -EA SilentlyContinue }
    return (Get-ChildItem "$psmuxDir\taxstale*.port" -EA SilentlyContinue).Count
}

Write-Host "`n=== Startup stale-port tax tests ===" -ForegroundColor Cyan

# === TEST 1: stale registries are reaped by the next invocation ===
Write-Host "`n[Test 1] Dead-PID registries are reaped without network probes" -ForegroundColor Yellow
# Another psmux process can reap the planted registries before our own first
# invocation does: measured 2026-11-20, 4 of 14 plantings were reaped within
# 1.5 s by a psmux started by a background daemon (amesh) that runs psmux
# commands periodically. That is correct behaviour, but then this test would not
# be the one exercising the reap, so plant again.
$made = 0
for ($attempt = 1; $attempt -le 3; $attempt++) {
    $made = Make-StaleRegistries 6
    if ($made -eq 6) { break }
    Write-Host "  [INFO] attempt ${attempt}: $made of 6 stale .port files left; another psmux process reaped the rest first, planting again" -ForegroundColor DarkGray
}
if ($made -eq 6) { Write-Pass "planted 6 genuine stale registries" }
else { Write-Fail "expected 6 stale .port files, got $made" }

& $PSMUX has-session -t nonexistent_probe_session 2>&1 | Out-Null   # any CLI invocation triggers cleanup
Start-Sleep -Milliseconds 300
$left = (Get-ChildItem "$psmuxDir\taxstale*.port" -EA SilentlyContinue).Count
if ($left -eq 0) { Write-Pass "all 6 stale .port files reaped by first invocation" }
else { Write-Fail "$left stale .port files survived cleanup" }
$leftPid = (Get-ChildItem "$psmuxDir\taxstale*.pid" -EA SilentlyContinue).Count
if ($leftPid -eq 0) { Write-Pass "stale .pid sentinels reaped too" }
else { Write-Fail "$leftPid stale .pid files survived cleanup" }

# === TEST 2: latency with stales present stays flat ===
Write-Host "`n[Test 2] new-session / list-sessions fast with 6 stale registries present" -ForegroundColor Yellow
$made = Make-StaleRegistries 6
$sw = [System.Diagnostics.Stopwatch]::StartNew()
& $PSMUX new-session -d -s taxmeas 2>&1 | Out-Null
$sw.Stop()
$tNew = [Math]::Round($sw.Elapsed.TotalMilliseconds)
$sw = [System.Diagnostics.Stopwatch]::StartNew()
& $PSMUX list-sessions 2>&1 | Out-Null
$sw.Stop()
$tLs = [Math]::Round($sw.Elapsed.TotalMilliseconds)
Write-Host "  new-session: ${tNew}ms | list-sessions: ${tLs}ms"
# Pre-fix these were ~2400ms and ~3500ms. Allow generous headroom for CI noise.
if ($tNew -lt 800) { Write-Pass "new-session with stales: ${tNew}ms (< 800ms)" }
else { Write-Fail "new-session with stales too slow: ${tNew}ms (stale-port tax is back)" }
if ($tLs -lt 800) { Write-Pass "list-sessions with stales: ${tLs}ms (< 800ms)" }
else { Write-Fail "list-sessions with stales too slow: ${tLs}ms (stale-port tax is back)" }

# === TEST 3: live session is never reaped by the pid-anchor path ===
Write-Host "`n[Test 3] Live sessions survive repeated cleanup passes" -ForegroundColor Yellow
foreach ($i in 1..6) { & $PSMUX list-sessions 2>&1 | Out-Null }
& $PSMUX has-session -t taxmeas 2>$null
if ($LASTEXITCODE -eq 0) { Write-Pass "live session survived repeated cleanup invocations" }
else { Write-Fail "LIVE SESSION WAS REAPED by pid-anchor cleanup" }
if (Test-Path "$psmuxDir\taxmeas.port") { Write-Pass "live .port intact" }
else { Write-Fail "live .port deleted" }
$echoed = $false
& $PSMUX send-keys -t taxmeas "echo TAX_ALIVE_MARKER" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$cap = & $PSMUX capture-pane -t taxmeas -p 2>&1 | Out-String
if ($cap -match "TAX_ALIVE_MARKER") { Write-Pass "live session pane still functional" }
else { Write-Fail "live session pane not responding after cleanups" }
& $PSMUX kill-session -t taxmeas 2>&1 | Out-Null

# === TEST 4: Win32 TUI visual verification (Layer 2) ===
Write-Host "`n[Test 4] TUI: attached session with stale registries present" -ForegroundColor Yellow
$made = Make-StaleRegistries 3
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s","taxtui" -PassThru
$alive = $false
while ($sw.ElapsedMilliseconds -lt 15000) {
    if (Test-Path "$psmuxDir\taxtui.port") {
        $port = (Get-Content "$psmuxDir\taxtui.port" -Raw -EA SilentlyContinue)
        if ($port -and $port.Trim() -match '^\d+$') {
            try {
                $tcp = [System.Net.Sockets.TcpClient]::new()
                if ($tcp.ConnectAsync("127.0.0.1", [int]$port.Trim()).Wait(100)) { $alive = $true; $tcp.Close(); break }
                $tcp.Close()
            } catch {}
        }
    }
    Start-Sleep -Milliseconds 10
}
$tAttach = $sw.ElapsedMilliseconds
if ($alive -and $tAttach -lt 2500) { Write-Pass "TUI attached session alive in ${tAttach}ms despite stale registries" }
elseif ($alive) { Write-Fail "TUI attached session slow: ${tAttach}ms" }
else { Write-Fail "TUI attached session never came up" }
Start-Sleep -Seconds 1
$sn = (& $PSMUX display-message -t taxtui -p '#{session_name}' 2>&1 | Out-String).Trim()
if ($sn -eq "taxtui") { Write-Pass "TUI session responds to display-message" }
else { Write-Fail "TUI display-message got: $sn" }
& $PSMUX kill-session -t taxtui 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
try { if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } } catch {}

# === TEARDOWN ===
Remove-Item "$psmuxDir\taxstale*.*","$psmuxDir\taxmeas.*","$psmuxDir\taxtui.*" -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
