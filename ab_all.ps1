param([int]$N = 5, [string]$Sides = 'bc2c,fix', [int]$Burners = 0, [string]$Suites = 'gate,661,686,spf', [string]$Out = 'C:\Users\godwin\Documents\workspace\psmux-cgate\ab_all.txt')
$w = 'C:\Users\godwin\Documents\workspace\psmux-cgate'
$bins = @{
  pre661 = 'C:\Users\godwin\Documents\workspace\psmux-pool661\binm\psmux.exe'
  e364   = 'C:\Users\godwin\Documents\workspace\psmux-surge\bnow\psmux.exe'
  bc2c   = 'C:\Users\godwin\Documents\workspace\psmux-surge\bfix\psmux.exe'
  fix    = "$w\bfix\psmux.exe"
}
"" | Set-Content $Out
$burn = @()
try {
  for ($i = 0; $i -lt $Burners; $i++) { $burn += Start-Process pwsh -ArgumentList '-NoProfile', '-Command', 'while($true){$x=1}' -WindowStyle Hidden -PassThru }
  if ($Burners) { Start-Sleep -Seconds 2 }
  for ($r = 1; $r -le $N; $r++) {
    foreach ($side in $Sides.Split(',')) {
      $bin = $bins[$side]
      foreach ($suite in $Suites.Split(',')) {
        $data = Join-Path $env:TEMP "aba-$side"
        New-Item -ItemType Directory -Force $data | Out-Null
        $env:PSMUX_DATA_DIR = $data
        $env:PSMUX_TEST_BIN = $bin
        switch ($suite) {
          'gate' { $o = pwsh -NoProfile -File "$w\tests\test_creation_latency_gate.ps1" -Binary $bin 2>&1
                   $info = @($o | Select-String '\[PERF\] (new-window|split-window -v|split-window -h|split -f) ' | ForEach-Object { $_.Line -replace '^\[PERF\] ', '' -replace 'med=[^\[]*', '' }) -join ' | ' }
          '661'  { $o = pwsh -NoProfile -File "$w\tests\test_issue661_warm_pool_depth_holds.ps1" -Binary $bin 2>&1
                   $info = ($o | Select-String 'INFO\] p50').Line -replace '^\[INFO\] p50=[^\[]*', '' }
          '686'  { $o = pwsh -NoProfile -File "$w\tests\test_issue686_pool_surge_and_reap.ps1" -Binary $bin -LeakRounds 1 2>&1
                   $info = ($o | Select-String 'per call:').Line -replace '^\[INFO\] ', '' }
          'spf'  { $o = pwsh -NoProfile -File "$w\tests\spf_ns.ps1" -Psmux $bin 2>&1
                   $info = @($o | Select-String '@ warm-pool-size 5  |burst of 5' | ForEach-Object { $_.Line.Trim() -replace 'med=.*\[', '[' }) -join ' | ' }
        }
        $code = $LASTEXITCODE
        $fails = @($o | Select-String '(\[FAIL\]|FAIL:)' | ForEach-Object { $_.Line.Trim() -replace '^(\[FAIL\]|FAIL:) ?', '' }) -join ' ; '
        $l = "r$r $side load=$Burners $suite exit=$code | $info | FAIL: $fails"
        $l | Add-Content $Out; $l
        Start-Sleep -Milliseconds 1500
        Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $bin } | ForEach-Object { "    LEFTOVER pid=$($_.ProcessId) $($_.CommandLine)" | Add-Content $Out; Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
        [IO.Directory]::Delete($data, $true)
      }
    }
  }
} finally {
  foreach ($b in $burn) { try { Stop-Process -Id $b.Id -Force } catch {} }
}
