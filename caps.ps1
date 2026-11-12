param([int]$N = 6, [string]$Caps = '4,6,99', [string]$Out = 'C:\Users\godwin\Documents\workspace\psmux-surge\caps.txt')
$w = 'C:\Users\godwin\Documents\workspace\psmux-surge'
$bin = "$w\bcap\psmux.exe"
"" | Set-Content $Out
for ($r = 1; $r -le $N; $r++) {
  foreach ($cap in $Caps.Split(',')) {
    $env:PSMUX_DBG_BOOT_CAP = $cap
    $data = "$env:TEMP\caps-$cap"
    New-Item -ItemType Directory -Force $data | Out-Null
    $env:PSMUX_DATA_DIR = $data
    $env:PSMUX_TEST_BIN = $bin
    $o = pwsh -NoProfile -File "$w\tests\test_issue661_warm_pool_depth_holds.ps1" -Binary $bin 2>&1
    $slow = ([regex]::Match(($o -join "`n"), '(\d+) of 24 splits over 150ms')).Groups[1].Value
    $lat = ($o | Select-String 'INFO\] p50').Line -replace '^\[INFO\] ', ''
    $f661 = @($o | Select-String '\[FAIL\]' | ForEach-Object { $_.Line.Trim() }) -join ' ; '
    $l1 = "r$r cap=$cap 661 exit=$LASTEXITCODE slow=$slow $lat $f661"
    $o = pwsh -NoProfile -File "$w\tests\test_issue686_pool_surge_and_reap.ps1" -Binary $bin -LeakRounds 1 2>&1
    $per = ($o | Select-String 'per call:').Line -replace '^\[INFO\] ', ''
    $f686 = @($o | Select-String '\[FAIL\]' | ForEach-Object { $_.Line.Trim() }) -join ' ; '
    $l2 = "r$r cap=$cap 686 exit=$LASTEXITCODE $per $f686"
    $p5 = pwsh -NoProfile -File "$w\repro_pool5.ps1" -Exe $bin -Ns "p5c$cap$r" 2>&1 | Select-String 'new-window@5'
    $l3 = "r$r cap=$cap pool5 $($p5.Line)"
    $l1, $l2, $l3 | Add-Content $Out
    $l1; $l2; $l3
    Start-Sleep -Milliseconds 1500
    Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $bin } | ForEach-Object { "    LEFTOVER pid=$($_.ProcessId) $($_.CommandLine)" | Add-Content $Out; Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
    Remove-Item -Recurse -Force $data -EA SilentlyContinue
  }
}
Remove-Item Env:PSMUX_DBG_BOOT_CAP -EA SilentlyContinue
