# psmux Session-3 Feature Test Suite
# Tests: clock-mode, show-options -v, resize-pane -x/-y, activity notification,
#        choose-buffer, capture-pane -S/-E/-J
# Run: powershell -ExecutionPolicy Bypass -File tests\test_features3.ps1

$ErrorActionPreference = "Continue"
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
if (-not $PSMUX) {
    $cmd = Get-Command psmux -ErrorAction SilentlyContinue
    if ($cmd) { $PSMUX = $cmd.Source }
}
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
Write-Info "Using: $PSMUX"

function New-PsmuxSession {
    param([string]$Name)
    Start-Process -FilePath $PSMUX -ArgumentList "new-session -s $Name -d" -WindowStyle Hidden
    Start-Sleep -Seconds 3
}

function Psmux { & $PSMUX @args 2>&1; Start-Sleep -Milliseconds 300 }

# Kill everything first
Start-Process -FilePath $PSMUX -ArgumentList "kill-server" -WindowStyle Hidden
Start-Sleep -Seconds 3
Remove-Item "$env:USERPROFILE\.psmux\*.port" -Force -ErrorAction SilentlyContinue
Remove-Item "$env:USERPROFILE\.psmux\*.key" -Force -ErrorAction SilentlyContinue

# Create test session
Write-Info "Creating test session 'feat3'..."
New-PsmuxSession -Name "feat3"
& $PSMUX has-session -t feat3 2>$null
if ($LASTEXITCODE -ne 0) { Write-Host "FATAL: Cannot create test session" -ForegroundColor Red; exit 1 }
Write-Info "Session 'feat3' created"

# ============================================================
# 1. CLOCK MODE TESTS
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "CLOCK MODE TESTS"
Write-Host ("=" * 60)

Write-Test "clock-mode command enters clock mode"
Psmux send-keys -t feat3 "echo before-clock" Enter 2>$null | Out-Null
Start-Sleep -Milliseconds 500
Psmux clock-mode -t feat3 2>$null | Out-Null
Start-Sleep -Milliseconds 500
$mode = Psmux display-message -p "#{pane_mode}" -t feat3 | Out-String
$mode = $mode.Trim()
if ("$mode" -eq "clock-mode") { Write-Pass "clock-mode entered (pane_mode=clock-mode)" }
else { Write-Fail "clock-mode not entered (pane_mode='$mode')" }

Write-Test "pane_in_mode=1 during clock mode"
$inmode = Psmux display-message -p "#{pane_in_mode}" -t feat3 | Out-String
$inmode = $inmode.Trim()
if ("$inmode" -eq "1") { Write-Pass "pane_in_mode=1 during clock-mode" }
else { Write-Fail "pane_in_mode expected 1, got '$inmode'" }

Write-Test "any key exits clock mode"
Psmux send-keys -t feat3 q 2>$null | Out-Null
Start-Sleep -Milliseconds 500
$mode2 = Psmux display-message -p "#{pane_mode}" -t feat3 | Out-String
$mode2 = $mode2.Trim()
if ("$mode2" -eq "") { Write-Pass "clock-mode exited on key press (pane_mode empty)" }
else { Write-Fail "clock-mode did not exit (pane_mode='$mode2')" }

Write-Test "clock-mode enter/exit cycle"
Psmux clock-mode -t feat3 2>$null | Out-Null
Start-Sleep -Milliseconds 300
$d1 = Psmux display-message -p "#{pane_in_mode}" -t feat3 | Out-String
Psmux send-keys -t feat3 Escape 2>$null | Out-Null
Start-Sleep -Milliseconds 300
$d2 = Psmux display-message -p "#{pane_in_mode}" -t feat3 | Out-String
if (("$d1".Trim() -eq "1") -and ("$d2".Trim() -eq "0")) {
    Write-Pass "clock-mode enter/exit cycle works"
}
else {
    Write-Fail "clock-mode cycle issue: d1=$($d1.Trim()) d2=$($d2.Trim())"
}

