# Issue #263 — DEFINITIVE PROOF.
#
# Question from issue: "Box-drawing chars (│) render in fixed light grey,
# ignoring SGR." (psmux 3.3.3)
#
# Test method (the only way to settle this without visual inspection):
#   1. Inject raw bytes via Python's sys.stdout.buffer (bypasses Windows
#      conhost code-page transformation): exactly what the issue's
#      Write-Host "`e[<sgr>m│ <tag>`e[0m" produces in the user's pane.
#   2. Query psmux's internal cell buffer via dump-state TCP command.
#   3. Inspect rows_v2[N].runs[K].fg for the cell containing │.
#   4. If fg matches the requested SGR's color: bug NOT present.
#   5. If fg is fixed grey (or any uniform value across all 7 SGRs):
#      bug IS present.
#
# This bypasses capture-pane (which had Windows pipe encoding artifacts
# in earlier tests) and goes straight to the parser's stored state.

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$VERSION = (& $PSMUX -V).Trim()
$PY = (Get-Command python -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

# Each case: { sgr, tag, expectedFg }
# expectedFg matches psmux's dump-state schema: "idx:N" or "rgb:R,G,B"
$cases = @(
    @{ Sgr = "90";              Tag = "T1"; ExpectedFg = "idx:8"             }, # bright black = idx 8
    @{ Sgr = "37";              Tag = "T2"; ExpectedFg = "idx:7"             }, # white      = idx 7
    @{ Sgr = "1;37";            Tag = "T3"; ExpectedFg = "idx:7|idx:15"      }, # bold + 7   may map to 15
    @{ Sgr = "38;5;240";        Tag = "T4"; ExpectedFg = "idx:240"           },
    @{ Sgr = "38;5;250";        Tag = "T5"; ExpectedFg = "idx:250"           },
    @{ Sgr = "38;2;128;128;128";Tag = "T6"; ExpectedFg = "rgb:128,128,128"   },
    @{ Sgr = "38;2;255;0;0";    Tag = "T7"; ExpectedFg = "rgb:255,0,0"       }
)

# --- Build single binary file with all 7 lines ---
function To-BinLine {
    param([string]$Sgr, [string]$Tag)
    $list = New-Object System.Collections.Generic.List[byte]
    $list.Add(0x1B); $list.Add(0x5B)
    foreach ($b in [System.Text.Encoding]::ASCII.GetBytes($Sgr)) { $list.Add($b) }
    $list.Add(0x6D)               # m
    $list.Add(0xE2); $list.Add(0x94); $list.Add(0x82)  # U+2502
    $list.Add(0x20)               # space
    foreach ($b in [System.Text.Encoding]::ASCII.GetBytes($Tag)) { $list.Add($b) }
    $list.Add(0x1B); $list.Add(0x5B); $list.Add(0x30); $list.Add(0x6D)  # ESC[0m
    $list.Add(0x0D); $list.Add(0x0A)
    return ,$list.ToArray()
}

$bin = "$env:TEMP\psmux_issue263_def.bin"
$accum = New-Object System.Collections.Generic.List[byte]
foreach ($c in $cases) {
    $line = To-BinLine -Sgr $c.Sgr -Tag $c.Tag
    foreach ($b in $line) { $accum.Add($b) }
}
[System.IO.File]::WriteAllBytes($bin, $accum.ToArray())
$verifyBytes = [System.IO.File]::ReadAllBytes($bin)
$verifyHex = ($verifyBytes | ForEach-Object { $_.ToString("X2") }) -join ''
$boxCount = ([regex]::Matches($verifyHex, "E29482")).Count
Write-Info "Bin file: $($verifyBytes.Length) bytes, U+2502 count=$boxCount (expected 7)"

# Python emit script
$pyScript = "$env:TEMP\psmux_issue263_def_emit.py"
$pyContent = @"
import sys
with open(r'$bin', 'rb') as f:
    sys.stdout.buffer.write(f.read())
sys.stdout.buffer.flush()
"@
[System.IO.File]::WriteAllText($pyScript, $pyContent, (New-Object System.Text.UTF8Encoding($false)))

# --- Spawn fresh session ---
$SESSION = "issue263_def"
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
& $PSMUX new-session -d -s $SESSION -x 200 -y 60 2>$null
Start-Sleep -Seconds 3

Write-Host "`n=== Issue #263 DEFINITIVE PROOF ===" -ForegroundColor Cyan
Write-Host "  Build under test: $VERSION"
Write-Host "  Issue env:        psmux 3.3.3"
Write-Host "  Method:           raw byte injection -> dump-state cell inspection"
Write-Host "  This is the cleanest test: looks at psmux's STORED cell.fg directly."
Write-Host ""

& $PSMUX send-keys -t $SESSION 'chcp 65001 | Out-Null' Enter
Start-Sleep -Milliseconds 800
& $PSMUX send-keys -t $SESSION 'Clear-Host' Enter
Start-Sleep -Seconds 1
& $PSMUX send-keys -t $SESSION "& '$PY' -B '$pyScript'" Enter
Write-Info "Python wrote raw bytes; waiting 4s for psmux parser..."
Start-Sleep -Seconds 4

# --- Get dump-state ---
$port = (Get-Content "$psmuxDir\$SESSION.port" -Raw).Trim()
$key = (Get-Content "$psmuxDir\$SESSION.key" -Raw).Trim()

$tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
$tcp.NoDelay = $true; $tcp.ReceiveTimeout = 10000
$stream = $tcp.GetStream()
$writer = [System.IO.StreamWriter]::new($stream); $writer.AutoFlush = $true
$reader = [System.IO.StreamReader]::new($stream)
$writer.Write("AUTH $key`n")
$auth = $reader.ReadLine()
if ($auth -ne "OK") { Write-Host "Auth failed" -F Red; $tcp.Close(); exit 1 }
$writer.Write("PERSISTENT`n")

$writer.Write("dump-state`n")
$state = $null
$tcp.ReceiveTimeout = 3000
for ($i = 0; $i -lt 50; $i++) {
    try { $line = $reader.ReadLine() } catch { break }
    if ($null -eq $line) { break }
    if ($line.Length -gt 100 -and $line.StartsWith("{")) { $state = $line; break }
}
$tcp.Close()

if (-not $state) { Write-Host "No dump returned" -F Red; exit 1 }
Write-Info "Got dump-state JSON ($($state.Length) bytes)"

$obj = $state | ConvertFrom-Json
$rowsV2 = $obj.layout.rows_v2
$BAR = [char]0x2502

# --- Find runs containing U+2502 ---
$boxRuns = New-Object System.Collections.Generic.List[object]
for ($i = 0; $i -lt $rowsV2.Count; $i++) {
    $row = $rowsV2[$i]
    if (-not $row.runs) { continue }
    for ($j = 0; $j -lt $row.runs.Count; $j++) {
        $r = $row.runs[$j]
        if ($r.text -and "$($r.text)".Contains($BAR)) {
            $boxRuns.Add(@{ Row=$i; Idx=$j; Text=$r.text; Fg=$r.fg; Bg=$r.bg; Flags=$r.flags }) | Out-Null
        }
    }
}

Write-Host "`n--- runs containing U+2502 in cell buffer ---" -ForegroundColor Yellow
Write-Host ("  Found {0} runs (expected 7)" -f $boxRuns.Count)
foreach ($r in $boxRuns) {
    $textShown = "$($r.Text)" -replace [char]0x2502, '|U+2502|'
    Write-Host ("    rows_v2[{0}].runs[{1}]: text='{2}' fg={3} bg={4} flags={5}" -f $r.Row, $r.Idx, $textShown, $r.Fg, $r.Bg, $r.Flags)
}

# --- Per-case verification ---
Write-Host "`n--- Per-case verification (each box char's fg vs requested SGR) ---" -ForegroundColor Yellow
$pass = 0; $fail = 0
$cellFgs = New-Object System.Collections.Generic.List[string]
foreach ($c in $cases) {
    $tag = $c.Tag
    $matchRun = $boxRuns | Where-Object { $_.Text.Contains($tag) } | Select-Object -First 1
    if (-not $matchRun) {
        Write-Fail "$tag (SGR $($c.Sgr)) : no run with this tag found"
        $fail++
        continue
    }
    $cellFgs.Add($matchRun.Fg) | Out-Null
    $okPatterns = $c.ExpectedFg -split '\|'
    $ok = $false
    foreach ($p in $okPatterns) { if ($matchRun.Fg -eq $p) { $ok = $true; break } }
    if ($ok) {
        Write-Pass ("$tag (SGR $($c.Sgr)) : box cell fg = '{0}' matches expected '{1}'" -f $matchRun.Fg, $c.ExpectedFg)
        $pass++
    } else {
        Write-Fail ("$tag (SGR $($c.Sgr)) : box cell fg = '{0}' but expected '{1}'" -f $matchRun.Fg, $c.ExpectedFg)
        $fail++
    }
}

# --- Anti-bug test: are ALL fg values the same? Bug claim is they all force light grey ---
$uniqueFgs = $cellFgs | Sort-Object -Unique
Write-Host "`n--- Anti-bug check: are all 7 box-char fg values UNIFORM? ---" -ForegroundColor Yellow
Write-Host ("  Unique fg values across 7 box cells: {0}" -f $uniqueFgs.Count)
foreach ($u in $uniqueFgs) { Write-Host "    $u" }

# Cleanup
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Remove-Item $bin -Force -EA SilentlyContinue
Remove-Item $pyScript -Force -EA SilentlyContinue

Write-Host "`n============================================" -ForegroundColor Cyan
Write-Host "FINAL VERDICT" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan

if ($pass -eq 7 -and $uniqueFgs.Count -ge 5) {
    Write-Host "  >>> BUG IS NOT PRESENT in $VERSION" -ForegroundColor Green
    Write-Host ""
    Write-Host "  Each of the 7 box-drawing characters in the cell buffer carries" -ForegroundColor Green
    Write-Host "  the EXACT fg color requested by its SGR. The fg values are" -ForegroundColor Green
    Write-Host "  DIFFERENT across the 7 cases (idx:8, idx:7, idx:240, idx:250," -ForegroundColor Green
    Write-Host "  rgb:128,128,128, rgb:255,0,0). This rules out the issue's claim" -ForegroundColor Green
    Write-Host "  of a uniform 'fixed light grey' override." -ForegroundColor Green
    Write-Host ""
    Write-Host "  Issue #263 cannot be reproduced on this build." -ForegroundColor Green
    Write-Host "  (Original report on 3.3.3; current build 3.3.4. Investigation" -ForegroundColor White
    Write-Host "  required to determine whether 3.3.3 had the bug or whether the" -ForegroundColor White
    Write-Host "  reporter saw a host-terminal rendering issue.)" -ForegroundColor White
    exit 0
}
elseif ($uniqueFgs.Count -eq 1 -and $uniqueFgs[0] -match 'idx:7|idx:8|rgb:1[0-9][0-9],1[0-9][0-9],1[0-9][0-9]') {
    Write-Host "  >>> BUG REPRODUCES" -ForegroundColor Red
    Write-Host "      All 7 box cells share fg = $($uniqueFgs[0]) (a uniform grey-ish color)" -ForegroundColor Red
    Write-Host "      regardless of the SGR sent. This matches issue #263 exactly." -ForegroundColor Red
    exit 1
}
else {
    Write-Host "  >>> MIXED RESULT - inspect details above" -ForegroundColor Yellow
    Write-Host "      Pass: $pass / 7    Unique fgs: $($uniqueFgs.Count)"
    exit 2
}
