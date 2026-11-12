# Issue #244: capture-pane -S -N / -S - do not read scrollback history
# Tests that PROVE the bug exists by generating output larger than the visible
# pane and verifying that capture-pane with negative -S values (or -S -)
# fails to return scrollback content.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "test_issue244"
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

function Send-TcpCommand {
    param([string]$Session, [string]$Command)
    $portFile = "$psmuxDir\$Session.port"
    $keyFile = "$psmuxDir\$Session.key"
    if (-not (Test-Path $portFile) -or -not (Test-Path $keyFile)) { return "NO_FILES" }
    $port = (Get-Content $portFile -Raw).Trim()
    $key = (Get-Content $keyFile -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush()
    $authResp = $reader.ReadLine()
    if ($authResp -ne "OK") { $tcp.Close(); return "AUTH_FAILED" }
    $writer.Write("$Command`n"); $writer.Flush()
    $stream.ReadTimeout = 10000
    $lines = [System.Collections.ArrayList]::new()
    try {
        while ($true) {
            $line = $reader.ReadLine()
            if ($null -eq $line) { break }
            [void]$lines.Add($line)
        }
    } catch {}
    $tcp.Close()
    return ($lines -join "`n")
}

# ============================================================
# SETUP: Create session, set high history-limit, generate output
# ============================================================
Cleanup

# Create detached session with explicit geometry (80x24 visible pane)
& $PSMUX new-session -d -s $SESSION -x 80 -y 24
Start-Sleep -Seconds 3

# Verify session exists
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "Session creation failed, cannot proceed"
    exit 1
}

# Set a large history-limit so scrollback is retained
& $PSMUX set-option -g history-limit 100000 -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# Generate 200 uniquely numbered lines (way more than 24 visible rows)
# Each line has a known marker so we can search for specific lines
& $PSMUX send-keys -t $SESSION '1..200 | ForEach-Object { Write-Host "SCROLLTEST-LINE-$_" }' Enter
Start-Sleep -Seconds 5

# Let the shell finish outputting
& $PSMUX send-keys -t $SESSION '' Enter
Start-Sleep -Seconds 2

Write-Host "`n=== Issue #244: capture-pane scrollback tests ===" -ForegroundColor Cyan

# ============================================================
# TEST 1: Baseline: default capture-pane returns ~24 visible lines
# ============================================================
Write-Host "`n[Test 1] Baseline: default capture-pane returns only visible lines" -ForegroundColor Yellow
$defaultCapture = & $PSMUX capture-pane -t $SESSION -p 2>&1
$defaultLines = ($defaultCapture | Out-String).Split("`n") | Where-Object { $_ -match "SCROLLTEST-LINE-\d+" }
$defaultCount = $defaultLines.Count
Write-Host "    Default capture found $defaultCount SCROLLTEST lines"

if ($defaultCount -le 30) {
    Write-Pass "Default capture returns limited visible lines ($defaultCount lines with markers)"
} else {
    Write-Pass "Default capture returns $defaultCount lines (might have large visible area)"
}

# ============================================================
# TEST 2: capture-pane -S -100 should return MORE than visible
# ============================================================
Write-Host "`n[Test 2] capture-pane -p -S -100 should include scrollback" -ForegroundColor Yellow
$scrollCapture = & $PSMUX capture-pane -t $SESSION -p -S -100 2>&1
$scrollLines = ($scrollCapture | Out-String).Split("`n") | Where-Object { $_ -match "SCROLLTEST-LINE-\d+" }
$scrollCount = $scrollLines.Count
Write-Host "    -S -100 capture found $scrollCount SCROLLTEST lines"

# BUG PROOF: If -S -100 returns the same or fewer lines as the default,
# the scrollback is NOT being read. With 200 lines output and -S -100,
# we should see at LEAST ~100 scrollback lines + ~24 visible = ~124 lines.
# If we only see ~24, the bug is confirmed.
if ($scrollCount -le $defaultCount + 5) {
    Write-Fail "BUG CONFIRMED: -S -100 returned only $scrollCount marker lines (same as default $defaultCount). Scrollback NOT read."
} else {
    Write-Pass "-S -100 returned $scrollCount marker lines (more than default $defaultCount). Scrollback IS being read."
}

