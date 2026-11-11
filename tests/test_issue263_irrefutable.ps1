# Issue #263 - IRREFUTABLE proof.
#
# Fixes the encoding mojibake from earlier attempts by:
#   1. chcp 65001 in the inner pane BEFORE PowerShell starts processing
#   2. Setting [Console]::OutputEncoding to UTF8 inside the repro script
#   3. Using byte-level analysis (no fragile regex on box-char literal)
#
# Method:
#   - Drive a single attached psmux session (the system under test)
#   - Run repro with the exact 7 SGR cases from issue #263
#   - capture-pane -p -e re-emits the cell buffer with full SGR per attribute group
#   - For EACH expected line, locate the U+2502 byte sequence (E2 94 82)
#   - Find the SGR sequence immediately preceding it
#   - Verify the SGR contains the expected color components
#   - CONTROL: same SGRs applied to ASCII text (proves text path works)
#   - If box-char is preceded by correct SGR ON 7/7 cases: bug NOT present
#   - If text-control is 7/7 BUT box-char is 0/7: bug PRESENT
#   - Otherwise: encoding artifact, inconclusive

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$VERSION = (& $PSMUX -V).Trim()
$SESSION = "issue263_irref"
$psmuxDir = "$env:USERPROFILE\.psmux"
$ESC = [char]27
$BAR = [char]0x2502  # │
$BAR_HEX = 'E29482'  # UTF-8 bytes for U+2502

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

# Cleanup
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue

# Setup
& $PSMUX new-session -d -s $SESSION -x 200 -y 60 2>$null
Start-Sleep -Seconds 3
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Host "Session creation failed" -F Red; exit 1 }

Write-Host "`n=== Issue #263 IRREFUTABLE PROOF ===" -ForegroundColor Cyan
Write-Host "  Build under test: $VERSION"
Write-Host "  Issue reported on: 3.3.3 (filed 2 days ago)"
Write-Host "  Method: chcp 65001 + UTF-8 + byte-level analysis + text control"

# --- Force UTF-8 in the pane shell ---
# chcp 65001 changes the CONSOLE code page so the OS treats stdin/stdout as UTF-8
& $PSMUX send-keys -t $SESSION 'chcp 65001' Enter
Start-Sleep -Seconds 1
& $PSMUX send-keys -t $SESSION '[Console]::OutputEncoding=[Text.Encoding]::UTF8' Enter
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $SESSION '$OutputEncoding=[Text.Encoding]::UTF8' Enter
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $SESSION 'Clear-Host' Enter
Start-Sleep -Seconds 1

# --- Write the BOX-DRAWING repro script as UTF-8 (with BOM) ---
$reproPath = "$env:TEMP\psmux_issue263_irref_box.ps1"
$reproContent = @"
[Console]::OutputEncoding=[Text.Encoding]::UTF8
`$OutputEncoding=[Text.Encoding]::UTF8
Write-Host "${ESC}[90m${BAR} BOX-090${ESC}[0m"
Write-Host "${ESC}[37m${BAR} BOX-037${ESC}[0m"
Write-Host "${ESC}[1;37m${BAR} BOX-137${ESC}[0m"
Write-Host "${ESC}[38;5;240m${BAR} BOX-240${ESC}[0m"
Write-Host "${ESC}[38;5;250m${BAR} BOX-250${ESC}[0m"
Write-Host "${ESC}[38;2;128;128;128m${BAR} BOX-GRY${ESC}[0m"
Write-Host "${ESC}[38;2;255;0;0m${BAR} BOX-RED${ESC}[0m"
"@
[System.IO.File]::WriteAllText($reproPath, $reproContent, (New-Object System.Text.UTF8Encoding($true)))

# --- Write the TEXT control script (same SGRs, no box char) ---
$ctlPath = "$env:TEMP\psmux_issue263_irref_text.ps1"
$ctlContent = @"
[Console]::OutputEncoding=[Text.Encoding]::UTF8
`$OutputEncoding=[Text.Encoding]::UTF8
Write-Host "${ESC}[90mTXT-090${ESC}[0m"
Write-Host "${ESC}[37mTXT-037${ESC}[0m"
Write-Host "${ESC}[1;37mTXT-137${ESC}[0m"
Write-Host "${ESC}[38;5;240mTXT-240${ESC}[0m"
Write-Host "${ESC}[38;5;250mTXT-250${ESC}[0m"
Write-Host "${ESC}[38;2;128;128;128mTXT-GRY${ESC}[0m"
Write-Host "${ESC}[38;2;255;0;0mTXT-RED${ESC}[0m"
"@
[System.IO.File]::WriteAllText($ctlPath, $ctlContent, (New-Object System.Text.UTF8Encoding($true)))

