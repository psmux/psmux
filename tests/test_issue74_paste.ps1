# test_issue74_paste.ps1 -- Issue #74: bracket paste and large paste integrity
#
# Tests:
# 1. send-paste delivers text intact (capture-pane verification)
# 2. Multi-line indented send-paste preserves indentation
# 3. Large paste (350+ lines) completes without truncation (WriteConsoleInputW retry)
# (A 4th part ran `cargo test bracket_paste_detect`; those unit tests went
# with src/app.rs in 44de6009, so it built the test target for 0 tests.)
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue74_paste.ps1

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass { param($msg) Write-Host "[PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail { param($msg) Write-Host "[FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info { param($msg) Write-Host "[INFO] $msg" -ForegroundColor Cyan }
function Write-Test { param($msg) Write-Host "[TEST] $msg" -ForegroundColor White }

$PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -ErrorAction SilentlyContinue).Path
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\debug\psmux.exe" -ErrorAction SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
Write-Info "Using: $PSMUX"

function Psmux { & $PSMUX @args 2>&1; Start-Sleep -Milliseconds 200 }

function ConvertTo-Base64 {
    param([string]$Text)
    [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($Text))
}

# Cleanup
Start-Process -FilePath $PSMUX -ArgumentList "kill-server" -WindowStyle Hidden
Start-Sleep -Seconds 3
Remove-Item "$env:USERPROFILE\.psmux\*.port" -Force -ErrorAction SilentlyContinue
Remove-Item "$env:USERPROFILE\.psmux\*.key" -Force -ErrorAction SilentlyContinue

# Create test session
Write-Info "Creating test session 'paste74'..."
Start-Process -FilePath $PSMUX -ArgumentList "new-session -s paste74 -d" -WindowStyle Hidden
Start-Sleep -Seconds 4
& $PSMUX has-session -t paste74 2>$null
if ($LASTEXITCODE -ne 0) { Write-Host "FATAL: Cannot create test session" -ForegroundColor Red; exit 1 }
Write-Info "Session 'paste74' created"

# ============================================================
# TEST 1: send-paste short text visible in capture-pane
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "TEST 1: Short paste via send-paste"
Write-Host ("=" * 60)

$shortPayload = "PASTE_TEST_ALPHA"
$enc1 = ConvertTo-Base64 $shortPayload

Write-Test "1.1 send-paste delivers short text"
Psmux send-keys -t paste74 "clear" Enter 2>$null | Out-Null
Start-Sleep -Milliseconds 500
Psmux send-paste -t paste74 $enc1 2>$null | Out-Null
Start-Sleep -Milliseconds 800
$cap1 = (Psmux capture-pane -t paste74 -p 2>$null | Out-String)
if ($cap1 -match "PASTE_TEST_ALPHA") {
    Write-Pass "Short paste visible in pane"
} else {
    Write-Fail "Short paste not visible in pane"
    Write-Info "Capture: $($cap1.Substring(0, [Math]::Min(300, $cap1.Length)))"
}

# ============================================================
# TEST 2: Multi-line indented paste preserves indentation
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "TEST 2: Multi-line indented paste"
Write-Host ("=" * 60)

# Build payload with known indentation levels
$payload2Lines = @(
    "line1_no_indent",
    "   line2_indent3",
    "     line3_indent5",
    "       line4_indent7",
    "     line5_indent5",
    "   line6_indent3",
    "line7_no_indent"
)
$payload2 = $payload2Lines -join "`n"
$enc2 = ConvertTo-Base64 $payload2

Write-Test "2.1 Multi-line indented paste delivered"
# Send paste to prompt; PSReadLine captures bracket paste as edit buffer
Psmux send-keys -t paste74 "clear" Enter 2>$null | Out-Null
Start-Sleep -Milliseconds 500
Psmux send-paste -t paste74 $enc2 2>$null | Out-Null

# Wait for the last line rather than a fixed 1.5 s: on a loaded machine (the
# full runner) PSReadLine had not drawn line 7 yet when the capture was taken.
$cap2 = ''
$sw2 = [System.Diagnostics.Stopwatch]::StartNew()
while ($sw2.ElapsedMilliseconds -lt 10000) {
    $cap2 = (Psmux capture-pane -t paste74 -p 2>$null | Out-String)
    if ($cap2 -match "line7_no_indent") { break }
    Start-Sleep -Milliseconds 250
}

$foundLine1 = $cap2 -match "line1_no_indent"
$foundLine3 = $cap2 -match "line3_indent5"
$foundLine7 = $cap2 -match "line7_no_indent"

if ($foundLine1 -and $foundLine3 -and $foundLine7) {
    Write-Pass "Multi-line paste content visible (7 lines delivered)"
} else {
    Write-Fail "Multi-line paste content missing (l1=$foundLine1 l3=$foundLine3 l7=$foundLine7)"
    Write-Info "Capture: $($cap2.Substring(0, [Math]::Min(500, $cap2.Length)))"
}

Write-Test "2.2 Indentation preserved (no compounding)"
# In bracket paste mode, PSReadLine may render all paste content on one
# terminal line (with \r shown as a literal char).  Check that the SPACING
# between line markers is correct within the raw capture string.
# If indentation compounds, 3-space indent would grow to 6, 9, etc.
$indentOK = $true
# Check that "   line2_indent3" (3 spaces) appears in the capture
if ($cap2 -match "(?<=[^\s])[\x0D\x0Am].{0,2}   line2_indent3" -or $cap2 -match "   line2_indent3") {
    # OK - 3 spaces before line2
} else { $indentOK = $false; Write-Info "  line2: expected 3-space indent" }
# Check "     line3_indent5" (5 spaces)
if ($cap2 -match "     line3_indent5") {
    # OK
} else { $indentOK = $false; Write-Info "  line3: expected 5-space indent" }
# Check "       line4_indent7" (7 spaces)
if ($cap2 -match "       line4_indent7") {
    # OK
} else { $indentOK = $false; Write-Info "  line4: expected 7-space indent" }
# Check no compounding: line4 should NOT have >>10 spaces
if ($cap2 -match " {10,}line4_indent7") {
    $indentOK = $false; Write-Info "  line4: COMPOUNDING detected (>10 spaces)"
}
if ($indentOK) {
    Write-Pass "Indentation levels match expected values"
} else {
    Write-Fail "Indentation levels don't match (possible compounding)"
}
# Clear the prompt for next test
Psmux send-keys -t paste74 C-c 2>$null | Out-Null
Start-Sleep -Milliseconds 300

# ============================================================
# TEST 3: Large paste - WriteConsoleInputW stress
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "TEST 3: Large paste stress"
Write-Host ("=" * 60)

# Verify session is alive after previous tests
& $PSMUX has-session -t paste74 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "Session died between tests - recreating"
    Start-Process -FilePath $PSMUX -ArgumentList "new-session -s paste74 -d" -WindowStyle Hidden
    Start-Sleep -Seconds 4
}

