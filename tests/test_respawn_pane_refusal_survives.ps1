# test_respawn_pane_refusal_survives.ps1
#
# A ROUTINE command refusal must never terminate the psmux server.
#
# MEASURED defect (isolated PSMUX_DATA_DIR, one session, two windows):
#
#   BEFORE: rsp_probe: 2 windows (created Tue Sep  8 03:04:00 2026)
#   --- psmux respawn-pane   (NO -k, pane is LIVE => routine refusal) ---
#   client rc=0 out=[]
#   session alive AFTER routine refusal : False
#   AFTER : psmux: no server running on ...\psmux_rsp_probe
#
# `respawn-pane` on a live pane WITHOUT `-k` is the documented tmux refusal.
# tmux (spawn.c / cmd-respawn-pane.c) answers
#
#   respawn pane failed: pane <session>:<window>.<pane> still active
#
# on stderr and exits 1, and the server carries on. psmux returned that refusal
# as an io::Error and the server event loop applied a bare `?` to it. Because
# `run_server` is `-> io::Result<()>`, the refusal unwound the entire event loop
# and the SERVER EXITED, destroying every window and pane in the session. The
# client had no reply channel on that path, so it printed nothing and exited 0:
# a routine typo silently deleted the user's session and reported success.
#
# What is asserted here, in order:
#   1. the session survives the refusal, with BOTH windows intact
#   2. the refused pane was not touched (same pane id, same pid, still live)
#   3. the client exits NON-ZERO with tmux's wording on stderr
#   4. the server is genuinely HEALTHY afterwards, not merely still running:
#      a follow-up new-window is created and reported back
#   5. the legitimate paths still work: `-k` on a live pane really respawns it
#      (new pid), and a bare `respawn-pane` on a DEAD pane still respawns it
#   6. `respawn-window`, which shares the same code path and shared the same
#      `?`, also survives and reports
#
# SAFETY: fully isolated. Unique -L socket namespace, PSMUX_DATA_DIR pointed at
# a throwaway temp root, sessions killed by name, and any surviving server
# killed only when its ExecutablePath is the binary under test.

$ErrorActionPreference = "Continue"

. (Join-Path $PSScriptRoot 'psmux_test_helpers.ps1')
$PSMUX = Get-PsmuxExe -TestsRoot $PSScriptRoot

$script:TestsPassed = 0; $script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "    $msg" -ForegroundColor DarkGray }

Write-Host "respawn-pane: a routine refusal must not kill the server"
Write-Host "  binary: $PSMUX" -ForegroundColor DarkGray

# ---- isolated data root -----------------------------------------------------
$TAG = [guid]::NewGuid().ToString('N').Substring(0, 8)
$ROOT = Join-Path $env:TEMP "psmux-rsp-$TAG"
New-Item -ItemType Directory -Force -Path $ROOT | Out-Null

$savedData = $env:PSMUX_DATA_DIR
$savedSess = $env:PSMUX_SESSION_NAME
$savedTarget = $env:PSMUX_TARGET_SESSION
$env:PSMUX_DATA_DIR = Join-Path $ROOT "data"
$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_TARGET_SESSION = $null
New-Item -ItemType Directory -Force -Path $env:PSMUX_DATA_DIR | Out-Null

$NS = "rsp$TAG"
$S = "rsp_refusal"

function P { & $PSMUX -L $NS @args 2>&1 }

# Run a psmux command capturing stdout+stderr AND the exit code.
function Run-P {
    param([string[]]$PArgs)
    $out = & $PSMUX -L $NS @PArgs 2>&1 | Out-String
    return [pscustomobject]@{ Code = $LASTEXITCODE; Out = $out.Trim() }
}

function Window-Count {
    $t = (P list-windows -t $S) -join "`n"
    if ($t -match 'no server running' -or $t -match "can't find session") { return -1 }
    return @($t -split "`n" | Where-Object { $_ -match '^\s*\d+:' }).Count
}

function Server-Alive {
    P has-session -t $S 2>&1 | Out-Null
    return ($LASTEXITCODE -eq 0)
}

function Pane-Field($target, $fmt) {
    ((P display-message -p -t $target $fmt) -join '').Trim()
}

# Wait until a pane reports a non-empty pid (a fresh child has really booted).
function Wait-PanePid($target, $timeoutMs = 15000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
        $p = Pane-Field $target '#{pane_pid}'
        if ($p -and $p -match '^\d+$' -and $p -ne '0') { return $p }
        Start-Sleep -Milliseconds 250
    }
    return $null
}