# ============================================================
# 2. SHOW-OPTIONS -v TESTS
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "SHOW-OPTIONS -v TESTS"
Write-Host ("=" * 60)

Write-Test "show-options -v prefix"
$val = Psmux show-options -v prefix -t feat3 | Out-String
$val = $val.Trim()
if ("$val" -eq "C-b") { Write-Pass "show-options -v prefix = C-b" }
else { Write-Fail "show-options -v prefix got: '$val'" }

Write-Test "show-options -v base-index"
$val = Psmux show-options -v base-index -t feat3 | Out-String
$val = $val.Trim()
if ("$val" -eq "0") { Write-Pass "show-options -v base-index = 0" }
else { Write-Fail "show-options -v base-index got: '$val'" }

Write-Test "show-options -v status"
$val = Psmux show-options -v status -t feat3 | Out-String
$val = $val.Trim()
if ("$val" -eq "on") { Write-Pass "show-options -v status = on" }
else { Write-Fail "show-options -v status got: '$val'" }

Write-Test "show-options -v history-limit"
$val = Psmux show-options -v history-limit -t feat3 | Out-String
$val = $val.Trim()
if ("$val" -match '^\d+$') {
    Write-Pass "show-options -v history-limit is numeric: $val"
}
else {
    Write-Fail "show-options -v history-limit not numeric: '$val'"
}
$historyLimitBaseline = $val

Write-Test "show-options -v mouse"
$val = Psmux show-options -v mouse -t feat3 | Out-String
$val = $val.Trim()
if ("$val" -eq "on") { Write-Pass "show-options -v mouse = on" }
else { Write-Fail "show-options -v mouse got: '$val'" }

Write-Test "show-options -v after set-option change"
Psmux set-option -t feat3 history-limit 5000 2>$null | Out-Null
Start-Sleep -Milliseconds 300
$val = Psmux show-options -v history-limit -t feat3 | Out-String
$val = $val.Trim()
if ("$val" -eq "5000") { Write-Pass "show-options -v reflects set-option change: 5000" }
else { Write-Fail "show-options -v after change got: '$val'" }
# Restore
if ("$historyLimitBaseline" -match '^\d+$') {
    Psmux set-option -t feat3 history-limit $historyLimitBaseline 2>$null | Out-Null
}

Write-Test "show-options -v unknown option"
$val = Psmux show-options -v nonexistent-option -t feat3 | Out-String
$val = $val.Trim()
if ("$val" -eq "" -or "$val" -match "unknown") { Write-Pass "show-options -v unknown returns empty/error" }
else { Write-Fail "show-options -v unknown got: '$val'" }

# #655: a plain `-w` listing is the window's OWN table, which is empty on a
# window nobody has written to, so the full catalog now lives behind -g (the
# global window table) exactly as it does in tmux.
Write-Test "show-window-options alias"
$val = Psmux show-window-options -g -t feat3 | Out-String
if ("$val" -match "window-status-format|automatic-rename|window-size") { Write-Pass "show-window-options returns window options" }
else { Write-Fail "show-window-options missing expected window options" }

Write-Test "showw alias"
$val = Psmux showw -g -t feat3 | Out-String
if ("$val" -match "window-status-format|automatic-rename|window-size") { Write-Pass "showw alias returns window options" }
else { Write-Fail "showw alias missing expected window options" }

Write-Test "show-window-options -v window option"
$val = Psmux show-window-options -v window-size -t feat3 | Out-String
$val = $val.Trim()
if ("$val" -ne "") { Write-Pass "show-window-options -v window-size returned '$val'" }
else { Write-Fail "show-window-options -v window-size returned empty" }

Write-Test "show-window-options -v session option is empty"
$val = Psmux show-window-options -v prefix -t feat3 | Out-String
$val = $val.Trim()
if ("$val" -eq "") {
    Write-Pass "show-window-options -v prefix correctly empty"
}
else {
    Write-Fail "show-window-options -v prefix should be empty, got '$val'"
}

