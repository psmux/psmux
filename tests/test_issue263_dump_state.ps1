# Issue #263 — dump-state inspection.
#
# This tells us whether psmux stored U+2502 in its cell buffer or
# stored 3 separate CP437 chars (Γ ö é). dump-state returns JSON which
# unambiguously reports each cell's character + fg color.
#
# Strategy:
#   1. Write raw bytes E2 94 82 + SGR via Python sys.stdout.buffer
#   2. Run dump-state via TCP PERSISTENT connection
#   3. Inspect the cells: did psmux store U+2502 or 3 CP437 chars?
#   4. If U+2502: examine its fg attribute - does it match the SGR sent?
#   5. If 3 CP437 chars: parser is reading bytes as CP437 (different bug)

# The runner console is OEM 437: read psmux output as UTF-8 so non-ASCII cells compare by their real bytes.
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$VERSION = (& $PSMUX -V).Trim()
$PY = (Get-Command python -EA Stop).Source
$SESSION = "issue263_dump"
$psmuxDir = "$env:USERPROFILE\.psmux"

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

# Single-line bin file: ESC [ 38 ; 2 ; 255 ; 0 ; 0 m E2 94 82 SP X ESC [ 0 m \n
$bin = "$env:TEMP\psmux_issue263_dump.bin"
$esc = [byte]0x1B; $lbrk = [byte]0x5B; $m = [byte]0x6D
$box = [byte[]](0xE2, 0x94, 0x82)
$sp = [byte]0x20; $crlf = [byte[]](0x0D, 0x0A)
$reset = [byte[]]($esc, $lbrk, 0x30, $m)
$sgr = [System.Text.Encoding]::ASCII.GetBytes("38;2;255;0;0")
$tag = [System.Text.Encoding]::ASCII.GetBytes("X")
$list = New-Object System.Collections.Generic.List[byte]
$list.Add($esc); $list.Add($lbrk)
foreach ($b in $sgr) { $list.Add($b) }
$list.Add($m)
foreach ($b in $box) { $list.Add($b) }
$list.Add($sp)
foreach ($b in $tag) { $list.Add($b) }
foreach ($b in $reset) { $list.Add($b) }
foreach ($b in $crlf) { $list.Add($b) }
[System.IO.File]::WriteAllBytes($bin, $list.ToArray())

# Python emit
$pyEmit = "$env:TEMP\psmux_issue263_dump_emit.py"
@"
import sys
with open(r'$bin', 'rb') as f:
    sys.stdout.buffer.write(f.read())
sys.stdout.buffer.flush()
"@ | Set-Content -Path $pyEmit -Encoding UTF8

# Cleanup
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue

# Spawn
& $PSMUX new-session -d -s $SESSION -x 200 -y 60 2>$null
Start-Sleep -Seconds 3

Write-Host "`n=== Issue #263 DUMP-STATE INSPECTION ===" -ForegroundColor Cyan
Write-Host "  Build: $VERSION"
Write-Host "  Question: did psmux store U+2502 with correct fg, or was input mangled?"

& $PSMUX send-keys -t $SESSION 'chcp 65001 | Out-Null' Enter
Start-Sleep -Milliseconds 800
& $PSMUX send-keys -t $SESSION 'Clear-Host' Enter
Start-Sleep -Seconds 1
& $PSMUX send-keys -t $SESSION "& '$PY' -B '$pyEmit'" Enter
Write-Info "Python emitted; waiting 4s for psmux parser..."
Start-Sleep -Seconds 4

# --- Connect to TCP server, get dump-state ---
$port = (Get-Content "$psmuxDir\$SESSION.port" -Raw).Trim()
$key = (Get-Content "$psmuxDir\$SESSION.key" -Raw).Trim()
Write-Info "Connecting to TCP $port..."

$tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
$tcp.NoDelay = $true; $tcp.ReceiveTimeout = 10000
$stream = $tcp.GetStream()
$writer = [System.IO.StreamWriter]::new($stream); $writer.AutoFlush = $true
$reader = [System.IO.StreamReader]::new($stream)
$writer.Write("AUTH $key`n")
$auth = $reader.ReadLine()
if ($auth -ne "OK") { Write-Host "Auth failed: $auth" -F Red; $tcp.Close(); exit 1 }
$writer.Write("PERSISTENT`n")

# Read dump-state response
$writer.Write("dump-state`n")
$state = $null
$tcp.ReceiveTimeout = 3000
for ($i = 0; $i -lt 50; $i++) {
    try { $line = $reader.ReadLine() } catch { break }
    if ($null -eq $line) { break }
    if ($line.Length -gt 100 -and $line.StartsWith("{")) { $state = $line; break }
}
$tcp.Close()