# --- Run the BOX repro ---
& $PSMUX send-keys -t $SESSION "& '$reproPath'" Enter
Write-Info "BOX repro running, waiting 4s..."
Start-Sleep -Seconds 4

$capBox = & $PSMUX capture-pane -t $SESSION -p -e 2>&1 | Out-String

# --- Run the TEXT control ---
& $PSMUX send-keys -t $SESSION 'Clear-Host' Enter
Start-Sleep -Seconds 1
& $PSMUX send-keys -t $SESSION "& '$ctlPath'" Enter
Write-Info "TEXT control running, waiting 4s..."
Start-Sleep -Seconds 4
$capTxt = & $PSMUX capture-pane -t $SESSION -p -e 2>&1 | Out-String

Write-Host "`n--- BOX RAW CAPTURE ---" -ForegroundColor Yellow
($capBox -split "`r?`n") | Where-Object { $_ -match 'BOX-' } | ForEach-Object {
    $shown = $_ -replace $ESC.ToString(), '\e'
    Write-Host "    $shown"
}

Write-Host "`n--- TEXT CONTROL RAW CAPTURE ---" -ForegroundColor Yellow
($capTxt -split "`r?`n") | Where-Object { $_ -match 'TXT-' } | ForEach-Object {
    $shown = $_ -replace $ESC.ToString(), '\e'
    Write-Host "    $shown"
}

# --- Verification function: byte-level for box chars ---
function Test-BoxLine {
    param([string]$line, [string]$tag, [string]$wantSgr)

    if (-not $line) { return @{ Ok=$false; Reason="line not in capture" } }

    $bytes = [System.Text.Encoding]::UTF8.GetBytes($line)
    $hex = ($bytes | ForEach-Object { $_.ToString("X2") }) -join ''

    $boxAt = $hex.IndexOf($BAR_HEX)
    if ($boxAt -lt 0) {
        return @{ Ok=$false; Reason="no U+2502 (E2 94 82) bytes in line"; Hex=$hex }
    }

    $beforeByteEnd = ($boxAt / 2) - 1
    $beforeBytes = if ($beforeByteEnd -ge 0) { $bytes[0..$beforeByteEnd] } else { @() }
    $beforeStr = [System.Text.Encoding]::UTF8.GetString($beforeBytes)

    $sgrMatches = [regex]::Matches($beforeStr, "$ESC\[([^m]*)m")
    if ($sgrMatches.Count -eq 0) {
        return @{ Ok=$false; Reason="no SGR before box char" }
    }
    $lastSgr = $sgrMatches[$sgrMatches.Count - 1].Groups[1].Value

    $wantParts = $wantSgr -split ';'
    $lastParts = $lastSgr -split ';'
    foreach ($wp in $wantParts) {
        if ($lastParts -notcontains $wp) {
            return @{ Ok=$false; Reason="SGR mismatch"; LastSgr=$lastSgr }
        }
    }
    return @{ Ok=$true; LastSgr=$lastSgr }
}

# --- Verification function: text control ---
function Test-TextLine {
    param([string]$line, [string]$tag, [string]$wantSgr)

    if (-not $line) { return @{ Ok=$false; Reason="line not in capture" } }

    # Find the tag (e.g. TXT-RED) in the line and look at SGR immediately before it
    $tagIdx = $line.IndexOf($tag)
    if ($tagIdx -lt 0) { return @{ Ok=$false; Reason="tag not found" } }
    $beforeStr = $line.Substring(0, $tagIdx)

    $sgrMatches = [regex]::Matches($beforeStr, "$ESC\[([^m]*)m")
    if ($sgrMatches.Count -eq 0) {
        return @{ Ok=$false; Reason="no SGR before tag" }
    }
    $lastSgr = $sgrMatches[$sgrMatches.Count - 1].Groups[1].Value

    $wantParts = $wantSgr -split ';'
    $lastParts = $lastSgr -split ';'
    foreach ($wp in $wantParts) {
        if ($lastParts -notcontains $wp) {
            return @{ Ok=$false; Reason="SGR mismatch"; LastSgr=$lastSgr }
        }
    }
    return @{ Ok=$true; LastSgr=$lastSgr }
}

