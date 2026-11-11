# Issue #263 — final irrefutable proof.
#
# Method: nested psmux. Inner psmux is fully attached to the outer pane, so
# client::render_layout_json writes ANSI sequences to the outer pane's PTY.
# Capture-pane on the OUTER pane shows exactly what the inner renderer wrote.
# If the SGR color is preserved before every box-drawing char, the bug is NOT
# present in this build.
#
# Verification fix: use [regex] match instead of String.IndexOf(char) to
# avoid PowerShell's char/string overload quirk.

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$VERSION = (& $PSMUX -V).Trim()
$OUTER = "issue263_outer"
$INNER = "issue263_inner"
$psmuxDir = "$env:USERPROFILE\.psmux"
$ESC = [char]27
$BAR = [char]0x2502  # │
$BAR_HEX = '2502'

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
& $PSMUX has-session -t $OUTER 2>$null
if ($LASTEXITCODE -ne 0) { Write-Host "Outer session failed" -F Red; exit 1 }

Write-Host "`n=== Issue #263 IRREFUTABLE PROOF ===" -ForegroundColor Cyan
Write-Host "  Build under test: $VERSION" -ForegroundColor White
Write-Host "  Issue reported on: 3.3.3"
Write-Host "  Test method: nested psmux, capture inner's live render output"

$reproPath = "$env:TEMP\psmux_issue263_proof.ps1"
$bar = $BAR
$reproContent = @"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
`$OutputEncoding = [System.Text.Encoding]::UTF8
Write-Host "${ESC}[90m${bar} SGR-90${ESC}[0m"
Write-Host "${ESC}[37m${bar} SGR-37${ESC}[0m"
Write-Host "${ESC}[1;37m${bar} SGR-1-37${ESC}[0m"
Write-Host "${ESC}[38;5;240m${bar} IDX-240${ESC}[0m"
Write-Host "${ESC}[38;5;250m${bar} IDX-250${ESC}[0m"
Write-Host "${ESC}[38;2;128;128;128m${bar} TC-grey${ESC}[0m"
Write-Host "${ESC}[38;2;255;0;0m${bar} TC-red${ESC}[0m"
"@
[System.IO.File]::WriteAllText($reproPath, $reproContent, (New-Object System.Text.UTF8Encoding($true)))

& $PSMUX send-keys -t $OUTER 'Clear-Host' Enter
Start-Sleep -Seconds 1
& $PSMUX send-keys -t $OUTER "`$env:TERM='xterm-256color'" Enter
Start-Sleep -Milliseconds 500

# Start inner attached
& $PSMUX send-keys -t $OUTER "psmux new-session -s $INNER" Enter
Write-Info "Inner psmux attaching... waiting 6s"
Start-Sleep -Seconds 6

& $PSMUX send-keys -t $OUTER "Clear-Host" Enter
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $OUTER "& '$reproPath'" Enter
Write-Info "Repro running through inner's renderer... waiting 4s"
Start-Sleep -Seconds 4

# CAPTURE: outer pane shows what inner's LIVE RENDERER wrote
$capOuter = & $PSMUX capture-pane -t $OUTER -p -e 2>&1 | Out-String

Write-Host "`n--- LIVE RENDER OUTPUT (raw, with ANSI) ---" -ForegroundColor Yellow
$relevant = ($capOuter -split "`n") | Where-Object { $_ -match '(SGR-|IDX-|TC-)' }
foreach ($line in $relevant) {
    $shown = $line -replace $ESC.ToString(), '\e'
    Write-Host "    $shown"
}

# --- Verification: regex find SGR immediately preceding the box char ---
# [regex] handles UTF-16 correctly even if PowerShell IndexOf misbehaves
$rx = [regex]::new("$ESC\[([^m]*)m│\s+(SGR-90|SGR-37|SGR-1-37|IDX-240|IDX-250|TC-grey|TC-red)\b")
$matches = $rx.Matches($capOuter)

Write-Host "`n--- ANALYSIS: SGR preceding each box-drawing char ---" -ForegroundColor Yellow
$expected = @{
    "SGR-90"   = "90";
    "SGR-37"   = "37";
    "SGR-1-37" = "1;37";
    "IDX-240"  = "38;5;240";
    "IDX-250"  = "38;5;250";
    "TC-grey"  = "38;2;128;128;128";
    "TC-red"   = "38;2;255;0;0"
}

$pass = 0; $fail = 0
foreach ($m in $matches) {
    $sgr = $m.Groups[1].Value
    $tag = $m.Groups[2].Value
    $want = $expected[$tag]

    $sgrParts = $sgr -split ';'
    $wantParts = $want -split ';'
    $allFound = $true
    foreach ($wp in $wantParts) {
        if ($sgrParts -notcontains $wp) { $allFound = $false; break }
    }
    if ($allFound) {
        Write-Pass "$tag : box preceded by SGR [$sgr] (contains expected '$want')"
        $pass++
    } else {
        Write-Fail "$tag : expected '$want', got '[$sgr]'"
        $fail++
    }
}

# Also report which tags we DID find
$foundTags = ($matches | ForEach-Object { $_.Groups[2].Value }) | Sort-Object -Unique
$missingTags = $expected.Keys | Where-Object { $_ -notin $foundTags }
foreach ($mt in $missingTags) {
    Write-Fail "$mt : NO match found (box+text combo missing in render output)"
    $fail++
}

# Cleanup
& $PSMUX send-keys -t $OUTER "psmux kill-server" Enter
Start-Sleep -Seconds 2
& $PSMUX kill-session -t $OUTER 2>&1 | Out-Null
Remove-Item $reproPath -Force -EA SilentlyContinue

Write-Host "`n=== VERDICT ===" -ForegroundColor Cyan
Write-Host "  Live-render keeps color (PASS): $pass" -ForegroundColor Green
Write-Host "  Live-render drops color (FAIL): $fail" -ForegroundColor Red

if ($fail -eq 0 -and $pass -eq $expected.Count) {
    Write-Host "`n  >>> BUG NOT PRESENT in $VERSION" -ForegroundColor Green
    Write-Host "  Every box-drawing char is preceded by its expected SGR in the live render output."
    Write-Host "  The vt100 buffer AND the renderer both preserve per-cell color attributes."
} elseif ($fail -gt 0) {
    Write-Host "`n  >>> BUG CONFIRMED in $VERSION" -ForegroundColor Red
}

exit $fail
