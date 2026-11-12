param([string]$Cap = '99', [string]$Surge2 = '', [int]$N = 3, [string]$Bin = 'C:\Users\godwin\Documents\workspace\psmux-surge\bcap\psmux.exe')
$env:PSMUX_DBG_BOOT_CAP = $Cap
if ($Surge2) { $env:PSMUX_DBG_SURGE2 = '1' } else { $env:PSMUX_DBG_SURGE2 = $null }
$data = Join-Path $env:TEMP 'miss661'
New-Item -ItemType Directory -Force $data | Out-Null
$env:PSMUX_DATA_DIR = $data
for ($i = 1; $i -le $N; $i++) {
  $o = pwsh -NoProfile -File C:\Users\godwin\Documents\workspace\psmux-pool661\repro661.ps1 -Binary $Bin 2>&1
  ($o | Select-String 'INFO\] p50').Line
  $m = [regex]::Match(($o -join "`n"), 'TRACE=([^ ]+) ')
  $tr = $m.Groups[1].Value
  if (-not (Test-Path $tr)) { "no trace"; continue }
  $t = Get-Content $tr
  $cl = @($t | Where-Object { $_ -match 'claim\(split\): pane=' })
  $srvPid = ([regex]::Match($cl[0], 'pid=(\d+)')).Groups[1].Value
  $srv = @($t | Where-Object { $_ -match "pid=$srvPid\]" })
  $first = [double]([regex]::Match($cl[0], '^\[\s*([\d.]+)').Groups[1].Value)
  for ($k = 0; $k -lt $srv.Count; $k++) {
    if ($srv[$k] -match 'NO READY') {
      $ts = [double]([regex]::Match($srv[$k], '^\[\s*([\d.]+)').Groups[1].Value)
      $j = $k + 1; while ($j -lt $srv.Count -and $srv[$j] -notmatch 'claim\((split|new-window)\): pane=') { $j++ }
      $next = $srv[$j] -replace '^.*claim\(split\): ', '' -replace 'last="PS [^"]*"', 'last=PROMPT' -replace ' inflight=.*', ''
      "  miss at +$([int]($ts - $first)) ms: $next"
    }
  }
  [IO.File]::Delete($tr)
}
$env:PSMUX_DBG_BOOT_CAP = $null; $env:PSMUX_DBG_SURGE2 = $null
[IO.Directory]::Delete($data, $true)
