# Issue #99 - psmux won't open with bash option set
# Tests that `set -g default-shell "C:/Program Files/Git/bin/bash.exe"` works correctly.
#
# The bug: When adding `set -g default-shell "C:/Program Files/Git/bin/bash.exe"`
# to the config, psmux refuses to open.
#
# This test verifies:
#   1. Config parsing correctly handles quoted paths with spaces
#   2. Server starts successfully with bash as default-shell
#   3. The pane actually runs bash (not pwsh/cmd)
#   4. Commands can be sent to and executed in the bash pane
#   5. Bare "bash" (no full path) works as default-shell
#   6. Paths with forward and backslashes both work
#
# https://github.com/psmux/psmux/issues/99
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue99_default_shell_bash.ps1

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

# Git bash needs 7 to 12 s to its first prompt on a machine where each MSYS
# fork costs about 150 ms (measured outside psmux as well), so the fixed 1 s
# waits after send-keys failed the sweep. Poll the pane for the expected line.
function Wait-PaneOutput {
    param([string]$Target, [string]$Pattern, [int]$TimeoutMs = 25000)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    do {
        Start-Sleep -Milliseconds 500
        $out = (& $PSMUX capture-pane -t $Target -p 2>&1) | Out-String
    } while ($out -notmatch $Pattern -and $sw.ElapsedMilliseconds -lt $TimeoutMs)
    return $out
}

$PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -ErrorAction SilentlyContinue).Path
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\debug\psmux.exe" -ErrorAction SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found. Build first: cargo build --release"; exit 1 }
Write-Info "Using: $PSMUX"

# Check if Git Bash exists
$bashPath = "C:/Program Files/Git/bin/bash.exe"
if (-not (Test-Path $bashPath)) {
    Write-Info "Git Bash not found at $bashPath — skipping tests"
    Write-Host "[SKIP] Git Bash not installed" -ForegroundColor Yellow
    exit 0
}
Write-Info "Git Bash found: $bashPath"

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
# Ensure all psmux processes are truly gone before starting fresh
Get-Process psmux -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
Remove-Item "$env:USERPROFILE\.psmux\*.port" -Force -ErrorAction SilentlyContinue
Remove-Item "$env:USERPROFILE\.psmux\*.key" -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host ("=" * 60)
Write-Host "  ISSUE #99: DEFAULT-SHELL BASH"
Write-Host ("=" * 60)

# ============================================================
# Test 1: Full path with spaces (the exact config from the issue)
# ============================================================
Write-Host ""
Write-Test "1. default-shell with full path containing spaces"

Set-Content -Path $confPath -Value 'set -g default-shell "C:/Program Files/Git/bin/bash.exe"'

$session = "issue99_test1"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 5

$hasSession = & $PSMUX has-session -t $session 2>&1
if ($LASTEXITCODE -eq 0) {
    Write-Pass "Session '$session' created successfully with bash default-shell"
} else {
    Write-Fail "Failed to create session with bash default-shell (full path with spaces)"
}

# Check the running command is bash
$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
$cmd = $cmd.Trim()
Write-Info "  pane_current_command: $cmd"
if ($cmd -match "bash") {
    Write-Pass "Pane is running bash"
} else {
    Write-Fail "Pane is NOT running bash (got: $cmd)"
}

