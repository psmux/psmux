#!/usr/bin/env pwsh
# test_issue133_hook_append.ps1
# Full functional test for set-hook -ga (append) behavior matching tmux semantics.
# Covers: -ga append, indexed show-hooks output, -g replace after -ga,
#          -gu clears all appended, multi-plugin simulation, -a without -g,
#          config reload with mixed -g/-ga, and event firing with multiple handlers.
$ErrorActionPreference = 'Continue'
$pass = 0; $fail = 0; $total = 0

function Test($name, $condition) {
    $script:total++
    if ($condition) {
        Write-Host "  PASS: $name" -ForegroundColor Green
        $script:pass++
    } else {
        Write-Host "  FAIL: $name" -ForegroundColor Red
        $script:fail++
    }
}

$exe = Get-Command psmux -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source
if (-not $exe) { $exe = Get-Command tmux -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source }
if (-not $exe) { $exe = Get-Command pmux -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source }
if (-not $exe) {
    Write-Host "SKIP: psmux/tmux/pmux not found" -ForegroundColor Yellow
    exit 0
}

$session = "test133ga_$(Get-Random)"

Write-Host "`n=== Issue #133 follow-up: set-hook -ga (append) full functional test ===" -ForegroundColor Cyan

# Start a detached session
& $exe new-session -d -s $session
Start-Sleep -Milliseconds 800

# ════════════════════════════════════════════════════════════════════
# Test Group 1: Basic -ga append behavior
# ════════════════════════════════════════════════════════════════════
Write-Host "`n  --- Group 1: Basic -ga append ---" -ForegroundColor Yellow

# Clean slate
& $exe set-hook -gu client-attached 2>$null
& $exe set-hook -gu after-new-window 2>$null
Start-Sleep -Milliseconds 200

# Set initial hook, then append
& $exe set-hook -g client-attached 'display-message "first"'
Start-Sleep -Milliseconds 200
& $exe set-hook -ga client-attached 'display-message "second"'
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String
$lines = ($hooks -split "`n" | Where-Object { $_ -match 'client-attached' })
Test "1.1 -ga creates two handlers" ($lines.Count -eq 2)
Test '1.2 First handler shows as client-attached[0]' ($hooks -match 'client-attached\[0\].*first')
Test '1.3 Second handler shows as client-attached[1]' ($hooks -match 'client-attached\[1\].*second')

# Append a third handler
& $exe set-hook -ga client-attached 'display-message "third"'
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String
$lines = ($hooks -split "`n" | Where-Object { $_ -match 'client-attached' })
Test "1.4 Three handlers after second -ga" ($lines.Count -eq 3)
Test '1.5 Third handler shows as client-attached[2]' ($hooks -match 'client-attached\[2\].*third')

# ════════════════════════════════════════════════════════════════════
# Test Group 2: -ga on nonexistent hook creates it
# ════════════════════════════════════════════════════════════════════
Write-Host "`n  --- Group 2: -ga creates hook if missing ---" -ForegroundColor Yellow

& $exe set-hook -gu after-new-window 2>$null
Start-Sleep -Milliseconds 200

& $exe set-hook -ga after-new-window 'display-message "created-by-ga"'
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String
Test "2.1 -ga on missing hook creates it" ($hooks -match 'after-new-window')
Test "2.2 Contains correct command" ($hooks -match 'created-by-ga')
# Single handler should NOT use indexed format
# tmux indexes every array element, a single one too: name[0] command.
Test '2.3 Single handler is listed as element 0' ($hooks -match '(?m)^after-new-window\[0\] display-message.*created-by-ga' -and $hooks -notmatch 'after-new-window\[1\]')

# ════════════════════════════════════════════════════════════════════
# Test Group 3: -g (replace) clears all appended handlers
# ════════════════════════════════════════════════════════════════════
Write-Host "`n  --- Group 3: -g replaces all -ga handlers ---" -ForegroundColor Yellow

# client-attached currently has 3 handlers from group 1
& $exe set-hook -g client-attached 'display-message "replaced-all"'
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String
$lines = ($hooks -split "`n" | Where-Object { $_ -match 'client-attached' })
Test "3.1 -g replaces all appended handlers (count=$($lines.Count))" ($lines.Count -eq 1)
Test "3.2 Only the replacement command remains" ($hooks -match 'replaced-all')
# tmux indexes a single element too, so the replacement is element 0 and nothing else.
Test '3.3 Replacement is the only element, [0]' ($hooks -match '(?m)^client-attached\[0\] ' -and $hooks -notmatch 'client-attached\[1\]')

# ════════════════════════════════════════════════════════════════════
# Test Group 4: -gu removes all appended handlers
# ════════════════════════════════════════════════════════════════════
Write-Host "`n  --- Group 4: -gu removes all handlers ---" -ForegroundColor Yellow

