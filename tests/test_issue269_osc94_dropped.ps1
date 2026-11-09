# Issue #269 - E2E PROOF OF FIX: OSC 9;4 (Windows Terminal progress
# indicator) sequences emitted from inside a psmux pane now surface as the
# `host_progress` field in dump-state JSON, so the client can re-emit them
# to the host terminal.
#
# Strategy:
#   1. Baseline: confirm host_title (issue #268) still works (sanity).
#   2. Drive each of the five OSC 9;4 states from a pane and assert that
#      `"host_progress":"<state>;<value>"` appears in dump-state with the
#      exact value emitted.
#   3. Verify the literal '9;4' bytes are NOT echoed into pane content
#      (still consumed by the emulator state machine).
#   4. Verify successive sequences overwrite the value rather than stacking.
#   5. Confirm the field disappears from dump-state for a fresh session
#      that has not received any OSC 9;4 (Option<None> serialization).
#
# This test FAILS on the buggy build and PASSES on the fixed build.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$SESSION = "issue269_e2e"
$script:Pass = 0
$script:Fail = 0

function Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function FailX($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Info($m)  { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

function Send-Tcp {
    param([string]$Cmd)
    $port = (Get-Content "$psmuxDir\$SESSION.port" -Raw).Trim()
    $key  = (Get-Content "$psmuxDir\$SESSION.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream()
    $w = [System.IO.StreamWriter]::new($stream)
    $r = [System.IO.StreamReader]::new($stream)
    $w.Write("AUTH $key`n"); $w.Flush()
    $null = $r.ReadLine()
    $w.Write("$Cmd`n"); $w.Flush()
    $stream.ReadTimeout = 5000
    $sb = [System.Text.StringBuilder]::new()
    try {
        while ($true) {
            $line = $r.ReadLine()
            if ($null -eq $line) { break }
            [void]$sb.AppendLine($line)
            if ($line.EndsWith('}') -and $sb.Length -gt 50) { break }
        }
    } catch {}
    $tcp.Close()
    return $sb.ToString().Trim()
}

function Write-OscScript {
    param(
        [string]$ScriptPath,
        [byte[]]$Bytes,
        [string]$Marker
    )
    $hex = ($Bytes | ForEach-Object { ('0x{0:X2}' -f $_) }) -join ', '
    @"
`$bytes = [byte[]]@($hex)
`$out = [Console]::OpenStandardOutput()
`$out.Write(`$bytes, 0, `$bytes.Length)
`$out.Flush()
Write-Host "$Marker"
"@ | Set-Content -Path $ScriptPath -Encoding UTF8
}

function Emit-Osc94 {
    param([int]$State, [int]$Progress, [string]$Marker)
    $stateAscii = [System.Text.Encoding]::ASCII.GetBytes("$State")
    $progAscii = [System.Text.Encoding]::ASCII.GetBytes("$Progress")
    $bytes = [byte[]]@(0x1B,0x5D,0x39,0x3B,0x34,0x3B) + $stateAscii + [byte[]]@(0x3B) + $progAscii + [byte[]]@(0x1B,0x5C)
    $script = "$env:TEMP\osc94_${State}_${Progress}.ps1"
    Write-OscScript -ScriptPath $script -Bytes $bytes -Marker $Marker
    & $PSMUX send-keys -t $SESSION 'cls' Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    & $PSMUX send-keys -t $SESSION (". '" + $script + "'") Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    Remove-Item $script -Force -EA SilentlyContinue
}

Cleanup
Write-Host "`n=== Issue #269 E2E PROOF OF FIX: OSC 9;4 forwarded ===" -ForegroundColor Cyan

& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 3
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { FailX "Session failed to start"; exit 2 }
Pass "Session started"

# =============================================================================
# Test 1: BASELINE - dump-state has NO host_progress field on a fresh
#         session that has emitted no OSC 9;4. Confirms the field is
#         conditional, not always-present.
# =============================================================================
Write-Host "`n[Test 1] Fresh session: host_progress absent" -ForegroundColor Yellow
$dumpFresh = Send-Tcp "dump-state"
if ($dumpFresh -match '"host_progress"') {
    FailX "host_progress present on fresh session - should only appear after OSC 9;4"
} else {
    Pass "host_progress absent on fresh session (expected)"
}

# =============================================================================
# Test 2: BASELINE - host_title still works (regression guard for #268).
# =============================================================================
Write-Host "`n[Test 2] Regression: host_title (#268) still works" -ForegroundColor Yellow
& $PSMUX set-option -g set-titles on 2>&1 | Out-Null
& $PSMUX set-option -g set-titles-string '#S/#W' 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$dumpTitle = Send-Tcp "dump-state"
if ($dumpTitle -match '"host_title"\s*:') {
    Pass "host_title still emitted (issue #268 fix intact)"
} else {
    FailX "host_title missing - #268 regression"
}

# =============================================================================
# Test 3: FIX - OSC 9;4 with each state surfaces as host_progress in dump-state.
# =============================================================================
Write-Host "`n[Test 3] OSC 9;4 round-trip: each state surfaces correctly" -ForegroundColor Yellow

$cases = @(
    @{State=1; Progress=50; Label='default 50%'; Marker='OSC94_DEFAULT_50'}
    @{State=2; Progress=75; Label='error 75%';   Marker='OSC94_ERROR_75'}
    # Indeterminate keeps the last value, as tmux's screen_set_progress_bar does.
    @{State=3; Progress=0;  Shown=75; Label='indeterminate'; Marker='OSC94_INDET'}
    @{State=4; Progress=90; Label='warning 90%';  Marker='OSC94_WARN_90'}
    @{State=0; Progress=0;  Label='hide';          Marker='OSC94_HIDE'}
)

foreach ($c in $cases) {
    Emit-Osc94 -State $c.State -Progress $c.Progress -Marker $c.Marker

    # Verify the marker appeared (script ran to completion)
    $cap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($cap -notmatch [regex]::Escape($c.Marker)) {
        FailX "$($c.Label): script did not complete (marker missing)"
        continue
    }
    if ($cap -match '9;4') {
        FailX "$($c.Label): raw '9;4' leaked into pane content"
        continue
    }

    $dump = Send-Tcp "dump-state"
    $shown = if ($null -ne $c.Shown) { $c.Shown } else { $c.Progress }
    $expected = '"host_progress"\s*:\s*"' + [regex]::Escape("$($c.State);$shown") + '"'
    if ($dump -match $expected) {
        Pass "$($c.Label): host_progress=`"$($c.State);$shown`" in dump-state"
    } else {
        FailX "$($c.Label): expected host_progress=`"$($c.State);$shown`" not found in dump-state"
        if ($dump -match '"host_progress"\s*:\s*"([^"]*)"') {
            Info "actual host_progress = '$($matches[1])'"
        } else {
            Info "host_progress field not present at all"
        }
    }
}

# =============================================================================
# Test 4: FIX - Successive OSC 9;4 sequences overwrite (final state wins).
# =============================================================================
Write-Host "`n[Test 4] Successive sequences: final state wins" -ForegroundColor Yellow

# Build a script that emits multiple OSC 9;4 sequences in one run; the final
# value should be what dump-state reports.
$multiBytes = @()
$states = @(@{S=1; P=10}, @{S=1; P=50}, @{S=2; P=80}, @{S=4; P=99})
foreach ($st in $states) {
    $sa = [System.Text.Encoding]::ASCII.GetBytes("$($st.S)")
    $pa = [System.Text.Encoding]::ASCII.GetBytes("$($st.P)")
    $seq = [byte[]]@(0x1B,0x5D,0x39,0x3B,0x34,0x3B) + $sa + [byte[]]@(0x3B) + $pa + [byte[]]@(0x1B,0x5C)
    $multiBytes += $seq
}
$multiScript = "$env:TEMP\osc94_multi.ps1"
Write-OscScript -ScriptPath $multiScript -Bytes ([byte[]]$multiBytes) -Marker "MULTI_DONE"

& $PSMUX send-keys -t $SESSION 'cls' Enter 2>&1 | Out-Null
Start-Sleep -Milliseconds 600
& $PSMUX send-keys -t $SESSION (". '" + $multiScript + "'") Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2

$dumpMulti = Send-Tcp "dump-state"
if ($dumpMulti -match '"host_progress"\s*:\s*"4;99"') {
    Pass "Final state (4;99) wins after barrage of 4 sequences"
} else {
    if ($dumpMulti -match '"host_progress"\s*:\s*"([^"]*)"') {
        FailX "Expected final host_progress=4;99, got '$($matches[1])'"
    } else {
        FailX "host_progress missing after multi-sequence script"
    }
}
Remove-Item $multiScript -Force -EA SilentlyContinue

# =============================================================================
# Test 5: FIX - host_progress JSON is well-formed (parses cleanly).
# =============================================================================
Write-Host "`n[Test 5] host_progress is valid JSON in dump-state" -ForegroundColor Yellow
Emit-Osc94 -State 1 -Progress 42 -Marker "JSON_PROOF"
Start-Sleep -Milliseconds 500
$dumpJson = Send-Tcp "dump-state"
try {
    $parsed = $dumpJson | ConvertFrom-Json
    if ($parsed.host_progress -eq "1;42") {
        Pass "dump-state parses as JSON; host_progress='1;42' (round-trip exact)"
    } else {
        FailX "JSON parsed but host_progress mismatch: '$($parsed.host_progress)'"
    }
} catch {
    FailX "dump-state did not parse as JSON after host_progress emission: $_"
}

# =============================================================================
# Test 6: FIX - BEL-terminated OSC 9;4 also works (alternate ST form).
# =============================================================================
Write-Host "`n[Test 6] BEL-terminated OSC 9;4 also surfaces" -ForegroundColor Yellow
$belBytes = [byte[]]@(0x1B,0x5D,0x39,0x3B,0x34,0x3B,0x31,0x3B,0x33,0x33,0x07)  # OSC 9;4;1;33 BEL
$belScript = "$env:TEMP\osc94_bel.ps1"
Write-OscScript -ScriptPath $belScript -Bytes $belBytes -Marker "BEL_DONE"
& $PSMUX send-keys -t $SESSION 'cls' Enter 2>&1 | Out-Null
Start-Sleep -Milliseconds 600
& $PSMUX send-keys -t $SESSION (". '" + $belScript + "'") Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2

$dumpBel = Send-Tcp "dump-state"
if ($dumpBel -match '"host_progress"\s*:\s*"1;33"') {
    Pass "BEL-terminated OSC 9;4 surfaces as host_progress=1;33"
} else {
    if ($dumpBel -match '"host_progress"\s*:\s*"([^"]*)"') {
        FailX "BEL-terminated: expected 1;33, got '$($matches[1])'"
    } else {
        FailX "BEL-terminated OSC 9;4 did not surface at all"
    }
}
Remove-Item $belScript -Force -EA SilentlyContinue

# =============================================================================
# Test 7: FIX - Source code now contains the OSC 9;4 handler.
# =============================================================================
Write-Host "`n[Test 7] Source code contains OSC 9;4 handler" -ForegroundColor Yellow
$repoRoot = (Resolve-Path "$PSScriptRoot\..").Path

$serverHits = Select-String -Path "$repoRoot\src\server\mod.rs", "$repoRoot\src\server\helpers.rs" -Pattern 'host_progress|active_pane_progress' -EA SilentlyContinue
if ($serverHits.Count -ge 2) {
    Pass "Server-side host_progress emission present ($($serverHits.Count) refs)"
} else {
    FailX "Server-side host_progress not present"
}

$clientHits = Select-String -Path "$repoRoot\src\client.rs" -Pattern 'host_progress|last_emitted_host_progress' -EA SilentlyContinue
if ($clientHits.Count -ge 2) {
    Pass "Client-side host_progress emission present ($($clientHits.Count) refs)"
} else {
    FailX "Client-side host_progress not present"
}

$vtHits = Select-String -Path "$repoRoot\crates\vt100-psmux\src\perform.rs", "$repoRoot\crates\vt100-psmux\src\screen.rs" -Pattern 'osc94_progress|set_progress|b"4"' -EA SilentlyContinue
if ($vtHits.Count -ge 2) {
    Pass "vt100-psmux OSC 9;4 dispatch arm present ($($vtHits.Count) refs)"
} else {
    FailX "vt100-psmux OSC 9;4 handler not present"
}

Cleanup
Write-Host "`n=== Result ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:Pass)" -ForegroundColor Green
Write-Host "  Failed: $($script:Fail)" -ForegroundColor $(if ($script:Fail -gt 0) { "Red" } else { "Green" })
if ($script:Fail -eq 0) {
    Write-Host "`n  *** Bug #269 FIXED: OSC 9;4 round-trips through psmux end-to-end. ***" -ForegroundColor Yellow
}
exit $script:Fail
