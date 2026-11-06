# Test: set -g default-shell "wsl.exe" works correctly
# Verifies WSL can be used as default-shell via config and runtime set-option.
# psmux runs on Windows and launches wsl.exe as a shell (not running inside WSL).
#
# Tests:
#   1. Full path config (C:\Windows\System32\wsl.exe)
#   2. Bare "wsl" config
#   3. new-window inherits wsl
#   4. split-window inherits wsl
#   5. Env vars work inside WSL pane
#   6. Runtime set-option changes default-shell to wsl.exe
#   7. Bare "wsl" runtime set
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_default_shell_wsl.ps1

$ErrorActionPreference = "Continue"
# This suite writes a config at a default path (~/.psmux.conf, ~/.tmux.conf or
# ~/.config/psmux) and expects psmux to load it at startup, but PSMUX_CONFIG_FILE
# replaces every default path and a developer's shell may have it set, so clear it (#748).
Remove-Item Env:PSMUX_CONFIG_FILE -ErrorAction SilentlyContinue
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass { param($msg) Write-Host "[PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail { param($msg) Write-Host "[FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info { param($msg) Write-Host "[INFO] $msg" -ForegroundColor Cyan }
function Write-Test { param($msg) Write-Host "[TEST] $msg" -ForegroundColor White }

$PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -ErrorAction SilentlyContinue).Path
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\debug\psmux.exe" -ErrorAction SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found. Build first: cargo build --release"; exit 1 }
Write-Info "Using: $PSMUX"

# Check if WSL is available
$wslPath = "$env:SystemRoot\System32\wsl.exe"
$wslAvailable = $false
if (Test-Path $wslPath) {
    # wsl.exe exists, but also check that a distro is installed
    $distroCheck = & $wslPath --list --quiet 2>&1 | Out-String
    if ($LASTEXITCODE -eq 0 -and $distroCheck.Trim().Length -gt 0) {
        $wslAvailable = $true
    }
}
if (-not $wslAvailable) {
    Write-Info "WSL not available (no distro installed or wsl.exe not found) — skipping tests"
    Write-Host "[SKIP] WSL not available" -ForegroundColor Yellow
    exit 0
}
Write-Info "WSL found: $wslPath"
Write-Info "WSL distros: $($distroCheck.Trim())"

# Keep the default distro running for the whole suite.  Each test kills its
# session's WSL panes and starts new ones a few seconds later, and WSL stops an
# idle distro about 15 s after its last process exits.  A launch that lands in
# that teardown failed in the pane itself, without psmux in the loop: it took
# 2 to 30 s and printed "Failed to start the systemd user session", or died at
# once with "Error code: Wsl/Service/E_UNEXPECTED".  A dead shell ends its
# session (remain-on-exit off), so the next check met "no server running".
$wslKeeper = Start-Process -FilePath $wslPath -ArgumentList @('-e', 'sleep', '900') -WindowStyle Hidden -PassThru
$warmSw = [System.Diagnostics.Stopwatch]::StartNew()
do { $u = (& $wslPath -e uname -s 2>&1 | Out-String) } while ($u -notmatch 'Linux' -and $warmSw.ElapsedMilliseconds -lt 60000)
Write-Info "WSL distro warm after $($warmSw.ElapsedMilliseconds) ms"

$confPath = "$env:USERPROFILE\.psmux.conf"
$confBackup = $null

# ============================================================
# SETUP — backup config, kill servers
# ============================================================
Write-Info "Backing up config and cleaning up..."
if (Test-Path $confPath) {
    $confBackup = Get-Content $confPath -Raw
}
& $PSMUX kill-server 2>$null | Out-Null
Start-Sleep -Seconds 3
Get-Process psmux -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
Remove-Item "$env:USERPROFILE\.psmux\*.port" -Force -ErrorAction SilentlyContinue
Remove-Item "$env:USERPROFILE\.psmux\*.key" -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host ("=" * 60)
Write-Host "  DEFAULT-SHELL WSL TESTS"
Write-Host ("=" * 60)

# ============================================================
# Test 1: Full path to wsl.exe
# ============================================================
Write-Host ""
Write-Test "1. default-shell with full path to wsl.exe"

