param([string]$Exe)
$ErrorActionPreference = 'Continue'
$tag = "rv909b_$([IO.Path]::GetFileNameWithoutExtension($Exe))"
$env:PSMUX_DATA_DIR = "$env:TEMP\$tag"
Remove-Item -Recurse -Force $env:PSMUX_DATA_DIR -EA SilentlyContinue
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_SERVER_MODE = 'shared'
$L = $tag
& $Exe -L $L new-session -d -s keep 2>&1 | Out-Null
& $Exe -L $L new-session -d -s vic 2>&1 | Out-Null
Start-Sleep -Milliseconds 1500
foreach ($c in @(
    @('resize-window', '-t', 'vic', '-x', '65535', '-y', '65535'),
    @('new-window', '-t', 'vic', '-x', '65535', '-y', '65535'),
    @('refresh-client', '-t', 'vic', '-C', '65535x65535'))) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $out = & $Exe -L $L @c 2>&1 | Out-String
    $rc = $LASTEXITCODE
    Start-Sleep -Milliseconds 800
    & $Exe -L $L has-session -t keep 2>$null; $keep = $LASTEXITCODE
    "  {0}: rc={1} out='{2}' keep_alive={3} ({4} ms) size={5}" -f ($c -join ' '), $rc, $out.Trim(), ($keep -eq 0), $sw.ElapsedMilliseconds, ((& $Exe -L $L display -t vic -p '#{window_width}x#{window_height}' 2>&1 | Out-String).Trim())
}
& $Exe -L $L kill-server 2>&1 | Out-Null