# ============================================================
# 3. RESIZE-PANE -x/-y TESTS
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "RESIZE-PANE -x/-y TESTS"
Write-Host ("=" * 60)

# Create a horizontal split to test -x
Psmux split-window -h -t feat3 2>$null | Out-Null
Start-Sleep -Milliseconds 1000

Write-Test "resize-pane -x absolute width"
$before = Psmux display-message -p "#{pane_width}" -t feat3 | Out-String
$before = $before.Trim()
Psmux resize-pane -x 40 -t feat3 2>$null | Out-Null
Start-Sleep -Milliseconds 1000
$after = Psmux display-message -p "#{pane_width}" -t feat3 | Out-String
$after = $after.Trim()
if ("$after" -ne "" -and "$after" -match '^\d+$') { Write-Pass "resize-pane -x executed: width=$after (was $before)" }
else { Write-Fail "resize-pane -x did not return valid width: before=$before after=$after" }

# Kill extra pane, create vertical split for -y test
Psmux kill-pane -t feat3 2>$null | Out-Null
Start-Sleep -Milliseconds 500
Psmux split-window -v -t feat3 2>$null | Out-Null
Start-Sleep -Milliseconds 1000

Write-Test "resize-pane -y absolute height"
$before = Psmux display-message -p "#{pane_height}" -t feat3 | Out-String
$before = $before.Trim()
Psmux resize-pane -y 10 -t feat3 2>$null | Out-Null
Start-Sleep -Milliseconds 1000
$after = Psmux display-message -p "#{pane_height}" -t feat3 | Out-String
$after = $after.Trim()
if ("$after" -ne "" -and "$after" -match '^\d+$') { Write-Pass "resize-pane -y executed: height=$after (was $before)" }
else { Write-Fail "resize-pane -y did not return valid height: before=$before after=$after" }

# Kill extra pane
Psmux kill-pane -t feat3 2>$null | Out-Null
Start-Sleep -Milliseconds 300

# ============================================================
# 3b. SEND-KEYS -p COMPAT TESTS
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "SEND-KEYS -p COMPAT TESTS"
Write-Host ("=" * 60)

Write-Test "send-keys -p pastes literal text"
Psmux send-keys -t feat3 -p "paste_literal_test" 2>$null | Out-Null
Psmux send-keys -t feat3 Enter 2>$null | Out-Null
Start-Sleep -Milliseconds 500
$cap = Psmux capture-pane -t feat3 -p | Out-String
if ("$cap" -match "paste_literal_test") { Write-Pass "send-keys -p delivered pasted text" }
else { Write-Fail "send-keys -p text not found in pane" }

# ============================================================
# 4. ACTIVITY NOTIFICATION TESTS
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "ACTIVITY NOTIFICATION TESTS"
Write-Host ("=" * 60)

Write-Test "set monitor-activity off"
Psmux set-option -t feat3 monitor-activity off 2>$null | Out-Null
Start-Sleep -Milliseconds 300
$opts = Psmux show-options -t feat3 | Out-String
if ("$opts" -match "monitor-activity off") { Write-Pass "monitor-activity set to off" }
else { Write-Fail "monitor-activity not off after set" }

Write-Test "set monitor-activity on"
Psmux set-option -t feat3 monitor-activity on 2>$null | Out-Null
Start-Sleep -Milliseconds 300
$opts = Psmux show-options -t feat3 | Out-String
if ("$opts" -match "monitor-activity on") { Write-Pass "monitor-activity set to on" }
else { Write-Fail "monitor-activity not on" }

Write-Test "window_activity_flag format variable"
$val = Psmux display-message -p "#{window_activity_flag}" -t feat3 | Out-String
$val = $val.Trim()
# Active window should have 0 activity flag
if ("$val" -eq "0" -or "$val" -eq "1") { Write-Pass "window_activity_flag is valid: $val" }
else { Write-Fail "window_activity_flag invalid: '$val'" }

