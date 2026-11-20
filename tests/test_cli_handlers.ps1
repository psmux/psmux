#!/usr/bin/env pwsh
# test_cli_handlers.ps1
# Tests for CLI handlers that were previously missing or stub-only:
# 1. command-prompt (CLI → server)
# 2. display-menu / menu (CLI → server)
# 3. display-popup / popup (CLI → server)
# 4. display-panes / displayp (CLI → server, now functional)
# 5. server-info / info (CLI → response)
# 6. start-server / start (no-op compat)
# 7. confirm-before / confirm (CLI → server)
# 8. refresh-client / refresh (CLI → server)
# 9. send-prefix (CLI → server)
# 10. show-messages / showmsgs (CLI → response)
# 11. Platform handlers: suspend-client, lock-*, resize-window, customize-mode
# 12. choose-client, respawn-window, link-window, unlink-window

$ErrorActionPreference = "Continue"
$exe = "$PSScriptRoot\..\target\release\psmux.exe"
if (-not (Test-Path $exe)) { $exe = "$PSScriptRoot\..\target\debug\psmux.exe" }
if (-not (Test-Path $exe)) { $exe = (Get-Command psmux -ErrorAction SilentlyContinue).Source }
if (-not $exe -or -not (Test-Path $exe)) { Write-Error "psmux binary not found"; exit 1 }

# Helper: cleanup sessions
function Cleanup-All {
    & $exe kill-session -t test-cli 2>$null
    & $exe kill-session -t test-cli2 2>$null
    Start-Sleep -Milliseconds 500
}

$pass = 0
$fail = 0
$total = 0

function Test-Assert {
    param(
        [string]$Name,
        [bool]$Condition,
        [string]$Detail = ""
    )
    $script:total++
    if ($Condition) {
        $script:pass++
        Write-Host "  PASS: $Name" -ForegroundColor Green
    } else {
        $script:fail++
        Write-Host "  FAIL: $Name" -ForegroundColor Red
        if ($Detail) { Write-Host "        Detail: $Detail" -ForegroundColor Yellow }
    }
}

Write-Host "`n================================================" -ForegroundColor Cyan
Write-Host "CLI Handlers Test Suite" -ForegroundColor Cyan
Write-Host "Tests for missing/stub CLI command handlers" -ForegroundColor Cyan
Write-Host "================================================`n" -ForegroundColor Cyan

# --- Cleanup before tests ---
Cleanup-All

# Create a test session
& $exe new-session -d -s test-cli 2>$null
Start-Sleep -Seconds 2

# ============================================================
# TEST GROUP 1: server-info / info
# ============================================================
Write-Host "[Test Group 1] server-info / info" -ForegroundColor Magenta

# Test 1.1: server-info returns version info
$infoOut = & $exe -t test-cli server-info 2>&1
$infoStr = ($infoOut | Out-String).Trim()
Test-Assert "server-info returns psmux version" ($infoStr -match 'psmux') "Got: '$infoStr'"

# Test 1.2: server-info contains pid
Test-Assert "server-info contains pid" ($infoStr -match 'pid: \d+') "Got: '$infoStr'"

# Test 1.3: server-info contains session name
Test-Assert "server-info contains session name" ($infoStr -match 'session: test-cli') "Got: '$infoStr'"

# Test 1.4: server-info contains windows count
Test-Assert "server-info contains windows count" ($infoStr -match 'windows: \d+') "Got: '$infoStr'"

# Test 1.5: server-info contains uptime
Test-Assert "server-info contains uptime" ($infoStr -match 'uptime: \d+s') "Got: '$infoStr'"

# Test 1.6: server-info contains socket path
Test-Assert "server-info contains socket path" ($infoStr -match 'socket:') "Got: '$infoStr'"

# Test 1.7: 'info' alias works the same
$aliasOut = & $exe -t test-cli info 2>&1
$aliasStr = ($aliasOut | Out-String).Trim()
Test-Assert "'info' alias returns same as 'server-info'" ($aliasStr -match 'psmux') "Got: '$aliasStr'"

# ============================================================
# TEST GROUP 2: command-prompt (no-error from CLI)
# ============================================================
Write-Host "`n[Test Group 2] command-prompt (CLI acceptance)" -ForegroundColor Magenta

