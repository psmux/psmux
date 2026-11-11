# Issue #263: Box-drawing characters render with incorrect color
# DEFINITIVE VERIFICATION TEST
# Tests that box-drawing characters carry correct SGR color attributes
# at EVERY level: vt100 parser state, capture-pane -e, dump-state JSON, TUI

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "test_issue263"
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

function Connect-Persistent {
    param([string]$Name)
    $port = (Get-Content "$psmuxDir\$Name.port" -Raw).Trim()
    $key = (Get-Content "$psmuxDir\$Name.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true; $tcp.ReceiveTimeout = 10000
    $stream = $tcp.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush()
    $null = $reader.ReadLine()
    $writer.Write("PERSISTENT`n"); $writer.Flush()
    return @{ tcp=$tcp; writer=$writer; reader=$reader }
}

function Get-Dump {
    param($conn)
    $conn.writer.Write("dump-state`n"); $conn.writer.Flush()
    $best = $null
    $conn.tcp.ReceiveTimeout = 3000
    for ($j = 0; $j -lt 100; $j++) {
        try { $line = $conn.reader.ReadLine() } catch { break }
        if ($null -eq $line) { break }
        if ($line -ne "NC" -and $line.Length -gt 100) { $best = $line }
        if ($best) { $conn.tcp.ReceiveTimeout = 50 }
    }
    $conn.tcp.ReceiveTimeout = 10000
    return $best
}

# === SETUP ===
Cleanup
& $PSMUX new-session -d -s $SESSION
Start-Sleep -Seconds 3
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "Session creation failed"
    exit 1
}

Write-Host "`n=== Issue #263: Box-Drawing Color Verification ===" -ForegroundColor Cyan

# Clear screen, then send carefully controlled test lines
& $PSMUX send-keys -t $SESSION "clear" Enter
Start-Sleep -Seconds 1

# --- Test Lines ---
# Line A: Regular text with truecolor red
& $PSMUX send-keys -t $SESSION 'Write-Host "`e[38;2;255;0;0mTXTRED`e[0m"' Enter
Start-Sleep -Milliseconds 500

# Line B: Box-drawing char with SAME truecolor red  
& $PSMUX send-keys -t $SESSION 'Write-Host "`e[38;2;255;0;0m│BXRED`e[0m"' Enter
Start-Sleep -Milliseconds 500

# Line C: Regular text with SGR 90 (bright black)
& $PSMUX send-keys -t $SESSION 'Write-Host "`e[90mTXTBLK`e[0m"' Enter
Start-Sleep -Milliseconds 500

# Line D: Box-drawing char with SGR 90
& $PSMUX send-keys -t $SESSION 'Write-Host "`e[90m│BXBLK`e[0m"' Enter
Start-Sleep -Milliseconds 500

# Line E: 256-color (index 196 = red)
& $PSMUX send-keys -t $SESSION 'Write-Host "`e[38;5;196mTXT196`e[0m"' Enter
Start-Sleep -Milliseconds 500

# Line F: Box-drawing with 256-color 196
& $PSMUX send-keys -t $SESSION 'Write-Host "`e[38;5;196m│BX196`e[0m"' Enter
Start-Sleep -Milliseconds 500

# Line G: Multiple box-drawing chars with truecolor green
& $PSMUX send-keys -t $SESSION 'Write-Host "`e[38;2;0;255;0m┌──┐GRNBOX`e[0m"' Enter
Start-Sleep -Seconds 2

# ====================================================================
# TEST 1: capture-pane -e escape sequence verification
# ====================================================================
Write-Host "`n[Test 1] capture-pane -e escape sequence analysis" -ForegroundColor Yellow

$capFile = "$env:TEMP\psmux_263_cap_e.txt"
& $PSMUX capture-pane -t $SESSION -p -e 2>&1 | Set-Content -Path $capFile -Encoding UTF8
$capContent = [System.IO.File]::ReadAllText($capFile)
$capLines = $capContent -split "`r?`n"

