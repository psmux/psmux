# Issue #263 — Python raw-byte proof.
#
# PowerShell's [Console]::OpenStandardOutput() is intercepted by Windows
# conhost, which transforms UTF-8 bytes (E2 94 82) into 3 separate CP437
# code points (Γöé) regardless of chcp 65001. This means we have NEVER
# actually been testing what happens when a real U+2502 reaches psmux's
# cell buffer.
#
# Python's sys.stdout.buffer is a direct binary handle that calls WriteFile
# without conhost transformation. Use it to inject the exact byte sequence
# the issue is about: ESC [ <SGR> m E2 94 82 <space> <tag> ESC [ 0 m \n
#
# Verification: capture-pane -p -e re-emits the cell buffer's stored chars
# as UTF-8 + SGR. If psmux's parser stored U+2502 correctly with cell.fg,
# we will see E2 94 82 bytes preceded by the expected SGR. If psmux had a
# special-case stripping color from box-drawing chars, we would see E2 94 82
# preceded by a different (or default) SGR, while the surrounding text
# carries the requested color.

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$VERSION = (& $PSMUX -V).Trim()
$PY = (Get-Command python -EA Stop).Source
$SESSION = "issue263_py"
$psmuxDir = "$env:USERPROFILE\.psmux"

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

# --- Build raw bytes file with EXACT byte sequence ---
function Build-Line {
    param([string]$Sgr, [string]$Tag)
    $esc = [byte]0x1B; $lbrk = [byte]0x5B; $m = [byte]0x6D
    $box = [byte[]](0xE2, 0x94, 0x82)
    $sp = [byte]0x20; $crlf = [byte[]](0x0D, 0x0A)
    $reset = [byte[]]($esc, $lbrk, 0x30, $m)
    $sgrBytes = [System.Text.Encoding]::ASCII.GetBytes($Sgr)
    $tagBytes = [System.Text.Encoding]::ASCII.GetBytes($Tag)
    $list = New-Object System.Collections.Generic.List[byte]
    $list.Add($esc); $list.Add($lbrk)
    foreach ($b in $sgrBytes) { $list.Add($b) }
    $list.Add($m)
    foreach ($b in $box) { $list.Add($b) }
    $list.Add($sp)
    foreach ($b in $tagBytes) { $list.Add($b) }
    foreach ($b in $reset) { $list.Add($b) }
    foreach ($b in $crlf) { $list.Add($b) }
    return $list.ToArray()
}

$expected = [ordered]@{
    "BOX-090" = "90"
    "BOX-037" = "37"
    "BOX-137" = "1;37"
    "BOX-240" = "38;5;240"
    "BOX-250" = "38;5;250"
    "BOX-GRY" = "38;2;128;128;128"
    "BOX-RED" = "38;2;255;0;0"
}

$binPath = "$env:TEMP\psmux_issue263_py.bin"
$allBytes = New-Object System.Collections.Generic.List[byte]
foreach ($k in $expected.Keys) {
    $line = Build-Line -Sgr $expected[$k] -Tag $k
    foreach ($b in $line) { $allBytes.Add($b) }
}
[System.IO.File]::WriteAllBytes($binPath, $allBytes.ToArray())

$verify = [System.IO.File]::ReadAllBytes($binPath)
$verifyHex = ($verify | ForEach-Object { $_.ToString("X2") }) -join ''
$boxOccurrences = ([regex]::Matches($verifyHex, "E29482")).Count
Write-Info "Bin file: $($verify.Length) bytes, U+2502 count = $boxOccurrences"
if ($boxOccurrences -ne 7) { Write-Host "Bin file generation failed" -F Red; exit 1 }

# Python emit script
$pyEmit = "$env:TEMP\psmux_issue263_emit.py"
$pyContent = @"
import sys
with open(r'$binPath', 'rb') as f:
    data = f.read()
sys.stdout.buffer.write(data)
sys.stdout.buffer.flush()
"@
[System.IO.File]::WriteAllText($pyEmit, $pyContent, (New-Object System.Text.UTF8Encoding($false)))

# --- Cleanup ---
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue

# --- Spawn pane ---
& $PSMUX new-session -d -s $SESSION -x 200 -y 60 2>$null
Start-Sleep -Seconds 3
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Host "Session creation failed" -F Red; exit 1 }

Write-Host "`n=== Issue #263 PYTHON RAW-BYTE PROOF ===" -ForegroundColor Cyan
Write-Host "  Build under test: $VERSION"
Write-Host "  Method: python sys.stdout.buffer.write() bypasses conhost"
Write-Host "  Byte path: file -> Python -> PTY -> psmux parser -> cell buffer"
Write-Host ""

