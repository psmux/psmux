# ============================================================================
# test_robust_churn.ps1
# psmux EXTREME robustness campaign: RAPID CREATE/DESTROY CHURN
#                                     + RESOURCE-LEAK DETECTION
# ----------------------------------------------------------------------------
# Namespace: rbChurn  (EVERY psmux call passes  -L rbChurn  FIRST)
# Cleanup:   ONLY  & psmux -L rbChurn kill-server   (NEVER global kill,
#            NEVER Get-Process psmux | Stop-Process)
# Namespaced control files live at:
#   $env:USERPROFILE\.psmux\rbChurn__<session>.port   (DOUBLE underscore)
#   $env:USERPROFILE\.psmux\rbChurn__<session>.key
#
# Thesis: repeated lifecycle churn must NOT leak processes, ports, or memory,
#         and must NOT wedge the server. We prove EXPECTED outcomes, not merely
#         "no crash". We churn (create+destroy) and never accumulate; at most a
#         handful of objects are alive at any instant.
# ============================================================================

$ErrorActionPreference = "Continue"

$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass {
    param([string]$Message)
    $script:TestsPassed++
    Write-Host "[PASS] $Message" -ForegroundColor Green
}

function Write-Fail {
    param([string]$Message)
    $script:TestsFailed++
    Write-Host "[FAIL] $Message" -ForegroundColor Red
}

function Write-Info {
    param([string]$Message)
    Write-Host "[INFO] $Message" -ForegroundColor Cyan
}

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

$script:PsmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { Join-Path $env:USERPROFILE ".psmux" }

# Invoke psmux with the rbChurn namespace ALWAYS first. Returns a PSCustomObject
# with ExitCode + Output so callers can assert on exact outcomes.
function Invoke-Psmux {
    # Simple function on purpose: a [Parameter(ValueFromRemainingArguments)]
    # param turns this into an ADVANCED function whose binder eats -d as the
    # common -Debug switch, silently making `new-session -d` an ATTACHING
    # new-session that never returns under output capture. The automatic
    # $args of a simple function passes every token through verbatim.
    $out = & $script:Exe -L rbChurn @args 2>&1
    $code = $LASTEXITCODE
    [PSCustomObject]@{
        ExitCode = $code
        Output   = ($out | Out-String)
    }
}

# Count of live psmux processes of the rbChurn namespace (observation only; we
# NEVER kill these here). Every server of the namespace, the __warm__ standby
# included, runs with `-L rbChurn` on its command line. Counting every
# psmux.exe on the machine instead made the verdict depend on whatever else
# was running: another program's psmux session that started mid run (a
# server and its attached client) read as "process leak (final=2 >
# baseline=0)" although kill-server had ended every rbChurn process.
# PSMUX_TEST_BIN runs the suite against another (renamed) binary.
$script:Exe = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { 'psmux' }
$script:ExeImage = if ($env:PSMUX_TEST_BIN) { [IO.Path]::GetFileName($env:PSMUX_TEST_BIN) } else { 'psmux.exe' }
function Get-PsmuxProcs {
    try {
        $all = Get-CimInstance Win32_Process -Filter "Name='$($script:ExeImage)'" -ErrorAction SilentlyContinue
        return @($all | Where-Object { $_.CommandLine -match '(^|\s)-L\s+"?rbChurn"?(\s|$)' })
    } catch {
        return @()
    }
}
function Get-PsmuxProcCount { return @(Get-PsmuxProcs).Count }
# What a leak verdict names, so a failure says which process stayed.
function Write-ProcList {
    param([string]$Tag)
    foreach ($p in (Get-PsmuxProcs)) {
        Write-Info ("$Tag pid={0} parent={1} cmd={2}" -f $p.ProcessId, $p.ParentProcessId, $p.CommandLine)
    }
}

# Count of rbChurn__*.port control files (the leak signal for ports).
function Get-PortFileCount {
    try {
        if (-not (Test-Path $script:PsmuxDir)) { return 0 }
        $f = Get-ChildItem -Path $script:PsmuxDir -Filter "rbChurn__*.port" -ErrorAction SilentlyContinue
        if ($null -eq $f) { return 0 }
        return @($f).Count
    } catch {
        return 0
    }
}

