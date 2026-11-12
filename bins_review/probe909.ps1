param([string]$Exe, [string]$Mode = 'shared')
$ErrorActionPreference = 'Continue'
$tag = "rv909_$([IO.Path]::GetFileNameWithoutExtension($Exe))_$Mode"
$env:PSMUX_DATA_DIR = "$env:TEMP\$tag"
Remove-Item -Recurse -Force $env:PSMUX_DATA_DIR -EA SilentlyContinue
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
if ($Mode -eq 'shared') { $env:PSMUX_SERVER_MODE = 'shared' } else { Remove-Item env:PSMUX_SERVER_MODE -EA SilentlyContinue }
$L = $tag
& $Exe -L $L new-session -d -s keep 2>&1 | Out-Null
Start-Sleep -Milliseconds 1500
"[$Mode $Exe]"
foreach ($sz in @(@(2048,2048), @(2049,2048), @(3000,3000), @(65535,65535))) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $out = & $Exe -L $L new-session -d -s "big$($sz[0])" -x $sz[0] -y $sz[1] 2>&1 | Out-String
    $rc = $LASTEXITCODE
    Start-Sleep -Milliseconds 800
    & $Exe -L $L has-session -t keep 2>$null; $keep = $LASTEXITCODE
    & $Exe -L $L has-session -t "big$($sz[0])" 2>$null; $big = $LASTEXITCODE
    "  {0}x{1}: rc={2} out='{3}' keep_alive={4} big_exists={5} ({6} ms)" -f $sz[0], $sz[1], $rc, $out.Trim(), ($keep -eq 0), ($big -eq 0), $sw.ElapsedMilliseconds
    if ($big -eq 0) { & $Exe -L $L kill-session -t "big$($sz[0])" 2>&1 | Out-Null }
}
& $Exe -L $L kill-server 2>&1 | Out-Null
