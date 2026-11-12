param([string]$Exe, [string]$Ns)
# repro_pool5 with every measured request sent over raw TCP instead of a CLI process.
$ErrorActionPreference = 'Continue'
$Data = Join-Path $env:TEMP "p5t-$Ns"
New-Item -ItemType Directory -Force $Data | Out-Null
$env:PSMUX_DATA_DIR = $Data
$s = 'pool'
function P { & $Exe -L $Ns @args 2>&1 }
function Req([string]$line) {
  $c = [Net.Sockets.TcpClient]::new(); $c.NoDelay = $true; $c.Connect('127.0.0.1', $script:port)
  $st = $c.GetStream(); $b = [Text.Encoding]::UTF8.GetBytes("AUTH $script:key`nTARGET $s`n$line`n"); $st.Write($b, 0, $b.Length); $c.Client.Shutdown('Send')
  $r = [IO.StreamReader]::new($st).ReadToEnd(); $c.Dispose(); $r
}
try {
  Start-Process -FilePath $Exe -ArgumentList '-L', $Ns, 'new-session', '-s', $s, '-d' -WindowStyle Hidden | Out-Null
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while (-not (Test-Path "$Data\$($Ns)__$s.key") -and $sw.ElapsedMilliseconds -lt 15000) { Start-Sleep -Milliseconds 50 }
  Start-Sleep -Milliseconds 300
  $script:port = [int](Get-Content "$Data\$($Ns)__$s.port"); $script:key = (Get-Content "$Data\$($Ns)__$s.key").Trim()
  while (-not ((Req 'capture-pane -p') -match 'PS [A-Z]:\\') -and $sw.ElapsedMilliseconds -lt 20000) { Start-Sleep -Milliseconds 100 }
  [void](Req 'set-option -g warm-pool-size 5')
  Start-Sleep -Milliseconds 1500
  $serverPid = [int](((Req "display-message -p '#{pid}'") -replace '^OK\n', '').Trim().Trim("'"))
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
  $win = @(); $cmdMs = @(); $cliMs = @()
  for ($i = 0; $i -lt 5; $i++) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    [void](Req 'new-window')
    $win += $sw.ElapsedMilliseconds
    $cw = [Diagnostics.Stopwatch]::StartNew(); cmd /c exit 0; $cmdMs += $cw.ElapsedMilliseconds
    $cw.Restart(); & $Exe -V | Out-Null; $cliMs += $cw.ElapsedMilliseconds
    $sw.Restart()
    while (-not ((Req 'capture-pane -p') -match 'PS [A-Z]:\\') -and $sw.ElapsedMilliseconds -lt 15000) { Start-Sleep -Milliseconds 5 }
    $win[-1] = "$($win[-1])+$($sw.ElapsedMilliseconds)"
    # pace like the CLI version (one CLI start is ~15 ms)
    Start-Sleep -Milliseconds 15
  }
  "tcp new-window@5: [$($win -join ', ')] cmd=[$($cmdMs -join ', ')] psmux-V=[$($cliMs -join ', ')]"
} finally {
  & $Exe -L $Ns kill-server 2>&1 | Out-Null
  Start-Sleep -Milliseconds 1500
  Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq $Exe -and $_.CommandLine -match [regex]::Escape("-L $Ns") } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
  [IO.Directory]::Delete($Data, $true)
}
