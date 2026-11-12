param([int]$N = 5, [string]$Sides = 'before,now', [string]$Out = 'C:\Users\godwin\Documents\workspace\psmux-surge\ab_surge.txt')
$w = 'C:\Users\godwin\Documents\workspace\psmux-surge'
$bins = @{
  before = 'C:\Users\godwin\Documents\workspace\psmux-pool661\binm\psmux.exe'
  now    = "$w\bnow\psmux.exe"
  fix    = "$w\bfix\psmux.exe"
}
"" | Set-Content $Out
for ($r = 1; $r -le $N; $r++) {
  foreach ($side in $Sides.Split(',')) {
    $bin = $bins[$side]
    $data = "$env:TEMP\absurge-$side"
    New-Item -ItemType Directory -Force $data | Out-Null
    $env:PSMUX_DATA_DIR = $data
    $env:PSMUX_TEST_BIN = $bin
    $o = pwsh -NoProfile -File "$w\tests\test_issue686_pool_surge_and_reap.ps1" -Binary $bin -LeakRounds 1 2>&1
    $code = $LASTEXITCODE
    $per = ($o | Select-String 'per call:').Line
    $miss = ($o | Select-String 'calls found no ready spare').Line
    $fails = @($o | Select-String '\[FAIL\]' | ForEach-Object { $_.Line.Trim() }) -join ' ; '
    $l1 = "r$r $side 686 exit=$code $per | $miss | $fails"
    $p5 = pwsh -NoProfile -File "$w\repro_pool5.ps1" -Exe $bin -Ns "p5$side$r" 2>&1 | Select-String 'new-window@5'
    $l2 = "r$r $side pool5 $($p5.Line)"
    $l1, $l2 | Add-Content $Out
    $l1; $l2
    Start-Sleep -Milliseconds 1500
    Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $bin } | ForEach-Object { "    LEFTOVER pid=$($_.ProcessId) $($_.CommandLine)" | Add-Content $Out; Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
    Remove-Item -Recurse -Force $data -EA SilentlyContinue
  }
}
