param([string]$Cap = '4', [int]$N = 3, [int]$Burners = 16, [string]$Bin = 'C:\Users\godwin\Documents\workspace\psmux-surge\bcap\psmux.exe')
$w = 'C:\Users\godwin\Documents\workspace\psmux-surge'
$burn = @()
try {
  for ($i = 0; $i -lt $Burners; $i++) { $burn += Start-Process pwsh -ArgumentList '-NoProfile', '-Command', 'while($true){$x=1}' -WindowStyle Hidden -PassThru }
  if ($Burners) { Start-Sleep -Seconds 2 }
  $env:PSMUX_DBG_BOOT_CAP = $Cap
  $data = Join-Path $env:TEMP 'grp686'
  New-Item -ItemType Directory -Force $data | Out-Null
  $env:PSMUX_DATA_DIR = $data
  for ($r = 1; $r -le $N; $r++) {
    $since = Get-Date
    $o = pwsh -NoProfile -File "$w\tests\test_issue686_pool_surge_and_reap.ps1" -Binary $Bin -LeakRounds 1 2>&1
    $fails = @($o | Select-String '\[FAIL\]' | ForEach-Object { $_.Line.Trim() }) -join ' ; '
    $tr = Get-ChildItem $env:TEMP -Filter 'psmux_i686_*.log' | Where-Object { $_.LastWriteTime -ge $since } | Sort-Object LastWriteTime | Select-Object -Last 1
    $spawns = @()
    if ($tr) {
      $lines = Get-Content $tr.FullName
      $srv = ([regex]::Match(($lines | Where-Object { $_ -match 'claim\(new-window\)' } | Select-Object -First 1), 'pid=(\d+)')).Groups[1].Value
      foreach ($l in ($lines | Where-Object { $_ -match "pid=$srv\]" })) {
        if ($l -match '^\[\s*([\d\.]+)\s+pid=\d+\]\s+pool: spawned spare pane=(\d+).* in ([\d\.]+)ms') { $spawns += [pscustomobject]@{ Pane = [int]$Matches[2]; Start = [double]$Matches[1] - [double]$Matches[3] } }
      }
      [IO.File]::Delete($tr.FullName)
    }
    $max = 0
    foreach ($a in $spawns) { $g = @($spawns | Where-Object { [math]::Abs($_.Start - $a.Start) -le 30 }).Count; if ($g -gt $max) { $max = $g } }
    $starts = ($spawns | Sort-Object Start | ForEach-Object { "$($_.Pane)@$([int]$_.Start)" }) -join ' '
    "  starts: $starts"
    "cap=$Cap load=$Burners run$r exit=$LASTEXITCODE spawns=$($spawns.Count) largest group started within 30ms=$max $fails"
  }
} finally {
  foreach ($b in $burn) { try { Stop-Process -Id $b.Id -Force } catch {} }
  $env:PSMUX_DBG_BOOT_CAP = $null
  Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $Bin } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
  [IO.Directory]::Delete((Join-Path $env:TEMP 'grp686'), $true)
}