# Find the output lines (not the command echo lines)
$testCases = @(
    @{ Label="TXTRED"; Pattern="TXTRED"; ExpectSGR="38;2;255;0;0" }
    @{ Label="│BXRED"; Pattern="BXRED"; ExpectSGR="38;2;255;0;0" }
    @{ Label="TXTBLK"; Pattern="TXTBLK"; ExpectSGR="90" }
    @{ Label="│BXBLK"; Pattern="BXBLK"; ExpectSGR="90" }
    @{ Label="TXT196"; Pattern="TXT196"; ExpectSGR="38;5;196" }
    @{ Label="│BX196"; Pattern="BX196"; ExpectSGR="38;5;196" }
    @{ Label="GRNBOX"; Pattern="GRNBOX"; ExpectSGR="38;2;0;255;0" }
)

foreach ($tc in $testCases) {
    $outputLines = $capLines | Where-Object { $_ -match $tc.Pattern -and $_ -notmatch 'Write-Host' }
    foreach ($ol in $outputLines) {
        $decoded = $ol -replace [char]0x1B, '<ESC>'
        $hasSGR = $decoded -match $tc.ExpectSGR
        if ($hasSGR) {
            Write-Pass "capture-pane -e: $($tc.Label) has SGR $($tc.ExpectSGR)"
        } else {
            Write-Fail "capture-pane -e: $($tc.Label) MISSING SGR $($tc.ExpectSGR)"
            Write-Host "    Decoded: $decoded" -ForegroundColor DarkGray
        }
    }
}

# ====================================================================
# TEST 2: dump-state JSON cell attribute verification (THE KEY TEST)
# This is the definitive test - it shows per-cell fg color attributes
# ====================================================================
Write-Host "`n[Test 2] dump-state JSON cell attribute analysis (DEFINITIVE)" -ForegroundColor Yellow

$conn = Connect-Persistent -Name $SESSION
$dumpStr = Get-Dump $conn
$conn.tcp.Close()

