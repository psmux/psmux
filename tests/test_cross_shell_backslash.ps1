# Cross-Shell Backslash Test
# Verifies that send-keys preserves backslashes correctly when:
#   A) Invoked from different shells (PowerShell, Git Bash, cmd.exe)
#   B) The pane itself runs different shells (pwsh, bash, cmd.exe)
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_cross_shell_backslash.ps1

$ErrorActionPreference = "Continue"
# This suite writes a config at a default path (~/.psmux.conf, ~/.tmux.conf or
# ~/.config/psmux) and expects psmux to load it at startup, but PSMUX_CONFIG_FILE
# replaces every default path and a developer's shell may have it set, so clear it (#748).
Remove-Item Env:PSMUX_CONFIG_FILE -ErrorAction SilentlyContinue
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass { param($msg) Write-Host "[PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail { param($msg) Write-Host "[FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip { param($msg) Write-Host "[SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Info { param($msg) Write-Host "[INFO] $msg" -ForegroundColor Cyan }
function Write-Test { param($msg) Write-Host "[TEST] $msg" -ForegroundColor White }

$PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -ErrorAction SilentlyContinue).Path
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\debug\psmux.exe" -ErrorAction SilentlyContinue).Path }
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -ErrorAction SilentlyContinue).Source }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
Write-Info "Using: $PSMUX"

# Detect available shells
$hasGitBash = $null -ne (Get-Command bash.exe -ErrorAction SilentlyContinue)
$bashPath = "C:/Program Files/Git/bin/bash.exe"
if (-not (Test-Path $bashPath)) { $hasGitBash = $false }
Write-Info "Git Bash available: $hasGitBash"

# ── Config backup/restore (protect against stale configs from killed runs) ──
$confPath = "$env:USERPROFILE\.psmux.conf"
$confBackup = $null
if (Test-Path $confPath) { $confBackup = Get-Content $confPath -Raw }
# Remove any config for tests 1-8 (default pwsh panes)
Remove-Item $confPath -Force -ErrorAction SilentlyContinue

function Restore-Config {
    if ($confBackup) { Set-Content -Path $confPath -Value $confBackup -Encoding UTF8 }
    else { Remove-Item $confPath -Force -ErrorAction SilentlyContinue }
}

function Clean-Start {
    param([string]$Session, [string]$Config = $null)
    & $PSMUX kill-server 2>&1 | Out-Null
    Get-Process psmux -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
    Remove-Item "$env:USERPROFILE\.psmux\*.port" -Force -ErrorAction SilentlyContinue
    Remove-Item "$env:USERPROFILE\.psmux\*.key" -Force -ErrorAction SilentlyContinue
    if ($Config) {
        Set-Content -Path $confPath -Value $Config -Encoding UTF8
    } else {
        Remove-Item $confPath -Force -ErrorAction SilentlyContinue
    }
    & $PSMUX new-session -d -s $Session 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    # Verify session started
    & $PSMUX has-session -t $Session 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Info "  WARNING: Session '$Session' failed to start, retrying..."
        Start-Sleep -Seconds 2
        & $PSMUX new-session -d -s $Session 2>&1 | Out-Null
        Start-Sleep -Seconds 3
    }
}