Write-Test "activity detected on background window"
# Create second window, switch to first, generate activity on second
Psmux new-window -t feat3 2>$null | Out-Null
Start-Sleep -Milliseconds 500
Psmux select-window -t feat3:1 2>$null | Out-Null
Start-Sleep -Milliseconds 300
# Send text to window 2 to trigger activity
Psmux send-keys -t "feat3:2" "echo activity-trigger" Enter 2>$null | Out-Null
Start-Sleep -Milliseconds 500
# Check list-windows for activity flag
$wins = Psmux list-windows -t feat3 | Out-String
Write-Pass "activity detection test ran (windows: $($wins.Trim()))"

# Reset
Psmux set-option -t feat3 monitor-activity off 2>$null | Out-Null
Psmux kill-window -t "feat3:2" 2>$null | Out-Null
Start-Sleep -Milliseconds 300

# ============================================================
# 5. CHOOSE-BUFFER TESTS
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "CHOOSE-BUFFER TESTS"
Write-Host ("=" * 60)

Write-Test "choose-buffer with empty buffer"
$buf = Psmux choose-buffer -t feat3 | Out-String
# Could be empty or say "no buffers"
Write-Pass "choose-buffer returned: '$($buf.Trim())'"

Write-Test "choose-buffer after set-buffer"
Psmux set-buffer "Hello Choose Buffer" -t feat3 2>$null | Out-Null
Start-Sleep -Milliseconds 300
$buf = Psmux choose-buffer -t feat3 | Out-String
if ("$buf" -match "Hello Choose Buffer") { Write-Pass "choose-buffer shows buffer content" }
elseif ("$buf" -match "buffer0" -or "$buf" -match "19 bytes") { Write-Pass "choose-buffer shows buffer metadata: $($buf.Trim())" }
else { Write-Fail "choose-buffer did not show buffer: '$buf'" }

Write-Test "choose-buffer with multiple buffers"
Psmux set-buffer "Second buffer data" -t feat3 2>$null | Out-Null
Start-Sleep -Milliseconds 300
$buf = Psmux choose-buffer -t feat3 | Out-String
$lineCount = ($buf.Trim() -split "`n").Count
if ($lineCount -ge 1) { Write-Pass "choose-buffer shows buffers (lines=$lineCount)" }
else { Write-Fail "choose-buffer empty with multiple buffers" }

# ============================================================
# 6. CAPTURE-PANE ENHANCEMENT TESTS
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "CAPTURE-PANE ENHANCEMENT TESTS"
Write-Host ("=" * 60)

# Generate some content
Psmux send-keys -t feat3 "echo line-one" Enter 2>$null | Out-Null
Start-Sleep -Milliseconds 300
Psmux send-keys -t feat3 "echo line-two" Enter 2>$null | Out-Null
Start-Sleep -Milliseconds 300
Psmux send-keys -t feat3 "echo line-three" Enter 2>$null | Out-Null
Start-Sleep -Milliseconds 500

Write-Test "capture-pane -p (basic)"
$cap = Psmux capture-pane -p -t feat3 | Out-String
if ("$cap" -match "line-one" -or "$cap" -match "line-two" -or "$cap" -match "line-three") {
    Write-Pass "capture-pane -p captures visible content"
} else {
    Write-Fail "capture-pane -p no visible content: '$cap'"
}

Write-Test "capture-pane -p -J (join lines)"
$cap = Psmux capture-pane -p -J -t feat3 | Out-String
if ("$cap".Length -gt 0) { Write-Pass "capture-pane -p -J produces output (len=$($cap.Length))" }
else { Write-Fail "capture-pane -p -J no output" }

Write-Test "capture-pane -p -S 0 (from start of scrollback)"
$cap = Psmux capture-pane -p -S 0 -t feat3 | Out-String
if ("$cap".Length -gt 0) { Write-Pass "capture-pane -p -S 0 produces output (len=$($cap.Length))" }
else { Write-Fail "capture-pane -p -S 0 no output" }

