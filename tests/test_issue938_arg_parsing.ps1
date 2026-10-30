# Issue #938: command arguments read differently by the CLI, the server
# handler and the in server route (hooks), and a kill-session -t that looked
# the target up outside the server's -L namespace.
#
# Fully isolated: a private PSMUX_DATA_DIR and config under %TEMP%, two
# namespaces of its own, and the unnamed namespace ONLY inside that private
# data dir (so it can never reach a real ~/.psmux). Cleans up with
# `-L <ns> kill-server` and a kill-server scoped to the private data dir.
#
# Usage: pwsh tests\test_issue938_arg_parsing.ps1 [-Exe path\to\psmux.exe]
param([string]$Exe = '')
$ErrorActionPreference = 'Continue'
if (-not $Exe) { $Exe = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { Join-Path $PSScriptRoot '..\target\release\psmux.exe' } }
if (-not (Test-Path -LiteralPath $Exe)) { Write-Output "psmux binary not found: $Exe"; exit 2 }
$Exe = (Resolve-Path -LiteralPath $Exe).Path

$saved = @{}
foreach ($n in 'PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'PSMUX_SERVER_MODE', 'TMUX', 'TMUX_PANE', 'PSMUX_TARGET_SESSION') {
    $saved[$n] = [Environment]::GetEnvironmentVariable($n, 'Process')
}
$Root = Join-Path ([IO.Path]::GetTempPath()) "psmux-938-$PID"
New-Item -ItemType Directory -Force (Join-Path $Root 'data') | Out-Null
$cfg = Join-Path $Root 'empty.conf'; [IO.File]::WriteAllText($cfg, '')
$env:PSMUX_DATA_DIR = Join-Path $Root 'data'
$env:PSMUX_CONFIG_FILE = $cfg
foreach ($n in 'PSMUX_SERVER_MODE', 'TMUX', 'TMUX_PANE', 'PSMUX_TARGET_SESSION') { Remove-Item "Env:$n" -ErrorAction SilentlyContinue }

$NS = "t938a$PID"
$NS2 = "t938b$PID"
$script:fail = 0; $script:pass = 0
function Check([bool]$ok, [string]$name, [string]$detail = '') {
    if ($ok) { $script:pass++; Write-Output "PASS: $name" } else { $script:fail++; Write-Output "FAIL: $name $detail" }
}
function P { & $Exe -L $NS @args 2>&1 | ForEach-Object { "$_" } }
function P2 { & $Exe -L $NS2 @args 2>&1 | ForEach-Object { "$_" } }
function P0 { & $Exe @args 2>&1 | ForEach-Object { "$_" } }   # unnamed namespace, private data dir only
function Alive([scriptblock]$runner, [string]$s) { & $runner has-session -t $s | Out-Null; $LASTEXITCODE -eq 0 }
function Wait-For([scriptblock]$cond, [int]$ms = 8000) {
    $d = (Get-Date).AddMilliseconds($ms)
    while ((Get-Date) -lt $d) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 200 }
    [bool](& $cond)
}
function Fresh([scriptblock]$runner, [string]$s) {
    & $runner new-session -d -s $s -x 120 -y 30 | Out-Null
    Wait-For { Alive $runner $s } | Out-Null
}
function Wins([string]$s) { (P list-windows -t $s -F '#{window_index}:#{window_name}:#{window_active}') -join ' ' }
function ThreeWindows([string]$s) {
    Fresh ${function:P} $s
    P new-window -t $s -n one | Out-Null
    P new-window -t $s -n two | Out-Null
    Start-Sleep -Milliseconds 500
    P select-window -t "${s}:0" | Out-Null; P select-window -t "${s}:1" | Out-Null; P select-window -t "${s}:2" | Out-Null
}

