param([string]$Exe, [string[]]$Lines)
$ErrorActionPreference = 'Continue'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$tmp = Join-Path $env:TEMP ("s1tcp_" + [guid]::NewGuid().ToString('N').Substring(0, 6))
New-Item -ItemType Directory -Force $tmp | Out-Null
$env:PSMUX_DATA_DIR = $tmp; $env:USERPROFILE = $tmp
$env:PSMUX_CONFIG_FILE = Join-Path $tmp 'empty.conf'; Set-Content $env:PSMUX_CONFIG_FILE ''
foreach ($n in @(Get-ChildItem Env: | ? { $_.Name -like 'PSMUX_TARGET*' -or $_.Name -like 'TMUX*' } | % Name)) { Remove-Item "Env:$n" }
$L = 's1tcp_' + [guid]::NewGuid().ToString('N').Substring(0, 4)
function P { & $Exe -L $L @args 2>&1 }
P new-session -d -s b -x 120 -y 30 | Out-Null
Start-Sleep 2
P new-window -d -t b -n two | Out-Null
P new-window -d -t b -n three | Out-Null
$port = (Get-Content "$tmp\${L}__b.port" -Raw).Trim(); $key = (Get-Content "$tmp\${L}__b.key" -Raw).Trim()
function Send($line) {
    $c = [Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port); $c.NoDelay = $true; $c.ReceiveTimeout = 3000
    $s = $c.GetStream(); $w = [IO.StreamWriter]::new($s); $r = [IO.StreamReader]::new($s)
    $w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
    $w.Write("$line`n"); $w.Flush()
    $out = try { $r.ReadToEnd() } catch { "<timeout>" }
    $c.Close(); return $out.Trim()
}
foreach ($line in $Lines) { "{0,-50} => [{1}]" -f $line, ((Send $line) -replace "`r?`n", ' | ') }
"windows: " + ((P list-windows -t b -F '#{window_index}:#{window_name}#{?window_active,*,}') -join ' ')
P kill-server | Out-Null
Remove-Item -Recurse -Force $tmp -EA SilentlyContinue
