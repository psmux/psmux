param([int]$N = 4, [string]$Caps = '4,5', [int]$Burners = 16, [string]$Out = 'C:\Users\godwin\Documents\workspace\psmux-surge\loaded.txt')
$w = 'C:\Users\godwin\Documents\workspace\psmux-surge'
$bin = "$w\bcap\psmux.exe"
"" | Set-Content $Out
$burn = @()
try {
  for ($i = 0; $i -lt $Burners; $i++) { $burn += Start-Process pwsh -ArgumentList '-NoProfile', '-Command', 'while($true){$x=1}' -WindowStyle Hidden -PassThru }
  Start-Sleep -Seconds 2
  for ($r = 1; $r -le $N; $r++) {
    foreach ($cap in $Caps.Split(',')) {
      $env:PSMUX_DBG_BOOT_CAP = $cap
      $data = "$env:TEMP\loaded-$cap"
      New-Item -ItemType Directory -Force $data | Out-Null
      $env:PSMUX_DATA_DIR = $data
      $env:PSMUX_TEST_BIN = $bin
      $o = pwsh -NoProfile -File "$w\tests\test_issue686_pool_surge_and_reap.ps1" -Binary $bin -LeakRounds 1 2>&1
      $per = ($o | Select-String 'per call:').Line -replace '^\[INFO\] ', ''
      $grp = ($o | Select-String 'worst group').Line -replace '^\[INFO\] ', ''
      $f686 = @($o | Select-String '\[FAIL\]' | ForEach-Object { $_.Line.Trim() }) -join ' ; '
      $l2 = "r$r cap=$cap 686 exit=$LASTEXITCODE $per | $grp | $f686"
      $o = pwsh -NoProfile -File "$w\tests\test_issue661_warm_pool_depth_holds.ps1" -Binary $bin 2>&1
      $slow = ([regex]::Match(($o -join "`n"), '(\d+) of 24 splits over 150ms')).Groups[1].Value
      $f661 = @($o | Select-String '\[FAIL\]' | ForEach-Object { $_.Line.Trim() }) -join ' ; '
      $l1 = "r$r cap=$cap 661 exit=$LASTEXITCODE slow=$slow $f661"
      $l2, $l1 | Add-Content $Out
      $l2; $l1
      Start-Sleep -Milliseconds 1500
      Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $bin } | ForEach-Object { "    LEFTOVER pid=$($_.ProcessId)" | Add-Content $Out; Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
      Remove-Item -Recurse -Force $data -EA SilentlyContinue
    }
  }
} finally {
  foreach ($b in $burn) { try { Stop-Process -Id $b.Id -Force } catch {} }
  $env:PSMUX_DBG_BOOT_CAP = $null
}