# The prompt opens on a client (tmux cmd-command-prompt.c status_prompt_set),
# and this session is detached: tmux answers "no current client", rc 1. The
# attached client path is covered by tests/test_cli_command_prompt.ps1.
# Test 2.1: command-prompt with no client is refused like tmux, not dropped
$cpResult = (& $exe -t test-cli command-prompt -b 2>&1 | Out-String)
$cpExitCode = $LASTEXITCODE
Test-Assert "command-prompt with no attached client: 'no current client', rc 1" ($cpExitCode -eq 1 -and $cpResult -match 'no current client') "Exit code: $cpExitCode Output: $cpResult"

# Test 2.2: the -I flag is parsed (no flag error), then the same refusal
$cpIResult = (& $exe -t test-cli command-prompt -b -I test 2>&1 | Out-String)
$cpIExitCode = $LASTEXITCODE
Test-Assert "command-prompt -I parsed, then 'no current client'" ($cpIExitCode -eq 1 -and $cpIResult -match 'no current client' -and $cpIResult -notmatch 'unknown flag|invalid') "Exit code: $cpIExitCode Output: $cpIResult"

# ============================================================
# TEST GROUP 3: refresh-client / refresh
# ============================================================
Write-Host "`n[Test Group 3] refresh-client / refresh" -ForegroundColor Magenta

# Test 3.1: refresh-client accepted
$refreshResult = & $exe -t test-cli refresh-client 2>&1
$refreshExitCode = $LASTEXITCODE
Test-Assert "refresh-client accepted without error" ($refreshExitCode -eq 0 -or $refreshExitCode -eq $null) "Exit code: $refreshExitCode"

# Test 3.2: 'refresh' alias accepted
$refreshAliasResult = & $exe -t test-cli refresh 2>&1
$refreshAliasExitCode = $LASTEXITCODE
Test-Assert "'refresh' alias accepted" ($refreshAliasExitCode -eq 0 -or $refreshAliasExitCode -eq $null) "Exit code: $refreshAliasExitCode"

# Test 3.3: refresh-client -S accepted
$refreshSResult = & $exe -t test-cli refresh-client -S 2>&1
$refreshSExitCode = $LASTEXITCODE
Test-Assert "refresh-client -S accepted" ($refreshSExitCode -eq 0 -or $refreshSExitCode -eq $null) "Exit code: $refreshSExitCode"

# ============================================================
# TEST GROUP 4: send-prefix
# ============================================================
Write-Host "`n[Test Group 4] send-prefix" -ForegroundColor Magenta

# Test 4.1: send-prefix accepted
$spResult = & $exe -t test-cli send-prefix 2>&1
$spExitCode = $LASTEXITCODE
Test-Assert "send-prefix accepted without error" ($spExitCode -eq 0 -or $spExitCode -eq $null) "Exit code: $spExitCode"

# ============================================================
# TEST GROUP 5: show-messages / showmsgs
# ============================================================
Write-Host "`n[Test Group 5] show-messages / showmsgs" -ForegroundColor Magenta

# Test 5.1: show-messages returns without error
$smResult = & $exe -t test-cli show-messages 2>&1
$smExitCode = $LASTEXITCODE
Test-Assert "show-messages returns without error" ($smExitCode -eq 0 -or $smExitCode -eq $null) "Exit code: $smExitCode"

# Test 5.2: showmsgs alias works
$smAliasResult = & $exe -t test-cli showmsgs 2>&1
$smAliasExitCode = $LASTEXITCODE
Test-Assert "'showmsgs' alias accepted" ($smAliasExitCode -eq 0 -or $smAliasExitCode -eq $null) "Exit code: $smAliasExitCode"

# ============================================================
# TEST GROUP 6: Platform handlers
# ============================================================
Write-Host "`n[Test Group 6] Platform handlers" -ForegroundColor Magenta

# Test 6.1: suspend-client
$suspResult = & $exe -t test-cli suspend-client 2>&1
$suspErr = ($suspResult | Out-String)
Test-Assert "suspend-client is silent no-op" (-not ($suspErr -match 'unknown command')) "Got: '$suspErr'"

