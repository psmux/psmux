# Issue #263 — RAW BYTE proof.
#
# Earlier attempts hit PowerShell's CP437 mojibake on stdin: when the inner
# pane's shell read `│` (UTF-8 E2 94 82), it stored 3 CP437 chars Γöé
# (CE 93 C3 B6 C3 A9 in UTF-8). The bug claim is about U+2502 specifically,
# so we MUST get exact byte E2 94 82 into psmux's parser.
#
# Strategy: write a binary file containing the EXACT bytes we want in the
# inner pane's PTY. Then have the inner shell pipe those raw bytes to its
# stdout via [Console]::OpenStandardOutput().Write(), which bypasses
# OutputEncoding entirely. chcp 65001 ensures the console host doesn't
# transform UTF-8 bytes on the way to psmux.
#
# Verification: capture-pane -p -e returns the live render output. Check
# that each line contains the literal byte sequence E2 94 82 AND that the
# SGR sequence immediately preceding it contains the expected color
# components.

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$VERSION = (& $PSMUX -V).Trim()
$SESSION = "issue263_raw"
$psmuxDir = "$env:USERPROFILE\.psmux"

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

# --- Build raw bytes file ---
# Each line: ESC [ <sgr> m <BOX> <space> <TAG> ESC [ 0 m <CRLF>
function Build-Line {
    param([string]$Sgr, [string]$Tag)
    $esc = [byte]0x1B
    $lbrk = [byte]0x5B
    $m = [byte]0x6D
    $box = [byte[]](0xE2, 0x94, 0x82)  # U+2502
    $sp = [byte]0x20
    $reset = [byte[]]($esc, $lbrk, 0x30, $m)
    $crlf = [byte[]](0x0D, 0x0A)
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

$binPath = "$env:TEMP\psmux_issue263_raw.bin"
$allBytes = New-Object System.Collections.Generic.List[byte]
foreach ($k in $expected.Keys) {
    $line = Build-Line -Sgr $expected[$k] -Tag $k
    foreach ($b in $line) { $allBytes.Add($b) }
}
[System.IO.File]::WriteAllBytes($binPath, $allBytes.ToArray())

# Verify file has the expected box bytes
$verify = [System.IO.File]::ReadAllBytes($binPath)
$verifyHex = ($verify | ForEach-Object { $_.ToString("X2") }) -join ''
$boxOccurrences = ([regex]::Matches($verifyHex, "E29482")).Count
Write-Info "Generated $($verify.Length) bytes; contains U+2502 occurrences: $boxOccurrences (expected 7)"
if ($boxOccurrences -ne 7) { Write-Host "Bin file generation failed" -F Red; exit 1 }

# --- Cleanup any prior state ---
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue

# --- Spawn detached session ---
& $PSMUX new-session -d -s $SESSION -x 200 -y 60 2>$null
Start-Sleep -Seconds 3
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) { Write-Host "Session creation failed" -F Red; exit 1 }

Write-Host "`n=== Issue #263 RAW-BYTE PROOF ===" -ForegroundColor Cyan
Write-Host "  Build under test: $VERSION"
Write-Host "  Method: write raw E2 94 82 bytes via [Console]::OpenStandardOutput()"
Write-Host "  Goal: prove the renderer's SGR-before-box-char output for U+2502"
Write-Host ""

# --- Drive the pane: chcp 65001, pipe binary file's raw bytes to stdout ---
& $PSMUX send-keys -t $SESSION 'chcp 65001 | Out-Null' Enter
Start-Sleep -Milliseconds 800
& $PSMUX send-keys -t $SESSION '[Console]::OutputEncoding=[Text.Encoding]::UTF8' Enter
Start-Sleep -Milliseconds 500
& $PSMUX send-keys -t $SESSION 'Clear-Host' Enter
Start-Sleep -Seconds 1

