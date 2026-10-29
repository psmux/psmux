# Issue #932: a pane image that sticks out of its pane is drawn cut to the
# pane, the cells text is written over are left out, and the client draws
# images at the cell size the terminal reports.
#
# tmux draws the visible part of an image, cut out and resampled from the cell
# it was laid out in to the terminal's cell (tty_cmd_sixelimage, tty.c:2180;
# sixel_scale, image-sixel.c:411 in tmux 3.8). psmux drew an image only when
# all of it lay inside its pane, as the pane received it.
#
# Needs PSMUX_CONPTY_DIR_FOR_TEST (a directory with conpty.dll and
# OpenConsole.exe, see tests/test_issue914_sixel_conpty_dir.ps1); without it
# the suite reports SKIP and exits 0. A pane on that OpenConsole writes an
# image, and a client attached under tests/conpty_da1_host.cs, which answers
# DA1 with sixel and, when told, CSI 16 t with a cell size, records what the
# client writes:
#   1. a whole image with no cell size reported is the stored blob as it came,
#   2. with 8x16 reported, it is resampled: 14 by 24 pixels become 11 by 19,
#   3. an image wider than its pane is drawn cut at the pane's right edge,
#   4. an image scrolled partly above the pane is drawn without the rows above,
#   5. the line written after an image lands on its partly filled last row
#      (OpenConsole leaves the cursor on the top of the last band), and that
#      row is left out, as Windows Terminal erases it,
#   6. keys typed beside that row redraw only the part on that row, not the
#      rest of the image,
#   7. a program in a pane on that OpenConsole is told the pane's size in
#      cells for CSI 18 t and the 10x20 cell for CSI 14 t and CSI 16 t, once
#      each. OpenConsole forwards CSI 18 t unanswered, and chafa, which asks
#      it first, sized its image for 80 by 24 cells in any pane.
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
$ns = "tcs_$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$data = Join-Path ([IO.Path]::GetTempPath()) "psmux_sixel_crop_$ns"
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

# The first image descriptor in a layout tree.
function FirstImage($node) {
    if ($null -eq $node) { return $null }
    if ($node.images -and $node.images.Count -gt 0) { return $node.images[0] }
    foreach ($child in @($node.children)) {
        $found = FirstImage $child
        if ($found) { return $found }
    }
    return $null
}

# Emit a sixel from the pane, then $newlines line feeds and a marker line.
# OpenConsole leaves the cursor on the row holding the top of the image's last
# band, so with one line feed the marker lands on the image's last row.
function Emit($target, $body, $marker, $newlines = 2) {
    $lf = (@('[char]10') * $newlines) -join ' + '
    P send-keys -t $target "cls; [Console]::Out.Write([char]27 + '$body' + [char]27 + '\' + $lf); '$marker'" Enter | Out-Null
    for ($i = 0; $i -lt 40; $i++) {
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

# What the client attached to session $s writes, with $cell ('' or 'WxH')
# reported for CSI 16 t.
function Attach($s, $cell, $tag) {
    if ($cell) { $env:CONPTY_DA1_HOST_CELL = $cell } else { Remove-Item Env:\CONPTY_DA1_HOST_CELL -EA SilentlyContinue }
    $out = Join-Path $data "client_$tag.bin"
    & $hostExe (Join-Path $conptyDir 'conpty.dll') $out yes 8 4000 "`"$exe`" -L $ns attach -t $s" | Out-Null
    Remove-Item Env:\CONPTY_DA1_HOST_CELL -EA SilentlyContinue
    if (Test-Path $out) { return [IO.File]::ReadAllBytes($out) }
    return [byte[]]@()
}

# The raster sizes of the sixels in a capture, "WxH" each.
function Rasters([byte[]]$bytes) {
    $text = [Text.Encoding]::GetEncoding(28591).GetString($bytes)
    @([regex]::Matches($text, "\x1bP[0-9;]*q`"(\d+);(\d+);(\d+);(\d+)") | ForEach-Object { "$($_.Groups[3].Value)x$($_.Groups[4].Value)" })
}

Write-Host "`n=== sixel cut to the pane and drawn at the terminal's cell ===" -ForegroundColor Cyan
Write-Host "  exe: $exe ($(& $exe -V 2>$null | Select-Object -Last 1))"