# Test 6.2: suspendc alias
$suspcResult = & $exe -t test-cli suspendc 2>&1
$suspcErr = ($suspcResult | Out-String)
Test-Assert "suspendc alias is silent no-op" (-not ($suspcErr -match 'unknown command')) "Got: '$suspcErr'"

# Test 6.3: lock-client
$lockCResult = & $exe -t test-cli lock-client 2>&1
$lockCErr = ($lockCResult | Out-String)
Test-Assert "lock-client is silent no-op" (-not ($lockCErr -match 'unknown command')) "Got: '$lockCErr'"

# Test 6.4: lockc alias
$lockcResult = & $exe -t test-cli lockc 2>&1
$lockcErr = ($lockcResult | Out-String)
Test-Assert "lockc alias is silent no-op" (-not ($lockcErr -match 'unknown command')) "Got: '$lockcErr'"

# Test 6.5: lock-server
$lockSResult = & $exe -t test-cli lock-server 2>&1
$lockSErr = ($lockSResult | Out-String)
Test-Assert "lock-server is silent no-op" (-not ($lockSErr -match 'unknown command')) "Got: '$lockSErr'"

# Test 6.6: lock-session
$lockSessResult = & $exe -t test-cli lock-session 2>&1
$lockSessErr = ($lockSessResult | Out-String)
Test-Assert "lock-session is silent no-op" (-not ($lockSessErr -match 'unknown command')) "Got: '$lockSessErr'"

# Test 6.7: lock alias
$lockResult = & $exe -t test-cli lock 2>&1
$lockErr = ($lockResult | Out-String)
Test-Assert "'lock' alias is silent no-op" (-not ($lockErr -match 'unknown command')) "Got: '$lockErr'"

# Test 6.8: locks alias
$locksResult = & $exe -t test-cli locks 2>&1
$locksErr = ($locksResult | Out-String)
Test-Assert "'locks' alias is silent no-op" (-not ($locksErr -match 'unknown command')) "Got: '$locksErr'"

# Test 6.9: resize-window
$rwResult = & $exe -t test-cli resize-window -x 90 -y 28 2>&1
$rwSize = "$((& $exe -t test-cli display-message -p '#{window_width}' 2>&1).Trim())x$((& $exe -t test-cli display-message -p '#{window_height}' 2>&1).Trim())"
Test-Assert "resize-window changes window geometry" ($rwSize -eq "90x28") "Got: '$rwSize'; command output: '$rwResult'"

# Test 6.10: resizew alias
$rwAliasResult = & $exe -t test-cli resizew -x 92 -y 29 2>&1
$rwAliasSize = "$((& $exe -t test-cli display-message -p '#{window_width}' 2>&1).Trim())x$((& $exe -t test-cli display-message -p '#{window_height}' 2>&1).Trim())"
Test-Assert "resizew alias changes window geometry" ($rwAliasSize -eq "92x29") "Got: '$rwAliasSize'; command output: '$rwAliasResult'"

# Test 6.11: customize-mode
$cmResult = & $exe -t test-cli customize-mode 2>&1
$cmErr = ($cmResult | Out-String)
Test-Assert "customize-mode is silent no-op" (-not ($cmErr -match 'unknown command')) "Got: '$cmErr'"

# ============================================================
# TEST GROUP 7: start-server
# ============================================================
Write-Host "`n[Test Group 7] start-server / start" -ForegroundColor Magenta

# Test 7.1: start-server accepted
$ssResult = & $exe -t test-cli start-server 2>&1
$ssErr = ($ssResult | Out-String)
Test-Assert "start-server is accepted" (-not ($ssErr -match 'unknown command')) "Got: '$ssErr'"

# Test 7.2: 'start' alias accepted  
$ssAliasResult = & $exe -t test-cli start 2>&1
$ssAliasErr = ($ssAliasResult | Out-String)
Test-Assert "'start' alias is accepted" (-not ($ssAliasErr -match 'unknown command')) "Got: '$ssAliasErr'"

# ============================================================
# TEST GROUP 8: confirm-before / confirm
# ============================================================
Write-Host "`n[Test Group 8] confirm-before / confirm" -ForegroundColor Magenta