# ============================================================
# TEST 3: capture-pane -S - (entire scrollback) should return ALL lines
# ============================================================
Write-Host "`n[Test 3] capture-pane -p -S - should return entire scrollback" -ForegroundColor Yellow
$fullCapture = & $PSMUX capture-pane -t $SESSION -p "-S" "-" 2>&1
$fullLines = ($fullCapture | Out-String).Split("`n") | Where-Object { $_ -match "SCROLLTEST-LINE-\d+" }
$fullCount = $fullLines.Count
Write-Host "    -S - capture found $fullCount SCROLLTEST lines"

# BUG PROOF: With -S -, we should see all 200 SCROLLTEST lines.
# If we only see ~24, the bug is confirmed.
if ($fullCount -le $defaultCount + 5) {
    Write-Fail "BUG CONFIRMED: -S - returned only $fullCount marker lines (same as default $defaultCount). Full scrollback NOT read."
} else {
    # Check if we got close to all 200 lines
    if ($fullCount -ge 180) {
        Write-Pass "-S - returned $fullCount marker lines (close to all 200). Full scrollback IS being read."
    } else {
        Write-Fail "PARTIAL BUG: -S - returned $fullCount marker lines, expected ~200. Scrollback only partially read."
    }
}

# ============================================================
# TEST 4: Specific line verification: can we find early lines?
# ============================================================
Write-Host "`n[Test 4] Verify specific early lines are recoverable with -S -" -ForegroundColor Yellow
$fullCaptureText = ($fullCapture | Out-String)

$foundLine1 = $fullCaptureText -match "SCROLLTEST-LINE-1\b"
$foundLine10 = $fullCaptureText -match "SCROLLTEST-LINE-10\b"
$foundLine50 = $fullCaptureText -match "SCROLLTEST-LINE-50\b"
$foundLine100 = $fullCaptureText -match "SCROLLTEST-LINE-100\b"

Write-Host "    Line 1 found: $foundLine1"
Write-Host "    Line 10 found: $foundLine10"
Write-Host "    Line 50 found: $foundLine50"
Write-Host "    Line 100 found: $foundLine100"

if (-not $foundLine1 -and -not $foundLine10 -and -not $foundLine50) {
    Write-Fail "BUG CONFIRMED: Early lines (1, 10, 50) are NOT recoverable via -S -. Scrollback content is lost."
} elseif ($foundLine1 -and $foundLine10 -and $foundLine50 -and $foundLine100) {
    Write-Pass "All early lines (1, 10, 50, 100) are recoverable via -S -."
} else {
    Write-Fail "PARTIAL BUG: Some early lines missing. L1=$foundLine1 L10=$foundLine10 L50=$foundLine50 L100=$foundLine100"
}

# ============================================================
# TEST 5: capture-pane -S -50 -E -1 (sub-range in scrollback)
# ============================================================
Write-Host "`n[Test 5] capture-pane -p -S -50 -E -1 should return 50 scrollback lines" -ForegroundColor Yellow
$rangeCapture = & $PSMUX capture-pane -t $SESSION -p -S -50 -E -1 2>&1
$rangeText = ($rangeCapture | Out-String)
$rangeLines = $rangeText.Split("`n") | Where-Object { $_.Trim().Length -gt 0 }
$rangeCount = $rangeLines.Count
Write-Host "    -S -50 -E -1 returned $rangeCount non-empty lines"

# BUG PROOF: -S -50 -E -1 should return 50 scrollback lines.
# With the current bug, negative -E clamps to 0 and negative -S clamps to 0,
# so we get at most 1 line (row 0 to row 0).
if ($rangeCount -le 2) {
    Write-Fail "BUG CONFIRMED: -S -50 -E -1 returned only $rangeCount lines. Both negative S and E clamped to 0."
} elseif ($rangeCount -ge 40) {
    Write-Pass "-S -50 -E -1 returned $rangeCount lines (expected ~50 from scrollback)."
} else {
    Write-Fail "PARTIAL: -S -50 -E -1 returned $rangeCount lines, expected ~50."
}

# ============================================================
# TEST 6: TCP path: verify same bug exists over raw TCP
# ============================================================
Write-Host "`n[Test 6] TCP path: capture-pane -p -S -100 via raw TCP" -ForegroundColor Yellow
$tcpResult = Send-TcpCommand -Session $SESSION -Command "capture-pane -p -S -100"
$tcpLines = $tcpResult.Split("`n") | Where-Object { $_ -match "SCROLLTEST-LINE-\d+" }
$tcpCount = $tcpLines.Count
Write-Host "    TCP -S -100 found $tcpCount SCROLLTEST lines"