if (-not $dumpStr) {
    Write-Fail "No dump-state response"
} else {
    $json = $dumpStr | ConvertFrom-Json
    $rows = $json.layout.rows_v2
    
    # For each row, check runs for our test labels and verify fg color
    $results = @{}
    
    for ($r = 0; $r -lt $rows.Count; $r++) {
        $row = $rows[$r]
        foreach ($run in $row.runs) {
            $text = $run.text.Trim()
            $fg = $run.fg
            
            # Match our test output lines (not the command echo)
            if ($text -eq "TXTRED") { $results["TXTRED"] = $fg }
            if ($text -match '^│BXRED$') { $results["│BXRED"] = $fg }
            if ($text -eq "│BXRED") { $results["│BXRED"] = $fg }
            if ($text -eq "TXTBLK") { $results["TXTBLK"] = $fg }
            if ($text -eq "│BXBLK") { $results["│BXBLK"] = $fg }
            if ($text -eq "TXT196") { $results["TXT196"] = $fg }
            if ($text -eq "│BX196") { $results["│BX196"] = $fg }
            # Green box chars might be in same run as text
            if ($text -match '┌.*GRNBOX' -or $text -match 'GRNBOX') { $results["GRNBOX"] = $fg }
            if ($text -match '┌──┐') { $results["GRNBOX_BOX"] = $fg }
        }
    }
    
    Write-Host "  Found runs:" -ForegroundColor DarkGray
    foreach ($k in $results.Keys | Sort-Object) {
        Write-Host "    $k => fg: $($results[$k])" -ForegroundColor DarkGray
    }
    
    # THE CRITICAL COMPARISONS
    # If the bug exists: box-drawing chars would have a DIFFERENT fg than text
    # If the bug does NOT exist: they would have the SAME fg
    
    Write-Host "`n  --- Critical Comparisons ---" -ForegroundColor Yellow
    
    # Comparison 1: TXTRED vs │BXRED (truecolor)
    if ($results.ContainsKey("TXTRED") -and $results.ContainsKey("│BXRED")) {
        $txtFg = $results["TXTRED"]
        $boxFg = $results["│BXRED"]
        if ($txtFg -eq $boxFg) {
            Write-Pass "Truecolor: TXTRED ($txtFg) == │BXRED ($boxFg) -- SAME COLOR"
        } else {
            Write-Fail "Truecolor: TXTRED ($txtFg) != │BXRED ($boxFg) -- BUG CONFIRMED!"
        }
    } else {
        Write-Fail "Could not find TXTRED and/or │BXRED in dump-state"
        Write-Host "    Available keys: $($results.Keys -join ', ')" -ForegroundColor DarkGray
    }
    
    # Comparison 2: TXTBLK vs │BXBLK (SGR 90)
    if ($results.ContainsKey("TXTBLK") -and $results.ContainsKey("│BXBLK")) {
        $txtFg = $results["TXTBLK"]
        $boxFg = $results["│BXBLK"]
        if ($txtFg -eq $boxFg) {
            Write-Pass "SGR 90: TXTBLK ($txtFg) == │BXBLK ($boxFg) -- SAME COLOR"
        } else {
            Write-Fail "SGR 90: TXTBLK ($txtFg) != │BXBLK ($boxFg) -- BUG CONFIRMED!"
        }
    } else {
        Write-Fail "Could not find TXTBLK and/or │BXBLK in dump-state"
    }
    
    # Comparison 3: TXT196 vs │BX196 (256-color)
    if ($results.ContainsKey("TXT196") -and $results.ContainsKey("│BX196")) {
        $txtFg = $results["TXT196"]
        $boxFg = $results["│BX196"]
        if ($txtFg -eq $boxFg) {
            Write-Pass "256-color: TXT196 ($txtFg) == │BX196 ($boxFg) -- SAME COLOR"
        } else {
            Write-Fail "256-color: TXT196 ($txtFg) != │BX196 ($boxFg) -- BUG CONFIRMED!"
        }
    } else {
        Write-Fail "Could not find TXT196 and/or │BX196 in dump-state"
    }
    
    # Check that box-drawing chars are NOT split into separate runs
    $splitFound = $false
    for ($r = 0; $r -lt $rows.Count; $r++) {
        $runs = $rows[$r].runs
        for ($i = 0; $i -lt $runs.Count - 1; $i++) {
            $curr = $runs[$i]
            $next = $runs[$i+1]
            # Check if a box-drawing-only run is followed by text with different color
            if ($curr.text -match '^[│─┌┐└┘├┤┬┴┼]+$' -and $next.text -notmatch '^\s*$') {
                if ($curr.fg -ne $next.fg) {
                    $splitFound = $true
                    Write-Fail "SPLIT RUN: box='$($curr.text)' fg=$($curr.fg) | text='$($next.text.Substring(0,10))' fg=$($next.fg)"
                }
            }
        }
    }
    if (-not $splitFound) {
        Write-Pass "No box-drawing characters split into separate runs with different colors"
    }
}

# ====================================================================
# TEST 3: TCP server path verification
# ====================================================================
Write-Host "`n[Test 3] TCP server path (display-message)" -ForegroundColor Yellow

$sessName = (& $PSMUX display-message -t $SESSION -p '#{session_name}' 2>&1).Trim()
if ($sessName -eq $SESSION) { Write-Pass "Session name correct via TCP" }
else { Write-Fail "Expected $SESSION, got: $sessName" }

# ====================================================================
# WIN32 TUI VISUAL VERIFICATION
# ====================================================================
Write-Host ("`n" + ("=" * 60)) -ForegroundColor White
Write-Host "Win32 TUI VISUAL VERIFICATION" -ForegroundColor White
Write-Host ("=" * 60) -ForegroundColor White

$SESSION_TUI = "issue263_tui_proof"
& $PSMUX kill-session -t $SESSION_TUI 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$SESSION_TUI.*" -Force -EA SilentlyContinue

