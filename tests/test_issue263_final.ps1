# Issue #263 - DEFINITIVE byte-level proof.
#
# Approach: single attached psmux session. Write UTF-8 repro script to disk.
# Run it. Capture pane WITH escape codes (-e). Then for each test line,
# inspect the UTF-8 byte stream and verify:
#   1. The U+2502 box char (E2 94 82) IS in the output for that line
#   2. The SGR sequence that immediately precedes it contains the
#      expected color components
#
# capture-pane -e re-emits the screen buffer state with full SGR per attribute
# group, exercising the same color-mapping code that the live renderer uses.

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$VERSION = (& $PSMUX -V).Trim()
$SESSION = "issue263_final"
$psmuxDir = "$env:USERPROFILE\.psmux"
$ESC = [char]27
$BAR = [char]0x2502

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

Write-Host "`n=== Issue #263 DEFINITIVE PROOF ===" -ForegroundColor Cyan
Write-Host "  Build: $VERSION"
Write-Host "  Issue env: psmux 3.3.3 (filed 6 days before 3.3.4)"

# Write repro script to disk as UTF-8 with BOM
$reproPath = "$env:TEMP\psmux_issue263_final.ps1"
$reproContent = @"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
`$OutputEncoding = [System.Text.Encoding]::UTF8
Write-Host "${ESC}[90m${BAR} TAG-090${ESC}[0m"
Write-Host "${ESC}[37m${BAR} TAG-037${ESC}[0m"
Write-Host "${ESC}[1;37m${BAR} TAG-137${ESC}[0m"
Write-Host "${ESC}[38;5;240m${BAR} TAG-240${ESC}[0m"
Write-Host "${ESC}[38;5;250m${BAR} TAG-250${ESC}[0m"
Write-Host "${ESC}[38;2;128;128;128m${BAR} TAG-GRY${ESC}[0m"
Write-Host "${ESC}[38;2;255;0;0m${BAR} TAG-RED${ESC}[0m"
"@
[System.IO.File]::WriteAllText($reproPath, $reproContent, (New-Object System.Text.UTF8Encoding($true)))

# Make sure shell is UTF-8 + run repro
& $PSMUX send-keys -t $SESSION 'Clear-Host' Enter
Start-Sleep -Seconds 1
& $PSMUX send-keys -t $SESSION '[Console]::OutputEncoding=[Text.Encoding]::UTF8' Enter
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $SESSION "& '$reproPath'" Enter
Start-Sleep -Seconds 3

# Capture WITH ANSI codes
$cap = & $PSMUX capture-pane -t $SESSION -p -e 2>&1 | Out-String

# Map TAG to expected SGR components
$expected = [ordered]@{
    "TAG-090" = "90"
    "TAG-037" = "37"
    "TAG-137" = "1;37"
    "TAG-240" = "38;5;240"
    "TAG-250" = "38;5;250"
    "TAG-GRY" = "38;2;128;128;128"
    "TAG-RED" = "38;2;255;0;0"
}

Write-Host "`n--- RAW LIVE CAPTURE (escapes shown as \e) ---" -ForegroundColor Yellow
$lines = $cap -split "`r?`n"
$relevant = $lines | Where-Object { $_ -match "TAG-" }
foreach ($l in $relevant) {
    $shown = $l -replace $ESC.ToString(), '\e'
    Write-Host "    $shown"
}

Write-Host "`n--- BYTE-LEVEL VERIFICATION ---" -ForegroundColor Yellow

$pass = 0; $fail = 0
foreach ($tag in $expected.Keys) {
    $want = $expected[$tag]
    $line = $relevant | Where-Object { $_.Contains($tag) } | Select-Object -First 1
    if (-not $line) {
        Write-Fail "$tag : line not in capture"
        $fail++
        continue
    }

    # Get UTF-8 bytes of line
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($line)
    $hex = ($bytes | ForEach-Object { $_.ToString("X2") }) -join ''

    # Find U+2502 (E2 94 82). If absent, line has mojibake or no box char.
    $boxAt = $hex.IndexOf("E29482")
    if ($boxAt -lt 0) {
        Write-Fail "$tag : NO U+2502 (E2 94 82) bytes in line"
        Write-Host "      Hex: $hex" -ForegroundColor DarkGray
        $fail++
        continue
    }

    # Bytes before the box char
    $beforeByteEnd = ($boxAt / 2) - 1
    $beforeBytes = $bytes[0..$beforeByteEnd]
    $beforeStr = [System.Text.Encoding]::UTF8.GetString($beforeBytes)

    # Find LAST SGR sequence before box
    $sgrMatches = [regex]::Matches($beforeStr, "$ESC\[([^m]*)m")
    if ($sgrMatches.Count -eq 0) {
        Write-Fail "$tag : no SGR sequence before box char"
        $fail++
        continue
    }
    $lastSgr = $sgrMatches[$sgrMatches.Count - 1].Groups[1].Value

    # Verify all expected components are in the last SGR
    $wantParts = $want -split ';'
    $lastParts = $lastSgr -split ';'
    $allFound = $true
    foreach ($wp in $wantParts) {
        if ($lastParts -notcontains $wp) { $allFound = $false; break }
    }
    if ($allFound) {
        Write-Pass "$tag : box char preceded by SGR [$lastSgr] containing expected '$want'"
        $pass++
    } else {
        Write-Fail "$tag : expected '$want' before box, got [$lastSgr]"
        $fail++
    }
}

# Cleanup
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Remove-Item $reproPath -Force -EA SilentlyContinue

Write-Host "`n=== VERDICT ===" -ForegroundColor Cyan
Write-Host "  Pass: $pass / $($expected.Count)"
Write-Host "  Fail: $fail / $($expected.Count)"

if ($fail -eq 0) {
    Write-Host "`n  >>> BUG NOT PRESENT in $VERSION" -ForegroundColor Green
    Write-Host "  Each U+2502 box-drawing char in the screen buffer is preceded" -ForegroundColor Green
    Write-Host "  by its requested SGR. The per-cell color attribute is preserved" -ForegroundColor Green
    Write-Host "  for box-drawing characters identically to plain text." -ForegroundColor Green
} else {
    Write-Host "`n  >>> BUG REPRODUCES" -ForegroundColor Red
}
exit $fail