# Test 8.1: confirm-before accepted (sends to server)
$cbResult = & $exe -t test-cli confirm-before "echo test" 2>&1
$cbExitCode = $LASTEXITCODE
Test-Assert "confirm-before accepted without error" ($cbExitCode -eq 0 -or $cbExitCode -eq $null) "Exit code: $cbExitCode"

# Test 8.2: confirm alias accepted
$confirmResult = & $exe -t test-cli confirm "echo test" 2>&1
$confirmExitCode = $LASTEXITCODE
Test-Assert "'confirm' alias accepted" ($confirmExitCode -eq 0 -or $confirmExitCode -eq $null) "Exit code: $confirmExitCode"

# ============================================================
# TEST GROUP 9: display-menu / menu
# ============================================================
Write-Host "`n[Test Group 9] display-menu / menu" -ForegroundColor Magenta

# Test 9.1: display-menu accepted
$dmResult = & $exe -t test-cli display-menu "Item1" a "echo hello" 2>&1
$dmExitCode = $LASTEXITCODE
Test-Assert "display-menu accepted without error" ($dmExitCode -eq 0 -or $dmExitCode -eq $null) "Exit code: $dmExitCode"

# Test 9.2: menu alias accepted
$menuResult = & $exe -t test-cli menu "Item1" a "echo hello" 2>&1
$menuExitCode = $LASTEXITCODE
Test-Assert "'menu' alias accepted" ($menuExitCode -eq 0 -or $menuExitCode -eq $null) "Exit code: $menuExitCode"

# ============================================================
# TEST GROUP 10: display-popup / popup
# ============================================================
Write-Host "`n[Test Group 10] display-popup / popup" -ForegroundColor Magenta

# Test 10.1: display-popup accepted
$dpResult = & $exe -t test-cli display-popup -E "echo hello" 2>&1
$dpExitCode = $LASTEXITCODE
Test-Assert "display-popup accepted without error" ($dpExitCode -eq 0 -or $dpExitCode -eq $null) "Exit code: $dpExitCode"

# Test 10.2: popup alias accepted
$popupResult = & $exe -t test-cli popup -E "echo hello" 2>&1
$popupExitCode = $LASTEXITCODE
Test-Assert "'popup' alias accepted" ($popupExitCode -eq 0 -or $popupExitCode -eq $null) "Exit code: $popupExitCode"

# ============================================================
# TEST GROUP 11: display-panes / displayp
# ============================================================
Write-Host "`n[Test Group 11] display-panes / displayp" -ForegroundColor Magenta

# Test 11.1: display-panes accepted
$dpResult = & $exe -t test-cli display-panes 2>&1
$dpExitCode = $LASTEXITCODE
Test-Assert "display-panes accepted without error" ($dpExitCode -eq 0 -or $dpExitCode -eq $null) "Exit code: $dpExitCode"

# Test 11.2: displayp alias accepted
$displaypResult = & $exe -t test-cli displayp 2>&1
$displaypExitCode = $LASTEXITCODE
Test-Assert "displayp alias accepted" ($displaypExitCode -eq 0 -or $displaypExitCode -eq $null) "Exit code: $displaypExitCode"

# ============================================================
# TEST GROUP 12: choose-client
# ============================================================
Write-Host "`n[Test Group 12] choose-client" -ForegroundColor Magenta

# Test 12.1: choose-client returns client info
$ccResult = & $exe -t test-cli choose-client 2>&1
$ccStr = ($ccResult | Out-String)
Test-Assert "choose-client returns without error" (-not ($ccStr -match 'unknown command')) "Got: '$ccStr'"

# ============================================================
# TEST GROUP 13: respawn-window
# ============================================================
Write-Host "`n[Test Group 13] respawn-window / respawnw" -ForegroundColor Magenta

# Test 13.1: respawn-window accepted
# -k: without it a live window is refused, as in tmux
$rwResult = & $exe -t test-cli respawn-window -k 2>&1
$rwExitCode = $LASTEXITCODE
Test-Assert "respawn-window accepted" ($rwExitCode -eq 0 -or $rwExitCode -eq $null) "Exit code: $rwExitCode"

