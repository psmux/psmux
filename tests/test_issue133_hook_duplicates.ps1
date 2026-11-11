#!/usr/bin/env pwsh
# test_issue133_hook_duplicates.ps1
# Validates fix for GitHub issue #133:
# - set-hook -g should replace (not append) existing hooks
# - set-hook -gu should remove hooks
# - set-hook -ga should append (multi-handler support)
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

$session = "test133_$(Get-Random)"

Write-Host "`n=== Issue #133: set-hook duplicates and -gu unset ===" -ForegroundColor Cyan

# Start a detached session
& $exe new-session -d -s $session
Start-Sleep -Milliseconds 500

# ── Test 1: set-hook replaces, not appends ──
& $exe set-hook -g client-attached "display-message first"
Start-Sleep -Milliseconds 200
& $exe set-hook -g client-attached "display-message second"
Start-Sleep -Milliseconds 200

$hooks = & $exe show-hooks -g 2>&1 | Out-String
$count = ([regex]::Matches($hooks, 'client-attached')).Count
Test "set-hook replaces existing hook (count=$count)" ($count -eq 1)
Test "set-hook has the second command" ($hooks -match 'display-message second')
Test "set-hook does NOT have the first command" ($hooks -notmatch 'display-message first')

# ── Test 2: set-hook -gu removes the hook ──
& $exe set-hook -gu client-attached
Start-Sleep -Milliseconds 200

$hooks2 = & $exe show-hooks -g 2>&1 | Out-String
Test "set-hook -gu removes hook" ($hooks2 -notmatch 'client-attached\[')

# ── Test 3: Multiple different hooks coexist ──
& $exe set-hook -g client-attached "display-message a"
Start-Sleep -Milliseconds 200
& $exe set-hook -g after-new-window "display-message b"
Start-Sleep -Milliseconds 200

$hooks3 = & $exe show-hooks -g 2>&1 | Out-String
Test "Different hooks coexist - client-attached" ($hooks3 -match 'client-attached')
Test "Different hooks coexist - after-new-window" ($hooks3 -match 'after-new-window')

# ── Test 4: Replace one hook, other stays ──
& $exe set-hook -g client-attached "display-message replaced"
Start-Sleep -Milliseconds 200

$hooks4 = & $exe show-hooks -g 2>&1 | Out-String
Test "Replace preserves other hooks - after-new-window still present" ($hooks4 -match 'after-new-window')
Test "Replace updates target hook" ($hooks4 -match 'display-message replaced')
$countReplaced = ([regex]::Matches($hooks4, 'client-attached')).Count
Test "Replace doesn't duplicate target hook (count=$countReplaced)" ($countReplaced -eq 1)

# ── Test 5: Config reload simulation (the core bug scenario) ──
# Clear first
& $exe set-hook -gu client-attached
& $exe set-hook -gu after-new-window
Start-Sleep -Milliseconds 200

# Simulate multiple config reloads setting the same hook
& $exe set-hook -g client-attached "run-shell 'echo autosave'"
Start-Sleep -Milliseconds 200
& $exe set-hook -g client-attached "run-shell 'echo autosave'"
Start-Sleep -Milliseconds 200
& $exe set-hook -g client-attached "run-shell 'echo autosave'"
Start-Sleep -Milliseconds 200

$hooks5 = & $exe show-hooks -g 2>&1 | Out-String
$countReload = ([regex]::Matches($hooks5, 'client-attached')).Count
Test "Config reload simulation: no duplicate hooks (count=$countReload)" ($countReload -eq 1)

# Cleanup
& $exe kill-session -t $session 2>$null

Write-Host "`n=== Results: $pass/$total passed, $fail failed ===" -ForegroundColor $(if ($fail -eq 0) { 'Green' } else { 'Red' })
exit $fail