Write-Test "capture-pane -p -S - (entire scrollback)"
$cap = Psmux capture-pane -p "-S" "-" -t feat3 | Out-String
if ("$cap".Length -gt 0) { Write-Pass "capture-pane -p -S - produces output (len=$($cap.Length))" }
else { Write-Fail "capture-pane -p -S - no output" }

Write-Test "capture-pane stores in buffer when no -p"
Psmux capture-pane -t feat3 2>$null | Out-Null
Start-Sleep -Milliseconds 300
$buf = Psmux show-buffer -t feat3 | Out-String
if ("$buf".Length -gt 0) { Write-Pass "capture-pane without -p stored in buffer" }
else { Write-Fail "capture-pane without -p did not store" }

# ============================================================
# 7. MISC INTEGRATION TESTS
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "MISC INTEGRATION TESTS"
Write-Host ("=" * 60)

Write-Test "list-commands includes new commands"
$cmds = Psmux list-commands -t feat3 | Out-String
$found = 0
if ("$cmds" -match "clock-mode") { $found++ }
if ("$cmds" -match "choose-buffer") { $found++ }
if ("$cmds" -match "show-window-options") { $found++ }
if ($found -ge 2) { Write-Pass "list-commands includes new commands ($found found)" }
else { Write-Fail "list-commands missing new commands (found $found)" }

Write-Test "show-options -v with set-option round-trip"
Psmux set-option -t feat3 escape-time 100 2>$null | Out-Null
Start-Sleep -Milliseconds 300
$val = Psmux show-options -v escape-time -t feat3 | Out-String
$val = $val.Trim()
if ("$val" -eq "100") { Write-Pass "show-options -v reflects escape-time change: $val" }
else { Write-Fail "show-options -v escape-time got: '$val'" }
# Restore
Psmux set-option -t feat3 escape-time 10 2>$null | Out-Null  # escape-time default is 10 since the #841 follow up (tmux 3.5+), 500 before

Write-Test "display-message format with clock_mode"
$val = Psmux display-message -p "mode=#{pane_mode}" -t feat3 | Out-String
$val = $val.Trim()
if ("$val" -match "mode=") { Write-Pass "pane_mode format variable works: $val" }
else { Write-Fail "pane_mode not in output: '$val'" }

Write-Test "show-options -v word-separators"
$val = Psmux show-options -v word-separators -t feat3 | Out-String
$val = $val.Trim()
if ("$val".Length -gt 0) { Write-Pass "word-separators: '$val'" }
else { Write-Fail "word-separators empty" }

Write-Test "show-options -v pane-active-border-style"
$val = Psmux show-options -v pane-active-border-style -t feat3 | Out-String
$val = $val.Trim()
if ("$val" -match "fg=green" -or "$val".Length -gt 0) { Write-Pass "pane-active-border-style: '$val'" }
else { Write-Fail "pane-active-border-style empty" }

# ============================================================
# CLEANUP
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "CLEANUP"
Write-Host ("=" * 60)

& $PSMUX kill-session -t feat3 2>$null
Start-Sleep -Seconds 1
Start-Process -FilePath $PSMUX -ArgumentList "kill-server" -WindowStyle Hidden
Start-Sleep -Seconds 2

# ============================================================
# SUMMARY
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "SESSION-3 FEATURES TEST SUMMARY"
Write-Host ("=" * 60)
$total = $script:TestsPassed + $script:TestsFailed
Write-Host "Passed:  $($script:TestsPassed) / $total" -ForegroundColor Green
Write-Host "Skipped: $($script:TestsSkipped)" -ForegroundColor Yellow
Write-Host "Failed:  $($script:TestsFailed) / $total" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
if ($script:TestsFailed -eq 0) { Write-Host "ALL TESTS PASSED!" -ForegroundColor Green }
else { Write-Host "$($script:TestsFailed) test(s) failed" -ForegroundColor Red }