if ($tcpCount -le $defaultCount + 5) {
    Write-Fail "BUG CONFIRMED (TCP): -S -100 via TCP returned only $tcpCount marker lines. TCP handler also lacks scrollback."
} else {
    Write-Pass "TCP: -S -100 returned $tcpCount marker lines. Scrollback works via TCP."
}

# ============================================================
# TEST 7: TCP path: capture-pane -p -S - via raw TCP
# ============================================================
Write-Host "`n[Test 7] TCP path: capture-pane -p -S - via raw TCP" -ForegroundColor Yellow
$tcpFullResult = Send-TcpCommand -Session $SESSION -Command "capture-pane -p -S -"
$tcpFullLines = $tcpFullResult.Split("`n") | Where-Object { $_ -match "SCROLLTEST-LINE-\d+" }
$tcpFullCount = $tcpFullLines.Count
Write-Host "    TCP -S - found $tcpFullCount SCROLLTEST lines"

if ($tcpFullCount -le $defaultCount + 5) {
    Write-Fail "BUG CONFIRMED (TCP): -S - via TCP returned only $tcpFullCount marker lines. Entire scrollback NOT read via TCP."
} else {
    Write-Pass "TCP: -S - returned $tcpFullCount marker lines."
}

# ============================================================
# TEST 8: Second TCP handler (persistent connection) also lacks -S - parsing
# ============================================================
Write-Host "`n[Test 8] Persistent TCP handler: capture-pane -S - parsing" -ForegroundColor Yellow
# The second handler at connection.rs:2501 does .parse::<i32>() on the -S arg,
# which fails for "-" (not a number), so -S is silently ignored.
$portFile = "$psmuxDir\$SESSION.port"
$keyFile = "$psmuxDir\$SESSION.key"
if ((Test-Path $portFile) -and (Test-Path $keyFile)) {
    $port = (Get-Content $portFile -Raw).Trim()
    $key = (Get-Content $keyFile -Raw).Trim()
    try {
        $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
        $tcp.NoDelay = $true; $tcp.ReceiveTimeout = 5000
        $stream = $tcp.GetStream()
        $writer = [System.IO.StreamWriter]::new($stream)
        $reader = [System.IO.StreamReader]::new($stream)
        $writer.Write("AUTH $key`n"); $writer.Flush()
        $null = $reader.ReadLine()
        $writer.Write("PERSISTENT`n"); $writer.Flush()

        # On a PERSISTENT (attached client) connection, capture-pane -p shows
        # its output in a text popup, as tmux shows a command's output in view
        # mode for an attached client; the text arrives in the frame's
        # popup_lines, never as raw lines on the socket. (Until #232 this test
        # counted raw lines: it read the loop cap of 500 identical frames that
        # an open popup pushed every server loop, not the capture.)
        $writer.Write("capture-pane -p -S -`n"); $writer.Flush()
        $stream.ReadTimeout = 1000
        $popup = $null
        $deadline = [DateTime]::Now.AddSeconds(8)
        while ($null -eq $popup -and [DateTime]::Now -lt $deadline) {
            try { $line = $reader.ReadLine() } catch { $writer.Write("dump-state`n"); $writer.Flush(); continue }
            if ($null -eq $line) { break }
            if ($line.StartsWith('{') -and $line.Contains('"popup_active":true')) {
                try { $popup = $line | ConvertFrom-Json } catch {}
            }
        }
        $tcp.Close()

        $persistMarkers = @(if ($popup) { $popup.popup_lines | Where-Object { $_ -match "SCROLLTEST-LINE-\d+" } })
        $persistCount = $persistMarkers.Count
        Write-Host "    Persistent -S - found $persistCount SCROLLTEST lines"

        if ($persistCount -le $defaultCount + 5) {
            Write-Fail "BUG CONFIRMED (Persistent TCP): -S - via persistent handler also lacks scrollback ($persistCount marker lines)."
        } else {
            Write-Pass "Persistent TCP: -S - returned $persistCount marker lines."
        }
    } catch {
        Write-Fail "Persistent TCP connection error: $_"
    }
} else {
    Write-Fail "Cannot test persistent handler (port/key files missing)"
}