$env:PSMUX_CONPTY_DIR = $conptyDir
P new-session -d -s a -x 120 -y 30 | Out-Null
P new-session -d -s b -x 120 -y 30 | Out-Null
P new-session -d -s c -x 120 -y 30 | Out-Null
P new-session -d -s d -x 120 -y 30 | Out-Null
# Part 6 needs the shell prompt, which lands on the image's last row, to be
# shorter than the image's 40 cells, so session e starts in the drive root:
# from a deep working directory `PS <cwd>> ` covers the whole row.
P new-session -d -s e -x 120 -y 30 -c "$env:SystemDrive\" | Out-Null
Remove-Item Env:\PSMUX_CONPTY_DIR
if (-not ((WaitSession 'a') -and (WaitSession 'b') -and (WaitSession 'c') -and (WaitSession 'd') -and (WaitSession 'e'))) { Check 'sessions start on OpenConsole' $false ''; Cleanup; exit $script:TestsFailed }
Start-Sleep -Seconds 2

# ── 1 and 2: a whole image ──────────────────────────────────────────────────
Write-Host "`n--- 1, 2: a whole image ---" -ForegroundColor Cyan
# The 914 suite's image: 14 by 24 pixels, 2 by 2 cells.
$small = 'Pq#1;2;100;100;0#2;2;0;100;0#1~~@@vv@@~~@@~~$#2??}}GG}}??}}??-#1@@@@@@@@@@@@@@'
Check 'the pane wrote the image' (Emit 'a' $small 'EMITTED_A') ''
$stored = [char]27 + 'P0q"2;1;14;24#1;2;100;100;0'
$bytes = Attach 'a' '' 'whole'
$text = [Text.Encoding]::GetEncoding(28591).GetString($bytes)
Check '1. no cell size reported: the stored blob as it came' ($text.Contains($stored)) "(rasters: $((Rasters $bytes) -join ', '))"
$bytes = Attach 'a' '8x16' 'whole8x16'
$r = Rasters $bytes
Check '2. 8x16 reported: resampled to 11x19 in the same 2x2 cells' ($r -contains '11x19') "(rasters: $($r -join ', '))"

# ── 3: wider than its pane ──────────────────────────────────────────────────
Write-Host "`n--- 3: an image wider than its pane ---" -ForegroundColor Cyan
# 40 by 24 pixels, 4 by 2 cells.
$wide = 'Pq#1;2;100;0;0#1!40~-!40~'
Check 'the pane wrote the wide image' (Emit 'b' $wide 'EMITTED_B') ''
P split-window -h -t b | Out-Null
Start-Sleep -Milliseconds 800
P resize-pane -t 'b:0.0' -x 3 | Out-Null
Start-Sleep -Milliseconds 800
$img = FirstImage (Frame 'b').layout
$width = (P display-message -p -t 'b:0.0' '#{pane_width}' | Out-String).Trim()
Check 'the image sticks out of its pane' ($img -and $img.cw -gt [int]$width) "(image $($img.cw) cells, pane $width)"
$r = Rasters (Attach 'b' '' 'wide')
Check '3. drawn cut to the 3 columns inside the pane: 30x24' ($r -contains '30x24') "(rasters: $($r -join ', '))"

# ── 4: scrolled partly above its pane ───────────────────────────────────────
Write-Host "`n--- 4: an image scrolled partly above its pane ---" -ForegroundColor Cyan
# 40 by 120 pixels, 4 by 6 cells.
$tall = 'Pq#1;2;0;0;100#1' + ((1..10 | ForEach-Object { '!40~' }) -join '-')
Check 'the pane wrote the tall image' (Emit 'c' $tall 'EMITTED_C') ''
$row = 0
for ($i = 0; $i -lt 60; $i++) {
    $img = FirstImage (Frame 'c').layout
    if ($img) { $row = [int]$img.row }
    if ($img -and $row -lt 0) { break }
    P send-keys -t c Enter | Out-Null
    Start-Sleep -Milliseconds 300
}
Check 'the image is partly above the pane' ($img -and $row -lt 0 -and $row -gt -6) "(row $row)"
$want = "40x$((6 + $row) * 20)"
$r = Rasters (Attach 'c' '' 'tall')
Check "4. drawn without the rows above: $want" ($r -contains $want) "(rasters: $($r -join ', '))"

# ── 5: the next line written over the image's last row ─────────────────────
Write-Host "`n--- 5: the next line on the image's last row ---" -ForegroundColor Cyan
# The wide image again: 40 by 24 pixels, the second row of cells 4 pixels deep.
Check 'the pane wrote the image and one line feed' (Emit 'd' $wide 'EMITTED_D' 1) ''
$img = FirstImage (Frame 'd').layout
Check 'the marker line erased the last row of cells' ($img -and $img.erased -and "$($img.erased[0] -join ',')" -eq '1,0,4') "(erased: $(@($img.erased | ForEach-Object { $_ -join ',' }) -join ' '))"
$r = Rasters (Attach 'd' '' 'erased')
Check '5. drawn without the row written over: 40x20' ($r -contains '40x20' -and -not ($r -contains '40x24')) "(rasters: $($r -join ', '))"