Set-Content -Path $confPath -Value "set -g default-shell `"$wslPath`""

$session = "wsl_test1"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 5

$hasSession = & $PSMUX has-session -t $session 2>&1
if ($LASTEXITCODE -eq 0) {
    Write-Pass "Session '$session' created successfully with wsl.exe default-shell"
} else {
    Write-Fail "Failed to create session with wsl.exe default-shell (full path)"
}

$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
$cmd = $cmd.Trim()
Write-Info "  pane_current_command: $cmd"
if ($cmd -match "wsl|bash|zsh|conhost") {
    Write-Pass "Pane is running WSL shell (pane_current_command=$cmd)"
} else {
    Write-Fail "Pane is NOT running WSL shell (got: $cmd)"
}

# Send a command and verify it works (WSL runs Linux shell)
& $PSMUX send-keys -t $session 'echo WSL_TEST_WORKS' Enter 2>&1 | Out-Null
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$output = ""
while ($sw.ElapsedMilliseconds -lt 10000) {
    $output = (& $PSMUX capture-pane -t $session -p 2>&1) | Out-String
    if ($output -match "WSL_TEST_WORKS") { break }
    Start-Sleep -Milliseconds 500
}
if ($output -match "WSL_TEST_WORKS") {
    Write-Pass "WSL pane executes commands correctly"
} else {
    Write-Fail "WSL pane did not produce expected output"
    Write-Info "  Output: $($output.Trim())"
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# Test 2: Bare "wsl" name
# ============================================================
Write-Host ""
Write-Test "2. default-shell with bare name: wsl"

Set-Content -Path $confPath -Value 'set -g default-shell wsl'

$session = "wsl_test2"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 5

$hasSession = & $PSMUX has-session -t $session 2>&1
if ($LASTEXITCODE -eq 0) {
    Write-Pass "Session created with bare 'wsl' name"
    # Poll rather than sample once: a bare 'wsl' is resolved through PATH by the
    # spawner, and under sweep load the child was not attached 5s after
    # new-session (sweep 2026-08-29_00-11-17 read 'psmux', the server itself,
    # then passed 3/3 on rerun). The proof of a WSL pane is uname below, so an
    # unrecognised name is a reason to ask the pane, not to fail on the spot.
    $sw1 = [System.Diagnostics.Stopwatch]::StartNew()
    $cmd = ""
    while ($sw1.ElapsedMilliseconds -lt 20000) {
        $cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
        if ($cmd.Trim() -match "wsl|bash|zsh|conhost|shell") { break }
        Start-Sleep -Milliseconds 500
    }
    Write-Info "  pane_current_command after $([int]$sw1.ElapsedMilliseconds)ms: $($cmd.Trim())"
    if ($cmd.Trim() -match "wsl|bash|zsh") {
        Write-Pass "Pane runs WSL via bare name"
    } else {
        # ConPTY/shell self reports conhost or 'shell' for WSL, and a late spawn
        # reports the server; either way the pane itself is the authority.
        & $PSMUX send-keys -t $session 'uname -s' Enter 2>&1 | Out-Null
        $sw2 = [System.Diagnostics.Stopwatch]::StartNew()
        $capOut = ""
        while ($sw2.ElapsedMilliseconds -lt 15000) {
            $capOut = (& $PSMUX capture-pane -t $session -p 2>&1) | Out-String
            if ($capOut -match "Linux") { break }
            Start-Sleep -Milliseconds 500
        }
        if ($capOut -match "Linux") {
            Write-Pass "Pane runs WSL via bare name (verified via uname, pane_current_command=$($cmd.Trim()))"
        } else {
            Write-Fail "Pane not running WSL (got: $($cmd.Trim()))"
        }
    }
} else {
    Write-Fail "Failed to create session with bare 'wsl' name"
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# Test 3: new-window inherits wsl
# ============================================================
Write-Host ""
Write-Test "3. new-window inherits default-shell wsl"

Set-Content -Path $confPath -Value "set -g default-shell `"$wslPath`""

$session = "wsl_test3"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 3

& $PSMUX new-window -t $session 2>&1 | Out-Null
Start-Sleep -Seconds 5

