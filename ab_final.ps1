param([int]$N = 6, [int]$Burners = 0, [switch]$NoPool5, [string]$Sides = 'now,fix', [string]$Out = 'C:\Users\godwin\Documents\workspace\psmux-surge\ab_final.txt')
$w = 'C:\Users\godwin\Documents\workspace\psmux-surge'
$bins = @{ before = 'C:\Users\godwin\Documents\workspace\psmux-pool661\binm\psmux.exe'; now = "$w\bnow\psmux.exe"; fix = "$w\bfix\psmux.exe" }
foreach ($n in 'PSMUX_DBG_BOOT_CAP', 'PSMUX_DBG_SURGE2', 'PSMUX_DBG_YOUNG_MS') { [Environment]::SetEnvironmentVariable($n, $null, 'Process') }
"" | Set-Content $Out
$burn = @()
try {
  for ($i = 0; $i -lt $Burners; $i++) { $burn += Start-Process pwsh -ArgumentList '-NoProfile', '-Command', 'while($true){$x=1}' -WindowStyle Hidden -PassThru }
  if ($Burners) { Start-Sleep -Seconds 2 }
  for ($r = 1; $r -le $N; $r++) {
    foreach ($side in $Sides.Split(',')) {
      $bin = $bins[$side]
      $data = Join-Path $env:TEMP "abf-$side"
      New-Item -ItemType Directory -Force $data | Out-Null
      $env:PSMUX_DATA_DIR = $data
      $env:PSMUX_TEST_BIN = $bin
      $tag = "r$r $side load=$Burners"
      $o = pwsh -NoProfile -File "$w\tests\test_issue661_warm_pool_depth_holds.ps1" -Binary $bin 2>&1
      $slow = ([regex]::Match(($o -join "`n"), '(\d+) of 24 splits over 150ms')).Groups[1].Value
      $lat = ($o | Select-String 'INFO\] p50').Line -replace '^\[INFO\] p50=[^[]*', ''
      $f = @($o | Select-String '\[FAIL\]' | ForEach-Object { $_.Line.Trim() }) -join ' ; '
      $l = "$tag 661 exit=$LASTEXITCODE slow=$slow $lat $f"; $l | Add-Content $Out; $l
      $o = pwsh -NoProfile -File "$w\tests\test_issue686_pool_surge_and_reap.ps1" -Binary $bin -LeakRounds 1 2>&1
      $per = ($o | Select-String 'per call:').Line -replace '^\[INFO\] ', ''
      $f = @($o | Select-String '\[FAIL\]' | ForEach-Object { $_.Line.Trim() }) -join ' ; '
      $l = "$tag 686 exit=$LASTEXITCODE $per $f"; $l | Add-Content $Out; $l
      if (-not $NoPool5) {
        $p5 = pwsh -NoProfile -File "$w\repro_pool5.ps1" -Exe $bin -Ns "af$side$r" 2>&1 | Select-String 'new-window@5'
        $l = "$tag pool5 $($p5.Line -replace 'settled=True after \d+ ms ', '')"; $l | Add-Content $Out; $l
      }
      Start-Sleep -Milliseconds 1500
      Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $bin } | ForEach-Object { "    LEFTOVER pid=$($_.ProcessId)" | Add-Content $Out; Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
      [IO.Directory]::Delete($data, $true)
    }
  }
} finally {
  foreach ($b in $burn) { try { Stop-Process -Id $b.Id -Force } catch {} }
}
