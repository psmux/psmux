param([string]$Exe)
$ErrorActionPreference = 'Continue'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$tmp = Join-Path $env:TEMP ("s1dot_" + [guid]::NewGuid().ToString('N').Substring(0, 6))
New-Item -ItemType Directory -Force $tmp | Out-Null
$env:PSMUX_DATA_DIR = $tmp; $env:USERPROFILE = $tmp
$env:PSMUX_CONFIG_FILE = Join-Path $tmp 'empty.conf'; Set-Content $env:PSMUX_CONFIG_FILE ''
foreach ($n in @(Get-ChildItem Env: | ? { $_.Name -like 'PSMUX_TARGET*' -or $_.Name -like 'TMUX*' } | % Name)) { Remove-Item "Env:$n" }
$L = 's1dot_' + [guid]::NewGuid().ToString('N').Substring(0, 4)
function P { & $Exe -L $L @args 2>&1 }
P new-session -d -s b -x 120 -y 30 | Out-Null
Start-Sleep 2
P new-window -d -t b -n two | Out-Null
P new-window -d -t b -n three | Out-Null
foreach ($t in 'b:1', 'b:1.', '=b:1.', 'b:two.', '=b:two.', 'b:.', ':1.', '.', 'b:1.0', 'b:1.%1', 'b:', ':') {
    $o = (P display -t $t -p '#{session_name}:#{window_index}.#{pane_index} #{window_name}') -join ' | '
    "{0,-10} rc={1} {2}" -f $t, $LASTEXITCODE, $o
}
"--- select-window -t b:2."; (P select-window -t 'b:2.') -join ' | '; "rc=$LASTEXITCODE active=" + ((P display -t b -p '#{window_index}') -join '')
"--- rename-window -t b:1. R1"; (P rename-window -t 'b:1.' R1) -join ' | '; "rc=$LASTEXITCODE name=" + ((P display -t b:1 -p '#{window_name}') -join '')
"--- send-keys -t b:three. 'echo HI_S1DOT' Enter"; (P send-keys -t 'b:three.' 'echo HI_S1DOT' Enter) -join ' | '; "rc=$LASTEXITCODE"
Start-Sleep 2; "captured: " + (((P capture-pane -t b:three -p) -join "`n") -match 'HI_S1DOT')
"--- kill-window -t =b:2."; (P kill-window -t '=b:2.') -join ' | '; "rc=$LASTEXITCODE windows=" + ((P list-windows -t b -F '#{window_index}') -join ',')
P kill-server | Out-Null
Remove-Item -Recurse -Force $tmp -EA SilentlyContinue