try {
    Write-Output "== CLI: clustered flags read as tmux reads them"
    Fresh ${function:P} c1
    P new-window -dt c1:1 -n X | Out-Null
    $ok = Wait-For { (Wins c1) -eq '0:pwsh:1 1:X:0' -or (Wins c1) -match '^0:\S+:1 1:X:0$' } 4000
    Start-Sleep 2
    Check ((Wins c1) -match '^0:\S+:1 1:X:0$') 'new-window -dt :1 -n X makes detached window 1 named X' "($(Wins c1))"

    Fresh ${function:P} c2
    P split-window -dt c2:0 | Out-Null
    Start-Sleep 3
    $panes = @(P list-panes -t c2:0 -F '#{pane_id}:#{pane_active}')
    Check ($panes.Count -eq 2 -and $panes[0] -like '*:1') 'split-window -dt :0 leaves two panes and the first one active' "($($panes -join ' '))"

    Fresh ${function:P} c3
    P new-window -t c3 -dn X | Out-Null
    Start-Sleep 3
    Check ((Wins c3) -match '^0:\S+:1 1:X:0$') 'new-window -dn X is detached and stays' "($(Wins c3))"

    ThreeWindows c4
    P select-window -lt c4:0 | Out-Null
    Start-Sleep -Milliseconds 500
    Check ((Wins c4) -match '^0:\S+:0 1:one:1 2:two:0$') 'select-window -lt :0 moves to the last window, 1' "($(Wins c4))"

    ThreeWindows c5
    P kill-window -at c5:1 | Out-Null
    Start-Sleep -Milliseconds 800
    Check ((Wins c5) -eq '1:one:1') 'kill-window -at :1 keeps window 1 and kills the others (not the active 2)' "($(Wins c5))"

    ThreeWindows c6
    P kill-window -a -t c6:1 | Out-Null
    Start-Sleep -Milliseconds 800
    Check ((Wins c6) -eq '1:one:1') 'kill-window -a -t :1 keeps window 1' "($(Wins c6))"

    ThreeWindows c6b
    P kill-window -t c6b:1 | Out-Null
    Start-Sleep -Milliseconds 800
    Check ((Wins c6b) -match '^0:\S+:0 2:two:1$') 'kill-window -t :1 without -a still kills window 1 only' "($(Wins c6b))"

    $lw = @(P list-windows -aF '#{session_name}/#{window_index}')
    Check ($lw.Count -gt 0 -and ($lw | Where-Object { $_ -notmatch '^[^/\s]+/\d+$' }).Count -eq 0) 'list-windows -aF applies the format' "($($lw | Select-Object -First 2))"

    Write-Output "== hooks: clustered flags on the in server route"
    ThreeWindows h1
    P set-hook -t h1 after-split-window 'kill-window -at :1' | Out-Null
    P split-window -t h1:2 | Out-Null
    Start-Sleep 2
    Check ((Wins h1) -eq '1:one:1') "hook kill-window -at :1 keeps window 1 (not the active 2)" "($(Wins h1))"

    ThreeWindows h2
    P set-hook -t h2 after-split-window 'select-window -lt :0' | Out-Null
    P split-window -t h2:2 | Out-Null
    Start-Sleep 2
    Check ((Wins h2) -match '^0:\S+:0 1:one:1 2:two:0$') 'hook select-window -lt :0 moves to the last window' "($(Wins h2))"

    ThreeWindows h3
    P set-hook -t h3 after-split-window 'new-window -dn X' | Out-Null
    P split-window -t h3:2 | Out-Null
    Start-Sleep 3
    Check ((Wins h3) -match '^0:\S+:0 1:one:0 2:two:1 3:X:0$') 'hook new-window -dn X makes a detached window X' "($(Wins h3))"

    Write-Output "== hooks: a target naming another session runs on that session"
    Fresh ${function:P} A; Fresh ${function:P} D; Fresh ${function:P} B
    P set-hook -t A after-split-window 'kill-session -t D' | Out-Null
    P split-window -t A | Out-Null
    $dGone = Wait-For { -not (Alive ${function:P} D) } 5000
    Check $dGone 'hook kill-session -t D in A ends D'
    Check (Alive ${function:P} A) 'A survives its own hook'

    Fresh ${function:P} A2
    P set-hook -t A2 after-split-window 'set -t B @x 1' | Out-Null
    P split-window -t A2 | Out-Null
    $onB = Wait-For { ((P show -v -t B '@x') -join '') -eq '1' } 5000
    Check $onB 'hook set -t B @x 1 in A2 sets @x on B'
    $onA = (P show -v -t A2 '@x') -join ''
    Check ($onA -ne '1') 'and not on A2' "(A2 @x '$onA')"

    Fresh ${function:P} A3
    P set-hook -t A3 after-split-window 'kill-session -t nosuch' | Out-Null
    P split-window -t A3 | Out-Null
    $msg = Wait-For {
        try { $f = (P dump-state -t A3) -join "`n" | ConvertFrom-Json -AsHashtable } catch { $f = $null }
        $null -ne $f -and $f['status_message'] -ceq "can't find session: nosuch"
    } 5000
    Check $msg "hook kill-session -t nosuch says can't find session: nosuch"
    Check (Alive ${function:P} A3) 'A3 survives a hook naming a missing session'

    Write-Output "== hooks: session parts resolved as tmux's cmd_find_get_session"
    Fresh ${function:P} mainx
    P set-hook -t mainx after-split-window 'kill-session -t mai' | Out-Null
    P split-window -t mainx | Out-Null
    $ownGone = Wait-For { -not (Alive ${function:P} mainx) } 5000
    Check $ownGone 'hook kill-session -t mai (unique prefix of its own name) ends its own session mainx'

    Fresh ${function:P} workx
    Fresh ${function:P} Q
    P set-hook -t Q after-split-window 'set -t wor: @y 7' | Out-Null
    P split-window -t Q | Out-Null
    $onW = Wait-For { ((P show -v -t workx '@y') -join '') -eq '7' } 5000
    Check $onW 'hook set -t wor: @y 7 in Q (unique prefix of workx) sets @y on workx'

    Write-Output "== namespaces: kill-session -t D stays in the server's namespace"
    Fresh ${function:P0} D
    Fresh ${function:P2} A
    Fresh ${function:P2} D
    $base = "${NS2}__A"
    $port = [int](Get-Content (Join-Path $env:PSMUX_DATA_DIR "$base.port")).Trim()
    $key = (Get-Content (Join-Path $env:PSMUX_DATA_DIR "$base.key")).Trim()
    $c = [Net.Sockets.TcpClient]::new('127.0.0.1', $port)
    $w = [IO.StreamWriter]::new($c.GetStream()); $w.Write("AUTH $key`nkill-session -t D`n"); $w.Flush()
    Start-Sleep -Milliseconds 800; $c.Close()
    $nsGone = Wait-For { -not (Alive ${function:P2} D) } 5000
    Check $nsGone "kill-session -t D on $NS2's listener ends $NS2's D"
    Check (Alive ${function:P0} D) "the unnamed namespace's D in the same data dir lives"

    Fresh ${function:P2} D
    Fresh ${function:P2} H
    P2 set-hook -t H after-split-window 'kill-session -t D' | Out-Null
    P2 split-window -t H | Out-Null
    $hookGone = Wait-For { -not (Alive ${function:P2} D) } 5000
    Check $hookGone "hook in $NS2/H ends $NS2's D"
    Check (Alive ${function:P2} H) 'H survives'
    Check (Alive ${function:P0} D) "the unnamed namespace's D still lives"
} finally {
    P kill-server | Out-Null
    P2 kill-server | Out-Null
    # The unnamed namespace of the PRIVATE data dir only (PSMUX_DATA_DIR set).
    if ($env:PSMUX_DATA_DIR -and $env:PSMUX_DATA_DIR.StartsWith($Root)) { P0 kill-server | Out-Null }
    foreach ($n in $saved.Keys) {
        if ($null -eq $saved[$n]) { Remove-Item "Env:$n" -ErrorAction SilentlyContinue }
        else { [Environment]::SetEnvironmentVariable($n, $saved[$n], 'Process') }
    }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue
}
Write-Output "passed $script:pass, failed $script:fail"
if ($script:fail -gt 0) { exit 1 }
exit 0