# Set up multiple handlers
& $exe set-hook -g client-attached 'display-message "a"'
Start-Sleep -Milliseconds 200
& $exe set-hook -ga client-attached 'display-message "b"'
Start-Sleep -Milliseconds 200
& $exe set-hook -ga client-attached 'display-message "c"'
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String
$pre = ($hooks -split "`n" | Where-Object { $_ -match 'client-attached' }).Count
Test "4.1 Pre-check: 3 handlers before -gu" ($pre -eq 3)

& $exe set-hook -gu client-attached
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String
# An unset hook stays listed under -g as a bare name, as in tmux.
Test "4.2 -gu removes ALL handlers" ($hooks -notmatch 'client-attached\[')

# ════════════════════════════════════════════════════════════════════
# Test Group 5: Multi-plugin simulation (the real-world use case)
# ════════════════════════════════════════════════════════════════════
Write-Host "`n  --- Group 5: Multi-plugin simulation ---" -ForegroundColor Yellow

# Clean everything
& $exe set-hook -gu client-attached 2>$null
& $exe set-hook -gu after-new-window 2>$null
Start-Sleep -Milliseconds 200

# Plugin A registers its hook
& $exe set-hook -g client-attached 'run-shell "echo plugin-a-autosave"'
Start-Sleep -Milliseconds 200

# Plugin B appends its own handler for the same event
& $exe set-hook -ga client-attached 'run-shell "echo plugin-b-status"'
Start-Sleep -Milliseconds 200

# Plugin C also appends
& $exe set-hook -ga client-attached 'run-shell "echo plugin-c-notify"'
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String
$lines = ($hooks -split "`n" | Where-Object { $_ -match 'client-attached' })
Test "5.1 All three plugin handlers coexist" ($lines.Count -eq 3)
Test "5.2 Plugin A handler present" ($hooks -match 'plugin-a-autosave')
Test "5.3 Plugin B handler present" ($hooks -match 'plugin-b-status')
Test "5.4 Plugin C handler present" ($hooks -match 'plugin-c-notify')

# Now simulate config reload: Plugin A re-registers with -g (should replace only)
& $exe set-hook -g client-attached 'run-shell "echo plugin-a-autosave-v2"'
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String
$lines = ($hooks -split "`n" | Where-Object { $_ -match 'client-attached' })
# After -g replace, only the new single handler should remain
Test "5.5 Config reload with -g replaces all (count=$($lines.Count))" ($lines.Count -eq 1)
Test "5.6 New version of Plugin A handler" ($hooks -match 'plugin-a-autosave-v2')

# ════════════════════════════════════════════════════════════════════
# Test Group 6: -a flag without -g
# ════════════════════════════════════════════════════════════════════
Write-Host "`n  --- Group 6: -a without -g ---" -ForegroundColor Yellow

& $exe set-hook -gu client-attached 2>$null
Start-Sleep -Milliseconds 200

& $exe set-hook client-attached 'display-message "no-flag-set"'
Start-Sleep -Milliseconds 200
& $exe set-hook -a client-attached 'display-message "a-only-append"'
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String
$lines = ($hooks -split "`n" | Where-Object { $_ -match 'client-attached' })
Test "6.1 -a without -g also appends" ($lines.Count -eq 2)
Test "6.2 Original handler present" ($hooks -match 'no-flag-set')
Test "6.3 Appended handler present" ($hooks -match 'a-only-append')

# ════════════════════════════════════════════════════════════════════
# Test Group 7: Different hook names with -ga don't interfere
# ════════════════════════════════════════════════════════════════════
Write-Host "`n  --- Group 7: Different hooks with -ga isolation ---" -ForegroundColor Yellow

& $exe set-hook -gu client-attached 2>$null
& $exe set-hook -gu after-new-window 2>$null
& $exe set-hook -gu client-detached 2>$null
Start-Sleep -Milliseconds 200

& $exe set-hook -g client-attached 'display-message "attach-1"'
Start-Sleep -Milliseconds 200
& $exe set-hook -ga client-attached 'display-message "attach-2"'
Start-Sleep -Milliseconds 200
& $exe set-hook -g after-new-window 'display-message "newwin-1"'
Start-Sleep -Milliseconds 200
& $exe set-hook -ga after-new-window 'display-message "newwin-2"'
Start-Sleep -Milliseconds 200
& $exe set-hook -g client-detached 'display-message "detach-only"'
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String

$attachLines = ($hooks -split "`n" | Where-Object { $_ -match 'client-attached' })
$newwinLines = ($hooks -split "`n" | Where-Object { $_ -match 'after-new-window' })
$detachLines = ($hooks -split "`n" | Where-Object { $_ -match 'client-detached' })

