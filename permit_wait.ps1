param([string]$Bin = 'C:\Users\godwin\Documents\workspace\psmux-surge\bfix\psmux.exe', [int]$N = 2)
$w = 'C:\Users\godwin\Documents\workspace\psmux-cgate'
for ($r = 1; $r -le $N; $r++) {
  $data = Join-Path $env:TEMP "pw-$r"
  New-Item -ItemType Directory -Force $data | Out-Null
  $env:PSMUX_DATA_DIR = $data
  $tr = Join-Path $env:TEMP "pw_trace_$r.log"
  if (Test-Path $tr) { [IO.File]::Delete($tr) }
  $env:PSMUX_WARM_TRACE = '1'; $env:PSMUX_WARM_TRACE_FILE = $tr
  $o = pwsh -NoProfile -File "$w\tests\test_creation_latency_gate.ps1" -Binary $Bin 2>&1
  $env:PSMUX_WARM_TRACE = $null; $env:PSMUX_WARM_TRACE_FILE = $null
  ($o | Select-String '\[PERF\] new-window ').Line
  $t = Get-Content $tr
  # first server that logs claim(new-window) after its own first window: the new-window cell
  $srvPid = ([regex]::Match(($t | Where-Object { $_ -match 'claim\(new-window\): pane=' } | Select-Object -First 1), 'pid=(\d+)')).Groups[1].Value
  $srv = @($t | Where-Object { $_ -match "pid=$srvPid\]" })
  $sched = @{}; $claims = @()
  foreach ($l in $srv) {
    $ts = [double]([regex]::Match($l, '^\[\s*([\d.]+)').Groups[1].Value)
    if ($l -match 'refill scheduled pane=(\d+)') { $sched[[int]$Matches[1]] = $ts }
    if ($l -match 'spawned spare pane=(\d+).* in ([\d.]+)ms') {
      $p = [int]$Matches[1]; $start = $ts - [double]$Matches[2]
      if ($sched.ContainsKey($p)) { "  pane=$p scheduled@$([int]$sched[$p]) start@$([int]$start) queued=$([int]($start - $sched[$p]))ms cost=$([int][double]$Matches[2])ms" }
    }
    if ($l -match 'claim\(new-window\): pane=(\d+)') { $claims += "$($Matches[1])@$([int]$ts)" }
  }
  "  claims took: $($claims -join ' ')"
  [IO.File]::Delete($tr)
  Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $Bin } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
  [IO.Directory]::Delete($data, $true)
}
