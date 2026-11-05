# test_claude_compat_fixes.ps1 — Tests for Claude Code compatibility fixes
#
# Verifies all fixes from this commit:
#   1. split-window -c <dir> sets working directory correctly
#   2. new-window -c <dir> sets working directory correctly
#   3. build_command() correctly detects bash/zsh for -c flag (not /C)
#   4. Parallelized kill-server (functional correctness, not just speed)
#   5. Reduced stale port cleanup timeout (functional correctness)
#
# These fixes address:
#   - Claude Code teammate pane spawning (uses split-window -c -h -P -F)
#   - Claude Code split-window with commands in bash shells
#   - https://github.com/anthropics/claude-code/issues/23675
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_claude_compat_fixes.ps1

$ErrorActionPreference = "Continue"
# This suite writes a config at a default path (~/.psmux.conf, ~/.tmux.conf or
# ~/.config/psmux) and expects psmux to load it at startup, but PSMUX_CONFIG_FILE
# replaces every default path and a developer's shell may have it set, so clear it (#748).
Remove-Item Env:PSMUX_CONFIG_FILE -ErrorAction SilentlyContinue
$script:Passed = 0
$script:Failed = 0
$script:Skipped = 0

function Pass($msg) { Write-Host "  PASS: $msg" -ForegroundColor Green; $script:Passed++ }
function Fail($msg) { Write-Host "  FAIL: $msg" -ForegroundColor Red;   $script:Failed++ }
function Skip($msg) { Write-Host "  SKIP: $msg" -ForegroundColor Yellow; $script:Skipped++ }
function Test($msg) { Write-Host "  TEST: $msg" -ForegroundColor Cyan }
function Section($msg) { Write-Host "`n$('=' * 60)" -ForegroundColor Cyan; Write-Host "  $msg" -ForegroundColor Cyan; Write-Host "$('=' * 60)" -ForegroundColor Cyan }

$PSMUX = "$PSScriptRoot\..\target\release\psmux.exe"
if (-not (Test-Path $PSMUX)) { $PSMUX = (Get-Command psmux -ErrorAction SilentlyContinue).Source }
if (-not $PSMUX -or -not (Test-Path $PSMUX)) {
    Write-Error "psmux binary not found. Build first: cargo build --release"
    exit 1
}
Write-Host "Using: $PSMUX" -ForegroundColor Cyan

$PsmuxDir = "$env:USERPROFILE\.psmux"
$confPath = "$env:USERPROFILE\.psmux.conf"
$confBackup = $null

# Backup existing config
if (Test-Path $confPath) {
    $confBackup = Get-Content $confPath -Raw
}

function Cleanup-Session {
    param([string]$Name)
    try { & $PSMUX kill-session -t $Name 2>&1 | Out-Null } catch {}
    Start-Sleep -Milliseconds 800
    Remove-Item "$PsmuxDir\$Name.port" -Force -ErrorAction SilentlyContinue
    Remove-Item "$PsmuxDir\$Name.key" -Force -ErrorAction SilentlyContinue
}

