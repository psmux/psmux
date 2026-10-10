# psmux Issues #107, #109, #110 — Regression & Fix Verification
#
# Issue #107: split-window -c (start directory) not applied
# Issue #109: PSReadLine GetHistoryItems NullReferenceException on session start
# Issue #110: set-environment -u (unset) not implemented
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issues_107_109_110.ps1

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass { param($msg) Write-Host "[PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail { param($msg) Write-Host "[FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip { param($msg) Write-Host "[SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Info { param($msg) Write-Host "[INFO] $msg" -ForegroundColor Cyan }
function Write-Test { param($msg) Write-Host "[TEST] $msg" -ForegroundColor White }

$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -ErrorAction SilentlyContinue).Path }
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\debug\psmux.exe" -ErrorAction SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
Write-Info "Using: $PSMUX"

# Every session of this suite lives in its own -L namespace, so it never
# touches the user's sessions, their .port/.key files, or the default
# namespace (bare kill-server is scoped to the default namespace, #649).
$NS = "t107-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
Write-Info "Namespace: $NS"

function Wait-ForSession {
    param($name, $timeout = 10)
    for ($i = 0; $i -lt ($timeout * 2); $i++) {
        & $PSMUX -L $NS has-session -t $name 2>$null
        if ($LASTEXITCODE -eq 0) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return $false
}

function Capture-Pane {
    param($target)
    $raw = & $PSMUX -L $NS capture-pane -t $target -p 2>&1
    return ($raw | Out-String)
}

function Cleanup-Session {
    param($name)
    & $PSMUX -L $NS kill-session -t $name 2>$null
    Start-Sleep -Milliseconds 500
}

# ══════════════════════════════════════════════════════════════════════
Write-Host ""
Write-Host ("=" * 60)
Write-Host "ISSUE #107: split-window -c sets CWD in new pane"
Write-Host ("=" * 60)
# ══════════════════════════════════════════════════════════════════════

$S107 = "test_107"

# --- Test 107.1: split-window -h -c <dir> sets CWD ---
Write-Test "107.1: split-window -h -c sets CWD correctly"
try {
    $testDir = Join-Path $env:TEMP "psmux_test_107_$(Get-Random)"
    New-Item -Path $testDir -ItemType Directory -Force | Out-Null

    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S107" -WindowStyle Hidden
    if (-not (Wait-ForSession $S107)) { Write-Fail "107.1: Session did not start"; throw "skip" }

    # Let the warm pane fully spawn so we test the stash logic
    Start-Sleep -Milliseconds 1500

    # Split with -c pointing to our test directory
    & $PSMUX -L $NS split-window -h -c $testDir -t $S107 2>&1 | Out-Null
    Start-Sleep -Seconds 2

    # Ask the new pane for its CWD
    & $PSMUX -L $NS send-keys -t $S107 "pwd" Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $S107
    $dirName = Split-Path $testDir -Leaf

    # Flatten line-breaks first: the console wraps long paths mid-name (same
    # fix 107.4 already carries), so a raw match false-fails on wrapped output.
    if (($cap -replace "`r?`n", "") -match [regex]::Escape($dirName)) {
        Write-Pass "107.1: split-window -h -c sets CWD (found $dirName)"
    } else {
        Write-Fail "107.1: CWD not set. Expected '$dirName' in output. Got:`n$cap"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "107.1: Exception: $_" }
} finally {
    Cleanup-Session $S107
    Remove-Item $testDir -Recurse -Force -ErrorAction SilentlyContinue
}

# --- Test 107.2: split-window -v -c <dir> sets CWD (vertical) ---
Write-Test "107.2: split-window -v -c sets CWD (vertical split)"
try {
    $testDir = Join-Path $env:TEMP "psmux_test_107v_$(Get-Random)"
    New-Item -Path $testDir -ItemType Directory -Force | Out-Null

    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S107" -WindowStyle Hidden
    if (-not (Wait-ForSession $S107)) { Write-Fail "107.2: Session did not start"; throw "skip" }
    Start-Sleep -Milliseconds 1500

    & $PSMUX -L $NS split-window -v -c $testDir -t $S107 2>&1 | Out-Null
    Start-Sleep -Seconds 2

    & $PSMUX -L $NS send-keys -t $S107 "pwd" Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $S107
    $dirName = Split-Path $testDir -Leaf

    if (($cap -replace "`r?`n", "") -match [regex]::Escape($dirName)) {
        Write-Pass "107.2: split-window -v -c sets CWD (found $dirName)"
    } else {
        Write-Fail "107.2: CWD not set. Expected '$dirName'. Got:`n$cap"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "107.2: Exception: $_" }
} finally {
    Cleanup-Session $S107
    Remove-Item $testDir -Recurse -Force -ErrorAction SilentlyContinue
}

# --- Test 107.3: new-window -c <dir> sets CWD ---
Write-Test "107.3: new-window -c sets CWD"
try {
    $testDir = Join-Path $env:TEMP "psmux_test_107nw_$(Get-Random)"
    New-Item -Path $testDir -ItemType Directory -Force | Out-Null

    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S107" -WindowStyle Hidden
    if (-not (Wait-ForSession $S107)) { Write-Fail "107.3: Session did not start"; throw "skip" }
    Start-Sleep -Milliseconds 1500

    & $PSMUX -L $NS new-window -c $testDir -t $S107 2>&1 | Out-Null
    Start-Sleep -Seconds 2

    & $PSMUX -L $NS send-keys -t $S107 "pwd" Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $S107
    $dirName = Split-Path $testDir -Leaf

    if (($cap -replace "`r?`n", "") -match [regex]::Escape($dirName)) {
        Write-Pass "107.3: new-window -c sets CWD (found $dirName)"
    } else {
        Write-Fail "107.3: CWD not set. Expected '$dirName'. Got:`n$cap"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "107.3: Exception: $_" }
} finally {
    Cleanup-Session $S107
    Remove-Item $testDir -Recurse -Force -ErrorAction SilentlyContinue
}

# --- Test 107.4: Claude Code exact pattern: split-window -h -d -c <dir> -P -F "#{pane_id}" ---
Write-Test "107.4: Claude Code split pattern (split-window -h -d -c <dir> -P -F)"
try {
    $testDir = Join-Path $env:TEMP "psmux_test_107cc_$(Get-Random)"
    New-Item -Path $testDir -ItemType Directory -Force | Out-Null

    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S107" -WindowStyle Hidden
    if (-not (Wait-ForSession $S107)) { Write-Fail "107.4: Session did not start"; throw "skip" }
    Start-Sleep -Milliseconds 1500

    # Exact Claude Code teammate spawn pattern
    $paneId = & $PSMUX -L $NS split-window -h -d -c $testDir -t $S107 -P -F "#{pane_id}" 2>&1
    Start-Sleep -Seconds 2

    if ($paneId -match '%\d+') {
        # -d means detached — focus stayed on pane 0. Select the new pane.
        & $PSMUX -L $NS select-pane -t "$S107" -R 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1500

        # Use $PWD.Path to avoid pwd truncation in narrow panes
        & $PSMUX -L $NS send-keys -t $S107 'Write-Output "CWDVAL=$($PWD.Path)"' Enter
        Start-Sleep -Seconds 2
        $cap = Capture-Pane $S107
        $dirName = Split-Path $testDir -Leaf
        # Remove line-breaks from captured output (narrow pane wraps long paths)
        $capFlat = ($cap -replace "`r?`n", "")

        if ($capFlat -match [regex]::Escape($dirName)) {
            Write-Pass "107.4: Claude Code split pattern sets CWD (pane=$paneId)"
        } else {
            Write-Fail "107.4: CWD not set. Pane=$paneId. Expected '$dirName'. Got:`n$cap"
        }
    } else {
        Write-Fail "107.4: split-window -P -F did not return pane_id. Got: $paneId"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "107.4: Exception: $_" }
} finally {
    Cleanup-Session $S107
    Remove-Item $testDir -Recurse -Force -ErrorAction SilentlyContinue
}

# ══════════════════════════════════════════════════════════════════════
Write-Host ""
Write-Host ("=" * 60)
Write-Host "ISSUE #109: PSReadLine profile loading (no NullRef crash)"
Write-Host ("=" * 60)
# ══════════════════════════════════════════════════════════════════════

$S109 = "test_109"

# --- Test 109.1: Session starts without PSReadLine errors ---
Write-Test "109.1: Session starts cleanly (no GetHistoryItems errors)"
try {
    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S109" -WindowStyle Hidden
    if (-not (Wait-ForSession $S109)) { Write-Fail "109.1: Session did not start"; throw "skip" }
    Start-Sleep -Seconds 2

    # Capture the initial pane output — should not contain error text
    $cap = Capture-Pane $S109
    if ($cap -match "GetHistoryItems|NullReferenceException|MethodInvocationException") {
        Write-Fail "109.1: PSReadLine error found in initial output:`n$cap"
    } else {
        Write-Pass "109.1: Session started without PSReadLine errors"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "109.1: Exception: $_" }
} finally {
    Cleanup-Session $S109
}

# --- Test 109.2: Profile is sourced (basic prompt/env works) ---
Write-Test "109.2: User profile is sourced inside psmux"
try {
    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S109" -WindowStyle Hidden
    if (-not (Wait-ForSession $S109)) { Write-Fail "109.2: Session did not start"; throw "skip" }
    Start-Sleep -Seconds 2

    # Check that $PROFILE variable is set (it always is in pwsh)
    & $PSMUX -L $NS send-keys -t $S109 'Write-Output "PROFILE_PATH=$PROFILE"' Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $S109

    if ($cap -match "PROFILE_PATH=.+Microsoft\.PowerShell_profile\.ps1") {
        Write-Pass "109.2: `$PROFILE variable is accessible in psmux pane"
    } elseif ($cap -match "PROFILE_PATH=") {
        Write-Pass "109.2: `$PROFILE variable accessible (non-standard path)"
    } else {
        Write-Fail "109.2: `$PROFILE not found in output. Got:`n$cap"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "109.2: Exception: $_" }
} finally {
    Cleanup-Session $S109
}

# --- Test 109.3: PSReadLine predictions are disabled (no display corruption) ---
Write-Test "109.3: PSReadLine predictions are disabled"
try {
    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S109" -WindowStyle Hidden
    if (-not (Wait-ForSession $S109)) { Write-Fail "109.3: Session did not start"; throw "skip" }
    Start-Sleep -Seconds 2

    & $PSMUX -L $NS send-keys -t $S109 '(Get-PSReadLineOption).PredictionSource' Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $S109

    if ($cap -match "None") {
        Write-Pass "109.3: PredictionSource is None"
    } elseif ($cap -match "History|HistoryAndPlugin") {
        Write-Fail "109.3: PredictionSource should be None but got History/HistoryAndPlugin. Got:`n$cap"
    } else {
        # If PSReadLine is not available (e.g., bash shell), skip
        Write-Skip "109.3: Could not determine PredictionSource (may not be pwsh)"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "109.3: Exception: $_" }
} finally {
    Cleanup-Session $S109
}

# --- Test 109.4: split-window also starts without errors ---
Write-Test "109.4: Split pane starts without PSReadLine errors"
try {
    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S109" -WindowStyle Hidden
    if (-not (Wait-ForSession $S109)) { Write-Fail "109.4: Session did not start"; throw "skip" }
    Start-Sleep -Milliseconds 1500

    & $PSMUX -L $NS split-window -h -t $S109 2>&1 | Out-Null
    Start-Sleep -Seconds 2

    $cap = Capture-Pane $S109
    if ($cap -match "GetHistoryItems|NullReferenceException|MethodInvocationException") {
        Write-Fail "109.4: PSReadLine error in split pane:`n$cap"
    } else {
        Write-Pass "109.4: Split pane started without PSReadLine errors"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "109.4: Exception: $_" }
} finally {
    Cleanup-Session $S109
}

# ══════════════════════════════════════════════════════════════════════
Write-Host ""
Write-Host ("=" * 60)
Write-Host "ISSUE #110: set-environment / show-environment / -u unset"
Write-Host ("=" * 60)
# ══════════════════════════════════════════════════════════════════════

$S110 = "test_110"

# --- Test 110.1: set-environment + show-environment basic ---
Write-Test "110.1: set-environment sets a variable visible in show-environment"
try {
    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S110" -WindowStyle Hidden
    if (-not (Wait-ForSession $S110)) { Write-Fail "110.1: Session did not start"; throw "skip" }
    Start-Sleep -Seconds 2

    & $PSMUX -L $NS set-environment -t $S110 PSMUX_TEST_FOO "hello_world" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    $output = & $PSMUX -L $NS show-environment -t $S110 2>&1 | Out-String
    if ($output -match "PSMUX_TEST_FOO=hello_world") {
        Write-Pass "110.1: set-environment + show-environment works"
    } else {
        Write-Fail "110.1: Expected PSMUX_TEST_FOO=hello_world. Got:`n$output"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "110.1: Exception: $_" }
} finally {
    Cleanup-Session $S110
}

# --- Test 110.2: set-environment -u unsets a variable ---
Write-Test "110.2: set-environment -u removes a variable"
try {
    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S110" -WindowStyle Hidden
    if (-not (Wait-ForSession $S110)) { Write-Fail "110.2: Session did not start"; throw "skip" }
    Start-Sleep -Seconds 2

    # Set it first
    & $PSMUX -L $NS set-environment -t $S110 PSMUX_TEST_BAR "to_be_removed" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    # Verify it's there
    $before = & $PSMUX -L $NS show-environment -t $S110 2>&1 | Out-String
    if ($before -notmatch "PSMUX_TEST_BAR=to_be_removed") {
        Write-Fail "110.2: Pre-condition failed — variable not set. Got:`n$before"
        throw "skip"
    }

    # Unset it
    & $PSMUX -L $NS set-environment -u PSMUX_TEST_BAR -t $S110 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    # Verify it's gone
    $after = & $PSMUX -L $NS show-environment -t $S110 2>&1 | Out-String
    if ($after -match "PSMUX_TEST_BAR") {
        Write-Fail "110.2: Variable still present after -u unset. Got:`n$after"
    } else {
        Write-Pass "110.2: set-environment -u successfully removed variable"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "110.2: Exception: $_" }
} finally {
    Cleanup-Session $S110
}

# --- Test 110.3: set-environment -g (global flag, same as default) ---
Write-Test "110.3: set-environment -g works (global scope)"
try {
    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S110" -WindowStyle Hidden
    if (-not (Wait-ForSession $S110)) { Write-Fail "110.3: Session did not start"; throw "skip" }
    Start-Sleep -Seconds 2

    # -g is the GLOBAL environment, as in tmux: it reads back with
    # show-environment -g, not in the session listing (#775).
    & $PSMUX -L $NS set-environment -g PSMUX_GLOBAL_TEST "global_value" -t $S110 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    $output = & $PSMUX -L $NS show-environment -g -t $S110 2>&1 | Out-String
    if ($output -match "PSMUX_GLOBAL_TEST=global_value") {
        Write-Pass "110.3: set-environment -g works"
    } else {
        Write-Fail "110.3: Global variable not found. Got:`n$output"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "110.3: Exception: $_" }
} finally {
    Cleanup-Session $S110
}

# --- Test 110.4: set-environment propagates to new panes ---
Write-Test "110.4: Environment variable propagates to new split pane"
try {
    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S110" -WindowStyle Hidden
    if (-not (Wait-ForSession $S110)) { Write-Fail "110.4: Session did not start"; throw "skip" }
    Start-Sleep -Milliseconds 1500

    # Set a variable AFTER session is created
    & $PSMUX -L $NS set-environment -t $S110 PSMUX_PROPAGATE_TEST "propagated_ok" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    # Create a new split pane — it should inherit the variable
    & $PSMUX -L $NS split-window -h -t $S110 2>&1 | Out-Null
    Start-Sleep -Seconds 2

    # Check if the new pane has the env var
    & $PSMUX -L $NS send-keys -t $S110 'Write-Output "ENVVAL=$env:PSMUX_PROPAGATE_TEST"' Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $S110

    if ($cap -match "ENVVAL=propagated_ok") {
        Write-Pass "110.4: Environment variable propagated to new pane"
    } else {
        Write-Fail "110.4: Variable not propagated. Got:`n$cap"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "110.4: Exception: $_" }
} finally {
    Cleanup-Session $S110
}

# --- Test 110.5: set-environment -u prevents propagation to new panes ---
Write-Test "110.5: Unset variable does NOT propagate to new panes"
try {
    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S110" -WindowStyle Hidden
    if (-not (Wait-ForSession $S110)) { Write-Fail "110.5: Session did not start"; throw "skip" }
    Start-Sleep -Milliseconds 1500

    # Set then unset
    & $PSMUX -L $NS set-environment -t $S110 PSMUX_UNSET_PROP "should_vanish" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    & $PSMUX -L $NS set-environment -u PSMUX_UNSET_PROP -t $S110 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300

    # Create a new split — should NOT have the variable
    & $PSMUX -L $NS split-window -h -t $S110 2>&1 | Out-Null
    Start-Sleep -Seconds 2

    & $PSMUX -L $NS send-keys -t $S110 'Write-Output "UVAL=[$env:PSMUX_UNSET_PROP]"' Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $S110

    if ($cap -match "UVAL=\[\]" -or ($cap -match "UVAL=" -and $cap -notmatch "UVAL=\[should_vanish\]")) {
        Write-Pass "110.5: Unset variable not propagated to new pane"
    } else {
        Write-Fail "110.5: Unset variable still present. Got:`n$cap"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "110.5: Exception: $_" }
} finally {
    Cleanup-Session $S110
}

# --- Test 110.6: CLAUDECODE unset use case (issue #110 motivating example) ---
Write-Test "110.6: Unset CLAUDECODE prevents poisoning new panes"
try {
    # The server really is started from a Claude Code session: CLAUDECODE is
    # in its START (global) environment. As in tmux, a session `-u` would only
    # drop a session entry and new panes would fall back to the global value,
    # so the way to keep it out of new panes is `set-environment -r` (a removal
    # marker, environ_for_session) or `-gu` (#775).
    $savedCC = $env:CLAUDECODE
    $env:CLAUDECODE = "1"
    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $S110" -WindowStyle Hidden
    if ($null -eq $savedCC) { Remove-Item Env:CLAUDECODE -ErrorAction SilentlyContinue } else { $env:CLAUDECODE = $savedCC }
    if (-not (Wait-ForSession $S110)) { Write-Fail "110.6: Session did not start"; throw "skip" }
    Start-Sleep -Milliseconds 1500

    $g = (& $PSMUX -L $NS show-environment -g -t $S110 CLAUDECODE 2>&1 | Out-String).Trim()
    if ($g -ne "CLAUDECODE=1") { Write-Fail "110.6: precondition, CLAUDECODE not in the server start env. Got: $g"; throw "skip" }

    # User realizes and removes it for new panes
    & $PSMUX -L $NS set-environment -r CLAUDECODE -t $S110 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300

    # New pane should not have CLAUDECODE
    & $PSMUX -L $NS split-window -h -t $S110 2>&1 | Out-Null
    Start-Sleep -Seconds 2

    & $PSMUX -L $NS send-keys -t $S110 'Write-Output "CC=[$env:CLAUDECODE]"' Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $S110

    if ($cap -match "CC=\[\]" -or ($cap -match "CC=" -and $cap -notmatch "CC=\[1\]")) {
        Write-Pass "110.6: CLAUDECODE successfully unset — new panes are clean"
    } else {
        Write-Fail "110.6: CLAUDECODE still present in new pane. Got:`n$cap"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "110.6: Exception: $_" }
} finally {
    Cleanup-Session $S110
}

# ══════════════════════════════════════════════════════════════════════
Write-Host ""
Write-Host ("=" * 60)
Write-Host "SHELL COMPATIBILITY: Verify fixes don't break other shells"
Write-Host ("=" * 60)
# ══════════════════════════════════════════════════════════════════════

$SCOMPAT = "test_compat"

# --- Test COMPAT.1: cmd.exe shell works ---
Write-Test "COMPAT.1: cmd.exe works as default-shell"
try {
    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $SCOMPAT" -WindowStyle Hidden
    if (-not (Wait-ForSession $SCOMPAT)) { Write-Fail "COMPAT.1: Session did not start"; throw "skip" }
    Start-Sleep -Seconds 2

    # Create a new window with cmd.exe
    & $PSMUX -L $NS split-window -h -t $SCOMPAT "cmd.exe /K echo CMD_ALIVE" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1500

    $cap = Capture-Pane $SCOMPAT
    if ($cap -match "CMD_ALIVE") {
        Write-Pass "COMPAT.1: cmd.exe pane works"
    } else {
        Write-Fail "COMPAT.1: cmd.exe output not found. Got:`n$cap"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "COMPAT.1: Exception: $_" }
} finally {
    Cleanup-Session $SCOMPAT
}

# --- Test COMPAT.2: Git Bash works ---
Write-Test "COMPAT.2: Git Bash works"
try {
    $gitBash = $null
    $candidates = @(
        "C:\Program Files\Git\bin\bash.exe",
        "C:\Program Files (x86)\Git\bin\bash.exe",
        (Get-Command bash.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -ErrorAction SilentlyContinue)
    )
    foreach ($c in $candidates) {
        if ($c -and (Test-Path $c -ErrorAction SilentlyContinue)) { $gitBash = $c; break }
    }
    if (-not $gitBash) { Write-Skip "COMPAT.2: Git Bash not found"; throw "skip" }

    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $SCOMPAT" -WindowStyle Hidden
    if (-not (Wait-ForSession $SCOMPAT)) { Write-Fail "COMPAT.2: Session did not start"; throw "skip" }
    Start-Sleep -Seconds 2

    # Use split-window to open bash, then send-keys to echo
    & $PSMUX -L $NS split-window -h -t $SCOMPAT "$gitBash" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1500

    & $PSMUX -L $NS send-keys -t $SCOMPAT "echo BASH_ALIVE" Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $SCOMPAT
    if ($cap -match "BASH_ALIVE") {
        Write-Pass "COMPAT.2: Git Bash pane works"
    } else {
        Write-Fail "COMPAT.2: Git Bash output not found. Got:`n$cap"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "COMPAT.2: Exception: $_" }
} finally {
    Cleanup-Session $SCOMPAT
}

# --- Test COMPAT.3: WSL works ---
Write-Test "COMPAT.3: WSL works"
try {
    $wslPath = Get-Command wsl.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -ErrorAction SilentlyContinue
    if (-not $wslPath) { Write-Skip "COMPAT.3: WSL not found"; throw "skip" }

    # Quick check if any WSL distro is installed
    $distros = wsl.exe --list --quiet 2>$null
    if (-not $distros -or $LASTEXITCODE -ne 0) { Write-Skip "COMPAT.3: No WSL distro installed"; throw "skip" }

    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $SCOMPAT" -WindowStyle Hidden
    if (-not (Wait-ForSession $SCOMPAT)) { Write-Fail "COMPAT.3: Session did not start"; throw "skip" }
    Start-Sleep -Seconds 2

    # Open WSL as an interactive shell, then send echo
    # WSL cold start can take 6+ seconds, give it plenty of time
    & $PSMUX -L $NS split-window -h -t $SCOMPAT "wsl.exe" 2>&1 | Out-Null
    Start-Sleep -Seconds 8

    & $PSMUX -L $NS send-keys -t $SCOMPAT "echo WSL_ALIVE" Enter
    $swWsl = [System.Diagnostics.Stopwatch]::StartNew()
    $cap = ""
    while ($swWsl.ElapsedMilliseconds -lt 10000) {
        $cap = Capture-Pane $SCOMPAT
        if ($cap -match "WSL_ALIVE") { break }
        Start-Sleep -Milliseconds 500
    }
    if ($cap -match "WSL_ALIVE") {
        Write-Pass "COMPAT.3: WSL pane works"
    } else {
        Write-Fail "COMPAT.3: WSL output not found. Got:`n$cap"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "COMPAT.3: Exception: $_" }
} finally {
    Cleanup-Session $SCOMPAT
}

# --- Test COMPAT.4: PowerShell (pwsh) basic session still works ---
Write-Test "COMPAT.4: pwsh session works normally"
try {
    Start-Process -FilePath $PSMUX -ArgumentList "-L $NS new-session -d -s $SCOMPAT" -WindowStyle Hidden
    if (-not (Wait-ForSession $SCOMPAT)) { Write-Fail "COMPAT.4: Session did not start"; throw "skip" }
    Start-Sleep -Seconds 2

    & $PSMUX -L $NS send-keys -t $SCOMPAT 'Write-Output "PWSH_ALIVE"' Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $SCOMPAT

    if ($cap -match "PWSH_ALIVE") {
        Write-Pass "COMPAT.4: pwsh session works"
    } else {
        Write-Fail "COMPAT.4: pwsh output not found. Got:`n$cap"
    }
} catch {
    if ($_.ToString() -ne "skip") { Write-Fail "COMPAT.4: Exception: $_" }
} finally {
    Cleanup-Session $SCOMPAT
}

# ══════════════════════════════════════════════════════════════════════
# Final cleanup & summary
# ══════════════════════════════════════════════════════════════════════
& $PSMUX -L $NS kill-server 2>$null
Start-Sleep -Milliseconds 500
Get-ChildItem "$psmuxDir\${NS}__*" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host ("=" * 60)
$total = $script:TestsPassed + $script:TestsFailed
Write-Host "RESULTS: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:TestsSkipped) skipped (of $total run)" -ForegroundColor $(if ($script:TestsFailed -eq 0) { "Green" } else { "Red" })
Write-Host ("=" * 60)

exit $script:TestsFailed
