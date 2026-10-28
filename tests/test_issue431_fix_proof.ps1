# Issue #431 - FIX PROOF (server-layer, machine-verifiable)
#
# BEFORE the fix: a sixel DCS emitted into a psmux pane was silently swallowed
# by the vt100 layer (proven earlier: BEFORE_MARK/AFTER_MARK survived, the sixel
# vanished with no trace in capture-pane OR any server state).
#
# AFTER the fix: the server parses the sixel into a SixelImage, stores it anchored
# in the pane, and transports it to the client via dump-state:
#   - a per-leaf "images":[{id,row,col,pw,ph,cw,ch}] descriptor, AND
#   - a one-shot "image_blobs":{"<id>":"<base64 of the raw ESC P..ST bytes>"}.
# This script PROVES that end-to-end at the server layer over raw TCP, and proves
# the pane stays functional. (Final rasterized pixels are an interactive WT step;
# see the checklist printed at the end.)

# The inbox conhost strips the sixel DCS before psmux reads the pane, so the
# image only reaches the parser under an OpenConsole: point
# PSMUX_CONPTY_DIR_FOR_TEST at a directory with conpty.dll and OpenConsole.exe
# (nuget Microsoft.Windows.Console.ConPTY). Without it the suite says SKIP.
$conptyDir = $env:PSMUX_CONPTY_DIR_FOR_TEST
if (-not $conptyDir -or -not (Test-Path (Join-Path $conptyDir 'conpty.dll')) -or -not (Test-Path (Join-Path $conptyDir 'OpenConsole.exe'))) {
    Write-Host "SKIP: set PSMUX_CONPTY_DIR_FOR_TEST to a directory with conpty.dll and OpenConsole.exe" -ForegroundColor Yellow
    exit 0
}
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
# Isolation: a unique -L namespace and a private data dir and empty config on
# every command; cleanup only with `-L <ns> kill-server`.
$ns = "t431_$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$psmuxDir = Join-Path ([IO.Path]::GetTempPath()) "psmux_$ns"
New-Item -ItemType Directory -Force $psmuxDir | Out-Null
$oldData = $env:PSMUX_DATA_DIR; $oldConf = $env:PSMUX_CONFIG_FILE; $oldConpty = $env:PSMUX_CONPTY_DIR
$env:PSMUX_DATA_DIR = $psmuxDir
$env:PSMUX_CONFIG_FILE = Join-Path $psmuxDir 'empty.conf'
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value '' -Encoding UTF8
Remove-Item Env:\TMUX -ErrorAction SilentlyContinue
Remove-Item Env:\TMUX_PANE -ErrorAction SilentlyContinue
function P { & $PSMUX -L $ns @args }
function Stop-Namespace {
    P kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $env:PSMUX_DATA_DIR = $oldData; $env:PSMUX_CONFIG_FILE = $oldConf
    if ($oldConpty) { $env:PSMUX_CONPTY_DIR = $oldConpty } else { Remove-Item Env:\PSMUX_CONPTY_DIR -ErrorAction SilentlyContinue }
    Remove-Item -Recurse -Force $psmuxDir -ErrorAction SilentlyContinue
}
$S = "issue431_fix"
$MARKER = "13;57;91"   # distinctive sixel color embedded in the raw bytes
$script:Pass = 0; $script:Fail = 0
function Pass($m){Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++}
function FailX($m){Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++}
function Info($m){Write-Host "  [INFO] $m" -ForegroundColor DarkCyan}

function Cleanup { Stop-Namespace }