if (-not $state) {
    Write-Fail "No JSON dump returned"
    exit 1
}

Write-Info "Got dump-state JSON: $($state.Length) bytes"

# --- Decode JSON and look for our painted cell ---
$json = $state | ConvertFrom-Json

# Find the line containing 'X' (our marker after the box char)
# The cell buffer is in pane.layout.{cells,etc} — depends on schema
# Let's just look at the raw cell text/string

# Print the structure briefly
function Find-Cells {
    param($obj, [string]$path = "")
    if ($null -eq $obj) { return }
    if ($obj -is [System.Array]) {
        for ($i = 0; $i -lt $obj.Count; $i++) {
            Find-Cells $obj[$i] "$path[$i]"
        }
        return
    }
    if ($obj -is [System.Management.Automation.PSCustomObject] -or $obj -is [hashtable]) {
        foreach ($p in $obj.PSObject.Properties) {
            if ($p.Name -match '^(text|chars?|content|cells?|line)$' -and $p.Value -is [string]) {
                if ($p.Value -match 'X$' -or $p.Value -match '^.X' -or $p.Value -match '│') {
                    Write-Host "    $path.$($p.Name) = $($p.Value)" -ForegroundColor Yellow
                }
            }
            Find-Cells $p.Value "$path.$($p.Name)"
        }
    }
}

Write-Host "`n--- Searching dump-state JSON for our cell ---" -ForegroundColor Yellow

# Save dump for inspection
$dumpPath = "$env:TEMP\psmux_issue263_dump.json"
$state | Set-Content -Path $dumpPath -Encoding UTF8
Write-Info "Dump saved to $dumpPath"

# Look for U+2502 (escaped as │ in JSON, or literal char in deserialized object)
$boxCharLiteral = [char]0x2502
$cpMojibake = [string][char]0x0393 + [char]0x00F6 + [char]0x00E9  # Γöé

if ($state.Contains([char]0x2502)) {
    Write-Pass "DUMP CONTAINS LITERAL U+2502 char"
} else {
    Write-Fail "DUMP DOES NOT contain U+2502 char"
}

if ($state.Contains("│")) {
    Write-Pass "DUMP CONTAINS \\u2502 escape"
}

if ($state.Contains($cpMojibake)) {
    Write-Fail "DUMP CONTAINS CP437 mojibake Γöé"
}

# Search the raw JSON (UTF-8 bytes) for E2 94 82 directly
$stateBytes = [System.Text.Encoding]::UTF8.GetBytes($state)
$stateHex = ($stateBytes | ForEach-Object { $_.ToString("X2") }) -join ''
$boxRawHits = ([regex]::Matches($stateHex, "E29482")).Count
$mojibakeHits = ([regex]::Matches($stateHex, "CE93C3B6C3A9")).Count
$escU2502Hits = ([regex]::Matches($stateHex, "5C7532353032")).Count  # │ ASCII

Write-Host "`n--- Raw byte search in dump JSON ---" -ForegroundColor Yellow
Write-Host "  E2 94 82 (literal U+2502 UTF-8):  $boxRawHits"
Write-Host "  CE 93 C3 B6 C3 A9 (CP437 mojibake): $mojibakeHits"
Write-Host "  │ (JSON escape):              $escU2502Hits"

# Cleanup
& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
Remove-Item $bin -Force -EA SilentlyContinue
Remove-Item $pyEmit -Force -EA SilentlyContinue

Write-Host "`n============================================" -ForegroundColor Cyan
Write-Host "WHAT THIS TELLS US" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan

if ($boxRawHits -gt 0 -or $escU2502Hits -gt 0) {
    Write-Host "  >>> psmux's cell buffer DID store U+2502 correctly" -ForegroundColor Green
    Write-Host "      Earlier capture-pane mojibake was a CAPTURE-SIDE issue."
    Write-Host "      (The renderer or capture path is converting U+2502 to CP437"
    Write-Host "       on the way out, NOT on the way in.)"
    Write-Host "      The 'fixed grey' bug must be tested via visual rendering."
} elseif ($mojibakeHits -gt 0) {
    Write-Host "  >>> psmux's parser stored 3 CP437 chars Γöé, NOT U+2502" -ForegroundColor Yellow
    Write-Host "      This is a UTF-8 PARSER issue (different from issue #263)."
    Write-Host "      Issue #263 is about color, not parsing. To test #263 we"
    Write-Host "      need to first get U+2502 into the cell buffer somehow."
} else {
    Write-Host "  >>> Neither U+2502 nor mojibake found in dump" -ForegroundColor Red
    Write-Host "      Cell may be empty or in unexpected format. Inspect $dumpPath"
}
