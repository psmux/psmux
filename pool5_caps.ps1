param([int]$N = 10, [string]$Caps = '5,4')
$w = 'C:\Users\godwin\Documents\workspace\psmux-surge'
$bin = "$w\bcap\psmux.exe"
$res = @{}
foreach ($c in $Caps.Split(',')) { $res[$c] = 0 }
for ($i = 1; $i -le $N; $i++) {
  foreach ($cap in $Caps.Split(',')) {
    $env:PSMUX_DBG_BOOT_CAP = $cap
    $o = pwsh -NoProfile -File "$w\repro_pool5.ps1" -Exe $bin -Ns "q$cap$i" 2>&1 | Select-String 'new-window@5'
    $v = [regex]::Match($o.Line, 'new-window@5: \[([^\]]*)').Groups[1].Value
    $slow = @($v.Split(',') | Where-Object { $_.Trim() -and [int]$_.Trim() -gt 100 }).Count
    if ($slow -gt 0) { $res[$cap]++ }
    "cap=$cap run$i [$v] slow=$slow"
  }
}
$env:PSMUX_DBG_BOOT_CAP = $null
foreach ($c in $Caps.Split(',')) { "cap $c runs with a slow window: $($res[$c])/$N" }
