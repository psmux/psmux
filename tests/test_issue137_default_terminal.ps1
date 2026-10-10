#!/usr/bin/env pwsh
# Test for issue #137: ParserError from $env:default-terminal='xterm-256color'
# https://github.com/psmux/psmux/issues/137
#
# Root cause: tmux options with hyphens (e.g. default-terminal, allow-rename,
# terminal-overrides) were stored in app.environment and injected into the
# warm pane via $env:NAME='value' PowerShell syntax. Hyphens are invalid in
# PowerShell $env: variable names, causing ParserError.
#
# The fix:
# 1. default-terminal now sets TERM (like real tmux)
# 2. Other tmux-specific options no longer go into app.environment
# 3. PowerShell injection uses ${env:NAME} brace syntax for safety

$ErrorActionPreference = "Continue"
$psmux = Get-Command psmux -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source
if (-not $psmux) { $psmux = "psmux" }

$pass = 0
$fail = 0
$total = 0

function Test-Assert($name, $condition) {
    $script:total++
    if ($condition) {
        Write-Host "  PASS: $name" -ForegroundColor Green
        $script:pass++
    } else {
        Write-Host "  FAIL: $name" -ForegroundColor Red
        $script:fail++
    }
}

function Cleanup-PsmuxState {
    & $psmux kill-server 2>$null
    Start-Sleep -Milliseconds 500
    $dir = Join-Path $env:USERPROFILE ".psmux"
    if (Test-Path $dir) {
        Get-ChildItem $dir -Filter "*.port" | Remove-Item -Force -ErrorAction SilentlyContinue
        Get-ChildItem $dir -Filter "*.key" | Remove-Item -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 200
}

$psmuxDir = Join-Path $env:USERPROFILE ".psmux"

Write-Host "`n=== Issue #137: ParserError from default-terminal ===" -ForegroundColor Cyan

# ─── Test 1: set -g default-terminal should NOT cause ParserError ──────
Write-Host "`nTest 1: default-terminal stores as TERM env var, not default-terminal" -ForegroundColor Yellow
Cleanup-PsmuxState

# Write a temp tmux.conf with default-terminal set
$tempConf = Join-Path $env:TEMP "psmux_test137.conf"
Set-Content -Path $tempConf -Value 'set -g default-terminal "xterm-256color"'

# Start a detached session with this config
$env:PSMUX_CONFIG_FILE = $tempConf
$output = & $psmux new-session -d -s "t137" 2>&1 | Out-String
$exitCode = $LASTEXITCODE
Remove-Item env:PSMUX_CONFIG_FILE -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 1500

Test-Assert "Session created without error (exit=$exitCode)" ($exitCode -eq 0)

# Check that the server accepted the config by querying show-option
$portFile = Join-Path $psmuxDir "t137.port"
$keyFile = Join-Path $psmuxDir "t137.key"
if (Test-Path $portFile) {
    $envOutput = & $psmux show-environment -t t137 2>&1 | Out-String
    Write-Host "  INFO: show-environment filtered: $(($envOutput -split "`n" | Select-String 'TERM') -join '; ')" -ForegroundColor Gray
    Test-Assert "TERM=xterm-256color in environment" ($envOutput -match "TERM=xterm-256color")
}

Cleanup-PsmuxState

# ─── Test 2: Warm claim with default-terminal does not produce error ───
Write-Host "`nTest 2: Second session via warm claim with default-terminal in config" -ForegroundColor Yellow
Cleanup-PsmuxState

$env:PSMUX_CONFIG_FILE = $tempConf

# Create first session (triggers warm server spawn)
$output1 = & $psmux new-session -d -s "w0" 2>&1 | Out-String
Start-Sleep -Milliseconds 2000

# Create second session (should use warm claim)
$output2 = & $psmux new-session -d -s "w1" 2>&1 | Out-String
$exitCode = $LASTEXITCODE
Start-Sleep -Milliseconds 1000

Remove-Item env:PSMUX_CONFIG_FILE -ErrorAction SilentlyContinue

Test-Assert "Second session created without error (exit=$exitCode)" ($exitCode -eq 0)

# The key test: check stderr for ParserError
$hasParserError = $output2 -match "ParserError"
Test-Assert "No ParserError in output" (-not $hasParserError)

$portW1 = Join-Path $psmuxDir "w1.port"
Test-Assert "Session w1 exists" (Test-Path $portW1)

Cleanup-PsmuxState

# ─── Test 3: verify TERM is set, not default-terminal ──────────────────
Write-Host "`nTest 3: Verify TERM env var is set (not default-terminal)" -ForegroundColor Yellow
Cleanup-PsmuxState

$env:PSMUX_CONFIG_FILE = $tempConf
$output = & $psmux new-session -d -s "env0" 2>&1
Start-Sleep -Milliseconds 1500
Remove-Item env:PSMUX_CONFIG_FILE -ErrorAction SilentlyContinue

# Use show-environment to check what's in the env
$showEnv = & $psmux show-environment -t env0 2>&1 | Out-String
Write-Host "  INFO: show-environment output: $($showEnv.Trim())" -ForegroundColor Gray

# TERM should be set
Test-Assert "TERM is in environment" ($showEnv -match "TERM=xterm-256color")

# default-terminal should NOT be in environment 
$hasDefaultTerminal = $showEnv -match "default-terminal=xterm-256color"
Test-Assert "default-terminal is NOT in environment" (-not $hasDefaultTerminal)

Cleanup-PsmuxState

# ─── Test 4: other hyphenated options don't leak into environment ──────
Write-Host "`nTest 4: Hyphenated tmux options don't leak as env vars" -ForegroundColor Yellow
Cleanup-PsmuxState

$tempConf2 = Join-Path $env:TEMP "psmux_test137b.conf"
@"
set -g default-terminal "xterm-256color"
set -g allow-rename on
set -g terminal-overrides "xterm*:Tc"
set -g activity-action other
"@ | Set-Content -Path $tempConf2

$env:PSMUX_CONFIG_FILE = $tempConf2
$output = & $psmux new-session -d -s "hyp0" 2>&1
Start-Sleep -Milliseconds 1500
Remove-Item env:PSMUX_CONFIG_FILE -ErrorAction SilentlyContinue

$showEnv = & $psmux show-environment -t hyp0 2>&1 | Out-String
Write-Host "  INFO: show-environment: $($showEnv.Trim())" -ForegroundColor Gray

Test-Assert "allow-rename not in environment" (-not ($showEnv -match "allow-rename"))
Test-Assert "terminal-overrides not in environment" (-not ($showEnv -match "terminal-overrides"))
Test-Assert "activity-action not in environment" (-not ($showEnv -match "activity-action"))

Cleanup-PsmuxState

# ─── Test 5: set-environment with valid names still works ──────────────
Write-Host "`nTest 5: Explicit set-environment with valid names works" -ForegroundColor Yellow
Cleanup-PsmuxState

$tempConf3 = Join-Path $env:TEMP "psmux_test137c.conf"
@"
set-environment -g MY_VAR hello_world
set-environment -g EDITOR vim
"@ | Set-Content -Path $tempConf3

$env:PSMUX_CONFIG_FILE = $tempConf3
$output = & $psmux new-session -d -s "env1" 2>&1
Start-Sleep -Milliseconds 1500
Remove-Item env:PSMUX_CONFIG_FILE -ErrorAction SilentlyContinue

# set-environment -g is the global environment, read back with -g (#775).
$showEnv = & $psmux show-environment -g -t env1 2>&1 | Out-String
Write-Host "  INFO: show-environment -g (filtered): $(($showEnv -split "`n" | Select-String 'MY_VAR|EDITOR') -join '; ')" -ForegroundColor Gray

Test-Assert "MY_VAR in environment" ($showEnv -match "MY_VAR=hello_world")
Test-Assert "EDITOR in environment" ($showEnv -match "EDITOR=vim")

Cleanup-PsmuxState

# ─── Test 6: env var injection NOT echoed in warm pane ─────────────────
Write-Host "`nTest 6: TERM env var is NOT echoed visibly in the pane (warm path)" -ForegroundColor Yellow
Cleanup-PsmuxState

$tempConf4 = Join-Path $env:TEMP "psmux_test137d.conf"
Set-Content -Path $tempConf4 -Value 'set -g default-terminal "xterm-256color"'

$env:PSMUX_CONFIG_FILE = $tempConf4
# Create the first (initial) session: this uses the early warm pane
$output = & $psmux new-session -d -s "echo0" 2>&1 | Out-String
Start-Sleep -Milliseconds 3000
Remove-Item env:PSMUX_CONFIG_FILE -ErrorAction SilentlyContinue

# Capture the pane buffer to see what the user would see
$captured = & $psmux capture-pane -t echo0 -p 2>&1 | Out-String
Write-Host "  INFO: captured pane: $($captured.Trim())" -ForegroundColor Gray

# The env assignment command must NOT appear in the pane output
$hasEnvEcho = $captured -match '\$\{?env:TERM\}?\s*='
Test-Assert "No env:TERM assignment echoed in pane" (-not $hasEnvEcho)

# Also check for the old broken format
$hasOldFormat = $captured -match '\$env:default-terminal'
Test-Assert "No old default-terminal env injection in pane" (-not $hasOldFormat)

Cleanup-PsmuxState

# ─── Test 7: second session via warm claim also has no echo ────────────
Write-Host "`nTest 7: Second session via warm claim has no env var echo" -ForegroundColor Yellow
Cleanup-PsmuxState

$env:PSMUX_CONFIG_FILE = $tempConf4

# First session
$output1 = & $psmux new-session -d -s "echo1" 2>&1 | Out-String
Start-Sleep -Milliseconds 2000

# Split to trigger warm pane consumption and respawn
$splitOutput = & $psmux split-window -t echo1 2>&1 | Out-String
Start-Sleep -Milliseconds 2000

Remove-Item env:PSMUX_CONFIG_FILE -ErrorAction SilentlyContinue

# Capture the second pane (the one from warm claim)
$captured2 = & $psmux capture-pane -t echo1 -p 2>&1 | Out-String
Write-Host "  INFO: captured pane 2: $($captured2.Trim())" -ForegroundColor Gray

$hasEnvEcho2 = $captured2 -match '\$\{?env:TERM\}?\s*='
Test-Assert "No env:TERM assignment echoed in split pane" (-not $hasEnvEcho2)

Cleanup-PsmuxState

# ─── Cleanup temp files ───────────────────────────────────────────────
Remove-Item $tempConf -Force -ErrorAction SilentlyContinue
Remove-Item $tempConf2 -Force -ErrorAction SilentlyContinue
Remove-Item $tempConf3 -Force -ErrorAction SilentlyContinue
Remove-Item $tempConf4 -Force -ErrorAction SilentlyContinue

# ─── Summary ───────────────────────────────────────────────────────────
Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "Passed: $pass / $total" -ForegroundColor $(if ($fail -eq 0) { "Green" } else { "Yellow" })
if ($fail -gt 0) {
    Write-Host "Failed: $fail / $total" -ForegroundColor Red
    exit 1
}
Write-Host "All tests passed!" -ForegroundColor Green
exit 0
