param([string]$Exe, [string]$Ns, [string]$Trace = '')
# TEST 3b of test_pane_startup_perf, standalone, with its own -L and data dir.
$ErrorActionPreference = 'Continue'
$Data = Join-Path $env:TEMP "p5-$Ns"
New-Item -ItemType Directory -Force $Data | Out-Null
$env:PSMUX_DATA_DIR = $Data
if ($Trace) { $env:PSMUX_WARM_TRACE = '1'; $env:PSMUX_WARM_TRACE_FILE = $Trace }
$s = 'pool'
function P { & $Exe -L $Ns @args 2>&1 }
function Prompt-Up { ((P capture-pane -p -t $s) -join "`n") -match 'PS [A-Z]:\\' }
try {
  Start-Process -FilePath $Exe -ArgumentList '-L', $Ns, 'new-session', '-s', $s, '-d' -WindowStyle Hidden | Out-Null
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while (-not (Prompt-Up) -and $sw.ElapsedMilliseconds -lt 20000) { Start-Sleep -Milliseconds 100 }
  P set-option -g warm-pool-size 5 | Out-Null
  # same settle as the suite: 6 shells under the server, none using CPU
  Start-Sleep -Milliseconds 1500
  $serverPid = [int](((P display-message -p -t $s '#{pid}') -join '').Trim())
  $settled = $false; $mw = [Diagnostics.Stopwatch]::StartNew()
  while (-not $settled -and $mw.ElapsedMilliseconds -lt 30000) {
    $procs = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name)
    $kids = @{}; foreach ($p in $procs) { $k = [int]$p.ParentProcessId; if (-not $kids.ContainsKey($k)) { $kids[$k] = @() }; $kids[$k] += $p }
    $shells = @(); $q = [Collections.Generic.Queue[int]]::new(); $q.Enqueue($serverPid)
    while ($q.Count) { $id = $q.Dequeue(); foreach ($c in @($kids[$id])) { if ($c) { $q.Enqueue([int]$c.ProcessId); if ($c.Name -match '^(pwsh|powershell)\.exe$') { $shells += [int]$c.ProcessId } } } }
    if ($shells.Count -ge 6) {
      $c1 = @($shells | ForEach-Object { (Get-Process -Id $_ -EA SilentlyContinue).TotalProcessorTime.TotalMilliseconds }); Start-Sleep -Milliseconds 700
      $c2 = @($shells | ForEach-Object { (Get-Process -Id $_ -EA SilentlyContinue).TotalProcessorTime.TotalMilliseconds })
      $busy = 0; for ($j = 0; $j -lt $c1.Count; $j++) { if (($c2[$j] - $c1[$j]) -gt 15) { $busy++ } }; $settled = $busy -eq 0
    } else { Start-Sleep -Milliseconds 300 }
  }
  $win = @(); $cli = @()
  for ($i = 0; $i -lt 5; $i++) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    P new-window -t $s | Out-Null
    $cli += $sw.ElapsedMilliseconds
    while (-not (Prompt-Up) -and $sw.ElapsedMilliseconds -lt 15000) { Start-Sleep -Milliseconds 5 }
    $win += $sw.ElapsedMilliseconds
  }
  "settled=$settled after $($mw.ElapsedMilliseconds) ms shells=$($shells.Count) new-window@5: [$($win -join ', ')] cli=[$($cli -join ', ')]"
} finally {
  & $Exe -L $Ns kill-server 2>&1 | Out-Null
  Start-Sleep -Milliseconds 1500
  Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $Exe -and $_.CommandLine -match [regex]::Escape("-L $Ns") } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
  Remove-Item -Recurse -Force $Data -EA SilentlyContinue
}
