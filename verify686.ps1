param([int]$N = 4, [int]$Burners = 0, [string]$Sides = 'now,before')
$w = 'C:\Users\godwin\Documents\workspace\psmux-surge'
$bins = @{ now = "$w\bnow\psmux.exe"; before = 'C:\Users\godwin\Documents\workspace\psmux-pool661\binm\psmux.exe' }
$burn = @()
try {
  for ($i = 0; $i -lt $Burners; $i++) { $burn += Start-Process pwsh -ArgumentList '-NoProfile', '-Command', 'while($true){$x=1}' -WindowStyle Hidden -PassThru }
  if ($Burners) { Start-Sleep -Seconds 2 }
  for ($r = 1; $r -le $N; $r++) {
    foreach ($side in $Sides.Split(',')) {
      $bin = $bins[$side]
      $data = Join-Path $env:TEMP "v686-$side"
      New-Item -ItemType Directory -Force $data | Out-Null
      $env:PSMUX_DATA_DIR = $data
      $o = pwsh -NoProfile -File "$w\tests\test_issue686_pool_surge_and_reap.ps1" -Binary $bin 2>&1
      $code = $LASTEXITCODE
      $grp = ($o | Select-String 'worst group').Line -replace '^\[INFO\] worst group by psmux.s own share ', ''
      $res = @($o | Select-String '\[(PASS|FAIL)\]' | ForEach-Object { if ($_.Line -match '\[PASS\]') { 'P' } else { 'F' } }) -join ''
      $f = @($o | Select-String '\[FAIL\]' | ForEach-Object { $_.Line.Trim() }) -join ' ; '
      "r$r $side load=$Burners exit=$code results=$res $f"
      "    $grp"
      Start-Sleep -Milliseconds 1500
      Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $bin } | ForEach-Object { "    LEFTOVER pid=$($_.ProcessId)"; Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
      [IO.Directory]::Delete($data, $true)
    }
  }
} finally {
  foreach ($b in $burn) { try { Stop-Process -Id $b.Id -Force } catch {} }
}