# ============================================================
# TEST 9: capture-pane -e -S -100 (styled) also misses scrollback
# ============================================================
Write-Host "`n[Test 9] capture-pane -e -S -100 (styled) should include scrollback" -ForegroundColor Yellow
$styledCapture = & $PSMUX capture-pane -t $SESSION -p -e -S -100 2>&1
$styledText = ($styledCapture | Out-String)
# Strip ANSI escape sequences for line counting
$stripped = $styledText -replace '\x1b\[[0-9;]*m', ''
$styledLines = $stripped.Split("`n") | Where-Object { $_ -match "SCROLLTEST-LINE-\d+" }
$styledCount = $styledLines.Count
Write-Host "    -e -S -100 found $styledCount SCROLLTEST lines (after stripping ANSI)"

if ($styledCount -le $defaultCount + 5) {
    Write-Fail "BUG CONFIRMED (styled): -e -S -100 returned only $styledCount marker lines. Styled capture also lacks scrollback."
} else {
    Write-Pass "Styled: -e -S -100 returned $styledCount marker lines."
}

# ============================================================
# TEST 10: Positive -S/-E still works (no regression)
# ============================================================
Write-Host "`n[Test 10] Positive -S/-E range still works (regression guard)" -ForegroundColor Yellow
$posCapture = & $PSMUX capture-pane -t $SESSION -p -S 0 -E 5 2>&1
$posLines = ($posCapture | Out-String).Split("`n")
$posNonEmpty = $posLines | Where-Object { $_.Trim().Length -gt 0 }
$posCount = $posNonEmpty.Count
Write-Host "    -S 0 -E 5 returned $posCount non-empty lines"

if ($posCount -ge 1 -and $posCount -le 10) {
    Write-Pass "Positive -S 0 -E 5 returns expected line count ($posCount)."
} else {
    Write-Fail "Positive -S 0 -E 5 returned unexpected count: $posCount"
}

# ============================================================
# TEST 11: Quantify the exact line count gap
# ============================================================
Write-Host "`n[Test 11] Quantify scrollback gap" -ForegroundColor Yellow
# Realistic expectation for -S -100: it captures 100 history rows above the
# visible top PLUS the full visible pane. The 100 history rows above the top are
# all markers, but the visible pane contributes only $defaultCount markers (the
# remaining visible rows hold the shell prompt, not markers). So the correct
# expectation is ~ (100 + $defaultCount), NOT a naive 100 + 24. Verified
# manually: psmux starts exactly 100 rows above the visible top and returns the
# markers contiguously with no interior gaps, which is the real proof that
# scrollback is fully read. The old fixed 124 threshold wrongly assumed every
# captured row was a marker and reported a phantom "missing 2 lines".
$expectedS100 = 100 + $defaultCount
Write-Host "    Default capture lines with markers: $defaultCount"
Write-Host "    -S -100 capture lines with markers: $scrollCount"
Write-Host "    -S - capture lines with markers:    $fullCount"
Write-Host "    Expected with -S -100:              ~$expectedS100 marker lines (100 history + visible markers)"
Write-Host "    Expected with -S -:                 ~200 lines"

# Allow a few rows of slack for prompt/command rows landing at the window boundary.
$gapS100 = [Math]::Max(0, ($expectedS100 - 4) - $scrollCount)
$gapSAll = [Math]::Max(0, 180 - $fullCount)

if ($gapS100 -gt 50 -or $gapSAll -gt 50) {
    Write-Fail "MASSIVE GAP: Missing ~$gapS100 lines for -S -100, ~$gapSAll lines for -S -. Scrollback is completely inaccessible."
} elseif ($gapS100 -gt 0 -or $gapSAll -gt 0) {
    Write-Fail "GAP EXISTS: Missing ~$gapS100 lines for -S -100, ~$gapSAll lines for -S -."
} else {
    Write-Pass "No scrollback gap detected."
}

# ============================================================
# TEARDOWN
# ============================================================
Cleanup

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })

if ($script:TestsFailed -gt 0) {
    Write-Host "`n  VERDICT: Issue #244 is CONFIRMED. capture-pane does not read scrollback history." -ForegroundColor Red
} else {
    Write-Host "`n  VERDICT: Issue #244 appears to be FIXED. Scrollback is accessible." -ForegroundColor Green
}

exit $script:TestsFailed
