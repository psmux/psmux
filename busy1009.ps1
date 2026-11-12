param([string]$Exe, [string]$Ns, [int]$N = 60)
$ErrorActionPreference = 'Stop'
$Data = Join-Path $env:TEMP "busy1009-$Ns"
New-Item -ItemType Directory -Force $Data | Out-Null
$env:PSMUX_DATA_DIR = $Data
function P { & $Exe -L $Ns @args 2>&1 | Out-String }
$burners = @()
try {
  P new-session -d -s A | Out-Null
  P new-session -d -s B | Out-Null
  Start-Sleep -Milliseconds 2000
  $portA = [int](Get-Content "$Data\$($Ns)__A.port"); $keyA = (Get-Content "$Data\$($Ns)__A.key").Trim()
  $portB = [int](Get-Content "$Data\$($Ns)__B.port"); $keyB = (Get-Content "$Data\$($Ns)__B.key").Trim()
  function Req([int]$port, [string]$key, [string]$line) {
    $c = [Net.Sockets.TcpClient]::new(); $c.NoDelay = $true; $c.Connect('127.0.0.1', $port)
    $s = $c.GetStream(); $b = [Text.Encoding]::UTF8.GetBytes("AUTH $key`n$line`n"); $s.Write($b, 0, $b.Length)
    $c.Client.Shutdown('Send'); $s.ReadTimeout = 15000
    $buf = New-Object byte[] 8192; $ms = [IO.MemoryStream]::new()
    try { while (($n = $s.Read($buf, 0, $buf.Length)) -gt 0) { $ms.Write($buf, 0, $n) } } catch {}
    $c.Dispose(); [Text.Encoding]::UTF8.GetString($ms.ToArray())
  }
  $cpus = [Environment]::ProcessorCount * 2
  for ($i = 0; $i -lt $cpus; $i++) { $burners += Start-Process pwsh -ArgumentList '-NoProfile', '-Command', 'while($true){$x=1}' -WindowStyle Hidden -PassThru }
  Start-Sleep -Milliseconds 1500
  $fail = 0; $times = @()
  for ($i = 1; $i -le $N; $i++) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $r = Req $portA $keyA "set-option -t B: @x v$i"
    $times += $sw.ElapsedMilliseconds
    $ok = $false
    for ($k = 0; $k -lt 50; $k++) { $v = (Req $portB $keyB 'show-options -v @x') -replace '^OK\n', '' -replace '\r?\n$', ''; if ($v -eq "v$i") { $ok = $true; break }; Start-Sleep -Milliseconds 20 }
    $d = Req $portA $keyA 'dump-state'
    if ($d -match "can't connect to session") { $ok = $false; "round $i status shows can't connect" }
    if (-not $ok) { $fail++; "round $i B @x=[$v]" }
  }
  $s = $times | Sort-Object
  "burners=$cpus forwards=$N failed=$fail reply_ms min=$($s[0]) p50=$($s[[int]($N/2)]) max=$($s[-1])"
} finally {
  foreach ($b in $burners) { try { Stop-Process -Id $b.Id -Force } catch {} }
  & $Exe -L $Ns kill-server 2>&1 | Out-Null
  Start-Sleep -Milliseconds 500
  Remove-Item -Recurse -Force $Data -ErrorAction SilentlyContinue
}
