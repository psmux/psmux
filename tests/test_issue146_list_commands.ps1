#!/usr/bin/env pwsh
# Test for issue #146: List commands do not work from within a psmux session
# Tests: list-panes, list-windows, list-clients, list-commands, show-hooks
# Both external CLI and internal command prompt execution paths.

$ErrorActionPreference = "Continue"
$results = @()

function Add-Result($name, $pass, $detail="") {
    $script:results += [PSCustomObject]@{
        Test=$name
        Result=if($pass){"PASS"}else{"FAIL"}
        Detail=$detail
    }
    # Standard per-check marker line so the test runner can count results
    if ($pass) { Write-Host "  [PASS] $name" -ForegroundColor Green }
    else       { Write-Host "  [FAIL] $name" -ForegroundColor Red }
}

$SESSION = "test146_$PID"

try {
    # Clean up any leftover session
    psmux kill-session -t $SESSION 2>$null
    Start-Sleep -Milliseconds 500

    # Create a detached session
    psmux new-session -d -s $SESSION -x 120 -y 30
    Start-Sleep -Seconds 3

    # ---- Test 1: list-windows via external CLI ----
    $lsw = psmux list-windows -t $SESSION 2>&1 | Out-String
    $pass = $lsw -match "\d+:.*panes\)" -or $lsw -match "\d+:.*\*"
    Add-Result "list-windows (external CLI)" $pass "Output: $($lsw.Trim())"

    # ---- Test 2: list-panes via external CLI ----
    $lsp = psmux list-panes -t $SESSION 2>&1 | Out-String
    $pass = $lsp -match "\d+:.*\[.*x.*\]"
    Add-Result "list-panes (external CLI)" $pass "Output: $($lsp.Trim())"

    # ---- Test 3: list-clients via external CLI ----
    # The session was created with -d and no client ever attached, so tmux parity
    # (and deliberate post-issue-434 behavior: no ghost client rows) is EMPTY
    # output with exit code 0. Empty output is the PASS condition here.
    $lsc = psmux list-clients -t $SESSION 2>&1 | Out-String
    $lscExit = $LASTEXITCODE
    $pass = ($lsc.Trim().Length -eq 0) -and ($lscExit -eq 0)
    Add-Result "list-clients (external CLI, detached session: empty output, exit 0)" $pass "Exit: $lscExit Output: '$($lsc.Trim())'"

    # ---- Test 4: show-hooks via external CLI ----
    $hooks = psmux show-hooks -t $SESSION 2>&1 | Out-String
    $hooksRc = $LASTEXITCODE
    # Like tmux, a session with no hooks of its own prints nothing (no "(no hooks)" line).
    # The command must still succeed and print no error.
    $pass = ($hooksRc -eq 0) -and ($hooks -notmatch 'unknown|error|no server')
    Add-Result "show-hooks (external CLI)" $pass "Output: $($hooks.Trim())"

    # ---- Test 5: list-commands via external CLI ----
    $lscm = psmux list-commands 2>&1 | Out-String
    $pass = $lscm -match "list-windows" -and $lscm -match "split-window"
    Add-Result "list-commands (external CLI)" $pass "Contains expected commands: $([bool]($lscm -match 'list-windows'))"

    # ---- Test 6: Verify list-windows works with aliases ----
    $lswa = psmux lsw -t $SESSION 2>&1 | Out-String
    $pass = $lswa -match "\d+:" -or $lswa.Trim().Length -gt 0
    Add-Result "lsw alias (external CLI)" $pass "Output: $($lswa.Trim())"

    # ---- Test 7: Verify list-panes works with aliases ----
    $lspa = psmux lsp -t $SESSION 2>&1 | Out-String
    $pass = $lspa -match "\d+:" -or $lspa.Trim().Length -gt 0
    Add-Result "lsp alias (external CLI)" $pass "Output: $($lspa.Trim())"

    # ---- Test 8: Internal command dispatch via send-keys to command prompt ----
    # Send prefix + : to open command prompt, then type list-windows and press Enter
    # The output should appear in a popup
    psmux send-keys -t $SESSION C-b 2>$null
    Start-Sleep -Milliseconds 500
    psmux send-keys -t $SESSION : 2>$null
    Start-Sleep -Milliseconds 500
    psmux send-keys -t $SESSION "list-windows" Enter 2>$null
    Start-Sleep -Seconds 2

    # Capture the pane to check if popup rendered (the popup title should appear)
    $cap = psmux capture-pane -t $SESSION -p 2>&1 | Out-String
    # The popup should show list-windows output or at least the session should still be alive
    $alive = psmux has-session -t $SESSION 2>&1
    $pass = $LASTEXITCODE -eq 0
    Add-Result "list-windows (command prompt, session alive)" $pass "Session still running after command"

    # Press q/Esc to dismiss any popup
    psmux send-keys -t $SESSION q 2>$null
    Start-Sleep -Milliseconds 500

    # ---- Test 9: list-panes from command prompt ----
    psmux send-keys -t $SESSION C-b 2>$null
    Start-Sleep -Milliseconds 500
    psmux send-keys -t $SESSION : 2>$null
    Start-Sleep -Milliseconds 500
    psmux send-keys -t $SESSION "list-panes" Enter 2>$null
    Start-Sleep -Seconds 2

    $alive = psmux has-session -t $SESSION 2>&1
    $pass = $LASTEXITCODE -eq 0
    Add-Result "list-panes (command prompt, session alive)" $pass "Session still running after command"

    psmux send-keys -t $SESSION q 2>$null
    Start-Sleep -Milliseconds 500

    # ---- Test 10: show-hooks from command prompt ----
    psmux send-keys -t $SESSION C-b 2>$null
    Start-Sleep -Milliseconds 500
    psmux send-keys -t $SESSION : 2>$null
    Start-Sleep -Milliseconds 500
    psmux send-keys -t $SESSION "show-hooks" Enter 2>$null
    Start-Sleep -Seconds 2

    $alive = psmux has-session -t $SESSION 2>&1
    $pass = $LASTEXITCODE -eq 0
    Add-Result "show-hooks (command prompt, session alive)" $pass "Session still running after command"

    psmux send-keys -t $SESSION q 2>$null
    Start-Sleep -Milliseconds 500

    # ---- Test 11: list-clients from command prompt ----
    psmux send-keys -t $SESSION C-b 2>$null
    Start-Sleep -Milliseconds 500
    psmux send-keys -t $SESSION : 2>$null
    Start-Sleep -Milliseconds 500
    psmux send-keys -t $SESSION "list-clients" Enter 2>$null
    Start-Sleep -Seconds 2

    $alive = psmux has-session -t $SESSION 2>&1
    $pass = $LASTEXITCODE -eq 0
    Add-Result "list-clients (command prompt, session alive)" $pass "Session still running after command"

    psmux send-keys -t $SESSION q 2>$null
    Start-Sleep -Milliseconds 500

    # ---- Test 12: Split panes then list-panes should show multiple ----
    psmux split-window -t $SESSION 2>$null
    Start-Sleep -Seconds 2

    $lsp2 = psmux list-panes -t $SESSION 2>&1 | Out-String
    $lines = ($lsp2.Trim() -split "`n").Count
    $pass = $lines -ge 2
    Add-Result "list-panes after split (2+ panes)" $pass "Pane count lines: $lines"

} finally {
    # Cleanup
    psmux kill-session -t $SESSION 2>$null
    Start-Sleep -Milliseconds 500
}

# Summary
Write-Host ""
Write-Host "=== Issue #146: List Commands Test Results ==="
$results | Format-Table -AutoSize
$fail = ($results | Where-Object { $_.Result -eq "FAIL" }).Count
$total = $results.Count
Write-Host "  Passed: $($total - $fail)" -ForegroundColor Green
Write-Host "  Failed: $fail" -ForegroundColor $(if ($fail -gt 0) {"Red"} else {"Green"})
exit $fail
