# Issue #263 — byte-level proof.
#
# Strategy: skip clever regex. Reuse same nested-psmux setup, then dump raw
# bytes of each rendered line and match them against expected byte patterns.

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$VERSION = (& $PSMUX -V).Trim()
$OUTER = "issue263_outer"
$INNER = "issue263_inner"
$psmuxDir = "$env:USERPROFILE\.psmux"
$ESC = [char]27
$BAR = [char]0x2502

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

& $PSMUX kill-session -t $OUTER 2>&1 | Out-Null
& $PSMUX kill-session -t $INNER 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$OUTER.*" -Force -EA SilentlyContinue
Remove-Item "$psmuxDir\$INNER.*" -Force -EA SilentlyContinue

& $PSMUX new-session -d -s $OUTER -x 200 -y 60 2>$null
Start-Sleep -Seconds 3

Write-Host "`n=== Issue #263 BYTE-LEVEL PROOF ===" -ForegroundColor Cyan
Write-Host "  Build under test: $VERSION"

$reproPath = "$env:TEMP\psmux_issue263_proof.ps1"
$reproContent = @"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
`$OutputEncoding = [System.Text.Encoding]::UTF8
Write-Host "${ESC}[90m${BAR} SGR-90${ESC}[0m"
Write-Host "${ESC}[37m${BAR} SGR-37${ESC}[0m"
Write-Host "${ESC}[1;37m${BAR} SGR-1-37${ESC}[0m"
Write-Host "${ESC}[38;5;240m${BAR} IDX-240${ESC}[0m"
Write-Host "${ESC}[38;5;250m${BAR} IDX-250${ESC}[0m"
Write-Host "${ESC}[38;2;128;128;128m${BAR} TC-grey${ESC}[0m"
Write-Host "${ESC}[38;2;255;0;0m${BAR} TC-red${ESC}[0m"
"@
[System.IO.File]::WriteAllText($reproPath, $reproContent, (New-Object System.Text.UTF8Encoding($true)))

& $PSMUX send-keys -t $OUTER 'Clear-Host' Enter
Start-Sleep -Seconds 1
& $PSMUX send-keys -t $OUTER "`$env:TERM='xterm-256color'" Enter
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $OUTER "psmux new-session -s $INNER" Enter
Start-Sleep -Seconds 6
& $PSMUX send-keys -t $OUTER "Clear-Host" Enter
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $OUTER "& '$reproPath'" Enter
Start-Sleep -Seconds 4

$capOuter = & $PSMUX capture-pane -t $OUTER -p -e 2>&1 | Out-String

# Map of tag -> expected ANSI parameter substring
$expected = @{
    "SGR-90"   = "90"
    "SGR-37"   = "37"
    "SGR-1-37" = "1;37"
    "IDX-240"  = "38;5;240"
    "IDX-250"  = "38;5;250"
    "TC-grey"  = "38;2;128;128;128"
    "TC-red"   = "38;2;255;0;0"
}

$pass = 0; $fail = 0
$lines = $capOuter -split "`r?`n"

Write-Host "`n--- BYTE-LEVEL VERIFICATION ---" -ForegroundColor Yellow

foreach ($tag in $expected.Keys) {
    $want = $expected[$tag]
    $matchLine = $lines | Where-Object { $_.Contains($tag) } | Select-Object -First 1
    if (-not $matchLine) {
        Write-Fail "$tag : line not present in capture"
        $fail++
        continue
    }

    # Examine bytes of this line
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($matchLine)
    $hex = ($bytes | ForEach-Object { $_.ToString("X2") }) -join ' '

    # Find the box-drawing char's UTF-8 bytes: E2 94 82
    $hexNoSpace = $hex -replace ' ', ''
    $boxIdx = $hexNoSpace.IndexOf("E29482")
    if ($boxIdx -lt 0) {
        Write-Fail "$tag : line has no E2 94 82 (U+2502) bytes. Hex: $hex"
        $fail++
        continue
    }

    # Get the bytes BEFORE the box-drawing char
    $byteIndex = $boxIdx / 2
    $beforeBytes = $bytes[0..($byteIndex - 1)]
    $beforeStr = [System.Text.Encoding]::UTF8.GetString($beforeBytes)

    # Find the LAST ESC[..m sequence in the bytes before the box char
    $lastEscRx = [regex]::new("$ESC\[([^m]*)m(?!.*$ESC\[)", 'Singleline')
    $lastSgrAll = [regex]::Matches($beforeStr, "$ESC\[([^m]*)m")
    if ($lastSgrAll.Count -eq 0) {
        Write-Fail "$tag : no SGR sequence before box char"
        $fail++
        continue
    }
    $lastSgr = $lastSgrAll[$lastSgrAll.Count - 1].Groups[1].Value

    # Check all expected components are in the last SGR
    $wantParts = $want -split ';'
    $lastParts = $lastSgr -split ';'
    $allFound = $true
    foreach ($wp in $wantParts) {
        if ($lastParts -notcontains $wp) { $allFound = $false; break }
    }
    if ($allFound) {
        Write-Pass "$tag : last SGR before box = [$lastSgr] (contains expected '$want')"
        $pass++
    } else {
        Write-Fail "$tag : expected '$want' before box, got [$lastSgr]"
        $fail++
    }
}

# Cleanup
& $PSMUX send-keys -t $OUTER "psmux kill-server" Enter
Start-Sleep -Seconds 2
& $PSMUX kill-session -t $OUTER 2>&1 | Out-Null
Remove-Item $reproPath -Force -EA SilentlyContinue

Write-Host "`n=== VERDICT ===" -ForegroundColor Cyan
Write-Host "  Pass (color preserved): $pass / $($expected.Count)" -ForegroundColor Green
Write-Host "  Fail (color dropped):   $fail / $($expected.Count)" -ForegroundColor $(if ($fail -gt 0) { "Red" } else { "Green" })

if ($fail -eq 0 -and $pass -eq $expected.Count) {
    Write-Host "`n  >>> BUG NOT PRESENT in $VERSION" -ForegroundColor Green
    Write-Host "  Live render output preserves SGR on every box-drawing character."
} else {
    Write-Host "`n  >>> BUG PRESENT" -ForegroundColor Red
}

exit $fail