# ── 6: keys typed beside the image's last row ──────────────────────────────
Write-Host "`n--- 6: keys typed beside the image's last row ---" -ForegroundColor Cyan
# 400 by 24 pixels, 40 by 2 cells, wider than the prompt that lands on its
# second row, so the cells right of the prompt are still drawn. That row
# holds the image's last 4 pixels, so its part is N by 4.
$long = 'Pq#1;2;0;100;0#1!400~-!400~'
P send-keys -t e "cls; [Console]::Out.Write([char]27 + '$long' + [char]27 + '\' + [char]10)" Enter | Out-Null
$img = $null
for ($i = 0; $i -lt 40; $i++) {
    $img = FirstImage (Frame 'e').layout
    if ($img -and $img.erased) { break }
    Start-Sleep -Milliseconds 250
}
Check 'the prompt erased the start of the last row' ($img -and $img.erased -and $img.erased[0][0] -eq 1 -and $img.erased[0][2] -lt 40) "(erased: $(@($img.erased | ForEach-Object { $_ -join ',' }) -join ' '))"
$out = Join-Path $data 'client_typing.bin'
$typer = Start-Process -FilePath $hostExe -ArgumentList @((Join-Path $conptyDir 'conpty.dll'), $out, 'yes', '8', '9000', "`"$exe`" -L $ns attach -t e") -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 4
foreach ($key in 'a', 'b', 'c') {
    P send-keys -t e -l $key | Out-Null
    Start-Sleep -Milliseconds 700
}
$typer.WaitForExit(15000) | Out-Null
$r = if (Test-Path $out) { Rasters ([IO.File]::ReadAllBytes($out)) } else { @() }
$top = @($r | Where-Object { $_ -eq '400x20' }).Count
$rowParts = @($r | Where-Object { $_ -match '^\d+x4$' }).Count
Check '6. the part above the row is drawn once, on attach' ($top -eq 1) "(400x20 drawn $top times; rasters: $($r -join ', '))"
Check '   and the part on the row once per key, plus the first' ($rowParts -ge 4) "($rowParts times)"

# ── 7: the size reports a program in the pane is given ─────────────────────
Write-Host "`n--- 7: CSI 18 t, 14 t and 16 t in a pane on OpenConsole ---" -ForegroundColor Cyan
$probe = Join-Path $data 'query_probe.exe'
& $csc.FullName /nologo /optimize /out:$probe (Join-Path $PSScriptRoot 'query_probe_child.cs') 2>&1 | Out-Null
$log = Join-Path $data 'probe.txt'
$env:PSMUX_CONPTY_DIR = $conptyDir
P new-session -d -s f -x 40 -y 12 "$probe $log 600" | Out-Null
Remove-Item Env:\PSMUX_CONPTY_DIR
$size = (P display-message -p -t f '#{pane_height} #{pane_width}' | Out-String).Trim() -split ' '
for ($i = 0; $i -lt 120 -and -not (Test-Path $log); $i++) { Start-Sleep -Milliseconds 250 }
Start-Sleep -Milliseconds 300
$probeOut = if (Test-Path $log) { Get-Content $log -Raw } else { '' }
function ProbeText($name) {
    foreach ($line in ($probeOut -split "`r?`n")) {
        if ($line.StartsWith("$name ") -and $line -match 'hex=\[([^\]]*)\]') {
            if (-not $Matches[1]) { return '' }
            return -join ($Matches[1] -split ' ' | ForEach-Object { [char][byte]("0x$_") })
        }
    }
    return $null
}
function Show($s) { if ($null -eq $s) { return '(no line)' } $s -replace [char]27, 'ESC' }
$rows = [int]$size[0]; $cols = [int]$size[1]
$e = [char]27
$r18 = ProbeText 'XTWINOPS_18'
Check "7. CSI 18 t answered once with the pane's ${rows} rows and ${cols} columns" ($r18 -eq "$e[8;${rows};${cols}t") "(got $(Show $r18))"
$r14 = ProbeText 'XTWINOPS_14'
Check '   CSI 14 t answered once with rows*20 and cols*10' ($r14 -eq "$e[4;$($rows * 20);$($cols * 10)t") "(got $(Show $r14))"
$r16 = ProbeText 'XTWINOPS_16'
Check '   CSI 16 t answered once with the 10x20 cell' ($r16 -eq "$e[6;20;10t") "(got $(Show $r16))"
# tmux built with sixel lists 4 in its DA1 answer (input.c:1586-1590).
$rda = ProbeText 'DA1'
Check '8. DA1 lists sixel, as tmux built with sixel does' ($rda -eq "$e[?1;2;4c") "(got $(Show $rda))"

Cleanup
Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