function Get-DumpState {
    param([string]$Session)
    $port = (Get-Content "$psmuxDir\${ns}__$Session.port" -Raw).Trim()
    $key  = (Get-Content "$psmuxDir\${ns}__$Session.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream()
    $w = [System.IO.StreamWriter]::new($stream); $r = [System.IO.StreamReader]::new($stream)
    $w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
    $w.Write("dump-state`n"); $w.Flush()
    $stream.ReadTimeout = 5000
    $sb = [System.Text.StringBuilder]::new()
    try {
        while ($true) {
            $line = $r.ReadLine()
            if ($null -eq $line) { break }
            [void]$sb.AppendLine($line)
            if ($line.EndsWith('}') -and $sb.Length -gt 100) { break }
        }
    } catch {}
    $tcp.Close()
    return $sb.ToString().Trim()
}

Write-Host "`n=== Issue #431 FIX PROOF (server carries sixel) ===" -ForegroundColor Cyan

# Build the sixel emitter: BEFORE_MARK + raw sixel (color 13;57;91) + AFTER_MARK
$mid = [System.Text.Encoding]::ASCII.GetBytes('q"1;1;24;12#7;2;13;57;91#7!24~$-!24~$-')
# Two line feeds after it: the cursor stays on the image's row, and text
# written there erases the image cells under it, as in Windows Terminal.
$sixel = [byte[]]@(0x1B,0x50) + $mid + [byte[]]@(0x1B,0x5C,0x0D,0x0A,0x0D,0x0A)
$hex = ($sixel | ForEach-Object { '0x{0:X2}' -f $_ }) -join ','
$emitter = Join-Path $psmuxDir "issue431_fix_emit.ps1"
@"
Write-Host "BEFORE_MARK"
`$b = [byte[]]@($hex)
`$o = [Console]::OpenStandardOutput()
`$o.Write(`$b, 0, `$b.Length); `$o.Flush()
Start-Sleep -Milliseconds 150
Write-Host "AFTER_MARK"
"@ | Set-Content -Path $emitter -Encoding ASCII

$env:PSMUX_CONPTY_DIR = $conptyDir
P new-session -d -s $S 2>&1 | Out-Null
Start-Sleep -Seconds 3
P has-session -t $S 2>$null
if ($LASTEXITCODE -ne 0) { FailX "session did not start"; Cleanup; exit 1 }
Pass "session started"

P send-keys -t $S 'cls' Enter 2>&1 | Out-Null
Start-Sleep -Milliseconds 600
P send-keys -t $S (". '" + $emitter + "'") Enter 2>&1 | Out-Null
Start-Sleep -Seconds 2

# 1) Pane stays functional: BEFORE/AFTER markers survive
$cap = P capture-pane -t $S -p 2>&1 | Out-String
if ($cap -match "BEFORE_MARK" -and $cap -match "AFTER_MARK") { Pass "pane functional: BEFORE_MARK and AFTER_MARK both survive the sixel" }
else { FailX "pane text markers missing (BEFORE=$($cap -match 'BEFORE_MARK') AFTER=$($cap -match 'AFTER_MARK'))" }

# The sixel raw bytes must NOT leak as text into the grid
if ($cap -notmatch [regex]::Escape($MARKER) -and $cap -notmatch '!24~') { Pass "sixel bytes did NOT leak into the text grid (clean DCS consumption)" }
else { FailX "sixel bytes leaked into text grid" }

# 2) Server carries the image: dump-state has images descriptor + image_blobs
$dump = Get-DumpState -Session $S
Info "dump-state length = $($dump.Length) chars"

# images descriptor present and non-empty
$hasImagesArr = $dump -match '"images"\s*:\s*\[\s*\{'
if ($hasImagesArr) { Pass "dump-state leaf carries a NON-EMPTY images descriptor array" }
else {
    # show whether the field exists at all
    if ($dump -match '"images"\s*:\s*\[') { FailX "images array present but EMPTY - image not captured/visible" }
    else { FailX "images field absent from dump-state" }
}

# descriptor fields
if ($dump -match '"images"\s*:\s*\[\s*\{[^}]*"id"\s*:\s*(\d+)[^}]*"cw"\s*:\s*(\d+)[^}]*"ch"\s*:\s*(\d+)') {
    $imgId = $matches[1]; $cw = $matches[2]; $ch = $matches[3]
    Pass "descriptor parsed: id=$imgId cw=$cw ch=$ch (cell footprint captured)"
} else { $imgId = $null; Info "could not parse descriptor id/cw/ch" }

# image_blobs present, contains base64 for the id, and decodes to the raw sixel with our marker
if ($dump -match '"image_blobs"\s*:\s*\{\s*"(\d+)"\s*:\s*"([A-Za-z0-9+/=]+)"') {
    $blobId = $matches[1]; $b64 = $matches[2]
    Pass "image_blobs carries base64 for id=$blobId ($($b64.Length) b64 chars)"
    try {
        $raw = [System.Convert]::FromBase64String($b64)
        $rawAscii = [System.Text.Encoding]::ASCII.GetString($raw)
        $isDcs = ($raw.Length -ge 2 -and $raw[0] -eq 0x1B -and $raw[1] -eq 0x50)
        $endsSt = ($raw.Length -ge 2 -and $raw[$raw.Length-2] -eq 0x1B -and $raw[$raw.Length-1] -eq 0x5C)
        if ($isDcs -and $endsSt) { Pass "decoded blob is a well-formed DCS (starts ESC P, ends ST), $($raw.Length) bytes" }
        else { FailX "decoded blob not a well-formed DCS (startDCS=$isDcs endST=$endsSt)" }
        if ($rawAscii -match [regex]::Escape($MARKER)) { Pass "decoded blob contains the EXACT sixel color marker '$MARKER' - it is our image, faithfully preserved" }
        else { FailX "decoded blob missing marker '$MARKER'" }
        if ($blobId -eq $imgId) { Pass "blob id matches leaf descriptor id ($blobId) - transport is coherent" }
    } catch { FailX "base64 decode failed: $_" }
} else { FailX "image_blobs missing base64 payload in first dump after sixel" }

# 3) one-shot: a SECOND immediate dump should have empty image_blobs (already shipped)
$dump2 = Get-DumpState -Session $S
if ($dump2 -and $dump2 -notmatch '"image_blobs"\s*:\s*\{\s*"') { Pass "second dump ships no blob (blob shipped once)" }
elseif ($dump2) { FailX "second dump re-shipped the blob" }
else { FailX "second dump returned nothing" }

# 4) option gating: set -g sixel off is accepted and reflected
P set-option -g sixel off 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
$so = (P show-options -g 2>&1 | Out-String)
if ($so -match 'sixel\s+off') { Pass "set -g sixel off reflected in show-options" }
else { FailX "sixel option not reflected off (show-options)" }
P set-option -g sixel on 2>&1 | Out-Null

Remove-Item $emitter -Force -EA SilentlyContinue
Cleanup

Write-Host "`n=== Result ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:Pass)" -ForegroundColor Green
Write-Host "  Failed: $($script:Fail)" -ForegroundColor $(if ($script:Fail -gt 0){"Red"}else{"Green"})
Write-Host "`n  INTERACTIVE WT CHECKLIST (pixels - user confirms in Windows Terminal):" -ForegroundColor Yellow
Write-Host "   1. Run psmux in Windows Terminal, then: chafa -f sixel <image.png>" -ForegroundColor DarkYellow
Write-Host "   2. The image should now RENDER inside the pane (was blank before)." -ForegroundColor DarkYellow
Write-Host "   3. It should stay clipped to the pane and repaint after redraw/scroll." -ForegroundColor DarkYellow
Write-Host "   4. 'set -g sixel off' then re-run chafa => image suppressed." -ForegroundColor DarkYellow
exit $script:Fail
