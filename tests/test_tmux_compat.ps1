#!/usr/bin/env pwsh
# test_tmux_compat.ps1
# Comprehensive tests for full tmux compatibility:
# 1. new-window -P uses full format engine (not manual .replace())
# 2. split-window -P uses full format engine
# 3. new-session -P default format matches tmux (session_name:)
# 4. -L namespace isolation (separate server namespaces)
# 5. TMUX env var resolution with -L namespaces

$ErrorActionPreference = "Continue"
$exe = "$PSScriptRoot\..\target\release\psmux.exe"
if (-not (Test-Path $exe)) { $exe = "$PSScriptRoot\..\target\debug\psmux.exe" }
if (-not (Test-Path $exe)) { $exe = (Get-Command psmux -ErrorAction SilentlyContinue).Source }
if (-not $exe -or -not (Test-Path $exe)) { Write-Error "psmux binary not found"; exit 1 }

# Helper: cleanup sessions
function Cleanup-All {
    # Kill namespaced sessions
    & $exe kill-session -t "ns1__worker1" 2>$null
    & $exe kill-session -t "ns1__worker2" 2>$null
    & $exe kill-session -t "ns2__worker1" 2>$null
    # Kill regular sessions
    & $exe kill-session -t test-fmt 2>$null
    & $exe kill-session -t test-splitfmt 2>$null
    & $exe kill-session -t test-newsess 2>$null
    & $exe kill-session -t test-pdefault 2>$null
    & $exe kill-session -t test-complex 2>$null
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
Write-Host "tmux Compatibility Test Suite" -ForegroundColor Cyan
Write-Host "Full format engine, -P defaults, -L namespaces" -ForegroundColor Cyan
Write-Host "================================================`n" -ForegroundColor Cyan

# --- Cleanup before tests ---
Cleanup-All

# ============================================================
# TEST GROUP 1: new-window -P with full format engine
# ============================================================
Write-Host "[Test Group 1] new-window -P full format engine" -ForegroundColor Magenta

# Create a session first
& $exe new-session -d -s test-fmt 2>$null
Start-Sleep -Milliseconds 800

# Test 1.1: new-window -P default format (tmux: #{session_name}:#{window_index})
$nwDefault = & $exe new-window -t test-fmt -P 2>&1
$nwDefaultStr = ($nwDefault | Out-String).Trim()
Test-Assert "new-window -P default format is 'session:window'" ($nwDefaultStr -match '^test-fmt:\d+$') "Got: '$nwDefaultStr'"

# Test 1.2: new-window -P -F '#{pane_id}' returns %N format
$nwPaneId = & $exe new-window -t test-fmt -P -F '#{pane_id}' 2>&1
$nwPaneIdStr = ($nwPaneId | Out-String).Trim()
Test-Assert "new-window -P -F '#{pane_id}' returns %N" ($nwPaneIdStr -match '^%\d+$') "Got: '$nwPaneIdStr'"

# Test 1.3: new-window -P -F '#{session_name}' returns session name
$nwSession = & $exe new-window -t test-fmt -P -F '#{session_name}' 2>&1
$nwSessionStr = ($nwSession | Out-String).Trim()
Test-Assert "new-window -P -F '#{session_name}' returns session" ($nwSessionStr -eq "test-fmt") "Got: '$nwSessionStr'"

# Test 1.4: new-window -P -F '#{window_index}' returns numeric index
$nwWinIdx = & $exe new-window -t test-fmt -P -F '#{window_index}' 2>&1
$nwWinIdxStr = ($nwWinIdx | Out-String).Trim()
Test-Assert "new-window -P -F '#{window_index}' returns number" ($nwWinIdxStr -match '^\d+$') "Got: '$nwWinIdxStr'"

# Test 1.5: new-window -P -F with complex format (conditional)
$nwComplex = & $exe new-window -t test-fmt -P -F '#{session_name}:#{window_index}:#{pane_id}' 2>&1
$nwComplexStr = ($nwComplex | Out-String).Trim()
Test-Assert "new-window -P -F complex format works" ($nwComplexStr -match '^test-fmt:\d+:%\d+$') "Got: '$nwComplexStr'"

# Test 1.6: new-window -P -F '#{window_name}' returns window name
$nwWinName = & $exe new-window -t test-fmt -P -F '#{window_name}' 2>&1
$nwWinNameStr = ($nwWinName | Out-String).Trim()
Test-Assert "new-window -P -F '#{window_name}' returns non-empty" ($nwWinNameStr.Length -gt 0) "Got: '$nwWinNameStr'"

# Test 1.7: new-window -P -F '#{pane_width}x#{pane_height}' returns dimensions
$nwDims = & $exe new-window -t test-fmt -P -F '#{pane_width}x#{pane_height}' 2>&1
$nwDimsStr = ($nwDims | Out-String).Trim()
Test-Assert "new-window -P -F dimensions format works" ($nwDimsStr -match '^\d+x\d+$') "Got: '$nwDimsStr'"

# Cleanup
& $exe kill-session -t test-fmt 2>$null
Start-Sleep -Milliseconds 500

# ============================================================
# TEST GROUP 2: split-window -P with full format engine
# ============================================================
Write-Host "`n[Test Group 2] split-window -P full format engine" -ForegroundColor Magenta

# Create a session first
# Room for four vertical splits: each halves the active pane, and at the
# default 24 rows the fourth is refused (pane too small), as in tmux.
& $exe new-session -d -s test-splitfmt -x 200 -y 80 2>$null
Start-Sleep -Milliseconds 800

# Test 2.1: split-window -P default format (tmux: #{session_name}:#{window_index}.#{pane_index})
$swDefault = & $exe split-window -t test-splitfmt -P 2>&1
$swDefaultStr = ($swDefault | Out-String).Trim()
Test-Assert "split-window -P default format is 'session:win.pane'" ($swDefaultStr -match '^test-splitfmt:\d+\.\d+$') "Got: '$swDefaultStr'"

# Test 2.2: split-window -P -F '#{pane_id}' returns %N format
$swPaneId = & $exe split-window -t test-splitfmt -P -F '#{pane_id}' 2>&1
$swPaneIdStr = ($swPaneId | Out-String).Trim()
Test-Assert "split-window -P -F '#{pane_id}' returns %N" ($swPaneIdStr -match '^%\d+$') "Got: '$swPaneIdStr'"

# Test 2.3: split-window -P -F '#{session_name}' returns session name
$swSession = & $exe split-window -t test-splitfmt -P -F '#{session_name}' 2>&1
$swSessionStr = ($swSession | Out-String).Trim()
Test-Assert "split-window -P -F '#{session_name}' returns session" ($swSessionStr -eq "test-splitfmt") "Got: '$swSessionStr'"

# Test 2.4: split-window -P -F with complex format
$swComplex = & $exe split-window -t test-splitfmt -P -F '#{session_name}:#{window_index}:#{pane_id}' 2>&1
$swComplexStr = ($swComplex | Out-String).Trim()
Test-Assert "split-window -P -F complex format works" ($swComplexStr -match '^test-splitfmt:\d+:%\d+$') "Got: '$swComplexStr'"

# Test 2.5: split-window -h -P -F '#{pane_index}' (horizontal split)
$swHPaneIdx = & $exe split-window -h -t test-splitfmt -P -F '#{pane_index}' 2>&1
$swHPaneIdxStr = ($swHPaneIdx | Out-String).Trim()
Test-Assert "split-window -h -P -F '#{pane_index}' returns number" ($swHPaneIdxStr -match '^\d+$') "Got: '$swHPaneIdxStr'"

# Cleanup
& $exe kill-session -t test-splitfmt 2>$null
Start-Sleep -Milliseconds 500

# ============================================================
# TEST GROUP 3: new-session -P default format matches tmux
# ============================================================
Write-Host "`n[Test Group 3] new-session -P default format" -ForegroundColor Magenta

# Test 3.1: new-session -d -P (no -F) should print "session_name:" (tmux default)
$nsDefault = & $exe new-session -d -s test-newsess -P 2>&1
$nsDefaultStr = ($nsDefault | Out-String).Trim()
Test-Assert "new-session -P default is 'session_name:'" ($nsDefaultStr -eq "test-newsess:") "Got: '$nsDefaultStr'"
& $exe kill-session -t test-newsess 2>$null
Start-Sleep -Milliseconds 500

# Test 3.2: new-session -d -P -F '#{pane_id}' returns %N
$nsPaneId = & $exe new-session -d -s test-pdefault -P -F '#{pane_id}' 2>&1
$nsPaneIdStr = ($nsPaneId | Out-String).Trim()
Test-Assert "new-session -P -F '#{pane_id}' returns %N" ($nsPaneIdStr -match '^%\d+$') "Got: '$nsPaneIdStr'"

# Test 3.3: new-session -d -P -F '#{session_name}:#{window_index}' returns full format
$nsFull = & $exe new-session -d -s test-complex -P -F '#{session_name}:#{window_index}' 2>&1
$nsFullStr = ($nsFull | Out-String).Trim()
Test-Assert "new-session -P -F complex returns expected" ($nsFullStr -eq "test-complex:0") "Got: '$nsFullStr'"

# Cleanup
& $exe kill-session -t test-pdefault 2>$null
& $exe kill-session -t test-complex 2>$null
Start-Sleep -Milliseconds 500

# ============================================================
# TEST GROUP 4: -L namespace isolation
# ============================================================
Write-Host "`n[Test Group 4] -L namespace isolation" -ForegroundColor Magenta

# Test 4.1: Create two sessions under same -L namespace
$out1 = & $exe -L ns1 new-session -d -s worker1 2>&1
$out1Str = ($out1 | Out-String).Trim()
$hasErr1 = $out1Str -match "error|unknown"
Test-Assert "-L ns1 new-session -s worker1 succeeds" (-not $hasErr1) "Output: '$out1Str'"
Start-Sleep -Milliseconds 600

$out2 = & $exe -L ns1 new-session -d -s worker2 2>&1
$out2Str = ($out2 | Out-String).Trim()
$hasErr2 = $out2Str -match "error|unknown"
Test-Assert "-L ns1 new-session -s worker2 succeeds" (-not $hasErr2) "Output: '$out2Str'"
Start-Sleep -Milliseconds 600

# Test 4.2: Create session with same name under different -L namespace
$out3 = & $exe -L ns2 new-session -d -s worker1 2>&1
$out3Str = ($out3 | Out-String).Trim()
$hasErr3 = $out3Str -match "error|unknown"
Test-Assert "-L ns2 new-session -s worker1 (same name, diff ns)" (-not $hasErr3) "Output: '$out3Str'"
Start-Sleep -Milliseconds 600

# Test 4.3: -L ns1 has-session should find worker1
& $exe -L ns1 has-session -t worker1 2>$null
Test-Assert "-L ns1 has-session -t worker1 finds it" ($LASTEXITCODE -eq 0) "Exit: $LASTEXITCODE"

# Test 4.4: -L ns2 has-session should find worker1 (different ns, same name)
& $exe -L ns2 has-session -t worker1 2>$null
Test-Assert "-L ns2 has-session -t worker1 finds it" ($LASTEXITCODE -eq 0) "Exit: $LASTEXITCODE"

# Test 4.5: -L ns1 has-session should NOT find worker1 under ns2's namespace
# (ns1__worker1 != ns2__worker1)
# Check that port files are correctly namespaced
$homeDir = $env:USERPROFILE
$ns1w1 = Test-Path "$homeDir\.psmux\ns1__worker1.port"
$ns1w2 = Test-Path "$homeDir\.psmux\ns1__worker2.port"
$ns2w1 = Test-Path "$homeDir\.psmux\ns2__worker1.port"
Test-Assert "Port file ns1__worker1.port exists" $ns1w1 "Checked: $homeDir\.psmux\ns1__worker1.port"
Test-Assert "Port file ns1__worker2.port exists" $ns1w2 "Checked: $homeDir\.psmux\ns1__worker2.port"
Test-Assert "Port file ns2__worker1.port exists" $ns2w1 "Checked: $homeDir\.psmux\ns2__worker1.port"

# Test 4.6: list-sessions with -L ns1 should only show ns1 sessions
$lsNs1 = & $exe -L ns1 ls 2>&1
$lsNs1Str = ($lsNs1 | Out-String).Trim()
Test-Assert "-L ns1 ls shows sessions" ($lsNs1Str.Length -gt 0) "Got: '$lsNs1Str'"

# Test 4.7: list-sessions without -L should NOT show namespaced sessions
$lsDefault = & $exe ls 2>&1
$lsDefaultStr = ($lsDefault | Out-String).Trim()
$hasNs1 = $lsDefaultStr -match "ns1__"
$hasNs2 = $lsDefaultStr -match "ns2__"
Test-Assert "ls (no -L) does not show namespaced sessions" (-not $hasNs1 -and -not $hasNs2) "Got: '$lsDefaultStr'"

# Test 4.8: kill-session with -L
& $exe -L ns1 kill-session -t worker1 2>$null
Start-Sleep -Milliseconds 500
$ns1w1After = Test-Path "$homeDir\.psmux\ns1__worker1.port"
Test-Assert "-L ns1 kill-session -t worker1 removes port file" (-not $ns1w1After) "Port file still exists"

# Test 4.9: ns2__worker1 should still be alive after killing ns1__worker1
& $exe -L ns2 has-session -t worker1 2>$null
Test-Assert "ns2 worker1 still alive after ns1 worker1 killed" ($LASTEXITCODE -eq 0) "Exit: $LASTEXITCODE"

# Cleanup
& $exe -L ns1 kill-session -t worker2 2>$null
& $exe -L ns2 kill-session -t worker1 2>$null
Start-Sleep -Milliseconds 500

# ============================================================
# TEST GROUP 5: TMUX env var with -L namespace resolution
# ============================================================
Write-Host "`n[Test Group 5] TMUX env var with -L" -ForegroundColor Magenta

# Create a namespaced session
& $exe -L myns new-session -d -s tmuxtest 2>$null
Start-Sleep -Milliseconds 800

# Read the port from the namespaced port file
$nsPortFile = "$homeDir\.psmux\myns__tmuxtest.port"
if (Test-Path $nsPortFile) {
    $port = (Get-Content $nsPortFile).Trim()
    
    # Set TMUX env var as psmux would set it (with socket name in path)
    $env:TMUX = "/tmp/psmux-0/myns,$port,0"
    $env:PSMUX_TARGET_SESSION = $null
    
    # Test 5.1: Commands without -t should resolve from TMUX env var port scan
    $displayOut = & $exe display-message -p '#{session_name}' 2>&1
    $displayOutStr = ($displayOut | Out-String).Trim()
    Test-Assert "TMUX env var resolves namespaced session" ($displayOutStr -eq "tmuxtest") "Got: '$displayOutStr'"
    
    # Clean up env
    $env:TMUX = $null
} else {
    Write-Host "  SKIP: Port file $nsPortFile not found" -ForegroundColor Yellow
    $script:total += 1
}

# Cleanup
& $exe -L myns kill-session -t tmuxtest 2>$null
Start-Sleep -Milliseconds 500

# ============================================================
# TEST GROUP 6: Regression - existing behavior preserved
# ============================================================
Write-Host "`n[Test Group 6] Regression tests" -ForegroundColor Magenta

# Test 6.1: Regular session (no -L) still works
& $exe new-session -d -s regtest 2>$null
Start-Sleep -Milliseconds 600
& $exe has-session -t regtest 2>$null
Test-Assert "Regular session (no -L) works" ($LASTEXITCODE -eq 0) "Exit: $LASTEXITCODE"

# Test 6.2: Regular session port file has no namespace prefix
$regPort = Test-Path "$homeDir\.psmux\regtest.port"
Test-Assert "Regular session port file is 'regtest.port'" $regPort

# Test 6.3: display-message -p works on regular session
$regDisplay = & $exe -t regtest display-message -p '#{session_name}' 2>&1
$regDisplayStr = ($regDisplay | Out-String).Trim()
Test-Assert "display-message on regular session returns name" ($regDisplayStr -eq "regtest") "Got: '$regDisplayStr'"

# Test 6.4: new-window -P -F on regular session works
$regNw = & $exe new-window -t regtest -P -F '#{session_name}:#{window_index}' 2>&1
$regNwStr = ($regNw | Out-String).Trim()
Test-Assert "new-window -P -F on regular session" ($regNwStr -match '^regtest:\d+$') "Got: '$regNwStr'"

# Cleanup
& $exe kill-session -t regtest 2>$null
Start-Sleep -Milliseconds 500

# ============================================================
# Group 7: list-windows -F format engine
# ============================================================
Write-Host "`n[Test Group 7] list-windows -F format engine" -ForegroundColor Yellow

& $exe new-session -d -s lswfmt 2>$null
Start-Sleep -Seconds 2
& $exe -t lswfmt new-window 2>$null
Start-Sleep -Milliseconds 500

# Test 7.1: default list-windows returns tmux-style output
$lswDefault = & $exe -t lswfmt list-windows 2>&1
$lswDefaultStr = ($lswDefault | Out-String).Trim()
Test-Assert "list-windows default shows window info" ($lswDefaultStr -match '\d+:.*panes') "Got: '$lswDefaultStr'"

# Test 7.2: list-windows -F '#{window_index}' returns numbers
$lswIdx = & $exe -t lswfmt list-windows -F '#{window_index}' 2>&1
$lswIdxStr = ($lswIdx | Out-String).Trim()
Test-Assert "list-windows -F '#{window_index}' returns numbers" ($lswIdxStr -match '^\d+\r?\n\d+$') "Got: '$lswIdxStr'"

# Test 7.3: list-windows -F '#{window_name}' returns names
$lswName = & $exe -t lswfmt list-windows -F '#{window_name}' 2>&1
$lswNameStr = ($lswName | Out-String).Trim()
Test-Assert "list-windows -F '#{window_name}' returns names" ($lswNameStr.Length -gt 0) "Got: '$lswNameStr'"

# Test 7.4: list-windows -F '#{pane_id}' returns %N format
$lswPid = & $exe -t lswfmt list-windows -F '#{pane_id}' 2>&1
$lswPidStr = ($lswPid | Out-String).Trim()
Test-Assert "list-windows -F '#{pane_id}' returns %N" ($lswPidStr -match '%\d+') "Got: '$lswPidStr'"

# Test 7.5: list-windows -F complex format
$lswComplex = & $exe -t lswfmt list-windows -F '#{window_index}:#{window_name} #{session_name}' 2>&1
$lswComplexStr = ($lswComplex | Out-String).Trim()
Test-Assert "list-windows -F complex format works" ($lswComplexStr -match '\d+:\S+ lswfmt') "Got: '$lswComplexStr'"

# Test 7.6: correct number of lines (2 windows = 2 lines)
$lineCount = ($lswIdxStr -split "`n" | Where-Object { $_.Trim() }).Count
Test-Assert "list-windows returns correct number of lines" ($lineCount -eq 2) "Got $lineCount lines"

# Cleanup
& $exe kill-session -t lswfmt 2>$null
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
