param([string]$Exe, [string]$Ns, [int]$Rounds = 3)
$ErrorActionPreference = 'Stop'
$Data = Join-Path $env:TEMP "repro1009-$Ns"
New-Item -ItemType Directory -Force $Data | Out-Null
$env:PSMUX_DATA_DIR = $Data
$env:PSMUX_NO_WARM = '1'
function P { & $Exe -L $Ns @args 2>&1 | Out-String }
P new-session -d -s A | Out-Null
P new-session -d -s B | Out-Null
Start-Sleep -Milliseconds 1500
$portA = [int](Get-Content "$Data\$($Ns)__A.port")
$keyA = (Get-Content "$Data\$($Ns)__A.key").Trim()

function Send-Async([string]$line) {
  $c = [Net.Sockets.TcpClient]::new(); $c.NoDelay = $true
  $c.Connect('127.0.0.1', $portA)
  $s = $c.GetStream(); $b = [Text.Encoding]::UTF8.GetBytes("AUTH $keyA`n$line`n")
  $s.Write($b, 0, $b.Length); $s.Flush(); return $c
}
function Read-All($c) {
  $s = $c.GetStream(); $s.ReadTimeout = 15000; $buf = New-Object byte[] 4096; $ms = [IO.MemoryStream]::new()
  try { while (($n = $s.Read($buf, 0, $buf.Length)) -gt 0) { $ms.Write($buf, 0, $n) } } catch {}
  $c.Dispose(); [Text.Encoding]::UTF8.GetString($ms.ToArray()) -replace "`n", '<LF>'
}
function Measure-Case([string]$label, [string]$line) {
  for ($r = 1; $r -le $Rounds; $r++) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $c = Send-Async $line
    Start-Sleep -Milliseconds 30
    $t0 = $sw.ElapsedMilliseconds
    $dm = (P display-message -t A -p 'alive').Trim()
    $dmMs = $sw.ElapsedMilliseconds - $t0
    $rep = Read-All $c
    $fwdMs = $sw.ElapsedMilliseconds
    $st = P display-message -t A -p '#{status_message}'
    $dump = P dump-state -t A
    $m = [regex]::Match($dump, '"status_message":"((?:[^"\\]|\\.)*)"')
    "{0,-12} r{1} forward_reply_ms={2,5} display_msg_on_A_ms={3,5} dm=[{4}] reply=[{5}] status=[{6}]" -f $label, $r, $fwdMs, $dmMs, $dm, $rep, $m.Groups[1].Value
  }
}
try {
  # live target
  Measure-Case 'live-B' 'set-option -t B: @x live'
  "B @x = [" + (P show-options -t B -v '@x').Trim() + "]"
  # stale port file: closed port
  $l = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0); $l.Start(); $closed = $l.LocalEndpoint.Port; $l.Stop()
  Set-Content "$Data\$($Ns)__dead.port" "$closed" -NoNewline
  Set-Content "$Data\$($Ns)__dead.key" "x" -NoNewline
  Measure-Case 'closed-port' 'set-option -t dead: @x v'
  # full backlog listener
  $g = [Net.Sockets.Socket]::new('InterNetwork', 'Stream', 'Tcp'); $g.Bind([Net.IPEndPoint]::new([Net.IPAddress]::Loopback, 0)); $g.Listen(1)
  $gp = $g.LocalEndPoint.Port; $held = @()
  for ($i = 0; $i -lt 64; $i++) { $s = [Net.Sockets.Socket]::new('InterNetwork', 'Stream', 'Tcp'); $t = $s.ConnectAsync([Net.IPAddress]::Loopback, $gp); try { if (-not $t.Wait(500)) { $s.Dispose(); break } } catch { $s.Dispose(); break }; $held += $s }
  "held=$($held.Count)"
  Set-Content "$Data\$($Ns)__full.port" "$gp" -NoNewline
  Set-Content "$Data\$($Ns)__full.key" "x" -NoNewline
  Measure-Case 'full-backlog' 'set-option -t full: @x v'
  # hook path to closed port
  P set-hook -t A after-select-window 'set -t dead: @x hook' | Out-Null
  P new-window -d -t A | Out-Null
  $sw = [Diagnostics.Stopwatch]::StartNew()
  $c = Send-Async 'select-window -t A:1'
  Start-Sleep -Milliseconds 30
  $t0 = $sw.ElapsedMilliseconds; $dm = (P display-message -t A -p 'alive').Trim(); $dmMs = $sw.ElapsedMilliseconds - $t0
  $rep = Read-All $c
  "hook-closed  forward_reply_ms={0} display_msg_on_A_ms={1} dm=[{2}]" -f $sw.ElapsedMilliseconds, $dmMs, $dm
  foreach ($s in $held) { $s.Dispose() }; $g.Dispose()
} finally {
  & $Exe -L $Ns kill-server 2>&1 | Out-Null
  Start-Sleep -Milliseconds 500
  Remove-Item -Recurse -Force $Data -ErrorAction SilentlyContinue
}