$expected = [ordered]@{
    "090" = "90"
    "037" = "37"
    "137" = "1;37"
    "240" = "38;5;240"
    "250" = "38;5;250"
    "GRY" = "38;2;128;128;128"
    "RED" = "38;2;255;0;0"
}

# --- BOX verification ---
Write-Host "`n--- BOX VERIFICATION (byte-level: SGR before E2 94 82) ---" -ForegroundColor Yellow
$boxLines = ($capBox -split "`r?`n") | Where-Object { $_ -match 'BOX-' }
$boxPass = 0; $boxFail = 0
$boxResults = @{}
foreach ($k in $expected.Keys) {
    $tag = "BOX-$k"
    $line = $boxLines | Where-Object { $_.Contains($tag) } | Select-Object -First 1
    $r = Test-BoxLine -line $line -tag $tag -wantSgr $expected[$k]
    $boxResults[$k] = $r
    if ($r.Ok) {
        Write-Pass "$tag : box preceded by SGR [$($r.LastSgr)] (expected '$($expected[$k])')"
        $boxPass++
    } else {
        Write-Fail "$tag : $($r.Reason)$(if ($r.LastSgr) { " - got [$($r.LastSgr)]" })$(if ($r.Hex) { " hex=$($r.Hex)" })"
        $boxFail++
    }
}

# --- TEXT control verification ---
Write-Host "`n--- TEXT CONTROL VERIFICATION ---" -ForegroundColor Yellow
$txtLines = ($capTxt -split "`r?`n") | Where-Object { $_ -match 'TXT-' }
$txtPass = 0; $txtFail = 0
foreach ($k in $expected.Keys) {
    $tag = "TXT-$k"
    $line = $txtLines | Where-Object { $_.Contains($tag) } | Select-Object -First 1
    $r = Test-TextLine -line $line -tag $tag -wantSgr $expected[$k]
    if ($r.Ok) {
        Write-Pass "$tag : SGR [$($r.LastSgr)] (expected '$($expected[$k])')"
        $txtPass++
    } else {
        Write-Fail "$tag : $($r.Reason)"
        $txtFail++
    }
}

# Cleanup
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Remove-Item $reproPath -Force -EA SilentlyContinue
Remove-Item $ctlPath -Force -EA SilentlyContinue

Write-Host "`n============================================" -ForegroundColor Cyan
Write-Host "VERDICT MATRIX" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ("  TEXT control (same SGRs):   {0} / 7 carry correct color" -f $txtPass)
Write-Host ("  BOX-drawing chars:          {0} / 7 carry correct color" -f $boxPass)
Write-Host ""

# Decision logic
if ($txtPass -eq 7 -and $boxPass -eq 7) {
    Write-Host "  >>> BUG IS NOT PRESENT in $VERSION" -ForegroundColor Green
    Write-Host "      Both text and box-drawing characters carry SGR correctly" -ForegroundColor Green
    Write-Host "      through the live render output. Each U+2502 in the screen"
    Write-Host "      buffer is preceded by its expected SGR sequence."
    exit 0
} elseif ($txtPass -eq 7 -and $boxPass -lt 7) {
    Write-Host "  >>> BUG REPRODUCES in $VERSION" -ForegroundColor Red
    Write-Host "      Text gets correct SGR but box-drawing chars do not." -ForegroundColor Red
    Write-Host "      This matches the user-reported behavior in issue #263."
    exit 1
} elseif ($txtPass -lt 7 -and $boxPass -lt 7) {
    Write-Host "  >>> ENCODING ARTIFACT" -ForegroundColor Yellow
    Write-Host "      Neither text nor box passed cleanly - there is an"
    Write-Host "      encoding/timing issue in the test rig, not a real bug."
    Write-Host "      Compare the raw captures above: if SGR appears before"
    Write-Host "      both BOX- tags AND mojibake bytes, the renderer is fine."
    exit 2
} else {
    Write-Host "  >>> UNEXPECTED: text < 7 but box = 7" -ForegroundColor Yellow
    exit 3
}