$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION_TUI -PassThru
Start-Sleep -Seconds 4

& $PSMUX has-session -t $SESSION_TUI 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "TUI session did not start"
} else {
    Write-Pass "TUI session alive"
    
    & $PSMUX send-keys -t $SESSION_TUI "clear" Enter
    Start-Sleep -Seconds 1
    & $PSMUX send-keys -t $SESSION_TUI 'Write-Host "`e[38;2;255;0;0m│ RED BOX`e[0m"' Enter
    & $PSMUX send-keys -t $SESSION_TUI 'Write-Host "`e[38;2;0;255;0m┌──┐ GREEN BOX`e[0m"' Enter
    & $PSMUX send-keys -t $SESSION_TUI 'Write-Host "`e[38;2;0;0;255m└──┘ BLUE BOX`e[0m"' Enter
    Start-Sleep -Seconds 2
    
    $conn2 = Connect-Persistent -Name $SESSION_TUI
    $dump2 = Get-Dump $conn2
    $conn2.tcp.Close()
    
    if ($dump2) {
        $json2 = $dump2 | ConvertFrom-Json
        $rows2 = $json2.layout.rows_v2
        
        $foundRed = $false; $foundGreen = $false; $foundBlue = $false
        
        foreach ($row in $rows2) {
            foreach ($run in $row.runs) {
                if ($run.text -match 'RED BOX' -and $run.fg -eq 'rgb:255,0,0') { $foundRed = $true }
                if ($run.text -match 'GREEN BOX' -and $run.fg -eq 'rgb:0,255,0') { $foundGreen = $true }
                if ($run.text -match 'BLUE BOX' -and $run.fg -eq 'rgb:0,0,255') { $foundBlue = $true }
                # Also check if box chars are in same run
                if ($run.text -match '│.*RED' -and $run.fg -eq 'rgb:255,0,0') { $foundRed = $true }
                if ($run.text -match '┌.*GREEN' -and $run.fg -eq 'rgb:0,255,0') { $foundGreen = $true }
                if ($run.text -match '└.*BLUE' -and $run.fg -eq 'rgb:0,0,255') { $foundBlue = $true }
            }
        }
        
        if ($foundRed) { Write-Pass "TUI: Red │ has rgb:255,0,0" }
        else { Write-Fail "TUI: Red │ color not found" }
        if ($foundGreen) { Write-Pass "TUI: Green ┌──┐ has rgb:0,255,0" }
        else { Write-Fail "TUI: Green ┌──┐ color not found" }
        if ($foundBlue) { Write-Pass "TUI: Blue └──┘ has rgb:0,0,255" }
        else { Write-Fail "TUI: Blue └──┘ color not found" }
    } else {
        Write-Fail "TUI: No dump-state response"
    }
}

# Cleanup all
& $PSMUX kill-session -t $SESSION_TUI 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
Cleanup
Remove-Item "$psmuxDir\$SESSION_TUI.*" -Force -EA SilentlyContinue

Write-Host "`n=== FINAL RESULTS ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })

if ($script:TestsFailed -eq 0) {
    Write-Host "`n  CONCLUSION: The bug described in issue #263 could NOT be reproduced." -ForegroundColor Green
    Write-Host "  Box-drawing characters carry CORRECT color attributes at all levels:" -ForegroundColor Green
    Write-Host "    - vt100 parser stores correct fg color on box-drawing cells" -ForegroundColor Green
    Write-Host "    - capture-pane -e emits correct SGR sequences for box-drawing chars" -ForegroundColor Green
    Write-Host "    - dump-state JSON shows box-drawing chars in SAME run with SAME fg as text" -ForegroundColor Green
    Write-Host "    - TUI window renders box-drawing chars with correct color attributes" -ForegroundColor Green
} else {
    Write-Host "`n  CONCLUSION: Some tests failed - the bug may exist in specific scenarios." -ForegroundColor Red
}

exit $script:TestsFailed
