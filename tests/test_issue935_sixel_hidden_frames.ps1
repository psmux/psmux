# Issue #935: a sixel drawn over an older one in the same place hides it, so
# a program that plays an animation leaves one image in the pane, and the
# client draws each frame once; and ESC D moves the cursor down a line.
#
# Windows Terminal, whose sixel parser the pane's OpenConsole runs, writes a
# sixel pixel by pixel and leaves a pixel the sixel does not set as it was
# (_maybeFlushImageBuffer in SixelParser.cpp), so an older image shows only
# through pixels a newer one leaves unset. chafa plays a GIF by drawing each
# frame in the same place. The pane kept every frame under the next until its
# cap of 64, and from then on each frame evicted the oldest and the client
# drew the 63 left again, up to 28 MB a frame: the animation ran through old
# frames far too fast and Ctrl+C took a second or more.
#
# Needs PSMUX_CONPTY_DIR_FOR_TEST (a directory with conpty.dll and
# OpenConsole.exe, see tests/test_issue914_sixel_conpty_dir.ps1); without it
# the suite reports SKIP and exits 0. A pane on that OpenConsole draws frames
# of 40 by 24 pixels at the top left with a transparent background
# (ESC P 0 ; 1 q, as chafa writes them):
#   1. after 70 frames the pane holds one image,
#   2. a client attached under tests/conpty_da1_host.cs while 80 frames are
#      drawn writes about one sixel a frame,
#   3. a frame that leaves pixels unset hides the older image only in the
#      cells it paints completely,
#   4. room made for an image with ESC D (IND), as chafa makes it, puts the
#      image on the row below the line it was started from.  vt100 ignored
#      IND, so the image sat over that line (tmux: input.c:1413-1415).
#
# Isolation: private PSMUX_DATA_DIR and empty config, unique -L namespace on
# every command, cleanup only with `-L <ns> kill-server`.

$ErrorActionPreference = 'Continue'

$conptyDir = $env:PSMUX_CONPTY_DIR_FOR_TEST
if (-not $conptyDir -or -not (Test-Path (Join-Path $conptyDir 'conpty.dll')) -or -not (Test-Path (Join-Path $conptyDir 'OpenConsole.exe'))) {
    Write-Host "SKIP: set PSMUX_CONPTY_DIR_FOR_TEST to a directory with conpty.dll and OpenConsole.exe" -ForegroundColor Yellow
    exit 0
}

$exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$ns = "thf_$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$data = Join-Path ([IO.Path]::GetTempPath()) "psmux_sixel_hidden_$ns"
New-Item -ItemType Directory -Force $data | Out-Null
$saved = @{}
foreach ($k in 'PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'PSMUX_CONPTY_DIR', 'PSMUX_SESSION', 'PSMUX_HOST_COLORS', 'TMUX', 'TMUX_PANE', 'CONPTY_DA1_HOST_CELL') {
    $saved[$k] = [Environment]::GetEnvironmentVariable($k)
    Remove-Item "Env:\$k" -ErrorAction SilentlyContinue
}
$env:PSMUX_DATA_DIR = $data
$env:PSMUX_CONFIG_FILE = Join-Path $data 'empty.conf'
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value '' -Encoding UTF8

$script:TestsPassed = 0
$script:TestsFailed = 0
function Check($name, $ok, $detail) {
    if ($ok) { $script:TestsPassed++; Write-Host "  [PASS] $name  $detail" -ForegroundColor Green }
    else { $script:TestsFailed++; Write-Host "  [FAIL] $name  $detail" -ForegroundColor Red }
}
function P { & $exe -L $ns @args 2>&1 }

function Cleanup {
    & $exe -L $ns kill-server 2>$null | Out-Null
    Start-Sleep -Milliseconds 500
    foreach ($k in $saved.Keys) {
        if ($null -ne $saved[$k]) { Set-Item "Env:\$k" $saved[$k] } else { Remove-Item "Env:\$k" -ErrorAction SilentlyContinue }
    }
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
}

