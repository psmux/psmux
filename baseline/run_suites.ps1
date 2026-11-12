param([string]$Exe, [string[]]$Suites)
$wt = "C:\Users\godwin\Documents\workspace\psmux\.claude\worktrees\agent-ad44d5289fdea1c2f"
$realHome = $env:USERPROFILE
$before = (& "$realHome\.cargo\bin\psmux.exe" ls 2>&1 | Out-String).Trim()
$results = @()
foreach ($s in $Suites) {
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("p912s_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force $tmp | Out-Null
    $env:USERPROFILE = $tmp
    $env:PSMUX_DATA_DIR = (Join-Path $tmp ".psmux")
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
    $env:PSMUX_TEST_BIN = $Exe
    $env:PSMUX_TEST_EXE = $Exe
    $env:PSMUX_EXE = $Exe
    $env:PATH = (Split-Path $Exe) + ";" + $env:PATH
    $tag = if ($Exe -like '*psmux_master*') { 'master' } else { 'fix' }
    $log = Join-Path $wt "baseline\$s.$tag.log"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    pwsh -NoProfile -File (Join-Path $wt "tests\$s.ps1") *> $log
    $code = $LASTEXITCODE
    $env:USERPROFILE = $realHome
    $fails = @(Select-String -Path $log -Pattern '\[FAIL\]|^\s*FAIL:' | ForEach-Object { $_.Line.Trim() })
    $passes = @(Select-String -Path $log -Pattern '\[PASS\]|^\s*PASS:').Count
    $results += "{0,-45} exit={1} pass={2} fail={3} {4:n0}s" -f $s, $code, $passes, $fails.Count, $sw.Elapsed.TotalSeconds
    $fails | Select-Object -First 5 | ForEach-Object { $results += "      $_" }
    Remove-Item -Recurse -Force $tmp -EA SilentlyContinue
}
$env:USERPROFILE = $realHome
$env:PSMUX_DATA_DIR = $null
$after = (& "$realHome\.cargo\bin\psmux.exe" ls 2>&1 | Out-String).Trim()
$results
"default ls before: $before"
"default ls after:  $after"
