# Issue #914: a sixel image written in a pane reaches the client when the pane
# is hosted by an OpenConsole that forwards sixel (PSMUX_CONPTY_DIR).
#
# The inbox conhost strips the sixel DCS before psmux reads the pane, so this
# suite needs a directory holding conpty.dll and OpenConsole.exe, for example
# from the Microsoft.Windows.Console.ConPTY package on nuget.org. Point
# PSMUX_CONPTY_DIR_FOR_TEST at it; without it the suite reports SKIP and exits 0.
#
# Checked over dump-state, the frame the client draws from:
#   1. one pane: its leaf carries one image descriptor, and image_blobs holds
#      the same id with the exact DCS the pane wrote,
#   2. two panes that each write a sixel: each leaf has its own image, the ids
#      differ, and after refresh-client both blobs are shipped.
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
$ns = "t431_$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$data = Join-Path ([IO.Path]::GetTempPath()) "psmux_issue431_$ns"
New-Item -ItemType Directory -Force $data | Out-Null
$oldData = $env:PSMUX_DATA_DIR
$oldConf = $env:PSMUX_CONFIG_FILE
$oldConpty = $env:PSMUX_CONPTY_DIR
$env:PSMUX_DATA_DIR = $data
$emptyConf = Join-Path $data 'empty.conf'
Set-Content -Path $emptyConf -Value '' -Encoding UTF8
$env:PSMUX_CONFIG_FILE = $emptyConf
$env:PSMUX_CONPTY_DIR = $conptyDir
Remove-Item Env:\TMUX -ErrorAction SilentlyContinue
Remove-Item Env:\TMUX_PANE -ErrorAction SilentlyContinue

$script:TestsPassed = 0
$script:TestsFailed = 0
function Check($name, $ok, $detail) {
    if ($ok) { $script:TestsPassed++; Write-Host "  [PASS] $name  $detail" -ForegroundColor Green }
    else { $script:TestsFailed++; Write-Host "  [FAIL] $name  $detail" -ForegroundColor Red }
}
function P { & $exe -L $ns @args 2>$null }

# A two colour 16 by 12 pixel sixel: two bands of six rows.
$sixelBody = 'Pq#1;2;100;100;0#2;2;0;100;0#1~~@@vv@@~~@@~~$#2??}}GG}}??}}??-#1@@@@@@@@@@@@@@'
$sixelBytes = [byte[]]@(0x1B) + [Text.Encoding]::ASCII.GetBytes($sixelBody) + [byte[]]@(0x1B, 0x5C)
# Two line feeds after it: the cursor stays on the row of the image's last
# band, and text written there erases the image cells under it, as in
# Windows Terminal.
$emit = "[Console]::Out.Write([char]27 + '$sixelBody' + [char]27 + '\' + [char]10 + [char]10); 'EMITTED'"