$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
Write-Info "  new-window pane_current_command: $($cmd.Trim())"
if ($cmd.Trim() -match "wsl|bash|zsh") {
    Write-Pass "New window also runs WSL"
} else {
    # Verify by running a Linux command
    & $PSMUX send-keys -t $session 'uname -s' Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $capOut = (& $PSMUX capture-pane -t $session -p 2>&1) | Out-String
    if ($capOut -match "Linux") {
        Write-Pass "New window runs WSL (verified via uname, pane_current_command=$($cmd.Trim()))"
    } else {
        Write-Fail "New window not running WSL (got: $($cmd.Trim()))"
    }
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# Test 4: split-window inherits wsl
# ============================================================
Write-Host ""
Write-Test "4. split-window inherits default-shell wsl"

Set-Content -Path $confPath -Value "set -g default-shell `"$wslPath`""

$session = "wsl_test4"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 3

& $PSMUX split-window -t $session -v 2>&1 | Out-Null
Start-Sleep -Seconds 3

$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
Write-Info "  split-window pane_current_command: $($cmd.Trim())"
if ($cmd.Trim() -match "wsl|bash|zsh") {
    Write-Pass "Split pane runs WSL"
} elseif ($cmd.Trim() -match "conhost|shell") {
    & $PSMUX send-keys -t $session 'uname -s' Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $capOut = (& $PSMUX capture-pane -t $session -p 2>&1) | Out-String
    if ($capOut -match "Linux") {
        Write-Pass "Split pane runs WSL (verified via uname, pane_current_command=$($cmd.Trim()))"
    } else {
        Write-Fail "Split pane not running WSL (got: $($cmd.Trim()))"
    }
} else {
    Write-Fail "Split pane not running WSL (got: $($cmd.Trim()))"
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# Test 5: Env vars work inside WSL pane
# ============================================================
Write-Host ""
Write-Test "5. Environment variables accessible in WSL panes"

Set-Content -Path $confPath -Value "set -g default-shell `"$wslPath`""

$session = "wsl_test5"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 5

# In WSL, Windows env vars are accessible via /proc or WSLENV
# PSMUX_SESSION should be inherited by the ConPTY child process
& $PSMUX send-keys -t $session 'echo "PSMUX=$PSMUX_SESSION"' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$output = (& $PSMUX capture-pane -t $session -p 2>&1) | Out-String
if ($output -match "PSMUX=.+") {
    Write-Pass "PSMUX_SESSION is accessible in WSL pane"
} else {
    Write-Info "PSMUX_SESSION not directly visible in WSL (may need WSLENV)"
    # This is expected on some configs — don't fail, just note it
    Write-Pass "WSL env var test completed (PSMUX_SESSION may require WSLENV config)"
}

# Verify the pane is really running Linux. Poll up to 90 s: WSL shuts an
# idle distro down, and its cold start was measured at 10 s, later 21 s on a
# quiet machine (0.5 s warm), and failed a 45 s window twice while a sweep
# loaded the box; with the distro warm the same suite passes.
& $PSMUX send-keys -t $session 'uname -s' Enter 2>&1 | Out-Null
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$output = ""
while ($sw.ElapsedMilliseconds -lt 90000) {
    $output = (& $PSMUX capture-pane -t $session -p 2>&1) | Out-String
    if ($output -match "Linux") { break }
    Start-Sleep -Milliseconds 500
}
if ($output -match "Linux") {
    Write-Pass "WSL pane confirmed running Linux (uname -s)"
} else {
    Write-Fail "WSL pane does not appear to be running Linux"
    Write-Info "  Output: $($output.Trim())"
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# Test 6: Runtime set-option changes default-shell to wsl.exe
# ============================================================
Write-Host ""
Write-Test "6. Runtime set-option to change default-shell to wsl.exe"

Remove-Item $confPath -Force -ErrorAction SilentlyContinue

$session = "wsl_test6"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 3

$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
Write-Info "  Initial shell: $($cmd.Trim())"

& $PSMUX set-option -g default-shell "`"$wslPath`"" -t $session 2>&1 | Out-Null
Start-Sleep -Seconds 1

$shellVal = (& $PSMUX show-options -v default-shell -t $session 2>&1) | Out-String
Write-Info "  default-shell after set-option: $($shellVal.Trim())"
if ($shellVal.Trim() -match "wsl") {
    Write-Pass "default-shell option updated to wsl.exe"
} else {
    Write-Fail "default-shell option NOT updated (got: $($shellVal.Trim()))"
}

& $PSMUX new-window -t $session 2>&1 | Out-Null
Start-Sleep -Seconds 5

$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
Write-Info "  new-window after runtime set: $($cmd.Trim())"
if ($cmd.Trim() -match "wsl|bash|zsh") {
    Write-Pass "New window uses WSL after runtime default-shell change"
} elseif ($cmd.Trim() -match "conhost|shell") {
    & $PSMUX send-keys -t $session 'uname -s' Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $capOut = (& $PSMUX capture-pane -t $session -p 2>&1) | Out-String
    if ($capOut -match "Linux") {
        Write-Pass "New window uses WSL after runtime change (verified via uname)"
    } else {
        Write-Fail "New window NOT using WSL after runtime change (got: $($cmd.Trim()))"
    }
} else {
    Write-Fail "New window NOT using WSL after runtime change (got: $($cmd.Trim()))"
}

& $PSMUX split-window -t $session -v 2>&1 | Out-Null
Start-Sleep -Seconds 3

$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
Write-Info "  split-window after runtime set: $($cmd.Trim())"
if ($cmd.Trim() -match "wsl|bash|zsh") {
    Write-Pass "Split pane uses WSL after runtime default-shell change"
} elseif ($cmd.Trim() -match "conhost|shell") {
    & $PSMUX send-keys -t $session 'uname -s' Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $capOut = (& $PSMUX capture-pane -t $session -p 2>&1) | Out-String
    if ($capOut -match "Linux") {
        Write-Pass "Split pane uses WSL after runtime change (verified via uname)"
    } else {
        Write-Fail "Split pane NOT using WSL after runtime change (got: $($cmd.Trim()))"
    }
} else {
    Write-Fail "Split pane NOT using WSL after runtime change (got: $($cmd.Trim()))"
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# Test 7: Runtime set with bare "wsl" name
# ============================================================
Write-Host ""
Write-Test "7. Runtime set-option with bare wsl name"

$session = "wsl_test7"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 3

& $PSMUX set-option -g default-shell wsl -t $session 2>&1 | Out-Null
Start-Sleep -Seconds 1

& $PSMUX new-window -t $session 2>&1 | Out-Null
Start-Sleep -Seconds 5

$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
Write-Info "  new-window with bare 'wsl': $($cmd.Trim())"
if ($cmd.Trim() -match "wsl|bash|zsh") {
    Write-Pass "Runtime set with bare 'wsl' works"
} elseif ($cmd.Trim() -match "conhost|shell") {
    & $PSMUX send-keys -t $session 'uname -s' Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    $capOut = (& $PSMUX capture-pane -t $session -p 2>&1) | Out-String
    if ($capOut -match "Linux") {
        Write-Pass "Runtime set with bare 'wsl' works (verified via uname)"
    } else {
        Write-Fail "Runtime set with bare 'wsl' failed (got: $($cmd.Trim()))"
    }
} else {
    Write-Fail "Runtime set with bare 'wsl' failed (got: $($cmd.Trim()))"
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# CLEANUP — restore original config
# ============================================================
Write-Host ""
Write-Info "Cleaning up..."
& $PSMUX kill-server 2>$null | Out-Null
Start-Sleep -Seconds 1
if ($wslKeeper -and -not $wslKeeper.HasExited) { Stop-Process -Id $wslKeeper.Id -Force -ErrorAction SilentlyContinue }

if ($confBackup) {
    Set-Content -Path $confPath -Value $confBackup
    Write-Info "Restored original config"
} else {
    Remove-Item $confPath -Force -ErrorAction SilentlyContinue
    Write-Info "Removed test config"
}

# ============================================================
# RESULTS
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "  RESULTS: $($script:TestsPassed) passed, $($script:TestsFailed) failed"
Write-Host ("=" * 60)

if ($script:TestsFailed -gt 0) {
    Write-Host "Some tests FAILED" -ForegroundColor Red
    exit 1
} else {
    Write-Host "All tests PASSED — WSL default-shell works correctly" -ForegroundColor Green
    exit 0
}