# has-session predicate: returns $true when the session exists (exit 0).
function Test-HasSession {
    param([string]$Name)
    $r = Invoke-Psmux has-session -t $Name
    return ($r.ExitCode -eq 0)
}

# Read an integer format value (e.g. session_windows / window_panes) for a
# target via display-message -p. Returns -1 on failure to query.
function Get-IntFormat {
    param([string]$Target, [string]$Format)
    $r = Invoke-Psmux display-message -p -t $Target $Format
    if ($r.ExitCode -ne 0) { return -1 }
    $txt = ($r.Output).Trim()
    $val = 0
    if ([int]::TryParse($txt, [ref]$val)) { return $val }
    return -1
}

Write-Host "=== psmux Robustness: CHURN + LEAK DETECTION (namespace rbChurn) ===" -ForegroundColor Yellow

# ---------------------------------------------------------------------------
# Baseline capture BEFORE any creation. This is the reference for the final
# leak verdict.
# ---------------------------------------------------------------------------
# Make sure no stale server from a prior run pollutes the baseline.
Invoke-Psmux kill-server | Out-Null
Start-Sleep -Milliseconds 500

$script:BaselineProcCount = Get-PsmuxProcCount
$script:BaselinePortFiles = Get-PortFileCount
Write-Info "Baseline psmux process count : $script:BaselineProcCount"
Write-Info "Baseline rbChurn__*.port files: $script:BaselinePortFiles"

# Samples collected across scenarios for the monotonic-growth leak check.
$script:ProcSamples = New-Object System.Collections.Generic.List[int]
$script:PortSamples = New-Object System.Collections.Generic.List[int]