# Write a small driver script that pipes raw bytes to stdout via [Console]
$driverPath = "$env:TEMP\psmux_issue263_drv.ps1"
$driverContent = @"
`$b=[IO.File]::ReadAllBytes('$binPath')
`$o=[Console]::OpenStandardOutput()
`$o.Write(`$b,0,`$b.Length)
`$o.Flush()
"@
[System.IO.File]::WriteAllText($driverPath, $driverContent, (New-Object System.Text.UTF8Encoding($true)))

& $PSMUX send-keys -t $SESSION "& '$driverPath'" Enter
Write-Info "Driver script invoked, waiting 4s..."
Start-Sleep -Seconds 4

# --- Capture with -e (re-emit cell buffer with full SGR) ---
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
Write-Host "`n--- BYTE-LEVEL VERIFICATION (looking for E2 94 82 in render output) ---" -ForegroundColor Yellow

$pass = 0; $fail = 0; $missingBoxBytes = 0
foreach ($k in $expected.Keys) {
    $line = $relevant | Where-Object { $_.Contains($k) } | Select-Object -First 1
    if (-not $line) {
        Write-Fail "$k : line missing from capture"
        $fail++
        continue
    }
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($line)
    $hex = ($bytes | ForEach-Object { $_.ToString("X2") }) -join ''
    $boxAt = $hex.IndexOf("E29482")

    if ($boxAt -lt 0) {
        Write-Fail "$k : NO U+2502 (E2 94 82) in rendered bytes"
        Write-Host "      hex=$hex" -ForegroundColor DarkGray
        $missingBoxBytes++
        $fail++
        continue
    }

    # Find the LAST SGR ESC[..m sequence preceding the box bytes
    $beforeEnd = ($boxAt / 2) - 1
    $beforeBytes = $bytes[0..$beforeEnd]
    $beforeStr = [System.Text.Encoding]::UTF8.GetString($beforeBytes)
    $sgrRx = [regex]::Matches($beforeStr, "$ESC\[([^m]*)m")
    if ($sgrRx.Count -eq 0) {
        Write-Fail "$k : no SGR before box bytes"
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
        Write-Pass "$k : E2 94 82 preceded by SGR [$lastSgr] (contains expected '$($expected[$k])')"
        $pass++
    } else {
        Write-Fail "$k : expected '$($expected[$k])' before E2 94 82, got [$lastSgr]"
        $fail++
    }
}

# --- Cleanup ---
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Remove-Item $binPath -Force -EA SilentlyContinue

Write-Host "`n============================================" -ForegroundColor Cyan
Write-Host "VERDICT" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ("  U+2502 cells with correct SGR before them: {0} / 7" -f $pass)
Write-Host ("  Failed:                                    {0} / 7" -f $fail)
Write-Host ("  (lines where E2 94 82 was missing:         {0})" -f $missingBoxBytes)
Write-Host ""

if ($pass -eq 7 -and $fail -eq 0) {
    Write-Host "  >>> BUG IS NOT PRESENT in $VERSION" -ForegroundColor Green
    Write-Host "      Real U+2502 (E2 94 82) bytes were placed in psmux's cell buffer."
    Write-Host "      The renderer's output preserves each box char's expected SGR."
    Write-Host "      All 7 SGR variants from issue #263 verify clean."
    exit 0
}
elseif ($missingBoxBytes -eq 7) {
    Write-Host "  >>> ENCODING ISSUE STILL PRESENT" -ForegroundColor Yellow
    Write-Host "      Raw E2 94 82 bytes did not survive the pane's PTY encoding chain."
    Write-Host "      This is NOT issue #263 (which is about color, not encoding)."
    Write-Host "      Need a different injection method (e.g. paste-buffer, native compiled binary)."
    exit 2
}
else {
    Write-Host "  >>> BUG REPRODUCES" -ForegroundColor Red
    Write-Host "      $fail of 7 box chars dropped or had wrong SGR."
    exit 1
}
