param([int]$N = 5, [string]$Sides = 'pre661,e364,bc2c', [string]$Out = 'C:\Users\godwin\Documents\workspace\psmux-cgate\ab_gate.txt')
$w = 'C:\Users\godwin\Documents\workspace\psmux-cgate'
$bins = @{
  pre661 = 'C:\Users\godwin\Documents\workspace\psmux-pool661\binm\psmux.exe'
  e364   = 'C:\Users\godwin\Documents\workspace\psmux-surge\bnow\psmux.exe'
  bc2c   = 'C:\Users\godwin\Documents\workspace\psmux-surge\bfix\psmux.exe'
  fix    = "$w\bfix\psmux.exe"
}
"" | Set-Content $Out
for ($r = 1; $r -le $N; $r++) {
  foreach ($side in $Sides.Split(',')) {
    $bin = $bins[$side]
    $data = Join-Path $env:TEMP "abg-$side"
    New-Item -ItemType Directory -Force $data | Out-Null
    $env:PSMUX_DATA_DIR = $data
    $o = pwsh -NoProfile -File "$w\tests\test_creation_latency_gate.ps1" -Binary $bin 2>&1
    $code = $LASTEXITCODE
    $cells = @($o | Select-String '\[PERF\] (new-window|split-window -v|split-window -h|split -f) ' | ForEach-Object { ($_.Line -replace '^\[PERF\] ', '' -replace 'med=.*slow=', 'slow=' -replace 'med=.*max=[^\[]*', '') })
    $fails = @($o | Select-String '\[FAIL\]' | ForEach-Object { $_.Line.Trim() -replace '^\[FAIL\] ', '' }) -join ' ; '
    $l = "r$r $side exit=$code | " + ($cells -join ' | ') + " | FAIL: $fails"
    $l | Add-Content $Out; $l
    Start-Sleep -Milliseconds 1500
    Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $bin } | ForEach-Object { "    LEFTOVER pid=$($_.ProcessId)" | Add-Content $Out; Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
    [IO.Directory]::Delete($data, $true)
  }
}