# Test 13.2: respawnw alias accepted
$rwAliasResult = & $exe -t test-cli respawnw -k 2>&1
$rwAliasExitCode = $LASTEXITCODE
Test-Assert "respawnw alias accepted" ($rwAliasExitCode -eq 0 -or $rwAliasExitCode -eq $null) "Exit code: $rwAliasExitCode"

# ============================================================
# TEST GROUP 14: link-window / unlink-window
# ============================================================
Write-Host "`n[Test Group 14] link-window / unlink-window" -ForegroundColor Magenta

# Test 14.1: link-window accepted (compat stub)
$lwResult = & $exe -t test-cli link-window 2>&1
$lwErr = ($lwResult | Out-String)
Test-Assert "link-window accepted" (-not ($lwErr -match 'unknown command')) "Got: '$lwErr'"

# Test 14.2: linkw alias accepted
$lwAliasResult = & $exe -t test-cli linkw 2>&1
$lwAliasErr = ($lwAliasResult | Out-String)
Test-Assert "linkw alias accepted" (-not ($lwAliasErr -match 'unknown command')) "Got: '$lwAliasErr'"

# Test 14.3: unlink-window accepted
$ulwResult = & $exe -t test-cli unlink-window 2>&1
$ulwErr = ($ulwResult | Out-String)
Test-Assert "unlink-window accepted" (-not ($ulwErr -match 'unknown command')) "Got: '$ulwErr'"

# Test 14.4: unlinkw alias accepted
$ulwAliasResult = & $exe -t test-cli unlinkw 2>&1
$ulwAliasErr = ($ulwAliasResult | Out-String)
Test-Assert "unlinkw alias accepted" (-not ($ulwAliasErr -match 'unknown command')) "Got: '$ulwAliasErr'"

# ============================================================
# TEST GROUP 15: Alias consistency check (pmux and tmux binaries)
# ============================================================
Write-Host "`n[Test Group 15] Binary alias consistency" -ForegroundColor Magenta

# Test 15.1: pmux server-info works
$pmuxExe = "$PSScriptRoot\..\target\release\pmux.exe"
if (-not (Test-Path $pmuxExe)) { $pmuxExe = (Get-Command pmux -ErrorAction SilentlyContinue).Source }
if ($pmuxExe -and (Test-Path $pmuxExe)) {
    $pmuxInfo = & $pmuxExe -t test-cli server-info 2>&1
    $pmuxInfoStr = ($pmuxInfo | Out-String).Trim()
    Test-Assert "pmux server-info works" ($pmuxInfoStr -match 'psmux') "Got: '$pmuxInfoStr'"
} else {
    Write-Host "  SKIP: pmux binary not found" -ForegroundColor Yellow
}

# Test 15.2: tmux server-info works (psmux's tmux shim)
$tmuxExe = "$PSScriptRoot\..\target\release\tmux.exe"
if (-not (Test-Path $tmuxExe)) { $tmuxExe = (Get-Command tmux -ErrorAction SilentlyContinue).Source }
if ($tmuxExe -and (Test-Path $tmuxExe)) {
    $tmuxInfo = & $tmuxExe -t test-cli server-info 2>&1
    $tmuxInfoStr = ($tmuxInfo | Out-String).Trim()
    Test-Assert "tmux server-info works" ($tmuxInfoStr -match 'psmux') "Got: '$tmuxInfoStr'"
} else {
    Write-Host "  SKIP: tmux binary not found" -ForegroundColor Yellow
}

# ============================================================
# Cleanup
# ============================================================
& $exe kill-session -t test-cli 2>$null
& $exe kill-session -t test-cli2 2>$null
Start-Sleep -Milliseconds 500

# ============================================================
# SUMMARY
# ============================================================
Write-Host "`n================================================" -ForegroundColor Cyan
Write-Host "Results: $pass/$total passed, $fail failed" -ForegroundColor $(if ($fail -eq 0) { "Green" } else { "Red" })
Write-Host "================================================`n" -ForegroundColor Cyan

if ($fail -gt 0) {
    exit 1
} else {
    exit 0
}