# --- Force UTF-8 console (for safety, though python.buffer doesn't use it) ---
& $PSMUX send-keys -t $SESSION 'chcp 65001 | Out-Null' Enter
Start-Sleep -Milliseconds 800
& $PSMUX send-keys -t $SESSION 'Clear-Host' Enter
Start-Sleep -Seconds 1

# Run python with the emit script. -B suppresses .pyc.
& $PSMUX send-keys -t $SESSION "& '$PY' -B '$pyEmit'" Enter
Write-Info "Python emitting raw bytes to inner pane PTY, waiting 4s..."
Start-Sleep -Seconds 4

# --- Capture rendered output (cell buffer re-emitted with SGR) ---
$cap = & $PSMUX capture-pane -t $SESSION -p -e 2>&1 | Out-String

Write-Host "`n--- LIVE RENDER CAPTURE (escapes shown as \e) ---" -ForegroundColor Yellow
$ESC = [char]27
$lines = $cap -split "`r?`n"
$relevant = $lines | Where-Object { $_ -match 'BOX-' }
foreach ($l in $relevant) {
    $shown = $l -replace $ESC.ToString(), '\e'
    Write-Host "    $shown"
}

# --- Byte-level verification ---
Write-Host "`n--- BYTE-LEVEL VERIFICATION (E2 94 82 + preceding SGR) ---" -ForegroundColor Yellow

$pass = 0; $fail = 0; $missingBoxBytes = 0
foreach ($k in $expected.Keys) {
    $line = $relevant | Where-Object { $_.Contains($k) } | Select-Object -First 1
    if (-not $line) { Write-Fail "$k : line missing"; $fail++; continue }

    $bytes = [System.Text.Encoding]::UTF8.GetBytes($line)
    $hex = ($bytes | ForEach-Object { $_.ToString("X2") }) -join ''
    $boxAt = $hex.IndexOf("E29482")

    if ($boxAt -lt 0) {
        Write-Fail "$k : NO U+2502 (E2 94 82) bytes -- still mojibake"
        Write-Host "      hex=$hex" -ForegroundColor DarkGray
        $missingBoxBytes++
        $fail++
        continue
    }

    $beforeEnd = ($boxAt / 2) - 1
    $beforeBytes = $bytes[0..$beforeEnd]
    $beforeStr = [System.Text.Encoding]::UTF8.GetString($beforeBytes)
    $sgrRx = [regex]::Matches($beforeStr, "$ESC\[([^m]*)m")
    if ($sgrRx.Count -eq 0) {
        Write-Fail "$k : no SGR before E2 94 82"
        $fail++
        continue
    }
    $lastSgr = $sgrRx[$sgrRx.Count - 1].Groups[1].Value
    $wantParts = $expected[$k] -split ';'
    $lastParts = $lastSgr -split ';'
    $allFound = $true
    foreach ($wp in $wantParts) {
        if ($lastParts -notcontains $wp) { $allFound = $false; break }
    }
    if ($allFound) {
        Write-Pass "$k : E2 94 82 preceded by SGR [$lastSgr] (expected '$($expected[$k])')"
        $pass++
    } else {
        Write-Fail "$k : expected '$($expected[$k])' before E2 94 82, got [$lastSgr]"
        $fail++
    }
}

# --- Cleanup ---
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Remove-Item $binPath -Force -EA SilentlyContinue
Remove-Item $pyEmit -Force -EA SilentlyContinue

Write-Host "`n============================================" -ForegroundColor Cyan
Write-Host "VERDICT" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ("  U+2502 cells with correct preceding SGR: {0} / 7" -f $pass)
Write-Host ("  Failed:                                  {0} / 7" -f $fail)
Write-Host ("  (lines missing E2 94 82:                 {0})" -f $missingBoxBytes)
Write-Host ""

if ($pass -eq 7) {
    Write-Host "  >>> BUG IS NOT PRESENT in $VERSION" -ForegroundColor Green
    Write-Host "      Real U+2502 (E2 94 82) bytes reached psmux's cell buffer."
    Write-Host "      Each U+2502 cell carries its requested SGR through the"
    Write-Host "      live render output. All 7 SGR variants verify clean."
    exit 0
}
elseif ($missingBoxBytes -eq 7) {
    Write-Host "  >>> ENCODING STILL DEFEATING US" -ForegroundColor Yellow
    Write-Host "      Even Python's sys.stdout.buffer was transformed."
    Write-Host "      Need a deeper injection (Rust integration test against parser)."
    exit 2
}
elseif ($pass -gt 0 -and $missingBoxBytes -eq 0) {
    Write-Host "  >>> PARTIAL FAILURE" -ForegroundColor Red
    Write-Host "      Some U+2502 cells lost their SGR. This matches the bug."
    exit 1
}
else {
    Write-Host "  >>> MIXED RESULTS" -ForegroundColor Yellow
    exit 3
}
