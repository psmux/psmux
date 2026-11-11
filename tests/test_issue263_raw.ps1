# Issue #263 - skip the encoding maze. Write capture-pane output as raw bytes
# to a file, then examine the file bytes directly.

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$VERSION = (& $PSMUX -V).Trim()
$SESSION = "issue263_raw"
$psmuxDir = "$env:USERPROFILE\.psmux"
$ESC = [char]27
$BAR = [char]0x2502

& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue

& $PSMUX new-session -d -s $SESSION -x 200 -y 60 2>$null
Start-Sleep -Seconds 3

Write-Host "`n=== Issue #263 RAW BYTE PROOF ===" -ForegroundColor Cyan
Write-Host "  Build: $VERSION"

$reproPath = "$env:TEMP\psmux_issue263_raw.ps1"
$reproContent = @"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
`$OutputEncoding = [System.Text.Encoding]::UTF8
chcp 65001 | Out-Null
Write-Host "${ESC}[90m${BAR} TAG-090${ESC}[0m"
Write-Host "${ESC}[37m${BAR} TAG-037${ESC}[0m"
Write-Host "${ESC}[1;37m${BAR} TAG-137${ESC}[0m"
Write-Host "${ESC}[38;5;240m${BAR} TAG-240${ESC}[0m"
Write-Host "${ESC}[38;5;250m${BAR} TAG-250${ESC}[0m"
Write-Host "${ESC}[38;2;128;128;128m${BAR} TAG-GRY${ESC}[0m"
Write-Host "${ESC}[38;2;255;0;0m${BAR} TAG-RED${ESC}[0m"
"@
[System.IO.File]::WriteAllText($reproPath, $reproContent, (New-Object System.Text.UTF8Encoding($true)))

& $PSMUX send-keys -t $SESSION 'Clear-Host' Enter
Start-Sleep -Seconds 1
& $PSMUX send-keys -t $SESSION 'chcp 65001' Enter
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $SESSION '[Console]::OutputEncoding=[Text.Encoding]::UTF8' Enter
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $SESSION "& '$reproPath'" Enter
Start-Sleep -Seconds 3

# Capture and write to file with NO encoding conversion
$capFile = "$env:TEMP\psmux_issue263_capture.bin"
# Use cmd to redirect raw bytes (bypasses PowerShell encoding)
& cmd.exe /c "psmux capture-pane -t $SESSION -p -e > `"$capFile`""
Start-Sleep -Milliseconds 500

if (-not (Test-Path $capFile)) {
    Write-Host "Capture file not created" -F Red
    exit 1
}

$bytes = [System.IO.File]::ReadAllBytes($capFile)
Write-Host "Capture file size: $($bytes.Length) bytes"

# Print raw hex of the file (filtered to relevant lines)
Write-Host "`n--- FULL HEX DUMP (first 1500 bytes) ---" -ForegroundColor Yellow
$len = [Math]::Min(1500, $bytes.Length)
for ($i = 0; $i -lt $len; $i += 32) {
    $end = [Math]::Min($i + 31, $len - 1)
    $hexLine = ($bytes[$i..$end] | ForEach-Object { $_.ToString("X2") }) -join ' '
    $asciiLine = ($bytes[$i..$end] | ForEach-Object {
        if ($_ -ge 32 -and $_ -lt 127) { [char]$_ } else { '.' }
    }) -join ''
    Write-Host ("{0:X4}: {1,-95} {2}" -f $i, $hexLine, $asciiLine)
}

# Search for U+2502 (E2 94 82) bytes anywhere in capture
$found = $false
$boxOffsets = @()
for ($i = 0; $i -lt $bytes.Length - 2; $i++) {
    if ($bytes[$i] -eq 0xE2 -and $bytes[$i+1] -eq 0x94 -and $bytes[$i+2] -eq 0x82) {
        $boxOffsets += $i
        $found = $true
    }
}

Write-Host "`n--- BOX CHAR (U+2502) OCCURRENCES ---" -ForegroundColor Yellow
Write-Host "Found $($boxOffsets.Count) U+2502 (E2 94 82) byte sequences in capture"