try {
    # ---- setup: one session, two windows ------------------------------------
    P new-session -d -s $S | Out-Null
    Start-Sleep -Seconds 3
    if (-not (Server-Alive)) {
        Write-Fail "setup: session $S did not come up"
        Write-Host ""
        Write-Host "Passed: $script:TestsPassed  Failed: $script:TestsFailed"
        exit 1
    }
    P new-window -t $S | Out-Null
    Start-Sleep -Seconds 2

    $before = Window-Count
    Write-Info "before: $before windows"
    if ($before -ne 2) {
        Write-Fail "setup: expected 2 windows, got $before"
    } else {
        Write-Pass "setup: session $S has 2 windows"
    }

    $paneIdBefore = Pane-Field $S '#{pane_id}'
    $panePidBefore = Wait-PanePid $S
    Write-Info "active pane before: id=$paneIdBefore pid=$panePidBefore"

    # ---- 1..3. the routine refusal ------------------------------------------
    Write-Host ""
    Write-Host "  respawn-pane with NO -k on a LIVE pane (the routine refusal)"
    $r = Run-P @('respawn-pane', '-t', $S)
    Write-Info "client rc=$($r.Code) out=[$($r.Out)]"
    Start-Sleep -Seconds 2

    if (Server-Alive) {
        Write-Pass "the server SURVIVED the refusal"
    } else {
        Write-Fail "the refusal KILLED the server (the whole session is gone)"
    }

    $after = Window-Count
    if ($after -eq 2) {
        Write-Pass "both windows are still present after the refusal ($after)"
    } else {
        Write-Fail "window count changed across the refusal: $before -> $after"
    }

    if ($r.Code -ne 0) {
        Write-Pass "the client exited non-zero ($($r.Code)), like tmux"
    } else {
        Write-Fail "the client exited 0 for a refused command"
    }

    # tmux: "respawn pane failed: pane <session>:<window>.<pane> still active"
    if ($r.Out -match 'respawn pane failed:' -and $r.Out -match 'still active') {
        Write-Pass "the client printed tmux's wording: $($r.Out)"
    } else {
        Write-Fail "expected 'respawn pane failed: ... still active', got [$($r.Out)]"
    }
    if ($r.Out -match [regex]::Escape("$S`:")) {
        Write-Pass "the message names the session:window.pane target"
    } else {
        Write-Fail "the message does not name the target: [$($r.Out)]"
    }

    # ---- 2. the refused pane was not touched --------------------------------
    $paneIdAfter = Pane-Field $S '#{pane_id}'
    $panePidAfter = Pane-Field $S '#{pane_pid}'
    if ($paneIdAfter -eq $paneIdBefore -and $panePidAfter -eq $panePidBefore) {
        Write-Pass "the refused pane is untouched (id $paneIdAfter, pid $panePidAfter)"
    } else {
        Write-Fail "the refusal had side effects: id $paneIdBefore->$paneIdAfter pid $panePidBefore->$panePidAfter"
    }

    # ---- 4. the server is genuinely healthy, not merely alive ---------------
    P new-window -t $S | Out-Null
    Start-Sleep -Seconds 2
    $grown = Window-Count
    if ($grown -eq 3) {
        Write-Pass "a follow-up new-window still works ($grown windows)"
    } else {
        Write-Fail "the server is not healthy after the refusal: expected 3 windows, got $grown"
    }
    $probe = Run-P @('display-message', '-p', '-t', $S, 'healthy')
    if ($probe.Code -eq 0 -and $probe.Out -eq 'healthy') {
        Write-Pass "the server still answers a round-trip request"
    } else {
        Write-Fail "round-trip probe failed: rc=$($probe.Code) out=[$($probe.Out)]"
    }

    # ---- 5a. -k on a live pane still respawns -------------------------------
    Write-Host ""
    Write-Host "  the legitimate paths"
    $pidBeforeKill = Wait-PanePid $S
    $rk = Run-P @('respawn-pane', '-k', '-t', $S)
    Start-Sleep -Seconds 3
    $pidAfterKill = Wait-PanePid $S
    if ($rk.Code -eq 0) {
        Write-Pass "respawn-pane -k on a live pane exits 0"
    } else {
        Write-Fail "respawn-pane -k exited $($rk.Code): [$($rk.Out)]"
    }
    if ($pidAfterKill -and $pidBeforeKill -and $pidAfterKill -ne $pidBeforeKill) {
        Write-Pass "respawn-pane -k really respawned the pane ($pidBeforeKill -> $pidAfterKill)"
    } else {
        Write-Fail "respawn-pane -k did not respawn: pid $pidBeforeKill -> $pidAfterKill"
    }
    if (Server-Alive) {
        Write-Pass "the server survived the successful respawn too"
    } else {
        Write-Fail "the server died on a SUCCESSFUL respawn"
    }

    # ---- 5b. a bare respawn-pane on a DEAD pane still works ------------------
    # Kill the pane's shell from inside so the pane goes dead but stays present
    # (remain-on-exit), then respawn it with no -k, which is the tmux use case.
    P set-option -t $S remain-on-exit on | Out-Null
    Start-Sleep -Milliseconds 500
    P send-keys -t $S 'exit' Enter | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $dead = $false
    while ($sw.ElapsedMilliseconds -lt 15000) {
        $d = Pane-Field $S '#{pane_dead}'
        if ($d -eq '1') { $dead = $true; break }
        Start-Sleep -Milliseconds 300
    }
    if (-not $dead) {
        Write-Info "pane did not report dead within 15s (remain-on-exit unavailable); skipping the dead-pane arm"
    } else {
        $rd = Run-P @('respawn-pane', '-t', $S)
        Start-Sleep -Seconds 3
        if ($rd.Code -eq 0) {
            Write-Pass "a bare respawn-pane on a DEAD pane exits 0"
        } else {
            Write-Fail "bare respawn-pane on a dead pane exited $($rd.Code): [$($rd.Out)]"
        }
        $alive = Pane-Field $S '#{pane_dead}'
        if ($alive -eq '0') {
            Write-Pass "the dead pane came back to life"
        } else {
            Write-Fail "the dead pane is still dead after respawn-pane (#{pane_dead}=$alive)"
        }
    }
    P set-option -t $S remain-on-exit off | Out-Null

    # ---- 6. respawn-window shares the code path and must also survive -------
    Write-Host ""
    Write-Host "  respawn-window (same respawn_active_pane, same old ? )"
    $winBefore = Window-Count
    # Without -k a window with a live pane is a routine refusal, as in tmux
    # ("respawn window failed: window s:i still active"); the server stays.
    $rw = Run-P @('respawn-window', '-t', $S)
    if ($rw.Code -ne 0 -and $rw.Out -match 'still active' -and (Server-Alive)) {
        Write-Pass "respawn-window without -k on a live window is refused and the server survives"
    } else {
        Write-Fail "respawn-window without -k: rc=$($rw.Code) [$($rw.Out)] alive=$(Server-Alive)"
    }
    $rw = Run-P @('respawn-window', '-k', '-t', $S)
    Start-Sleep -Seconds 3
    if (Server-Alive -and (Window-Count) -eq $winBefore) {
        Write-Pass "respawn-window -k kept the server and all $winBefore windows"
    } else {
        Write-Fail "respawn-window -k disturbed the session (alive=$(Server-Alive) windows=$(Window-Count) was $winBefore)"
    }
    if ($rw.Code -eq 0) {
        Write-Pass "respawn-window -k exits 0 on the success path"
    } else {
        Write-Fail "respawn-window -k exited $($rw.Code): [$($rw.Out)]"
    }
}
finally {
    # ---- cleanup ------------------------------------------------------------
    & $PSMUX -L $NS kill-session -t $S 2>&1 | Out-Null
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1200
    Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" -ErrorAction SilentlyContinue |
        Where-Object {
            $_.ExecutablePath -and ($_.ExecutablePath -ieq $PSMUX) -and
            $_.CommandLine -and ($_.CommandLine -match [regex]::Escape($NS))
        } |
        ForEach-Object {
            Write-Info "cleaning up server pid $($_.ProcessId)"
            Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
        }
    Start-Sleep -Milliseconds 800
    Remove-Item -LiteralPath $ROOT -Recurse -Force -EA SilentlyContinue

    $env:PSMUX_DATA_DIR = $savedData
    $env:PSMUX_SESSION_NAME = $savedSess
    $env:PSMUX_TARGET_SESSION = $savedTarget
}

Write-Host ""
Write-Host "Passed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
