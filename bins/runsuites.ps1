param([string]$Bin, [string[]]$Suites, [string]$Out)
$exe = (Resolve-Path "$Bin\psmux.exe").Path
$results = @()
foreach ($s in $Suites) {
    $home2 = Join-Path $env:TEMP ("s1run_" + [guid]::NewGuid().ToString('N').Substring(0, 6))
    New-Item -ItemType Directory -Force "$home2\.psmux" | Out-Null
    $psi = [Diagnostics.ProcessStartInfo]::new('pwsh', "-NoProfile -File `"$s`"")
    $psi.UseShellExecute = $false; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $psi.WorkingDirectory = (Split-Path $s -Parent | Split-Path -Parent)
    $psi.Environment['PATH'] = (Resolve-Path $Bin).Path + ';' + $env:PATH
    foreach ($k in 'PSMUX_EXE', 'PSMUX_TEST_BIN', 'PSMUX_TEST_EXE', 'PSMUX_BIN', 'PSMUX') { $psi.Environment[$k] = $exe }
    $psi.Environment['USERPROFILE'] = $home2
    $psi.Environment['PSMUX_DATA_DIR'] = "$home2\.psmux"
    $p = [Diagnostics.Process]::Start($psi)
    $so = $p.StandardOutput.ReadToEndAsync(); $se = $p.StandardError.ReadToEndAsync()
    if (-not $p.WaitForExit(300000)) { try { $p.Kill($true) } catch {}; $code = 'TIMEOUT' } else { $code = $p.ExitCode }
    $text = $so.Result + $se.Result
    $name = Split-Path $s -Leaf
    Set-Content -Path (Join-Path $Out "$name.log") -Value $text
    $fails = @($text -split "`n" | Where-Object { $_ -match '\[FAIL\]|FAIL:' } | Select-Object -First 4)
    $results += "{0,-55} exit={1} {2}" -f $name, $code, ($fails -join ' || ')
    Remove-Item -Recurse -Force $home2 -EA SilentlyContinue
}
$results