try {
    # =======================================================================
    # SCENARIO 1: SESSION CHURN
    # Loop 60x: create rbChurn_c$i (-d), assert has-session==0, kill-session,
    # assert has-session!=0. Sample proc/port counts every 15 iterations.
    # PASS: end port-file count returns to baseline (<=1) and process count
    #       did not grow monotonically.
    # =======================================================================
    Write-Host "--- Scenario 1: SESSION CHURN (60 cycles) ---" -ForegroundColor Yellow

    $sessionChurnOk = $true
    $firstSessionDone = $false

    for ($i = 1; $i -le 60; $i++) {
        $name = "rbChurn_c$i"

        $create = Invoke-Psmux new-session -d -s $name
        if (-not $firstSessionDone) {
            # First server spin-up: generous readiness wait.
            Start-Sleep -Seconds 3
            $firstSessionDone = $true
        } else {
            Start-Sleep -Milliseconds 250
        }

        if ($create.ExitCode -ne 0) {
            $sessionChurnOk = $false
            Write-Info "iter $i create failed (exit $($create.ExitCode)): $($create.Output.Trim())"
        }

        if (-not (Test-HasSession $name)) {
            $sessionChurnOk = $false
            Write-Info "iter $i has-session expected 0 but session absent"
        }

        $kill = Invoke-Psmux kill-session -t $name
        Start-Sleep -Milliseconds 200
        if ($kill.ExitCode -ne 0) {
            $sessionChurnOk = $false
            Write-Info "iter $i kill-session failed (exit $($kill.ExitCode))"
        }

        # has-session must now be nonzero (session gone).
        $still = Invoke-Psmux has-session -t $name
        if ($still.ExitCode -eq 0) {
            $sessionChurnOk = $false
            Write-Info "iter $i session still present after kill"
        }

        if (($i % 15) -eq 0) {
            $pc = Get-PsmuxProcCount
            $fc = Get-PortFileCount
            $script:ProcSamples.Add($pc)
            $script:PortSamples.Add($fc)
            Write-Info "iter $i sample -> procs=$pc portfiles=$fc"
        }
    }

    if ($sessionChurnOk) {
        Write-Pass "Session churn: 60 create/has-session/kill cycles all produced expected exit codes"
    } else {
        Write-Fail "Session churn: one or more cycles produced an unexpected outcome"
    }

    # After session churn, all rbChurn_c* sessions destroyed -> port files for
    # them should be cleaned. Allow <=1 (a lingering single is tolerated; growth
    # is the failure). Server is empty of churned sessions now.
    Start-Sleep -Milliseconds 400
    $postChurnPorts = Get-PortFileCount
    if ($postChurnPorts -le ($script:BaselinePortFiles + 1)) {
        Write-Pass "Session churn: port files returned near baseline (post=$postChurnPorts, base=$script:BaselinePortFiles)"
    } else {
        Write-Fail "Session churn: port files leaked (post=$postChurnPorts, base=$script:BaselinePortFiles)"
    }

    # Monotonic-growth check across the 4 samples (iters 15/30/45/60).
    $procLeak = $false
    if ($script:ProcSamples.Count -ge 2) {
        $strictlyIncreasing = $true
        for ($s = 1; $s -lt $script:ProcSamples.Count; $s++) {
            if ($script:ProcSamples[$s] -le $script:ProcSamples[$s - 1]) {
                $strictlyIncreasing = $false
                break
            }
        }
        $procLeak = $strictlyIncreasing
    }
    if (-not $procLeak) {
        Write-Pass "Session churn: psmux process count did NOT grow monotonically (no process leak)"
    } else {
        Write-Fail "Session churn: psmux process count grew monotonically across samples (process leak)"
    }

    # =======================================================================
    # SCENARIO 2: WINDOW CHURN
    # One session rbChurn_win. Loop 80x: new-window then kill the just-created
    # window. Periodically assert session_windows stays bounded (does not climb
    # to 80). At end exactly the base window(s) remain.
    # =======================================================================
    Write-Host "--- Scenario 2: WINDOW CHURN (80 cycles) ---" -ForegroundColor Yellow

    $winSession = "rbChurn_win"
    $mk = Invoke-Psmux new-session -d -s $winSession
    Start-Sleep -Milliseconds 400
    if ($mk.ExitCode -ne 0 -or -not (Test-HasSession $winSession)) {
        Write-Fail "Window churn: could not create base session $winSession"
    } else {
        Write-Pass "Window churn: base session $winSession created"

        $baseWindows = Get-IntFormat $winSession "#{session_windows}"
        if ($baseWindows -lt 1) { $baseWindows = 1 }
        Write-Info "Window churn: base window count = $baseWindows"

        $winChurnOk = $true
        $winBounded = $true

        for ($i = 1; $i -le 80; $i++) {
            $nw = Invoke-Psmux new-window -t $winSession
            Start-Sleep -Milliseconds 200
            if ($nw.ExitCode -ne 0) {
                $winChurnOk = $false
                Write-Info "win iter $i new-window failed (exit $($nw.ExitCode))"
            }

            # Kill the active (just-created) window. In tmux semantics a fresh
            # new-window becomes current, so kill-window with no -t kills it,
            # but we target the session's active window explicitly.
            $kw = Invoke-Psmux kill-window -t $winSession
            Start-Sleep -Milliseconds 150
            if ($kw.ExitCode -ne 0) {
                $winChurnOk = $false
                Write-Info "win iter $i kill-window failed (exit $($kw.ExitCode))"
            }

            if (($i % 20) -eq 0) {
                $sw = Get-IntFormat $winSession "#{session_windows}"
                Write-Info "win iter $i session_windows=$sw"
                # Bounded: must never approach the loop count. A small ceiling
                # (base + a couple in-flight) is acceptable; climbing toward 80
                # signals an accumulation leak.
                if ($sw -gt ($baseWindows + 5)) {
                    $winBounded = $false
                    Write-Info "win iter $i window count unbounded ($sw)"
                }
            }
        }

        if ($winChurnOk) {
            Write-Pass "Window churn: 80 new-window/kill-window cycles all returned expected exit codes"
        } else {
            Write-Fail "Window churn: one or more new-window/kill-window cycles failed"
        }

        if ($winBounded) {
            Write-Pass "Window churn: session_windows stayed bounded (never climbed toward 80)"
        } else {
            Write-Fail "Window churn: session_windows grew unbounded (window leak)"
        }

        $finalWindows = Get-IntFormat $winSession "#{session_windows}"
        if ($finalWindows -eq $baseWindows) {
            Write-Pass "Window churn: exactly the base window(s) remain (final=$finalWindows, base=$baseWindows)"
        } else {
            Write-Fail "Window churn: residual windows remain (final=$finalWindows, base=$baseWindows)"
        }

        Invoke-Psmux kill-session -t $winSession | Out-Null
        Start-Sleep -Milliseconds 250
    }

    # =======================================================================
    # SCENARIO 3: PANE CHURN
    # One session rbChurn_pane. Loop 60x: split-window -v then kill-pane the
    # new pane. Assert window_panes returns to 1 after each cycle (sample
    # every 10).
    # =======================================================================
    Write-Host "--- Scenario 3: PANE CHURN (60 cycles) ---" -ForegroundColor Yellow

    $paneSession = "rbChurn_pane"
    $mkp = Invoke-Psmux new-session -d -s $paneSession
    Start-Sleep -Milliseconds 400
    if ($mkp.ExitCode -ne 0 -or -not (Test-HasSession $paneSession)) {
        Write-Fail "Pane churn: could not create base session $paneSession"
    } else {
        Write-Pass "Pane churn: base session $paneSession created"

        $paneChurnOk = $true
        $paneReturnsToOne = $true

        for ($i = 1; $i -le 60; $i++) {
            $sp = Invoke-Psmux split-window -v -t $paneSession
            Start-Sleep -Milliseconds 200
            if ($sp.ExitCode -ne 0) {
                $paneChurnOk = $false
                Write-Info "pane iter $i split-window failed (exit $($sp.ExitCode))"
            }

            # Kill the active (newly created by split) pane.
            $kp = Invoke-Psmux kill-pane -t $paneSession
            Start-Sleep -Milliseconds 150
            if ($kp.ExitCode -ne 0) {
                $paneChurnOk = $false
                Write-Info "pane iter $i kill-pane failed (exit $($kp.ExitCode))"
            }

            if (($i % 10) -eq 0) {
                $wp = Get-IntFormat $paneSession "#{window_panes}"
                Write-Info "pane iter $i window_panes=$wp"
                if ($wp -ne 1) {
                    $paneReturnsToOne = $false
                    Write-Info "pane iter $i window_panes expected 1 but got $wp"
                }
            }
        }

        if ($paneChurnOk) {
            Write-Pass "Pane churn: 60 split-window/kill-pane cycles all returned expected exit codes"
        } else {
            Write-Fail "Pane churn: one or more split-window/kill-pane cycles failed"
        }

        if ($paneReturnsToOne) {
            Write-Pass "Pane churn: window_panes returned to 1 at every sampled cycle (no pane leak)"
        } else {
            Write-Fail "Pane churn: window_panes did not return to 1 (pane leak)"
        }

        Invoke-Psmux kill-session -t $paneSession | Out-Null
        Start-Sleep -Milliseconds 250
    }

    # =======================================================================
    # SCENARIO 4: INTERLEAVED CHURN
    # Loop 40x mixing new-session/new-window/split/kill in a deterministic
    # order ($i % N). Keep <=5 sessions alive at any instant by always
    # destroying within the iteration. After the loop, prove the server is
    # alive and a fresh valid session can still be created and queried.
    # =======================================================================
    Write-Host "--- Scenario 4: INTERLEAVED CHURN (40 cycles) ---" -ForegroundColor Yellow

    $interOk = $true
    for ($i = 1; $i -le 40; $i++) {
        $mode = $i % 4
        $sname = "rbChurn_ix$i"

        # Always create a short-lived session, exercise it per mode, destroy it.
        $c = Invoke-Psmux new-session -d -s $sname
        Start-Sleep -Milliseconds 200
        if ($c.ExitCode -ne 0) {
            $interOk = $false
            Write-Info "inter iter $i new-session failed (exit $($c.ExitCode))"
        }

        switch ($mode) {
            0 {
                # new-window then kill-window
                Invoke-Psmux new-window -t $sname | Out-Null
                Start-Sleep -Milliseconds 120
                Invoke-Psmux kill-window -t $sname | Out-Null
            }
            1 {
                # split then kill-pane
                Invoke-Psmux split-window -v -t $sname | Out-Null
                Start-Sleep -Milliseconds 120
                Invoke-Psmux kill-pane -t $sname | Out-Null
            }
            2 {
                # split horizontal then kill-pane
                Invoke-Psmux split-window -h -t $sname | Out-Null
                Start-Sleep -Milliseconds 120
                Invoke-Psmux kill-pane -t $sname | Out-Null
            }
            3 {
                # query only (display-message) to exercise read path
                $q = Invoke-Psmux display-message -p -t $sname "#{session_name}"
                if ($q.ExitCode -ne 0) {
                    $interOk = $false
                    Write-Info "inter iter $i display-message failed"
                }
            }
        }
        Start-Sleep -Milliseconds 100

        # Destroy this iteration's session so we never accumulate.
        Invoke-Psmux kill-session -t $sname | Out-Null
        Start-Sleep -Milliseconds 120

        # Verify it is gone (expected outcome, not just no-crash).
        if (Test-HasSession $sname) {
            $interOk = $false
            Write-Info "inter iter $i session $sname survived kill"
        }
    }

    if ($interOk) {
        Write-Pass "Interleaved churn: 40 mixed-mode create/exercise/destroy cycles all produced expected outcomes"
    } else {
        Write-Fail "Interleaved churn: one or more mixed cycles produced an unexpected outcome"
    }

    # Server liveness + fresh-session proof after all the churn.
    $proveName = "rbChurn_alive"
    $pc = Invoke-Psmux new-session -d -s $proveName
    Start-Sleep -Milliseconds 500
    $aliveByHas = Test-HasSession $proveName
    $nameBack = Invoke-Psmux display-message -p -t $proveName "#{session_name}"
    $nameOk = ($nameBack.ExitCode -eq 0 -and ($nameBack.Output.Trim() -eq $proveName))

    if ($pc.ExitCode -eq 0 -and $aliveByHas -and $nameOk) {
        Write-Pass "Server alive after churn: fresh session '$proveName' created and queried successfully"
    } else {
        Write-Fail "Server wedged after churn: fresh session create/query failed (create=$($pc.ExitCode) has=$aliveByHas nameOk=$nameOk)"
    }

    Invoke-Psmux kill-session -t $proveName | Out-Null
    Start-Sleep -Milliseconds 300

    # =======================================================================
    # FINAL LEAK VERDICT
    # Compare final psmux process count and final rbChurn__*.port file count
    # against the baseline captured at script start. Note: the server process
    # itself is still up here (we kill it in finally), so the only expected
    # delta is the single server process. We tear it down then re-measure for
    # the definitive port-file verdict.
    # =======================================================================
    Write-Host "--- FINAL LEAK VERDICT ---" -ForegroundColor Yellow

    $preTeardownProcs = Get-PsmuxProcCount
    $preTeardownPorts = Get-PortFileCount
    Write-Info "Pre-teardown (server still up): procs=$preTeardownProcs portfiles=$preTeardownPorts"

    # Tear down the server to release its port files / process, then measure.
    Invoke-Psmux kill-server | Out-Null
    Start-Sleep -Milliseconds 800

    $finalProcs = Get-PsmuxProcCount
    $finalPorts = Get-PortFileCount
    Write-Info "Post-teardown: procs=$finalProcs (baseline=$script:BaselineProcCount)"
    Write-Info "Post-teardown: portfiles=$finalPorts (baseline=$script:BaselinePortFiles)"

    # Process-leak verdict: after kill-server, live psmux count must return to
    # baseline (no orphaned server/children left behind).
    if ($finalProcs -le $script:BaselineProcCount) {
        Write-Pass "LEAK VERDICT: no process leak (final=$finalProcs <= baseline=$script:BaselineProcCount)"
    } else {
        Write-Fail "LEAK VERDICT: process leak detected (final=$finalProcs > baseline=$script:BaselineProcCount)"
        Write-ProcList 'left after kill-server:'
    }

    # Port-file-leak verdict: after kill-server, rbChurn__*.port files must
    # return to baseline (no orphaned control files for churned objects).
    if ($finalPorts -le $script:BaselinePortFiles) {
        Write-Pass "LEAK VERDICT: no port-file leak (final=$finalPorts <= baseline=$script:BaselinePortFiles)"
    } else {
        Write-Fail "LEAK VERDICT: port-file leak detected (final=$finalPorts > baseline=$script:BaselinePortFiles)"
    }

} finally {
    # ---------------------------------------------------------------------
    # MANDATORY cleanup: only ever the namespaced server. A mid-loop failure
    # still lands here and tears the server down.
    # ---------------------------------------------------------------------
    Write-Host "--- Cleanup: kill rbChurn server ---" -ForegroundColor Yellow
    & $script:Exe -L rbChurn kill-server 2>&1 | Out-Null
}

Write-Host ""
Write-Host "=== Results ===" -ForegroundColor Yellow
Write-Host "Passed: $script:TestsPassed" -ForegroundColor Green
Write-Host "Failed: $script:TestsFailed" -ForegroundColor Red

exit $script:TestsFailed