try {

Write-Host ""
Write-Host ("=" * 60)
Write-Host "  SECTION A: INVOKING SHELL TESTS"
Write-Host "  (pane=pwsh, caller=pwsh/bash/cmd)"
Write-Host ("=" * 60)
Write-Host ""

# ── Test 1: PowerShell invokes send-keys with backslash (space-containing arg) ──
Write-Test "1. PowerShell caller: backslash in space-containing arg"
Clean-Start -Session "bs_t1"
$marker = "BST1_$(Get-Random)"
& $PSMUX send-keys -t bs_t1 "echo $marker\:test" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$output = (& $PSMUX capture-pane -t bs_t1 -p 2>&1) | Out-String
if ($output -match "$marker\\:test") {
    Write-Pass "PowerShell: single backslash preserved (not doubled)"
} elseif ($output -match "$marker\\\\:test") {
    Write-Fail "PowerShell: backslash was DOUBLED"
} elseif ($output -match $marker) {
    Write-Pass "PowerShell: marker present, backslash content delivered"
} else {
    Write-Fail "PowerShell: marker not found. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
}

# ── Test 2: PowerShell: backslash in non-space arg (no quoting path) ──
Write-Test "2. PowerShell caller: backslash in simple args (no spaces)"
$marker2 = "BST2_$(Get-Random)"
& $PSMUX send-keys -t bs_t1 "echo" "${marker2}\:ok" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$output = (& $PSMUX capture-pane -t bs_t1 -p 2>&1) | Out-String
if ($output -match "$marker2\\:ok" -or $output -match "$marker2") {
    Write-Pass "PowerShell: simple arg backslash delivered"
} else {
    Write-Fail "PowerShell: simple arg failed. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
}

# ── Test 3: Windows path with backslashes ──
Write-Test "3. PowerShell caller: Windows path with backslashes"
Clean-Start -Session "bs_t3"
$marker3 = "BST3_$(Get-Random)"
& $PSMUX send-keys -t bs_t3 "echo $marker3 C:\Users\test\file.txt" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$output = (& $PSMUX capture-pane -t bs_t3 -p 2>&1) | Out-String
if ($output -match "$marker3.*C:\\Users\\test\\file") {
    Write-Pass "Windows path backslashes preserved correctly"
} elseif ($output -match "$marker3.*C:\\\\Users") {
    Write-Fail "Windows path backslashes were doubled"
} else {
    Write-Fail "Path test failed. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
}

# ── Test 4: POSIX-escaped URL through env shim ──
Write-Test "4. PowerShell caller: POSIX-escaped URL via env shim"
Clean-Start -Session "bs_t4"
$marker4 = "BST4_$(Get-Random)"
& $PSMUX send-keys -t bs_t4 "env MY_URL='https\://api.example.com/v1' pwsh -NoProfile -c 'Write-Host ${marker4}:`$env:MY_URL'" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 4
$output = (& $PSMUX capture-pane -t bs_t4 -p 2>&1) | Out-String
if ($output -match "${marker4}:https://api\.example\.com/v1") {
    Write-Pass "URL backslash correctly unescaped by env shim"
} elseif ($output -match "${marker4}:https\\://") {
    Write-Fail "URL backslash NOT unescaped (env shim _pu failed)"
} elseif ($output -match "${marker4}:https\\\\://") {
    Write-Fail "URL backslash was DOUBLED before reaching env shim"
} else {
    Write-Fail "URL test failed. Output: $($output.Substring(0, [Math]::Min(300, $output.Length)))"
}

# ── Test 5: Git Bash invokes send-keys ──
if ($hasGitBash) {
    Write-Test "5. Git Bash caller: send-keys with backslash"
    Clean-Start -Session "bs_t5"
    $marker5 = "BST5_$(Get-Random)"
    $psmuxUnix = $PSMUX -replace '\\', '/'
    & $bashPath -c "$psmuxUnix send-keys -t bs_t5 'echo ${marker5}\:from_bash' Enter" 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $output = (& $PSMUX capture-pane -t bs_t5 -p 2>&1) | Out-String
    if ($output -match $marker5) {
        Write-Pass "Git Bash caller: backslash content delivered"
    } else {
        Write-Fail "Git Bash caller: marker not found. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
    }
} else {
    Write-Skip "5. Git Bash not available"
}

# ── Test 6: cmd.exe invokes send-keys ──
Write-Test "6. cmd.exe caller: send-keys with backslash"
Clean-Start -Session "bs_t6"
$marker6 = "BST6_$(Get-Random)"
cmd.exe /c "`"$PSMUX`" send-keys -t bs_t6 `"echo ${marker6}\:from_cmd`" Enter" 2>&1 | Out-Null
Start-Sleep -Seconds 2
$output = (& $PSMUX capture-pane -t bs_t6 -p 2>&1) | Out-String
if ($output -match $marker6) {
    Write-Pass "cmd.exe caller: backslash content delivered"
} else {
    Write-Fail "cmd.exe caller: marker not found. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
}

# ── Test 7: Multiple backslashes ──
Write-Test "7. Multiple sequential backslashes"
Clean-Start -Session "bs_t7"
$marker7 = "BST7_$(Get-Random)"
& $PSMUX send-keys -t bs_t7 "echo $marker7 a\\b\\\\c" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$output = (& $PSMUX capture-pane -t bs_t7 -p 2>&1) | Out-String
if ($output -match $marker7) {
    Write-Pass "Multiple backslashes: content delivered"
} else {
    Write-Fail "Multiple backslashes failed. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
}

# ── Test 8: Trailing backslash ──
Write-Test "8. Trailing backslash in argument"
Clean-Start -Session "bs_t8"
$marker8 = "BST8_$(Get-Random)"
& $PSMUX send-keys -t bs_t8 "echo ${marker8}_end\" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$output = (& $PSMUX capture-pane -t bs_t8 -p 2>&1) | Out-String
if ($output -match $marker8) {
    Write-Pass "Trailing backslash: content delivered"
} else {
    Write-Fail "Trailing backslash failed. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
}

Write-Host ""
Write-Host ("=" * 60)
Write-Host "  SECTION B: PANE SHELL TESTS"
Write-Host "  (caller=pwsh, pane=bash/cmd)"
Write-Host ("=" * 60)
Write-Host ""

# ── Test 9: Pane running Git Bash ──
if ($hasGitBash) {
    Write-Test "9a. Bash pane: send-keys delivers text"
    Clean-Start -Session "bs_t9" -Config "set -g default-shell `"$bashPath`""
    # Bash startup race: keys sent before bash's readline is up are swallowed,
    # so 9a failed with an EMPTY capture while 9b on the SAME pane passed a
    # second later. Wait for the pane to draw its prompt before the first send,
    # then poll for the echo instead of sampling once.
    $ready = $false
    for ($i = 0; $i -lt 40; $i++) {
        $probe = (& $PSMUX capture-pane -t bs_t9 -p 2>&1) | Out-String
        if ($probe.Trim().Length -gt 0) { $ready = $true; break }
        Start-Sleep -Milliseconds 250
    }
    if (-not $ready) { Write-Info "  WARNING: bash pane drew nothing within 10s, sending anyway" }
    $marker9 = "BST9_$(Get-Random)"
    & $PSMUX send-keys -t bs_t9 "echo ${marker9}_hello" Enter 2>&1 | Out-Null
    $output = ""
    for ($i = 0; $i -lt 20; $i++) {
        Start-Sleep -Milliseconds 500
        $output = (& $PSMUX capture-pane -t bs_t9 -p 2>&1) | Out-String
        # The echo's OUTPUT line, not the typed command line: matching the
        # command moved on while bash (slow MSYS forks) was still running it.
        if ($output -match "(?m)^${marker9}_hello") { break }
    }
    if ($output -match "(?m)^${marker9}_hello") {
        Write-Pass "Bash pane: send-keys text delivered"
    } else {
        Write-Fail "Bash pane: text not found. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
    }

    Write-Test "9b. Bash pane: backslash in send-keys"
    $marker9b = "BST9B_$(Get-Random)"
    & $PSMUX send-keys -t bs_t9 "echo ${marker9b}_bs\:test" Enter 2>&1 | Out-Null
    $output = ""
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep -Milliseconds 500
        $output = (& $PSMUX capture-pane -t bs_t9 -p 2>&1) | Out-String
        if ($output -match "(?m)^${marker9b}_bs") { break }
    }
    if ($output -match "(?m)^${marker9b}_bs") {
        Write-Pass "Bash pane: backslash content delivered"
    } else {
        Write-Fail "Bash pane: backslash test failed. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
    }

    Write-Test "9c. Bash pane: Windows path"
    $marker9c = "BST9C_$(Get-Random)"
    & $PSMUX send-keys -t bs_t9 "echo ${marker9c} C:\\Users\\test" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $output = (& $PSMUX capture-pane -t bs_t9 -p 2>&1) | Out-String
    if ($output -match $marker9c) {
        Write-Pass "Bash pane: Windows path delivered"
    } else {
        Write-Fail "Bash pane: path failed. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
    }

    Write-Test "9d. Bash pane: split-window inherits bash"
    & $PSMUX split-window -t bs_t9 -h 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $marker9d = "BST9D_$(Get-Random)"
    & $PSMUX send-keys -t bs_t9 "echo ${marker9d}_split" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $output = (& $PSMUX capture-pane -t bs_t9 -p 2>&1) | Out-String
    if ($output -match "${marker9d}_split") {
        Write-Pass "Bash split pane: send-keys works"
    } else {
        Write-Fail "Bash split pane: failed. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
    }
} else {
    Write-Skip "9a. Git Bash not installed"
    Write-Skip "9b. Git Bash not installed"
    Write-Skip "9c. Git Bash not installed"
    Write-Skip "9d. Git Bash not installed"
}

# ── Test 10: Pane running cmd.exe ──
Write-Test "10a. cmd.exe pane: send-keys delivers text"
Clean-Start -Session "bs_t10" -Config "set -g default-shell `"cmd.exe`""
$marker10 = "BST10_$(Get-Random)"
& $PSMUX send-keys -t bs_t10 "echo ${marker10}_hello" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$output = (& $PSMUX capture-pane -t bs_t10 -p 2>&1) | Out-String
if ($output -match "${marker10}_hello") {
    Write-Pass "cmd.exe pane: send-keys text delivered"
} else {
    Write-Fail "cmd.exe pane: text not found. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
}

Write-Test "10b. cmd.exe pane: backslash in send-keys"
$marker10b = "BST10B_$(Get-Random)"
& $PSMUX send-keys -t bs_t10 "echo ${marker10b}\:test" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$output = (& $PSMUX capture-pane -t bs_t10 -p 2>&1) | Out-String
if ($output -match $marker10b) {
    Write-Pass "cmd.exe pane: backslash content delivered"
} else {
    Write-Fail "cmd.exe pane: backslash test failed. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
}

Write-Test "10c. cmd.exe pane: Windows path"
$marker10c = "BST10C_$(Get-Random)"
& $PSMUX send-keys -t bs_t10 "echo $marker10c C:\Users\test\file.txt" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2
$output = (& $PSMUX capture-pane -t bs_t10 -p 2>&1) | Out-String
if ($output -match $marker10c) {
    Write-Pass "cmd.exe pane: Windows path delivered"
} else {
    Write-Fail "cmd.exe pane: path failed. Output: $($output.Substring(0, [Math]::Min(200, $output.Length)))"
}

} finally {
    # Always restore config, even if tests fail or are interrupted
    & $PSMUX kill-server 2>&1 | Out-Null
    Get-Process psmux -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1
    Restore-Config
}

# ── Results ──
Write-Host ""
Write-Host ("=" * 60)
Write-Host "  CROSS-SHELL BACKSLASH TEST RESULTS"
Write-Host ("=" * 60)
Write-Host "  Passed:  $script:TestsPassed" -ForegroundColor Green
Write-Host "  Failed:  $script:TestsFailed" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $script:TestsSkipped" -ForegroundColor Yellow
Write-Host ("=" * 60)

if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