function WaitSession($s) {
    for ($i = 0; $i -lt 60; $i++) {
        & $exe -L $ns has-session -t $s 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

# One dump-state frame over the session's port.
function Frame($s) {
    $port = (Get-Content (Join-Path $data "${ns}__$s.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $data "${ns}__$s.key") -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port)
    $tcp.ReceiveTimeout = 4000
    $st = $tcp.GetStream()
    $w = [System.IO.StreamWriter]::new($st); $r = [System.IO.StreamReader]::new($st)
    $w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
    $w.Write("dump-state`n"); $w.Flush()
    $json = $null
    for ($j = 0; $j -lt 300; $j++) {
        try { $line = $r.ReadLine() } catch { break }
        if ($null -eq $line) { break }
        if ($line -ne 'NC' -and $line.Length -gt 100) { $json = $line; break }
    }
    $tcp.Close()
    if ($json) { return $json | ConvertFrom-Json }
    return $null
}

# Every image descriptor in a layout tree.
function Images($node) {
    if ($null -eq $node) { return @() }
    $all = @()
    if ($node.images) { $all += @($node.images) }
    foreach ($child in @($node.children)) { $all += @(Images $child) }
    return $all
}

# The pane command that draws $count frames at the top left, $delay ms apart,
# then moves below them and prints $marker. $body is the sixel after ESC P.
function FramesCommand($body, $count, $delay, $marker) {
    "cls; `$f = [char]27 + '[H' + [char]27 + '$body' + [char]27 + '\'; 1..$count | % { [Console]::Out.Write(`$f); Start-Sleep -Milliseconds $delay }; [Console]::Out.Write([char]10 + [char]10); '$marker'"
}

function WaitMarker($target, $marker, $seconds) {
    for ($i = 0; $i -lt $seconds * 4; $i++) {
        if ((P capture-pane -p -t $target | Out-String) -match "(?m)^$marker") { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$hostExe = Join-Path $data 'conpty_da1_host.exe'
if ($csc) { & $csc.FullName /nologo /optimize /out:$hostExe (Join-Path $PSScriptRoot 'conpty_da1_host.cs') 2>&1 | Out-Null }
if (-not (Test-Path $hostExe)) {
    Check 'build tests\conpty_da1_host.cs' $false '(csc.exe unavailable)'
    Cleanup
    exit $script:TestsFailed
}

# The raster sizes of the sixels in a capture, "WxH" each.
function Rasters([byte[]]$bytes) {
    $text = [Text.Encoding]::GetEncoding(28591).GetString($bytes)
    @([regex]::Matches($text, "\x1bP[0-9;]*q`"(\d+);(\d+);(\d+);(\d+)") | ForEach-Object { "$($_.Groups[3].Value)x$($_.Groups[4].Value)" })
}

Write-Host "`n=== sixel frames drawn in one place ===" -ForegroundColor Cyan
Write-Host "  exe: $exe ($(& $exe -V 2>$null | Select-Object -Last 1))"

$env:PSMUX_CONPTY_DIR = $conptyDir
P new-session -d -s a -x 120 -y 30 | Out-Null
P new-session -d -s b -x 120 -y 30 | Out-Null
P new-session -d -s c -x 120 -y 30 | Out-Null
P new-session -d -s d -x 120 -y 30 | Out-Null
Remove-Item Env:\PSMUX_CONPTY_DIR
if (-not ((WaitSession 'a') -and (WaitSession 'b') -and (WaitSession 'c') -and (WaitSession 'd'))) { Check 'sessions start on OpenConsole' $false ''; Cleanup; exit $script:TestsFailed }
Start-Sleep -Seconds 2

# 40 by 24 pixels, 4 by 2 cells, every pixel set, transparent background.
$full = 'P0;1q#1;2;100;0;0#1!40~-!40~'

# ── 1: frames in one place leave one image ─────────────────────────────────
Write-Host "`n--- 1: 70 frames in one place ---" -ForegroundColor Cyan
P send-keys -t a (FramesCommand $full 70 10 'DONE_A') Enter | Out-Null
Check 'the pane drew 70 frames' (WaitMarker 'a' 'DONE_A' 30) ''
$imgs = @(Images (Frame 'a').layout)
Check '1. the pane holds one image' ($imgs.Count -eq 1) "($($imgs.Count) images)"

# ── 2: a client attached while frames are drawn ────────────────────────────
Write-Host "`n--- 2: a client attached while 80 frames are drawn ---" -ForegroundColor Cyan
$out = Join-Path $data 'client_frames.bin'
$client = Start-Process -FilePath $hostExe -ArgumentList @((Join-Path $conptyDir 'conpty.dll'), $out, 'yes', '8', '14000', "`"$exe`" -L $ns attach -t b") -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 4
P send-keys -t b (FramesCommand $full 80 40 'DONE_B') Enter | Out-Null
Check 'the pane drew 80 frames' (WaitMarker 'b' 'DONE_B' 30) ''
$client.WaitForExit(20000) | Out-Null
$r = if (Test-Path $out) { @(Rasters ([IO.File]::ReadAllBytes($out))) } else { @() }
$frames = @($r | Where-Object { $_ -eq '40x24' }).Count
$size = if (Test-Path $out) { (Get-Item $out).Length } else { 0 }
Check '2. the client drew each frame at most once' ($frames -ge 1 -and $frames -le 82) "($frames sixels of 40x24, $([math]::Round($size / 1024)) KB written)"

# ── 3: a frame that leaves pixels unset ────────────────────────────────────
Write-Host "`n--- 3: a frame that leaves pixels unset ---" -ForegroundColor Cyan
# The second frame draws only the first 20 columns: 2 by 2 cells.
$half = 'P0;1q#2;2;0;100;0#2!20~-!20~'
P send-keys -t c "cls; [Console]::Out.Write([char]27 + '[H' + [char]27 + '$full' + [char]27 + '\' + [char]27 + '[H' + [char]27 + '$half' + [char]27 + '\' + [char]10 + [char]10); 'DONE_C'" Enter | Out-Null
Check 'the pane drew both' (WaitMarker 'c' 'DONE_C' 30) ''
$imgs = @(Images (Frame 'c').layout)
$first = $imgs | Where-Object { $_.pw -eq 40 } | Select-Object -First 1
$runs = if ($first -and $first.erased) { @($first.erased | ForEach-Object { $_ -join ',' }) -join ' ' } else { '' }
Check '3. both images stay, the older one hidden in the 2 by 2 cells the newer paints' ($imgs.Count -eq 2 -and $runs -eq '0,0,2 1,0,2') "($($imgs.Count) images, erased: $runs)"

# ── 4: room made with ESC D, as chafa makes it ──────────────────────────────
Write-Host "`n--- 4: room for the image made with ESC D ---" -ForegroundColor Cyan
# chafa writes ESC D once per row of the image, moves back up and saves the
# place, then draws there: the row below the line it was started from.
$room = "([char]27 + 'D') * 3 + [char]27 + '[3A' + [char]27 + '[s'"
P send-keys -t d "cls; 'MARK_D'; [Console]::Out.Write($room + [char]27 + '$full' + [char]27 + '\' + [char]27 + '[u' + [char]10 + [char]10 + [char]10); 'DONE_D'" Enter | Out-Null
Check 'the pane drew the image' (WaitMarker 'd' 'DONE_D' 30) ''
$img = @(Images (Frame 'd').layout) | Select-Object -First 1
$rows = @(P capture-pane -p -t d)
$markRow = [array]::FindIndex([string[]]$rows, [Predicate[string]] { param($l) $l -match '^MARK_D' })
Check '4. the image starts on the row below the marker line, which stays' ($img -and $markRow -ge 0 -and [int]$img.row -eq $markRow + 1) "(marker on row $markRow, image on row $(if ($img) { $img.row } else { '-' }))"

Cleanup
Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
