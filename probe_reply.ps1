param([string]$Exe, [string]$Ns, [int]$N = 12, [int]$DelayMs = 0, [switch]$NoDm)
$ErrorActionPreference = 'Stop'
$Data = Join-Path $env:TEMP "kpr-$Ns"
New-Item -ItemType Directory -Force $Data | Out-Null
$env:PSMUX_DATA_DIR = $Data
$env:PSMUX_NO_WARM = '1'
function Open([string]$line) {
  $c = [Net.Sockets.TcpClient]::new(); $c.NoDelay = $true; $c.Connect('127.0.0.1', $script:port)
  $s = $c.GetStream(); $b = [Text.Encoding]::UTF8.GetBytes("AUTH $script:key`n$line`n"); $s.Write($b, 0, $b.Length); $c.Client.Shutdown('Send'); return @{ c = $c; s = $s }
}
function ReadAll($h) { $r = [IO.StreamReader]::new($h.s).ReadToEnd(); $h.c.Dispose(); $r }
function Req([string]$line) { ReadAll (Open $line) }
$k = @(); $d = @()
try {
  & $Exe -L $Ns new-session -d -s s -x 160 -y 50 | Out-Null
  Start-Sleep -Milliseconds 1500
  $script:port = [int](Get-Content "$Data\$($Ns)__s.port"); $script:key = (Get-Content "$Data\$($Ns)__s.key").Trim()
  for ($i = 0; $i -lt $N; $i++) {
    Req 'split-window -v' | Out-Null
    Start-Sleep -Milliseconds 1500
    $t = [Diagnostics.Stopwatch]::StartNew()
    $h = Open 'kill-pane'
    if (-not $NoDm) {
      if ($DelayMs -gt 0) { Start-Sleep -Milliseconds $DelayMs }
      $t2 = [Diagnostics.Stopwatch]::StartNew(); [void](Req 'display-message -p alive'); $d += [math]::Round($t2.Elapsed.TotalMilliseconds, 1)
    }
    [void](ReadAll $h); $k += [math]::Round($t.Elapsed.TotalMilliseconds, 1)
  }
  "kill reply: $($k -join ',')"
  if (-not $NoDm) { "dm: $($d -join ',')" }
} finally { & $Exe -L $Ns kill-server 2>&1 | Out-Null; Start-Sleep -Milliseconds 800; Remove-Item -Recurse -Force $Data -EA SilentlyContinue }
