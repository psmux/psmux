param([string]$Exe, [string]$Ns, [int]$N = 30, [switch]$WithChild)
$ErrorActionPreference = 'Stop'
$Data = Join-Path $env:TEMP "kp-$Ns"
New-Item -ItemType Directory -Force $Data | Out-Null
$env:PSMUX_DATA_DIR = $Data
$env:PSMUX_NO_WARM = '1'
function P { & $Exe -L $Ns @args 2>&1 | Out-String }
function Port { [int](Get-Content "$Data\$($Ns)__s.port") }
function Key { (Get-Content "$Data\$($Ns)__s.key").Trim() }
function Open([string]$line) {
  $c = [Net.Sockets.TcpClient]::new(); $c.NoDelay = $true; $c.Connect('127.0.0.1', $script:port)
  $s = $c.GetStream(); $b = [Text.Encoding]::UTF8.GetBytes("AUTH $script:key`n$line`n"); $s.Write($b, 0, $b.Length); $c.Client.Shutdown('Send'); return @{ c = $c; s = $s }
}
function ReadAll($h) {
  $c = $h.c; $s = $h.s; $s.ReadTimeout = 15000; $buf = New-Object byte[] 4096; $ms = [IO.MemoryStream]::new()
  try { while (($n = $s.Read($buf, 0, $buf.Length)) -gt 0) { $ms.Write($buf, 0, $n) } } catch {}
  $c.Dispose(); [Text.Encoding]::UTF8.GetString($ms.ToArray())
}
function Req([string]$line) { ReadAll (Open $line) }
function Pct($a, $p) { $s = @($a | Sort-Object); $s[[Math]::Min($s.Count - 1, [int][Math]::Ceiling($p * $s.Count) - 1)] }
$killed = @(); $killMs = @(); $dmMs = @()
try {
  P new-session -d -s s -x 160 -y 50 | Out-Null
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while (-not (Test-Path "$Data\$($Ns)__s.key") -and $sw.ElapsedMilliseconds -lt 15000) { Start-Sleep -Milliseconds 50 }
  $script:port = Port; $script:key = Key
  for ($i = 1; $i -le $N; $i++) {
    Req 'split-window -v' | Out-Null
    # wait for the new pane's shell prompt
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 15000) { if ((Req 'capture-pane -p') -match 'PS [A-Z]:\\') { break }; Start-Sleep -Milliseconds 50 }
    $ppid = [int](((Req "display-message -p '#{pane_pid}'") -replace '^OK\n', '').Trim().Trim("'"))
    if ($WithChild) {
      Req 'send-keys "ping -n 300 127.0.0.1" Enter' | Out-Null
      $sw = [Diagnostics.Stopwatch]::StartNew()
      while ($sw.ElapsedMilliseconds -lt 10000) { if (@(Get-CimInstance Win32_Process -Filter "ParentProcessId=$ppid" | Where-Object Name -eq 'PING.EXE').Count) { break }; Start-Sleep -Milliseconds 100 }
    }
    $kids = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$ppid" | ForEach-Object { $_.ProcessId })
    $killed += [pscustomobject]@{ Shell = $ppid; Kids = $kids }
    Start-Sleep -Milliseconds 300
    $t = [Diagnostics.Stopwatch]::StartNew()
    $k = Open 'kill-pane'
    Start-Sleep -Milliseconds 5
    $t2 = [Diagnostics.Stopwatch]::StartNew()
    $d = Req "display-message -p alive"
    $dmMs += [math]::Round($t2.Elapsed.TotalMilliseconds, 1)
    [void](ReadAll $k)
    $killMs += [math]::Round($t.Elapsed.TotalMilliseconds, 1)
    if ($d -notmatch 'alive') { "display-message reply [$d]" }
  }
  Start-Sleep -Milliseconds 2000
  $alive = 0; $orph = @()
  foreach ($k in $killed) {
    foreach ($p in @($k.Shell) + $k.Kids) { $pr = Get-Process -Id $p -EA SilentlyContinue; if ($pr) { $alive++; $orph += "$p($($pr.ProcessName))" } }
  }
  "kill-pane reply ms:     p50={0} p90={1} max={2}" -f (Pct $killMs 0.5), (Pct $killMs 0.9), (Pct $killMs 1.0)
  "display-message 5ms after kill-pane: p50={0} p90={1} max={2}" -f (Pct $dmMs 0.5), (Pct $dmMs 0.9), (Pct $dmMs 1.0)
  "dm samples: $($dmMs -join ',')"
  "killed panes=$($killed.Count) tracked pids=$(($killed | ForEach-Object { 1 + $_.Kids.Count } | Measure-Object -Sum).Sum) still alive after 2 s=$alive $($orph -join ' ')"
} finally {
  & $Exe -L $Ns kill-server 2>&1 | Out-Null
  Start-Sleep -Milliseconds 800
  Remove-Item -Recurse -Force $Data -EA SilentlyContinue
}