# Also search for the Greek-Gamma mojibake encoding
$mojiOffsets = @()
for ($i = 0; $i -lt $bytes.Length - 5; $i++) {
    if ($bytes[$i] -eq 0xCE -and $bytes[$i+1] -eq 0x93 -and
        $bytes[$i+2] -eq 0xC3 -and $bytes[$i+3] -eq 0xB6 -and
        $bytes[$i+4] -eq 0xC3 -and $bytes[$i+5] -eq 0xA9) {
        $mojiOffsets += $i
    }
}
Write-Host "Found $($mojiOffsets.Count) Γöé (CE 93 C3 B6 C3 A9) mojibake sequences in capture"

# Also search for raw E2 94 82 split across lines (just E2 94 82 anywhere)
Write-Host "`n--- INTERPRETATION ---" -ForegroundColor Yellow
if ($boxOffsets.Count -ge 7) {
    Write-Host "  >>> The screen buffer contains U+2502 box-drawing chars correctly." -ForegroundColor Green
    Write-Host "  Now check the SGR bytes preceding each one." -ForegroundColor Green
    foreach ($off in $boxOffsets) {
        # Look backwards for ESC [
        $start = [Math]::Max(0, $off - 40)
        $segment = $bytes[$start..($off - 1)]
        $hex = ($segment | ForEach-Object { $_.ToString("X2") }) -join ''
        # Extract last ESC [...m
        $escBytes = "1B5B"  # ESC [
        $lastEscPos = $hex.LastIndexOf($escBytes)
        if ($lastEscPos -ge 0) {
            $sgrEndIdx = $hex.IndexOf("6D", $lastEscPos)  # find 'm' (0x6D)
            if ($sgrEndIdx -gt 0) {
                $paramHex = $hex.Substring($lastEscPos + 4, $sgrEndIdx - $lastEscPos - 4)
                # Decode hex string back to ASCII (params are ASCII digits/semicolons)
                $paramStr = ""
                for ($k = 0; $k -lt $paramHex.Length; $k += 2) {
                    $b = [Convert]::ToInt32($paramHex.Substring($k, 2), 16)
                    $paramStr += [char]$b
                }
                Write-Host ("  At offset {0}: SGR before box = [{1}]" -f $off, $paramStr) -ForegroundColor Cyan
            }
        }
    }
} elseif ($mojiOffsets.Count -ge 7) {
    Write-Host "  >>> Box chars are present as Γöé MOJIBAKE in capture (encoding bug)" -ForegroundColor Yellow
    Write-Host "  Each Γöé is preceded by an SGR sequence — check colors:"
    foreach ($off in $mojiOffsets) {
        $start = [Math]::Max(0, $off - 40)
        $segment = $bytes[$start..($off - 1)]
        $hex = ($segment | ForEach-Object { $_.ToString("X2") }) -join ''
        $escBytes = "1B5B"
        $lastEscPos = $hex.LastIndexOf($escBytes)
        if ($lastEscPos -ge 0) {
            $sgrEndIdx = $hex.IndexOf("6D", $lastEscPos)
            if ($sgrEndIdx -gt 0) {
                $paramHex = $hex.Substring($lastEscPos + 4, $sgrEndIdx - $lastEscPos - 4)
                $paramStr = ""
                for ($k = 0; $k -lt $paramHex.Length; $k += 2) {
                    $b = [Convert]::ToInt32($paramHex.Substring($k, 2), 16)
                    $paramStr += [char]$b
                }
                Write-Host ("  At offset {0}: SGR before mojibake = [{1}]" -f $off, $paramStr) -ForegroundColor Cyan
            }
        }
    }
} else {
    Write-Host "  >>> Could not find $($boxOffsets.Count) box chars or $($mojiOffsets.Count) mojibake sequences" -ForegroundColor Red
}

# Cleanup
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Remove-Item $reproPath, $capFile -Force -EA SilentlyContinue