Test "7.1 client-attached has 2 handlers" ($attachLines.Count -eq 2)
Test "7.2 after-new-window has 2 handlers" ($newwinLines.Count -eq 2)
Test "7.3 client-detached has 1 handler (plain format)" ($detachLines.Count -eq 1)
Test '7.4 client-detached is listed as element 0' ($hooks -match 'client-detached\[0\] ' -and $hooks -notmatch 'client-detached\[1\]')

# Removing one hook doesn't affect others
& $exe set-hook -gu after-new-window
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String
Test "7.5 Removing after-new-window doesn't affect client-attached" ($hooks -match 'client-attached')
Test "7.6 after-new-window is gone" ($hooks -notmatch 'after-new-window\[')
Test "7.7 client-detached still present" ($hooks -match 'client-detached')

# ════════════════════════════════════════════════════════════════════
# Test Group 8: show-hooks output format correctness
# ════════════════════════════════════════════════════════════════════
Write-Host "`n  --- Group 8: show-hooks output format ---" -ForegroundColor Yellow

& $exe set-hook -gu client-attached 2>$null
& $exe set-hook -gu client-detached 2>$null
& $exe set-hook -gu after-new-window 2>$null
Start-Sleep -Milliseconds 200

# Empty state
$hooks = & $exe show-hooks -g 2>&1 | Out-String
# tmux lists every session hook under -g, empty ones as bare names.
Test "8.1 Empty hooks: bare names, no commands" ($hooks -match '(?m)^client-attached\s*$' -and $hooks -notmatch '\[\d+\]' -and $hooks -notmatch 'no hooks')

# Single handler: plain format
& $exe set-hook -g client-attached 'display-message "solo"'
Start-Sleep -Milliseconds 200
$hooks = & $exe show-hooks -g 2>&1 | Out-String
Test '8.2 Single handler: tmux format "name[0] cmd"' ($hooks -match '(?m)^client-attached\[0\] display-message')
Test '8.3 Single handler: no second element' ($hooks -notmatch 'client-attached\[1\]')

# Multi handler: indexed format
& $exe set-hook -ga client-attached 'display-message "duo"'
Start-Sleep -Milliseconds 200
$hooks = & $exe show-hooks -g 2>&1 | Out-String
Test '8.4 Multi handler: indexed format "name[0] cmd"' ($hooks -match 'client-attached\[0\] \S')
Test '8.5 Multi handler: indexed format "name[1] cmd"' ($hooks -match 'client-attached\[1\] \S')

# ════════════════════════════════════════════════════════════════════
# Test Group 9: Continuum-style reload scenario (the original bug)
# ════════════════════════════════════════════════════════════════════
Write-Host "`n  --- Group 9: Continuum-style reload (original bug scenario) ---" -ForegroundColor Yellow

& $exe set-hook -gu client-attached 2>$null
Start-Sleep -Milliseconds 200

# Simulate what psmux-continuum does: set-hook -g on each config reload
# With the fix, repeated -g should NOT duplicate
for ($i = 0; $i -lt 5; $i++) {
    & $exe set-hook -g client-attached 'run-shell "echo continuum-autosave-loop"'
    Start-Sleep -Milliseconds 100
}

$hooks = & $exe show-hooks -g 2>&1 | Out-String
$count = ([regex]::Matches($hooks, 'client-attached')).Count
Test "9.1 5 config reloads with -g: no duplicates (count=$count)" ($count -eq 1)

# But if a plugin uses -ga, it should append (not be affected by the -g dedup)
& $exe set-hook -ga client-attached 'run-shell "echo status-plugin"'
Start-Sleep -Milliseconds 200
$hooks = & $exe show-hooks -g 2>&1 | Out-String
$count = ([regex]::Matches($hooks, 'client-attached')).Count
Test "9.2 -ga after -g reloads correctly appends (count=$count)" ($count -eq 2)

# ════════════════════════════════════════════════════════════════════
# Test Group 10: -u flag (without -g prefix) also works for removal
# ════════════════════════════════════════════════════════════════════
Write-Host "`n  --- Group 10: -u removal flag variants ---" -ForegroundColor Yellow

& $exe set-hook -gu client-attached 2>$null
Start-Sleep -Milliseconds 200

& $exe set-hook -g client-attached 'display-message "to-remove"'
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String
Test "10.1 Pre-check: hook exists" ($hooks -match 'client-attached')

& $exe set-hook -u client-attached
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String
Test "10.2 -u (without g) removes hook" ($hooks -notmatch 'client-attached\[')

# Cleanup
& $exe kill-session -t $session 2>$null

Write-Host "`n=== Results: $pass/$total passed, $fail failed ===" -ForegroundColor $(if ($fail -eq 0) { 'Green' } else { 'Red' })
exit $fail