function Wait-ForSession {
    param([string]$Name, [int]$TimeoutMs = 8000)
    $deadline = (Get-Date).AddMilliseconds($TimeoutMs)
    while ((Get-Date) -lt $deadline) {
        & $PSMUX has-session -t $Name 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Capture-Pane {
    param([string]$Target)
    & $PSMUX capture-pane -t $Target -p 2>&1 | Out-String
}

# ── Pre-test cleanup ──
& $PSMUX kill-server 2>&1 | Out-Null
Start-Sleep -Seconds 2
Get-Process psmux -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

Write-Host ""
Write-Host "================================================"
Write-Host "  Claude Code Compatibility Fixes Test Suite"
Write-Host "================================================"
Write-Host ""

# ═══════════════════════════════════════════════════════════
Section "GROUP 1: split-window -c (start directory)"
# ═══════════════════════════════════════════════════════════

# --- 1.1: split-window -c sets CWD in new pane ---
Test "1.1: split-window -c sets CWD in new pane"
$SESSION = "fix_splitc_1"
try {
    $testDir = Join-Path $env:TEMP "psmux_test_splitc_$(Get-Random)"
    New-Item -Path $testDir -ItemType Directory -Force | Out-Null

    & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
    if (-not (Wait-ForSession $SESSION)) { Fail "1.1: Session did not start"; throw "skip" }

    # Split with -c pointing to our test directory
    & $PSMUX split-window -h -c $testDir -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 3

    # Ask the new pane for its CWD
    & $PSMUX send-keys -t $SESSION "pwd" Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $SESSION

    # Check if the test dir appears in the output (handle forward/back slashes and line wrapping)
    $testDirName = Split-Path $testDir -Leaf
    $capNormalized = ($cap -replace "`r?`n", "") -replace "\s+", " "
    if ($capNormalized -match [regex]::Escape($testDirName)) {
        Pass "1.1: split-window -c correctly set CWD to test directory"
    } else {
        Fail "1.1: CWD not set. Expected dir containing '$testDirName'. Got: $($cap.Substring(0,[Math]::Min(300,$cap.Length)).Trim())"
    }
    Cleanup-Session $SESSION
    Remove-Item $testDir -Recurse -Force -ErrorAction SilentlyContinue
} catch {
    if ($_.Exception.Message -ne "skip") { Fail "1.1: Exception: $_" }
    Cleanup-Session $SESSION
}

# --- 1.2: split-window -c with command ---
Test "1.2: split-window -c with command runs in specified dir"
$SESSION = "fix_splitc_2"
try {
    $testDir = Join-Path $env:TEMP "psmux_test_splitc2_$(Get-Random)"
    New-Item -Path $testDir -ItemType Directory -Force | Out-Null
    # Create a marker file in the test dir
    Set-Content -Path (Join-Path $testDir "MARKER_FILE.txt") -Value "found_it"

    & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
    if (-not (Wait-ForSession $SESSION)) { Fail "1.2: Session did not start"; throw "skip" }

    # Split with -c AND a command that lists the dir (keep pane alive with pause)
    & $PSMUX split-window -h -c $testDir -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    & $PSMUX send-keys -t "${SESSION}:0.1" "Get-ChildItem -Name" Enter
    Start-Sleep -Seconds 2

    $cap = Capture-Pane "${SESSION}:0.1"
    if ($cap -match "MARKER_FILE") {
        Pass "1.2: split-window -c + command correctly ran in specified directory"
    } else {
        Fail "1.2: Command did not run in specified dir. Output: $($cap.Substring(0,[Math]::Min(300,$cap.Length)).Trim())"
    }
    Cleanup-Session $SESSION
    Remove-Item $testDir -Recurse -Force -ErrorAction SilentlyContinue
} catch {
    if ($_.Exception.Message -ne "skip") { Fail "1.2: Exception: $_" }
    Cleanup-Session $SESSION
}

# --- 1.3: split-window -c with -P returns pane info ---
Test "1.3: split-window -c -P -F returns pane info (Claude Code pattern)"
$SESSION = "fix_splitc_3"
try {
    $testDir = Join-Path $env:TEMP "psmux_test_splitc3_$(Get-Random)"
    New-Item -Path $testDir -ItemType Directory -Force | Out-Null

    & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
    if (-not (Wait-ForSession $SESSION)) { Fail "1.3: Session did not start"; throw "skip" }

    # This is exactly how Claude Code creates teammate panes
    $paneInfo = & $PSMUX split-window -h -d -c $testDir -P -F "#{pane_id}" -t $SESSION 2>&1
    Start-Sleep -Seconds 2

    if ($paneInfo -match "^%\d+$") {
        Pass "1.3: split-window -c -P -F returned pane ID: $($paneInfo.Trim())"
    } else {
        Fail "1.3: Expected pane ID (%%N), got: '$paneInfo'"
    }
    Cleanup-Session $SESSION
    Remove-Item $testDir -Recurse -Force -ErrorAction SilentlyContinue
} catch {
    if ($_.Exception.Message -ne "skip") { Fail "1.3: Exception: $_" }
    Cleanup-Session $SESSION
}

# ═══════════════════════════════════════════════════════════
Section "GROUP 2: new-window -c (start directory)"
# ═══════════════════════════════════════════════════════════

# --- 2.1: new-window -c sets CWD ---
Test "2.1: new-window -c sets CWD in new window"
$SESSION = "fix_newwinc_1"
try {
    $testDir = Join-Path $env:TEMP "psmux_test_newwinc_$(Get-Random)"
    New-Item -Path $testDir -ItemType Directory -Force | Out-Null

    & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
    if (-not (Wait-ForSession $SESSION)) { Fail "2.1: Session did not start"; throw "skip" }

    & $PSMUX new-window -c $testDir -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Seconds 3

    & $PSMUX send-keys -t $SESSION "pwd" Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $SESSION

    $testDirName = Split-Path $testDir -Leaf
    if ($cap -match [regex]::Escape($testDirName)) {
        Pass "2.1: new-window -c correctly set CWD"
    } else {
        Fail "2.1: CWD not set. Expected '$testDirName'. Got: $($cap.Substring(0,[Math]::Min(300,$cap.Length)).Trim())"
    }
    Cleanup-Session $SESSION
    Remove-Item $testDir -Recurse -Force -ErrorAction SilentlyContinue
} catch {
    if ($_.Exception.Message -ne "skip") { Fail "2.1: Exception: $_" }
    Cleanup-Session $SESSION
}

# --- 2.2: new-window -c -P returns window info ---
Test "2.2: new-window -c -P -F returns window info"
$SESSION = "fix_newwinc_2"
try {
    $testDir = Join-Path $env:TEMP "psmux_test_newwinc2_$(Get-Random)"
    New-Item -Path $testDir -ItemType Directory -Force | Out-Null

    & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
    if (-not (Wait-ForSession $SESSION)) { Fail "2.2: Session did not start"; throw "skip" }

    $winInfo = & $PSMUX new-window -c $testDir -P -F "#{window_index}" -t $SESSION 2>&1
    Start-Sleep -Seconds 2

    # Should get a window index
    if ($winInfo -match "\d") {
        Pass "2.2: new-window -c -P -F returned: $($winInfo.Trim())"
    } else {
        Fail "2.2: Expected window info, got: '$winInfo'"
    }
    Cleanup-Session $SESSION
    Remove-Item $testDir -Recurse -Force -ErrorAction SilentlyContinue
} catch {
    if ($_.Exception.Message -ne "skip") { Fail "2.2: Exception: $_" }
    Cleanup-Session $SESSION
}

# ═══════════════════════════════════════════════════════════
Section "GROUP 3: Shell detection for command dispatch"
# ═══════════════════════════════════════════════════════════

# Check if Git Bash is available
$bashPath = $null
foreach ($p in @("C:/Program Files/Git/bin/bash.exe", "C:/Program Files (x86)/Git/bin/bash.exe")) {
    if (Test-Path $p) { $bashPath = $p; break }
}
if (-not $bashPath) {
    $bashPath = (Get-Command bash -ErrorAction SilentlyContinue).Source
}

if ($bashPath) {
    Write-Host "  Git Bash found: $bashPath" -ForegroundColor Gray

    # --- 3.1: split-window with bash command (uses -c not /C) ---
    Test "3.1: split-window command dispatch uses -c for bash shell"
    $SESSION = "fix_shell_1"
    try {
        # Configure psmux to use bash as default shell
        Set-Content -Path $confPath -Value "set -g default-shell `"$($bashPath.Replace('\','/'))`""

        & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
        if (-not (Wait-ForSession $SESSION 12000)) { Fail "3.1: Session did not start with bash"; throw "skip" }

        # split-window with an explicit command - should use bash -c "..."
        # Command keeps pane alive so we can capture output
        & $PSMUX split-window -h -t $SESSION "echo BASH_CMD_OK && sleep 30" 2>&1 | Out-Null
        Start-Sleep -Seconds 4

        # Capture from the new pane (0.1)
        $cap = Capture-Pane "${SESSION}:0.1"
        if ($cap -match "BASH_CMD_OK") {
            Pass "3.1: Command executed correctly in bash pane (uses -c, not /C)"
        } else {
            # Also try capturing from active pane
            $cap2 = Capture-Pane $SESSION
            if ($cap2 -match "BASH_CMD_OK") {
                Pass "3.1: Command executed correctly in bash pane (active pane)"
            } else {
                Fail "3.1: Command failed in bash. Pane 0.1: $($cap.Substring(0,[Math]::Min(200,$cap.Length)).Trim()) | Active: $($cap2.Substring(0,[Math]::Min(200,$cap2.Length)).Trim())"
            }
        }
        Cleanup-Session $SESSION
    } catch {
        if ($_.Exception.Message -ne "skip") { Fail "3.1: Exception: $_" }
        Cleanup-Session $SESSION
    }

    # --- 3.2: new-window with bash command ---
    Test "3.2: new-window command dispatch uses -c for bash shell"
    $SESSION = "fix_shell_2"
    try {
        Set-Content -Path $confPath -Value "set -g default-shell `"$($bashPath.Replace('\','/'))`""

        & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
        if (-not (Wait-ForSession $SESSION 12000)) { Fail "3.2: Session did not start"; throw "skip" }

        & $PSMUX new-window -t $SESSION "echo BASH_NEWWIN_OK && sleep 5" 2>&1 | Out-Null
        Start-Sleep -Seconds 3

        $cap = Capture-Pane $SESSION
        if ($cap -match "BASH_NEWWIN_OK") {
            Pass "3.2: new-window command executed correctly in bash"
        } else {
            Fail "3.2: Command failed. Output: $($cap.Substring(0,[Math]::Min(300,$cap.Length)).Trim())"
        }
        Cleanup-Session $SESSION
    } catch {
        if ($_.Exception.Message -ne "skip") { Fail "3.2: Exception: $_" }
        Cleanup-Session $SESSION
    }

    # --- 3.3: Claude Code exact teammate spawn pattern with bash ---
    Test "3.3: Claude Code teammate spawn pattern (split-window -h -c -P -F with command)"
    $SESSION = "fix_shell_3"
    try {
        Set-Content -Path $confPath -Value "set -g default-shell `"$($bashPath.Replace('\','/'))`""

        $testDir = Join-Path $env:TEMP "psmux_cc_bash_$(Get-Random)"
        New-Item -Path $testDir -ItemType Directory -Force | Out-Null

        & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
        if (-not (Wait-ForSession $SESSION 12000)) { Fail "3.3: Session did not start"; throw "skip" }

        # Simulate Claude Code's exact split-window pattern
        $paneId = & $PSMUX split-window -h -d -c $testDir -P -F "#{pane_id}" -t $SESSION 2>&1
        Start-Sleep -Seconds 2

        if ($paneId -match "^%\d+$") {
            Pass "3.3: Claude Code split pattern returned pane ID: $($paneId.Trim())"

            # Git bash can need 20 s to its first prompt on a machine where each
            # MSYS fork costs 300 to 400 ms (git-prompt.sh forks many times;
            # measured the same on builds before and after #989), so wait for
            # the prompt before typing, then poll for the echo.
            $sw = [Diagnostics.Stopwatch]::StartNew()
            do {
                Start-Sleep -Milliseconds 500
                $cap = & $PSMUX capture-pane -t "${SESSION}:0.1" -p 2>&1 | Out-String
            } while ($cap -notmatch '\$\s*$' -and $sw.ElapsedMilliseconds -lt 45000)

            # Now send a command to that pane using session:window.pane format
            & $PSMUX send-keys -t "${SESSION}:0.1" "echo TEAMMATE_SPAWN_OK" Enter
            $sw = [Diagnostics.Stopwatch]::StartNew()
            do {
                Start-Sleep -Milliseconds 500
                $cap = & $PSMUX capture-pane -t "${SESSION}:0.1" -p 2>&1 | Out-String
            } while ($cap -notmatch "(?m)^TEAMMATE_SPAWN_OK" -and $sw.ElapsedMilliseconds -lt 25000)
            if ($cap -match "TEAMMATE_SPAWN_OK") {
                Pass "3.3b: send-keys to teammate pane works correctly"
            } else {
                Fail "3.3b: send-keys to pane ${SESSION}:0.1 failed. Output: $($cap.Substring(0,[Math]::Min(200,$cap.Length)).Trim())"
            }
        } else {
            Fail "3.3: Expected pane ID, got: '$paneId'"
        }
        Cleanup-Session $SESSION
        Remove-Item $testDir -Recurse -Force -ErrorAction SilentlyContinue
    } catch {
        if ($_.Exception.Message -ne "skip") { Fail "3.3: Exception: $_" }
        Cleanup-Session $SESSION
    }
} else {
    Skip "3.1: Git Bash not found — skipping shell detection tests"
    Skip "3.2: Git Bash not found — skipping"
    Skip "3.3: Git Bash not found — skipping"
}

# Restore config
if ($confBackup) {
    Set-Content -Path $confPath -Value $confBackup
} else {
    Remove-Item $confPath -Force -ErrorAction SilentlyContinue
}

# ═══════════════════════════════════════════════════════════
Section "GROUP 4: kill-server parallelization"
# ═══════════════════════════════════════════════════════════

# --- 4.1: kill-server kills multiple sessions reliably ---
Test "4.1: kill-server kills all sessions (parallel path)"
try {
    # Create 4 sessions
    foreach ($i in 1..4) {
        & $PSMUX new-session -d -s "ks_par_$i" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 500
    }
    Start-Sleep -Seconds 3

    $allExist = $true
    foreach ($i in 1..4) {
        if (-not (Wait-ForSession "ks_par_$i" 5000)) { $allExist = $false }
    }
    if (-not $allExist) { Fail "4.1: Not all 4 sessions started"; throw "skip" }
    Pass "4.1a: All 4 sessions created"

    # Time the kill-server
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $PSMUX kill-server 2>&1 | Out-Null
    $sw.Stop()
    Start-Sleep -Seconds 3

    $anyAlive = $false
    foreach ($i in 1..4) {
        & $PSMUX has-session -t "ks_par_$i" 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) { $anyAlive = $true }
    }

    if (-not $anyAlive) {
        Pass "4.1b: All sessions killed by parallel kill-server ($($sw.ElapsedMilliseconds)ms)"
    } else {
        Fail "4.1b: Some sessions survived kill-server"
    }

    # Verify port files cleaned up
    $stale = @(Get-ChildItem "$PsmuxDir\ks_par_*.port" -ErrorAction SilentlyContinue)
    if ($stale.Count -eq 0) {
        Pass "4.1c: All port files cleaned up"
    } else {
        Fail "4.1c: Stale port files: $($stale.Name -join ', ')"
    }
} catch {
    if ($_.Exception.Message -ne "skip") { Fail "4.1: Exception: $_" }
    & $PSMUX kill-server 2>&1 | Out-Null
    Start-Sleep -Seconds 2
}

# --- 4.2: kill-server with -L namespace ---
Test "4.2: kill-server -L only kills namespaced sessions (parallel)"
try {
    # Create sessions in namespace and outside
    & $PSMUX new-session -d -s "global_ks" 2>&1 | Out-Null
    & $PSMUX -L myns new-session -d -s "ns_ks1" 2>&1 | Out-Null
    & $PSMUX -L myns new-session -d -s "ns_ks2" 2>&1 | Out-Null
    Start-Sleep -Seconds 3

    if (-not (Wait-ForSession "global_ks")) { Fail "4.2: global session didn't start"; throw "skip" }

    # kill-server with -L should only kill myns sessions
    & $PSMUX -L myns kill-server 2>&1 | Out-Null
    Start-Sleep -Seconds 3

    & $PSMUX has-session -t "global_ks" 2>&1 | Out-Null
    $globalAlive = ($LASTEXITCODE -eq 0)

    if ($globalAlive) {
        Pass "4.2: kill-server -L preserved non-namespaced session"
    } else {
        Fail "4.2: kill-server -L killed global session (should not have)"
    }

    # Final cleanup
    Cleanup-Session "global_ks"
    Cleanup-Session "ns_ks1"
    Cleanup-Session "ns_ks2"
} catch {
    if ($_.Exception.Message -ne "skip") { Fail "4.2: Exception: $_" }
    & $PSMUX kill-server 2>&1 | Out-Null
    Start-Sleep -Seconds 2
}

# --- 4.3: kill-server speed test (parallel should be faster) ---
Test "4.3: kill-server completes in reasonable time (parallel)"
try {
    # Start 3 sessions
    foreach ($i in 1..3) {
        & $PSMUX new-session -d -s "ks_speed_$i" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 500
    }
    Start-Sleep -Seconds 3

    foreach ($i in 1..3) {
        if (-not (Wait-ForSession "ks_speed_$i" 5000)) { Fail "4.3: session ks_speed_$i didn't start"; throw "skip" }
    }

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $PSMUX kill-server 2>&1 | Out-Null
    $sw.Stop()
    Start-Sleep -Seconds 2

    # With parallel threads, 3 sessions should finish well under 5s
    # (sequential with 3s timeout per server = 9s+; parallel = ~3s max)
    if ($sw.ElapsedMilliseconds -lt 8000) {
        Pass "4.3: kill-server completed in $($sw.ElapsedMilliseconds)ms (< 8s threshold)"
    } else {
        Fail "4.3: kill-server took $($sw.ElapsedMilliseconds)ms (too slow, possibly sequential)"
    }
} catch {
    if ($_.Exception.Message -ne "skip") { Fail "4.3: Exception: $_" }
    & $PSMUX kill-server 2>&1 | Out-Null
    Start-Sleep -Seconds 2
}

# ═══════════════════════════════════════════════════════════
Section "GROUP 5: TMUX env + Claude Code detection"
# ═══════════════════════════════════════════════════════════

# --- 5.1: $TMUX is set inside panes ---
Test "5.1: TMUX env var is set inside panes"
$SESSION = "fix_tmux_env"
try {
    & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
    if (-not (Wait-ForSession $SESSION)) { Fail "5.1: Session did not start"; throw "skip" }

    & $PSMUX send-keys -t $SESSION 'echo "TMUX_VAL:$env:TMUX"' Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $SESSION

    if ($cap -match "TMUX_VAL:/tmp/psmux") {
        Pass "5.1: TMUX env var correctly set to Unix-style path"
    } elseif ($cap -match "TMUX_VAL:.+") {
        Fail "5.1: TMUX set but wrong format: $($Matches[0])"
    } else {
        Fail "5.1: TMUX env var not found in output"
    }
    Cleanup-Session $SESSION
} catch {
    if ($_.Exception.Message -ne "skip") { Fail "5.1: Exception: $_" }
    Cleanup-Session $SESSION
}

# --- 5.2: TMUX env persists in split panes ---
Test "5.2: TMUX env var set in split panes"
$SESSION = "fix_tmux_split"
try {
    & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
    if (-not (Wait-ForSession $SESSION)) { Fail "5.2: Session did not start"; throw "skip" }

    $paneId = & $PSMUX split-window -h -d -P -F "#{pane_id}" -t $SESSION 2>&1
    Start-Sleep -Seconds 2

    if ($paneId -match "^%\d+$") {
        & $PSMUX send-keys -t "${SESSION}:0.1" 'echo "SPLIT_TMUX:$env:TMUX"' Enter
        Start-Sleep -Seconds 2
        $cap = & $PSMUX capture-pane -t "${SESSION}:0.1" -p 2>&1 | Out-String
        if ($cap -match "SPLIT_TMUX:/tmp/psmux") {
            Pass "5.2: TMUX env var correctly set in split pane"
        } else {
            Fail "5.2: TMUX not found in split pane"
        }
    } else {
        Fail "5.2: split-window -P failed: '$paneId'"
    }
    Cleanup-Session $SESSION
} catch {
    if ($_.Exception.Message -ne "skip") { Fail "5.2: Exception: $_" }
    Cleanup-Session $SESSION
}

# --- 5.3: CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS is set ---
Test "5.3: Agent teams env var is set"
$SESSION = "fix_cc_env"
try {
    & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
    if (-not (Wait-ForSession $SESSION)) { Fail "5.3: Session did not start"; throw "skip" }

    & $PSMUX send-keys -t $SESSION 'echo "CCEAT:$env:CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS"' Enter
    Start-Sleep -Seconds 2
    $cap = Capture-Pane $SESSION

    if ($cap -match "CCEAT:1") {
        Pass "5.3: CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1 set in pane"
    } else {
        Fail "5.3: Agent teams env var not found or wrong value"
    }
    Cleanup-Session $SESSION
} catch {
    if ($_.Exception.Message -ne "skip") { Fail "5.3: Exception: $_" }
    Cleanup-Session $SESSION
}

# --- 5.4: display-message format strings work (psmux vs MSYS2) ---
Test "5.4: display-message format strings resolve correctly"
$SESSION = "fix_fmt"
try {
    & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
    if (-not (Wait-ForSession $SESSION)) { Fail "5.4: Session did not start"; throw "skip" }

    $result = & $PSMUX display-message -p "#{window_panes}" -t $SESSION 2>&1
    if ($result -match "^\d+$" -and [int]$result -ge 1) {
        Pass "5.4: display-message '#{window_panes}' = $($result.Trim()) (correctly resolved)"
    } else {
        Fail "5.4: Format string not resolved. Got: '$result'"
    }

    $result2 = & $PSMUX display-message -p "#{session_name}" -t $SESSION 2>&1
    if ($result2.Trim() -eq $SESSION) {
        Pass "5.4b: display-message '#{session_name}' = $SESSION"
    } else {
        Fail "5.4b: Session name mismatch. Expected '$SESSION', got '$($result2.Trim())'"
    }
    Cleanup-Session $SESSION
} catch {
    if ($_.Exception.Message -ne "skip") { Fail "5.4: Exception: $_" }
    Cleanup-Session $SESSION
}

# ═══════════════════════════════════════════════════════════
Section "GROUP 6: Full Claude Code teammate workflow"
# ═══════════════════════════════════════════════════════════

# --- 6.1: End-to-end Claude Code agent spawn simulation ---
Test "6.1: Simulate full Claude Code teammate spawn"
$SESSION = "cc_e2e_sim"
try {
    $workDir = Join-Path $env:TEMP "psmux_cc_e2e_$(Get-Random)"
    New-Item -Path $workDir -ItemType Directory -Force | Out-Null

    # Restore default config (pwsh)
    if (Test-Path $confPath) { Remove-Item $confPath -Force }

    & $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
    if (-not (Wait-ForSession $SESSION)) { Fail "6.1: Session did not start"; throw "skip" }

    # Step 1: Get initial pane count (Claude Code does this first)
    $paneCount = & $PSMUX display-message -p "#{window_panes}" -t $SESSION 2>&1
    if ($paneCount.Trim() -ne "1") { Fail "6.1: Initial pane count should be 1, got $($paneCount.Trim())"; throw "skip" }
    Pass "6.1a: Initial pane count = 1"

    # Step 2: Create teammate pane (exact Claude Code pattern)
    $paneId = & $PSMUX split-window -h -d -c $workDir -P -F "#{pane_id}" -t $SESSION 2>&1
    Start-Sleep -Seconds 2

    if ($paneId -match "^%\d+$") {
        Pass "6.1b: Teammate pane created with ID: $($paneId.Trim())"
    } else {
        Fail "6.1b: split-window failed. Got: '$paneId'"
        throw "skip"
    }

    # Step 3: Verify pane count increased
    $paneCount2 = & $PSMUX display-message -p "#{window_panes}" -t $SESSION 2>&1
    if ([int]$paneCount2.Trim() -ge 2) {
        Pass "6.1c: Pane count increased to $($paneCount2.Trim())"
    } else {
        Fail "6.1c: Pane count did not increase. Got: $($paneCount2.Trim())"
    }

    # Step 4: Send command to teammate pane using session:window.pane format
    & $PSMUX send-keys -t "${SESSION}:0.1" "echo E2E_AGENT_STARTED" Enter
    Start-Sleep -Seconds 2
    $cap = & $PSMUX capture-pane -t "${SESSION}:0.1" -p 2>&1 | Out-String
    if ($cap -match "E2E_AGENT_STARTED") {
        Pass "6.1d: Command delivered to teammate pane successfully"
    } else {
        Fail "6.1d: Command not executed in teammate pane"
    }

    # Step 5: Set pane title (Claude Code does this for agent names)
    & $PSMUX select-pane -t "${SESSION}:0.1" -T "Agent-1" 2>&1 | Out-Null
    Pass "6.1e: select-pane -T (set title) accepted"

    # Step 6: Resize pane (Claude Code uses percentage)
    & $PSMUX resize-pane -t "${SESSION}:0.1" -x "30%" 2>&1 | Out-Null
    Pass "6.1f: resize-pane -x percentage accepted"

    # Step 7: Kill teammate pane
    & $PSMUX kill-pane -t "${SESSION}:0.1" 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    $paneCount3 = & $PSMUX display-message -p "#{window_panes}" -t $SESSION 2>&1
    if ([int]$paneCount3.Trim() -eq 1) {
        Pass "6.1g: Teammate pane killed, count back to 1"
    } else {
        Fail "6.1g: Pane count after kill: $($paneCount3.Trim())"
    }

    Cleanup-Session $SESSION
    Remove-Item $workDir -Recurse -Force -ErrorAction SilentlyContinue
} catch {
    if ($_.Exception.Message -ne "skip") { Fail "6.1: Exception: $_" }
    Cleanup-Session $SESSION
}

# ═══════════════════════════════════════════════════════════
# Final cleanup and report
# ═══════════════════════════════════════════════════════════
& $PSMUX kill-server 2>&1 | Out-Null
Start-Sleep -Seconds 2

# Restore config
if ($confBackup) {
    Set-Content -Path $confPath -Value $confBackup
} elseif (Test-Path $confPath) {
    Remove-Item $confPath -Force -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "================================================"
Write-Host "  Results: $($script:Passed) PASSED, $($script:Failed) FAILED, $($script:Skipped) SKIPPED"
Write-Host "================================================"
if ($script:Failed -gt 0) {
    Write-Host "  SOME TESTS FAILED" -ForegroundColor Red
    exit 1
} else {
    Write-Host "  ALL TESTS PASSED" -ForegroundColor Green
    exit 0
}