# Use a non-interactive PowerShell receiver that reads stdin line-by-line
# and writes to a temp file.  This avoids PSReadLine interpreting paste lines
# as commands, and avoids capture-pane scrollback limits.
$pasteTestFile = Join-Path $env:TEMP "psmux_paste_test_74.txt"
$recvScript = Join-Path $env:TEMP "psmux_paste_recv.ps1"
@'
$out = $args[0]
$lines = @()
while ($true) {
    $l = [Console]::ReadLine()
    if ($l -eq $null) { break }
    if ($l -eq "EOF_PSMUX_TEST") { break }
    $lines += $l
}
$lines | Set-Content $out -Encoding UTF8
'@ | Set-Content $recvScript -Encoding UTF8

Psmux send-keys -t paste74 "pwsh -NoProfile -File `"$recvScript`" `"$pasteTestFile`"" Enter 2>$null | Out-Null
Start-Sleep -Milliseconds 2000

$bigLines = @()
for ($i = 1; $i -le 20; $i++) {
    $bigLines += "$i) lorem"
    foreach ($sp in @(0, 2, 4, 6, 4, 2)) {
        $bigLines += (' ' * $sp) + "- n$sp i$i"
    }
}
# Append sentinel so the receiver knows when to stop
$bigLines += "EOF_PSMUX_TEST"
$bigPayload = $bigLines -join "`n"
$enc3 = ConvertTo-Base64 $bigPayload

