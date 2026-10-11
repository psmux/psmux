# psmux Environment & env Shim Test Suite
# Tests for:
#   1. set-environment -g propagation to panes (config + runtime)
#   2. show-environment correctness
#      Globals (set-environment -g) are read back with show-environment -g, as in
#      tmux, where a plain listing shows only the session environment (#775).
#   3. env shim function (POSIX `env VAR=val cmd` syntax in PowerShell)
#   4. env-shim on/off config option
#   5. Claude Code-compatible env invocation patterns

$ErrorActionPreference = "Stop"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass { param($msg) Write-Host "[PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail { param($msg) Write-Host "[FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip { param($msg) Write-Host "[SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Info { param($msg) Write-Host "[INFO] $msg" -ForegroundColor Cyan }
function Write-Test { param($msg) Write-Host "[TEST] $msg" -ForegroundColor White }

$PSMUX = "$PSScriptRoot\..\target\release\psmux.exe"
if (-not (Test-Path $PSMUX)) {
    $PSMUX = "$PSScriptRoot\..\target\debug\psmux.exe"
}
if (-not (Test-Path $PSMUX)) {
    $PSMUX = (Get-Command psmux -ErrorAction SilentlyContinue).Source
}
if (-not $PSMUX -or -not (Test-Path $PSMUX)) {
    Write-Error "psmux binary not found. Please build the project first."
    exit 1
}

Write-Info "Using psmux binary: $PSMUX"
Write-Info "Starting environment & env-shim test suite..."
Write-Host ""

$SESSION = "test_env_shim"

function Start-TestSession {
    param(
        [string]$Name = $SESSION,
        [string]$ConfigContent = $null
    )
    # When config content is provided, kill the entire server so the new
    # session starts a fresh server that reads the config file.
    if ($ConfigContent) {
        try { & $PSMUX kill-server 2>&1 | Out-Null } catch {}
        Start-Sleep -Seconds 2
    } else {
        try { & $PSMUX kill-session -t $Name 2>&1 | Out-Null } catch {}
        Start-Sleep -Milliseconds 500
    }

    $args_ = @("new-session", "-s", $Name, "-d")
    if ($ConfigContent) {
        $tmpCfg = [System.IO.Path]::GetTempFileName()
        Set-Content -Path $tmpCfg -Value $ConfigContent -Encoding UTF8
        $env:PSMUX_CONFIG_FILE = $tmpCfg
    }

    $proc = Start-Process -FilePath $PSMUX -ArgumentList $args_ -PassThru -WindowStyle Hidden
    Start-Sleep -Milliseconds 1000

    & $PSMUX has-session -t $Name 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to start test session '$Name'"
    }
    return $proc
}

function Stop-TestSession {
    param([string]$Name = $SESSION)
    try { & $PSMUX kill-server 2>&1 | Out-Null } catch {}
    if ($env:PSMUX_CONFIG_FILE) { Remove-Item $env:PSMUX_CONFIG_FILE -ErrorAction SilentlyContinue; $env:PSMUX_CONFIG_FILE = $null }
    Start-Sleep -Seconds 1
}

# ============================================================
Write-Host "=" * 60
Write-Host "SECTION 1: set-environment & show-environment"
Write-Host "=" * 60
Write-Host ""

# --- Test 1.1: Runtime set-environment -g stores and shows variable ---
Write-Test "1.1 Runtime set-environment -g stores variable"
try {
    $proc = Start-TestSession
    & $PSMUX set-environment -t $SESSION -g CLAUDE_TEST_VAR "hello_world" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    $env_output = & $PSMUX show-environment -g -t $SESSION 2>&1 | Out-String
    if ($env_output -match "CLAUDE_TEST_VAR=hello_world") {
        Write-Pass "Runtime set-environment stored and visible in show-environment"
    } else {
        Write-Fail "CLAUDE_TEST_VAR not found in show-environment output: $env_output"
    }
    Stop-TestSession
} catch {
    Write-Fail "1.1 failed: $_"
    Stop-TestSession
}

# --- Test 1.2: Multiple set-environment variables ---
Write-Test "1.2 Multiple set-environment variables"
try {
    $proc = Start-TestSession
    & $PSMUX set-environment -t $SESSION -g CLAUDECODE "1" 2>&1 | Out-Null
    & $PSMUX set-environment -t $SESSION -g CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS "1" 2>&1 | Out-Null
    & $PSMUX set-environment -t $SESSION -g ANTHROPIC_BASE_URL "https://api.minimax.io/anthropic" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    $env_output = & $PSMUX show-environment -g -t $SESSION 2>&1 | Out-String
    $found = 0
    if ($env_output -match "CLAUDECODE=1") { $found++ }
    if ($env_output -match "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1") { $found++ }
    if ($env_output -match "ANTHROPIC_BASE_URL=https://api.minimax.io/anthropic") { $found++ }
    if ($found -eq 3) {
        Write-Pass "All 3 Claude Code env vars stored correctly"
    } else {
        Write-Fail "Only $found/3 env vars found. Output: $env_output"
    }
    Stop-TestSession
} catch {
    Write-Fail "1.2 failed: $_"
    Stop-TestSession
}

# --- Test 1.3: set-environment from config file ---
Write-Test "1.3 set-environment from config file"
try {
    $config = @"
set-environment -g PSMUX_CFG_TEST_VAR config_value_123
set-environment -g PSMUX_CFG_TEST_VAR2 quoted_value
"@
    $proc = Start-TestSession -ConfigContent $config
    $env_output = & $PSMUX show-environment -g -t $SESSION 2>&1 | Out-String
    $found = 0
    if ($env_output -match "PSMUX_CFG_TEST_VAR=config_value_123") { $found++ }
    if ($env_output -match "PSMUX_CFG_TEST_VAR2=quoted_value") { $found++ }
    if ($found -eq 2) {
        Write-Pass "Config file set-environment propagated correctly ($found/2)"
    } else {
        Write-Fail "Config set-environment: only $found/2 found. Output: $env_output"
    }
    Stop-TestSession
} catch {
    Write-Fail "1.3 failed: $_"
    Stop-TestSession
}

# --- Test 1.4: set-environment vars are inherited by child pane ---
Write-Test "1.4 set-environment vars inherited by child pane (send-keys check)"
try {
    $proc = Start-TestSession
    & $PSMUX set-environment -t $SESSION -g PSMUX_INHERIT_TEST "inherited_ok" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300

    # Create a new pane (split) — it should inherit the env var
    & $PSMUX split-window -t $SESSION -h 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000

    # Send a command to echo the env var via the new pane
    $marker = "ENVCHECK_$(Get-Random)"
    & $PSMUX send-keys -t $SESSION "Write-Host '${marker}:' `$env:PSMUX_INHERIT_TEST" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000

    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:\s*inherited_ok") {
        Write-Pass "New pane inherited PSMUX_INHERIT_TEST=inherited_ok"
    } elseif ($captured -match $marker) {
        Write-Fail "Pane received command but PSMUX_INHERIT_TEST was empty/wrong. Capture: $captured"
    } else {
        Write-Skip "Could not capture pane output (timing issue). Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "1.4 failed: $_"
    Stop-TestSession
}

# --- Test 1.5: Config set-environment vars inherited by child pane ---
Write-Test "1.5 Config set-environment vars inherited by child pane"
try {
    $config = @"
set-environment -g PSMUX_CFG_INHERIT from_config
"@
    $proc = Start-TestSession -ConfigContent $config
    Start-Sleep -Milliseconds 1000

    $marker = "CFGINH_$(Get-Random)"
    & $PSMUX send-keys -t $SESSION "Write-Host '${marker}:' `$env:PSMUX_CFG_INHERIT" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000

    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:\s*from_config") {
        Write-Pass "Config env var inherited by first pane"
    } elseif ($captured -match $marker) {
        Write-Fail "Pane received command but env var was empty/wrong. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    } else {
        Write-Skip "Could not capture output (timing). Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "1.5 failed: $_"
    Stop-TestSession
}

# ============================================================
Write-Host ""
Write-Host "=" * 60
Write-Host "SECTION 2: env SHIM FUNCTION"
Write-Host "=" * 60
Write-Host ""

# --- Test 2.1: env shim is defined in pane (env exists as function) ---
Write-Test "2.1 env shim function exists in pane"
try {
    $proc = Start-TestSession
    Start-Sleep -Milliseconds 1000
    $marker = "ENVDEF_$(Get-Random)"
    & $PSMUX send-keys -t $SESSION "if(Get-Command env -EA 0){Write-Host '${marker}:defined'}else{Write-Host '${marker}:missing'}" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:defined") {
        Write-Pass "env shim function is defined in pane"
    } elseif ($captured -match "${marker}:missing") {
        Write-Fail "env shim function is NOT defined in pane"
    } else {
        Write-Skip "Could not determine env shim status. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "2.1 failed: $_"
    Stop-TestSession
}

# --- Test 2.2: env VAR=val sets variable ---
Write-Test "2.2 env VAR=val sets environment variable"
try {
    $proc = Start-TestSession
    Start-Sleep -Milliseconds 1000
    $marker = "ENVSET_$(Get-Random)"
    & $PSMUX send-keys -t $SESSION "env MY_TEST_VAR=hello_from_env; Write-Host '${marker}:' `$env:MY_TEST_VAR" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:\s*hello_from_env") {
        Write-Pass "env VAR=val correctly set the variable in process"
    } elseif ($captured -match $marker) {
        Write-Fail "env VAR=val did not set the variable. Capture: $($captured.Substring(0, [Math]::Min(300, $captured.Length)))"
    } else {
        Write-Skip "Could not capture output. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "2.2 failed: $_"
    Stop-TestSession
}

# --- Test 2.3: env VAR=val command args (runs command with env vars) ---
Write-Test "2.3 env VAR=val command args (Claude Code pattern)"
try {
    $proc = Start-TestSession
    Start-Sleep -Milliseconds 1000
    $marker = "ENVCMD_$(Get-Random)"
    & $PSMUX send-keys -t $SESSION "env TESTVAR=abc123 pwsh -NoProfile -c 'Write-Host ${marker}:`$env:TESTVAR'" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:abc123") {
        Write-Pass "env VAR=val command correctly passed env to child process"
    } elseif ($captured -match $marker) {
        Write-Fail "env VAR=val command ran but env var was wrong. Capture: $($captured.Substring(0, [Math]::Min(300, $captured.Length)))"
    } else {
        Write-Skip "Could not capture output. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "2.3 failed: $_"
    Stop-TestSession
}

# --- Test 2.4: env with multiple VAR=val pairs ---
Write-Test "2.4 env with multiple VAR=val pairs + command"
try {
    $proc = Start-TestSession
    Start-Sleep -Milliseconds 1000
    $marker = "ENVMULTI_$(Get-Random)"
    & $PSMUX send-keys -t $SESSION "env AA=one BB=two CC=three pwsh -NoProfile -c 'Write-Host ${marker}:AA=`$env:AA+BB=`$env:BB+CC=`$env:CC'" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:AA=one\+BB=two\+CC=three") {
        Write-Pass "Multiple VAR=val pairs correctly passed to child"
    } elseif ($captured -match "${marker}:AA=one") {
        Write-Fail "Only some vars passed. Capture: $($captured.Substring(0, [Math]::Min(300, $captured.Length)))"
    } else {
        Write-Skip "Could not capture output. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "2.4 failed: $_"
    Stop-TestSession
}

# --- Test 2.5: env handles backslash-escaped values (POSIX style) ---
Write-Test "2.5 env with POSIX backslash escapes (https\://...)"
try {
    $proc = Start-TestSession
    Start-Sleep -Milliseconds 1000
    $marker = "ENVESC_$(Get-Random)"
    # Claude Code sends URLs like: ANTHROPIC_BASE_URL=https\://api.example.com
    & $PSMUX send-keys -t $SESSION "env MY_URL='https\://api.example.com/v1' pwsh -NoProfile -c 'Write-Host ${marker}:`$env:MY_URL'" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:https://api\.example\.com/v1") {
        Write-Pass "Backslash-escaped URL correctly unescaped"
    } elseif ($captured -match $marker) {
        Write-Fail "URL not properly unescaped. Capture: $($captured.Substring(0, [Math]::Min(300, $captured.Length)))"
    } else {
        Write-Skip "Could not capture output. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "2.5 failed: $_"
    Stop-TestSession
}

# --- Test 2.6: env with no args lists environment (bare env) ---
Write-Test "2.6 bare env lists environment variables"
try {
    $proc = Start-TestSession
    Start-Sleep -Milliseconds 1000
    $marker = "ENVBARE_$(Get-Random)"
    & $PSMUX send-keys -t $SESSION "Write-Host '${marker}:start'; env | Select-Object -First 3; Write-Host '${marker}:end'" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:start" -and $captured -match "${marker}:end" -and $captured -match "=") {
        Write-Pass "bare env listed environment variables"
    } elseif ($captured -match "${marker}:start") {
        Write-Fail "bare env did not produce output. Capture: $($captured.Substring(0, [Math]::Min(300, $captured.Length)))"
    } else {
        Write-Skip "Could not capture output. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "2.6 failed: $_"
    Stop-TestSession
}

# --- Test 2.7: Full Claude Code agent team spawn pattern ---
Write-Test "2.7 Claude Code agent spawn pattern (env VAR1=val1 VAR2=val2 ... node_cmd)"
try {
    $proc = Start-TestSession
    Start-Sleep -Milliseconds 1000
    $marker = "CLAUDE_$(Get-Random)"
    # Simulate the exact pattern Claude Code uses to spawn agents
    & $PSMUX send-keys -t $SESSION "env CLAUDECODE=1 CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1 ANTHROPIC_BASE_URL='https\://api.minimax.io/anthropic' pwsh -NoProfile -c 'Write-Host ${marker}:CC=`$env:CLAUDECODE+TEAMS=`$env:CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS+URL=`$env:ANTHROPIC_BASE_URL'" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:CC=1\+TEAMS=1\+URL=https://api\.minimax\.io/anthropic") {
        Write-Pass "Full Claude Code agent spawn pattern works"
    } elseif ($captured -match $marker) {
        Write-Fail "Pattern partially worked. Capture: $($captured.Substring(0, [Math]::Min(400, $captured.Length)))"
    } else {
        Write-Skip "Could not capture output. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "2.7 failed: $_"
    Stop-TestSession
}

# ============================================================
Write-Host ""
Write-Host "=" * 60
Write-Host "SECTION 3: env-shim CONFIG OPTION"
Write-Host "=" * 60
Write-Host ""

# --- Test 3.1: env-shim on (default) --- env should exist ---
Write-Test "3.1 env-shim on (default) — env function exists"
try {
    $config = @"
# env-shim defaults to on
"@
    $proc = Start-TestSession -ConfigContent $config
    Start-Sleep -Milliseconds 1000
    $marker = "SHIMON_$(Get-Random)"
    & $PSMUX send-keys -t $SESSION "if(Get-Command env -EA 0){Write-Host '${marker}:yes'}else{Write-Host '${marker}:no'}" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:yes") {
        Write-Pass "env-shim on (default): env function available"
    } elseif ($captured -match "${marker}:no") {
        Write-Fail "env-shim on (default): env function NOT available"
    } else {
        Write-Skip "Could not determine. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "3.1 failed: $_"
    Stop-TestSession
}

# --- Test 3.2: env-shim off — env function should NOT be defined ---
Write-Test "3.2 env-shim off — env function should NOT be defined"
try {
    $config = @"
set -g env-shim off
"@
    $proc = Start-TestSession -ConfigContent $config
    Start-Sleep -Milliseconds 1000
    $marker = "SHIMOFF_$(Get-Random)"
    & $PSMUX send-keys -t $SESSION "if(Get-Command env -EA 0 -Type Function){Write-Host '${marker}:func_exists'}else{Write-Host '${marker}:no_func'}" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:no_func") {
        Write-Pass "env-shim off: no env function defined"
    } elseif ($captured -match "${marker}:func_exists") {
        Write-Fail "env-shim off: env function is still defined (should not be)"
    } else {
        Write-Skip "Could not determine. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "3.2 failed: $_"
    Stop-TestSession
}

# --- Test 3.3: env-shim on explicitly ---
Write-Test "3.3 env-shim on explicitly — env function defined"
try {
    $config = @"
set -g env-shim on
"@
    $proc = Start-TestSession -ConfigContent $config
    Start-Sleep -Milliseconds 1000
    $marker = "SHIMEXP_$(Get-Random)"
    & $PSMUX send-keys -t $SESSION "if(Get-Command env -EA 0){Write-Host '${marker}:yes'}else{Write-Host '${marker}:no'}" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:yes") {
        Write-Pass "env-shim on explicit: env function available"
    } elseif ($captured -match "${marker}:no") {
        Write-Fail "env-shim on explicit: env function NOT available"
    } else {
        Write-Skip "Could not determine. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "3.3 failed: $_"
    Stop-TestSession
}

# --- Test 3.4: env-shim + set-environment together ---
Write-Test "3.4 env-shim + set-environment work together"
try {
    $config = @"
set -g env-shim on
set-environment -g COMBINED_TEST it_works
"@
    $proc = Start-TestSession -ConfigContent $config
    Start-Sleep -Milliseconds 1000
    $marker = "COMBO_$(Get-Random)"
    # Use env shim to set an additional var, then check both
    & $PSMUX send-keys -t $SESSION "env EXTRA_VAR=bonus pwsh -NoProfile -c 'Write-Host ${marker}:COMBINED=`$env:COMBINED_TEST+EXTRA=`$env:EXTRA_VAR'" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:COMBINED=it_works\+EXTRA=bonus") {
        Write-Pass "env-shim + set-environment work together perfectly"
    } elseif ($captured -match "${marker}:COMBINED=it_works") {
        Write-Fail "set-environment worked but env shim did not. Capture: $($captured.Substring(0, [Math]::Min(300, $captured.Length)))"
    } elseif ($captured -match $marker) {
        Write-Fail "Neither worked fully. Capture: $($captured.Substring(0, [Math]::Min(300, $captured.Length)))"
    } else {
        Write-Skip "Could not capture output. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "3.4 failed: $_"
    Stop-TestSession
}

# ============================================================
Write-Host ""
Write-Host "=" * 60
Write-Host "SECTION 4: EDGE CASES & ROBUSTNESS"
Write-Host "=" * 60
Write-Host ""

# --- Test 4.1: env shim with values containing spaces ---
Write-Test "4.1 env with values containing spaces"
try {
    $proc = Start-TestSession
    Start-Sleep -Milliseconds 1000
    $marker = "ENVSP_$(Get-Random)"
    & $PSMUX send-keys -t $SESSION "env SPACE_VAR='hello world' pwsh -NoProfile -c 'Write-Host ${marker}:`$env:SPACE_VAR'" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:hello world") {
        Write-Pass "env handles values with spaces"
    } elseif ($captured -match $marker) {
        Write-Fail "Space handling failed. Capture: $($captured.Substring(0, [Math]::Min(300, $captured.Length)))"
    } else {
        Write-Skip "Could not capture output"
    }
    Stop-TestSession
} catch {
    Write-Fail "4.1 failed: $_"
    Stop-TestSession
}

# --- Test 4.2: env shim survives split-window (new pane gets it too) ---
Write-Test "4.2 env shim available after split-window"
try {
    $proc = Start-TestSession
    & $PSMUX split-window -t $SESSION -h 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $marker = "ENVSPLIT_$(Get-Random)"
    & $PSMUX send-keys -t $SESSION "if(Get-Command env -EA 0){Write-Host '${marker}:defined'}else{Write-Host '${marker}:missing'}" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:defined") {
        Write-Pass "env shim exists in split pane"
    } elseif ($captured -match "${marker}:missing") {
        Write-Fail "env shim missing from split pane"
    } else {
        Write-Skip "Could not determine. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "4.2 failed: $_"
    Stop-TestSession
}

# --- Test 4.3: env shim in a new-window ---
Write-Test "4.3 env shim available in new-window"
try {
    $proc = Start-TestSession
    & $PSMUX new-window -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $marker = "ENVNW_$(Get-Random)"
    & $PSMUX send-keys -t $SESSION "if(Get-Command env -EA 0){Write-Host '${marker}:defined'}else{Write-Host '${marker}:missing'}" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($captured -match "${marker}:defined") {
        Write-Pass "env shim exists in new window"
    } elseif ($captured -match "${marker}:missing") {
        Write-Fail "env shim missing from new window"
    } else {
        Write-Skip "Could not determine. Capture: $($captured.Substring(0, [Math]::Min(200, $captured.Length)))"
    }
    Stop-TestSession
} catch {
    Write-Fail "4.3 failed: $_"
    Stop-TestSession
}

# --- Test 4.4: setenv shorthand works ---
Write-Test "4.4 setenv shorthand alias works"
try {
    $proc = Start-TestSession
    & $PSMUX setenv -t $SESSION -g SHORTHAND_TEST "aliased_ok" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    $env_output = & $PSMUX show-environment -g -t $SESSION 2>&1 | Out-String
    if ($env_output -match "SHORTHAND_TEST=aliased_ok") {
        Write-Pass "setenv shorthand works"
    } else {
        Write-Fail "setenv shorthand did not store variable. Output: $env_output"
    }
    Stop-TestSession
} catch {
    Write-Fail "4.4 failed: $_"
    Stop-TestSession
}

# --- Test 4.5: showenv shorthand works ---
Write-Test "4.5 showenv shorthand alias works"
try {
    $proc = Start-TestSession
    & $PSMUX set-environment -t $SESSION -g SHOW_ALIAS_TEST "visible" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    $env_output = & $PSMUX showenv -g -t $SESSION 2>&1 | Out-String
    if ($env_output -match "SHOW_ALIAS_TEST=visible") {
        Write-Pass "showenv shorthand works"
    } else {
        Write-Fail "showenv shorthand did not return variable. Output: $env_output"
    }
    Stop-TestSession
} catch {
    Write-Fail "4.5 failed: $_"
    Stop-TestSession
}

# ============================================================
# SUMMARY
# ============================================================

Write-Host ""
Write-Host "=" * 60
Write-Host "TEST SUMMARY"
Write-Host "=" * 60
Write-Host "Passed:  $script:TestsPassed" -ForegroundColor Green
Write-Host "Failed:  $script:TestsFailed" -ForegroundColor Red
Write-Host "Skipped: $script:TestsSkipped" -ForegroundColor Yellow
Write-Host ""

$total = $script:TestsPassed + $script:TestsFailed + $script:TestsSkipped
if ($total -gt 0) {
    $passRate = [math]::Round(($script:TestsPassed / $total) * 100, 1)
    Write-Host "Pass Rate: $passRate% ($script:TestsPassed/$total)"
}

if ($script:TestsFailed -gt 0) {
    exit 1
} else {
    exit 0
}