function WaitSession($s) {
    for ($i = 0; $i -lt 60; $i++) {
        P has-session -t $s | Out-Null
        if ($LASTEXITCODE -eq 0) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

function WaitText($t, $text) {
    for ($i = 0; $i -lt 40; $i++) {
        if (((P capture-pane -t $t -p) -join "`n") -match $text) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

# One dump-state frame over the session's TCP port, parsed.
function Get-Dump($s) {
    $port = (Get-Content (Join-Path $data "${ns}__$s.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $data "${ns}__$s.key") -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port)
    try {
        $tcp.ReceiveTimeout = 4000
        $st = $tcp.GetStream()
        $w = [IO.StreamWriter]::new($st); $r = [IO.StreamReader]::new($st)
        $w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
        $w.Write("dump-state`n"); $w.Flush()
        for ($j = 0; $j -lt 300; $j++) {
            $line = $r.ReadLine()
            if ($null -eq $line) { break }
            if ($line -ne 'NC' -and $line.StartsWith('{')) { return ($line | ConvertFrom-Json -AsHashtable) }
        }
    } catch {
    } finally {
        $tcp.Close()
    }
    return $null
}

function Get-Leaves($node) {
    if ($null -eq $node) { return @() }
    if ($node['type'] -eq 'leaf') { return @(, $node) }
    $out = @()
    foreach ($c in $node['children']) { $out += Get-Leaves $c }
    return $out
}

function Blob-Bytes($dump, $id) {
    $blobs = $dump['image_blobs']
    if ($null -eq $blobs -or -not $blobs.ContainsKey("$id")) { return $null }
    return [Convert]::FromBase64String($blobs["$id"])
}

try {
    Write-Host "`n=== Issue #431: sixel through PSMUX_CONPTY_DIR ($conptyDir) ===" -ForegroundColor Cyan

    P new-session -d -s a -x 100 -y 30 'pwsh -NoProfile -NoLogo' | Out-Null
    if (-not (WaitSession 'a')) { Check 'session starts' $false ''; throw 'no session' }
    Start-Sleep -Seconds 3

    Write-Host "`n[1] one pane" -ForegroundColor Yellow
    P send-keys -t a -l $emit | Out-Null
    P send-keys -t a Enter | Out-Null
    $null = WaitText 'a' 'EMITTED'
    Start-Sleep -Milliseconds 500
    P refresh-client -t a | Out-Null
    $d = Get-Dump 'a'
    Check 'dump-state answers' ($null -ne $d) ''
    $imgs = @((Get-Leaves $d['layout']) | ForEach-Object { $_['images'] } | Where-Object { $_ })
    Check 'the leaf has one image' ($imgs.Count -eq 1) "count=$($imgs.Count)"
    if ($imgs.Count -lt 1) {
        Check 'the image is 2 by 2 cells' $false 'no image'
        Check 'image_blobs ships the id' $false 'no image'
        Check 'the blob is the DCS the pane wrote, bounded' $false 'no image'
    } else {
        $id = $imgs[0]['id']
        # 14 sixel columns, and two bands of 6 pixels at the default 2:1
        # ratio, so 24 pixels: 10 by 20 pixel cells make that 2 by 2.
        Check 'the image is 2 by 2 cells' ($imgs[0]['cw'] -eq 2 -and $imgs[0]['ch'] -eq 2) "cw=$($imgs[0]['cw']) ch=$($imgs[0]['ch'])"
        $blob = Blob-Bytes $d $id
        Check 'image_blobs ships the id' ($null -ne $blob) "id=$id"
        if ($null -eq $blob) {
            Check 'the blob is the DCS the pane wrote, bounded' $false 'no blob'
        } else {
            # The parser rebuilds the introducer from its parameters, and an
            # empty list comes back as the default 0 (ESC P 0 q).  After the
            # q it puts a raster command with the image's size (14 columns, two
            # bands of 12 pixels at the default 2:1 ratio), so the background
            # stops at the image, then the data the pane wrote.
            $text = [Text.Encoding]::ASCII.GetString($blob)
            $want = [string][char]27 + 'P0q"2;1;14;24' + $sixelBody.Substring(2) + [char]27 + '\'
            Check 'the blob is the DCS the pane wrote, bounded' ($text -ceq $want) "$($blob.Length) bytes, wrote $($sixelBytes.Length)"
        }
    }

    Write-Host "`n[2] the text after the image stays on the host's rows" -ForegroundColor Yellow
    # OpenConsole leaves the cursor in its column on the image's last band
    # row.  When psmux moved it below the image instead, its copy of the pane
    # ran one row lower than the host, and the next command line the host
    # redrew landed over the prompt (`echo MARK` on the row above, `PS` gone).
    P send-keys -t a -l 'echo MARK431' | Out-Null
    P send-keys -t a Enter | Out-Null
    $null = WaitText 'a' '(?m)^MARK431'
    $lines = @(P capture-pane -t a -p)
    $cmd = @($lines | Where-Object { $_ -match 'echo MARK431' })
    Check 'the command line keeps its prompt' ($cmd.Count -eq 1 -and $cmd[0] -match '^PS .*> echo MARK431\s*$') "line='$($cmd -join '|')'"
    Check 'the output follows on the next row' (($lines | Where-Object { $_ -match '^MARK431\s*$' }).Count -eq 1) ''

    Write-Host "`n[3] two panes, one image each" -ForegroundColor Yellow
    P new-session -d -s b -x 100 -y 30 'pwsh -NoProfile -NoLogo' | Out-Null
    if (-not (WaitSession 'b')) { Check 'second session starts' $false ''; throw 'no session b' }
    P split-window -t b -h 'pwsh -NoProfile -NoLogo' | Out-Null
    Start-Sleep -Seconds 3
    foreach ($t in 'b.0', 'b.1') {
        P send-keys -t $t -l $emit | Out-Null
        P send-keys -t $t Enter | Out-Null
        $null = WaitText $t 'EMITTED'
    }
    Start-Sleep -Milliseconds 500
    P refresh-client -t b | Out-Null
    $d = Get-Dump 'b'
    $leaves = @(Get-Leaves $d['layout'])
    Check 'two leaves' ($leaves.Count -eq 2) "count=$($leaves.Count)"
    $ids = @($leaves | ForEach-Object { @($_['images']) | Where-Object { $_ } | ForEach-Object { $_['id'] } })
    Check 'each pane has its image' ($ids.Count -eq 2) "ids=$($ids -join ',')"
    Check 'the two images have different ids' (($ids | Select-Object -Unique).Count -eq 2) "ids=$($ids -join ',')"
    $shipped = @($ids | Select-Object -Unique | Where-Object { $null -ne (Blob-Bytes $d $_) })
    Check 'both blobs are shipped' ($shipped.Count -eq 2) "shipped=$($shipped -join ',')"
} finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    $env:PSMUX_DATA_DIR = $oldData
    $env:PSMUX_CONFIG_FILE = $oldConf
    if ($oldConpty) { $env:PSMUX_CONPTY_DIR = $oldConpty } else { Remove-Item Env:\PSMUX_CONPTY_DIR -ErrorAction SilentlyContinue }
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
}

Write-Host "`nResults: $($script:TestsPassed) passed, $($script:TestsFailed) failed"
exit $script:TestsFailed