# Send a command and verify it works
& $PSMUX send-keys -t $session 'echo ISSUE99_WORKS' Enter 2>&1 | Out-Null
$output = Wait-PaneOutput $session '(?m)^ISSUE99_WORKS'
if ($output -match "ISSUE99_WORKS") {
    Write-Pass "Bash pane executes commands correctly"
} else {
    Write-Fail "Bash pane did not produce expected output"
    Write-Info "  Output: $($output.Trim())"
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# Test 2: Backslash path style
# ============================================================
Write-Host ""
Write-Test '2. default-shell with backslash path: "C:\Program Files\Git\bin\bash.exe"'

Set-Content -Path $confPath -Value 'set -g default-shell "C:\Program Files\Git\bin\bash.exe"'

$session = "issue99_test2"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 3

$hasSession = & $PSMUX has-session -t $session 2>&1
if ($LASTEXITCODE -eq 0) {
    Write-Pass "Session created with backslash path"
    $cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
    if ($cmd.Trim() -match "bash") {
        Write-Pass "Pane runs bash with backslash path"
    } else {
        Write-Fail "Pane not running bash (got: $($cmd.Trim()))"
    }
} else {
    Write-Fail "Failed to create session with backslash path"
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# Test 3: Bare "bash" name (relies on PATH resolution)
# ============================================================
Write-Host ""
Write-Test "3. default-shell with bare name: bash"

Set-Content -Path $confPath -Value 'set -g default-shell bash'

$session = "issue99_test3"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 3

$hasSession = & $PSMUX has-session -t $session 2>&1
if ($LASTEXITCODE -eq 0) {
    Write-Pass "Session created with bare 'bash' name"
    $cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
    if ($cmd.Trim() -match "bash|wsl|conhost") {
        Write-Pass "Pane runs bash via PATH resolution (got: $($cmd.Trim()))"
    } else {
        Write-Fail "Pane not running bash (got: $($cmd.Trim()))"
    }
} else {
    Write-Fail "Failed to create session with bare 'bash' name"
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# Test 4: new-window also uses bash when default-shell is set
# ============================================================
Write-Host ""
Write-Test "4. new-window inherits default-shell"

Set-Content -Path $confPath -Value 'set -g default-shell "C:/Program Files/Git/bin/bash.exe"'

$session = "issue99_test4"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 3

# Window 0 first, so an asymmetry between the session's own pane and the one
# new-window creates is visible rather than inferred.
$w0cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
$w0shell = (& $PSMUX show-options -g -v default-shell -t $session 2>&1) | Out-String
Write-Info "  window 0 pane_current_command: $($w0cmd.Trim()) (default-shell '$($w0shell.Trim())')"

# Create a second window
& $PSMUX new-window -t $session 2>&1 | Out-Null
Start-Sleep -Seconds 5

$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
Write-Info "  new-window pane_current_command: $($cmd.Trim())"
# pane_current_command is a name, not evidence: on this PATH a bare `bash` is
# System32's WSL launcher, which also reports "bash" and also answers
# BASH_VERSION (in 18 ms when the distro is warm). The assertion is the IMAGE
# the pane runs, for the session's own pane and for the new window, because
# sweep 2026-09-24_23-37-41 had both on WSL bash with the Git path configured:
# the session had claimed a warm standby spawned under test 3's `default-shell
# bash`, and the claim reloaded the option without respawning the standby's
# pane or its spare pool.
function Get-PaneImage([string]$target) {
    $pp = (& $PSMUX display-message -t $target -p '#{pane_pid}' 2>&1 | Out-String).Trim()
    $pi = Get-CimInstance Win32_Process -Filter "ProcessId=$pp" -ErrorAction SilentlyContinue
    return @{ Pid = $pp; Image = [string]$pi.ExecutablePath; Cmd = [string]$pi.CommandLine }
}
$img0 = Get-PaneImage "${session}:0"
$img1 = Get-PaneImage "${session}:1"
Write-Info "  window 0 pane_pid=$($img0.Pid) image=$($img0.Image)"
Write-Info "  window 1 pane_pid=$($img1.Pid) image=$($img1.Image)"
$gitBash = 'C:\Program Files\Git\bin\bash.exe'
if ($img0.Image -ieq $gitBash) {
    Write-Pass "The session's own pane runs the configured Git bash"
} else {
    Write-Fail "The session's own pane runs '$($img0.Image)', not the configured Git bash (a claimed standby kept the shell it booted with)"
}
if ($img1.Image -ieq $gitBash) {
    Write-Pass "New window runs the configured Git bash"
} else {
    Write-Fail "New window runs '$($img1.Image)', not the configured Git bash"
}
# And the shell really is a live Git bash: BASH_VERSION answers.
& $PSMUX send-keys -t $session 'echo BASH_CHECK_$BASH_VERSION' Enter 2>&1 | Out-Null
$sw99 = [System.Diagnostics.Stopwatch]::StartNew()
$capOut = ""
while ($sw99.ElapsedMilliseconds -lt 10000) {
    $capOut = (& $PSMUX capture-pane -t $session -p 2>&1) | Out-String
    if ($capOut -match "BASH_CHECK_\d") { break }
    Start-Sleep -Milliseconds 500
}
if ($capOut -match "BASH_CHECK_\d") {
    Write-Pass "New window answers BASH_VERSION after $($sw99.ElapsedMilliseconds)ms"
} else {
    $srv = (& $PSMUX display-message -t $session -p '#{pid}' 2>&1 | Out-String).Trim()
    Write-Info "  BASH_VERSION echo never appeared within $($sw99.ElapsedMilliseconds)ms; server pid=$srv SHELL='$env:SHELL'; last pane lines: $(($capOut -split "`n" | Where-Object { $_ -match '\S' } | Select-Object -Last 3) -join ' / ')"
    Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" -ErrorAction SilentlyContinue | Where-Object { $_.CommandLine -match '__warm__' } | ForEach-Object {
        $kids = (Get-CimInstance Win32_Process -Filter "ParentProcessId=$($_.ProcessId)" | Where-Object { $_.Name -ne 'conhost.exe' } | ForEach-Object { "$($_.Name)[$($_.CommandLine)]" }) -join ' ; '
        Write-Info "  warm standby $($_.ProcessId): $kids"
    }
    Write-Fail "New window did not answer BASH_VERSION (pane_current_command=$($cmd.Trim()))"
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# Test 5: split-window also uses bash when default-shell is set
# ============================================================
Write-Host ""
Write-Test "5. split-window inherits default-shell"

Set-Content -Path $confPath -Value 'set -g default-shell "C:/Program Files/Git/bin/bash.exe"'

$session = "issue99_test5"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 3

# Split the window
& $PSMUX split-window -t $session -v 2>&1 | Out-Null
Start-Sleep -Seconds 2

# Check the new pane (should be the active one after split)
$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
Write-Info "  split-window pane_current_command: $($cmd.Trim())"
if ($cmd.Trim() -match "bash") {
    Write-Pass "Split pane runs bash"
} else {
    Write-Fail "Split pane not running bash (got: $($cmd.Trim()))"
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# Test 6: TMUX/PSMUX_SESSION env vars are set in bash panes
# ============================================================
Write-Host ""
Write-Test "6. Environment variables set correctly in bash panes"

Set-Content -Path $confPath -Value 'set -g default-shell "C:/Program Files/Git/bin/bash.exe"'

$session = "issue99_test6"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 3

# Check PSMUX_SESSION is set
& $PSMUX send-keys -t $session 'echo "PSMUX=$PSMUX_SESSION"' Enter 2>&1 | Out-Null
$output = Wait-PaneOutput $session '(?m)^PSMUX=.+'
if ($output -match "PSMUX=.+") {
    Write-Pass "PSMUX_SESSION is set in bash pane"
} else {
    Write-Fail "PSMUX_SESSION not set in bash pane"
    Write-Info "  Output: $($output.Trim())"
}

# Check TMUX is set
& $PSMUX send-keys -t $session 'echo "TMUX_VAR=$TMUX"' Enter 2>&1 | Out-Null
$output = Wait-PaneOutput $session '(?m)^TMUX_VAR=.+'
if ($output -match "TMUX_VAR=.+") {
    Write-Pass "TMUX env var is set in bash pane"
} else {
    Write-Fail "TMUX env var not set in bash pane"
    Write-Info "  Output: $($output.Trim())"
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# Test 7: RUNTIME set-option changes default-shell (THE ACTUAL BUG)
# This is the exact user scenario from issue #99: user types
# set -g default-shell "C:/Program Files/Git/bin/bash.exe"
# at the command prompt (prefix+:), then tries new-window.
# ============================================================
Write-Host ""
Write-Test "7. Runtime set-option to change default-shell to bash"

# Start session with NO custom default-shell (uses pwsh)
Remove-Item $confPath -Force -ErrorAction SilentlyContinue

$session = "issue99_test7"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 3

# Verify first pane runs pwsh (default shell)
$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
Write-Info "  Initial shell: $($cmd.Trim())"
if ($cmd.Trim() -match "pwsh|powershell|cmd") {
    Write-Pass "Initial pane runs default shell (pwsh/cmd)"
} else {
    Write-Info "  Initial pane command: $($cmd.Trim()) (expected pwsh/cmd but got something else)"
}

# NOW change default-shell at runtime via set-option (simulates prefix+: input)
& $PSMUX set-option -g default-shell '"C:/Program Files/Git/bin/bash.exe"' -t $session 2>&1 | Out-Null
Start-Sleep -Seconds 1

# Verify the option was applied
$shellVal = (& $PSMUX show-options -v default-shell -t $session 2>&1) | Out-String
Write-Info "  default-shell after set-option: $($shellVal.Trim())"
if ($shellVal.Trim() -match "bash") {
    Write-Pass "default-shell option updated to bash"
} else {
    Write-Fail "default-shell option NOT updated (got: $($shellVal.Trim()))"
}

# Create a new window — this MUST use bash now
& $PSMUX new-window -t $session 2>&1 | Out-Null
Start-Sleep -Seconds 3

$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
Write-Info "  new-window after runtime set: $($cmd.Trim())"
if ($cmd.Trim() -match "bash") {
    Write-Pass "New window uses bash after runtime default-shell change"
} else {
    Write-Fail "New window NOT using bash after runtime change (got: $($cmd.Trim()))"
}

# Split window — also must use bash
& $PSMUX split-window -t $session -v 2>&1 | Out-Null
Start-Sleep -Seconds 2

$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
Write-Info "  split-window after runtime set: $($cmd.Trim())"
if ($cmd.Trim() -match "bash") {
    Write-Pass "Split pane uses bash after runtime default-shell change"
} else {
    Write-Fail "Split pane NOT using bash after runtime change (got: $($cmd.Trim()))"
}

& $PSMUX kill-session -t $session 2>$null | Out-Null
Start-Sleep -Seconds 1

# ============================================================
# Test 8: Runtime set with bare "bash" name
# ============================================================
Write-Host ""
Write-Test "8. Runtime set-option with bare bash name"

$session = "issue99_test8"
& $PSMUX new-session -d -s $session 2>&1 | Out-Null
Start-Sleep -Seconds 3

& $PSMUX set-option -g default-shell bash -t $session 2>&1 | Out-Null
Start-Sleep -Seconds 1

& $PSMUX new-window -t $session 2>&1 | Out-Null
Start-Sleep -Seconds 3

$cmd = (& $PSMUX display-message -t $session -p '#{pane_current_command}' 2>&1) | Out-String
Write-Info "  new-window with bare 'bash': $($cmd.Trim())"
if ($cmd.Trim() -match "bash|wsl|conhost") {
    Write-Pass "Runtime set with bare 'bash' works (got: $($cmd.Trim()))"
} else {
    Write-Fail "Runtime set with bare 'bash' failed (got: $($cmd.Trim()))"
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
    Write-Host "Some tests FAILED — issue #99 may be present" -ForegroundColor Red
    exit 1
} else {
    Write-Host "All tests PASSED — bash default-shell works correctly" -ForegroundColor Green
    exit 0
}