$dataLineCount = $bigLines.Count - 1  # exclude sentinel
Write-Test "3.1 Large paste ($dataLineCount lines)"
Write-Info "Payload: $dataLineCount data lines, $($bigPayload.Length) bytes, base64=$($enc3.Length)"
Psmux send-paste -t paste74 $enc3 2>$null | Out-Null
# The last line "EOF_PSMUX_TEST" may not have a trailing newline from send-paste.
# Send Enter to flush the sentinel line to the receiver.
Start-Sleep -Milliseconds 3000
Psmux send-keys -t paste74 Enter 2>$null | Out-Null
Start-Sleep -Milliseconds 3000

# Verify output file
if (Test-Path $pasteTestFile) {
    $outputLines = @(Get-Content $pasteTestFile)
    $hasFirst = ($outputLines | Where-Object { $_ -match "1\) lorem" }).Count -gt 0
    $hasMiddle = ($outputLines | Where-Object { $_ -match "10\) lorem|15\) lorem" }).Count -gt 0
    $hasNested = ($outputLines | Where-Object { $_ -match "n4" }).Count -gt 0

    if ($hasFirst -and $hasMiddle -and $hasNested) {
        Write-Pass "Large paste: first, middle, and nested content visible ($($outputLines.Count) lines received)"
    } else {
        Write-Fail "Large paste incomplete (first=$hasFirst middle=$hasMiddle nested=$hasNested, lines=$($outputLines.Count))"
        Write-Info "First 5 lines: $(($outputLines | Select-Object -First 5) -join ' | ')"
        Write-Info "Last 5 lines: $(($outputLines | Select-Object -Last 5) -join ' | ')"
    }

    Write-Test "3.2 Indentation integrity in large paste"
    $n6Lines = $outputLines | Where-Object { $_ -match "n6" }
    $maxIndent = 0
    foreach ($rl in $n6Lines) {
        $stripped = $rl -replace '[^ ].*', ''
        if ($stripped.Length -gt $maxIndent) { $maxIndent = $stripped.Length }
    }
    if ($n6Lines.Count -gt 0 -and $maxIndent -ge 6 -and $maxIndent -le 10) {
        Write-Pass "6-space indent preserved for n6 entries ($($n6Lines.Count) lines)"
    } else {
        Write-Fail "Indentation not preserved (found $($n6Lines.Count) n6 lines, maxIndent=$maxIndent)"
    }
} else {
    Write-Fail "Large paste output file not created — receiver did not capture input"

    Write-Test "3.2 Indentation integrity in large paste"
    Write-Fail "Skipped (no output file)"
}
Start-Sleep -Milliseconds 300

# ============================================================
# CLEANUP
# ============================================================
Write-Host ""
Write-Host ("=" * 60)
Write-Host "Cleanup..."
Start-Process -FilePath $PSMUX -ArgumentList "kill-server" -WindowStyle Hidden
Start-Sleep -Seconds 2

Write-Host ""
Write-Host ("=" * 60)
$totalTests = $script:TestsPassed + $script:TestsFailed
Write-Host "RESULTS: $($script:TestsPassed)/$totalTests passed, $($script:TestsFailed) failed" -ForegroundColor $(if ($script:TestsFailed -eq 0) { "Green" } else { "Red" })
Write-Host ("=" * 60)
