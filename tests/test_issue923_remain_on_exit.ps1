# Issue #923: remain-on-exit takes tmux's off, on, failed, key and
# failed-key, remain-on-exit-format writes the dead pane's last line, and
# split-window -k / -m set both on the new pane, as in tmux 3.8
# (options-table.c:93-95, :1699-1721, server-fn.c:354-437,
# server-client.c:1411-1428, cmd-split-window.c:256-262).
#
# Each expected line is what tmux 3.8 printed for the same steps (WSL,
# -f /dev/null), with the date read back from the pane instead of compared.
#
#   [1] failed: an exit 0 closes, an exit 3 stays with "Pane is dead (status 3, <date>)"
#   [2] key: send-keys does not close the dead pane; a key typed in a client
#       (the attached client's `send-key` line on the session port) does,
#       but not while the pane is in copy mode, where the mode takes it
#   [2b] failed-key (new in 3.8): an exit 0 closes, an exit 3 stays until a
#       key is typed in it
#   [3] remain-on-exit-format: a custom text, an empty one, a pane's own
#   [4] split-window -k and -m
#   [5] respawn-pane clears pane_dead, pane_dead_status and pane_dead_time
#   [6] 1, true and an unknown word are refused as tmux refuses them
#   [7] kill-pane on a window's last pane closes the window whatever
#       remain-on-exit says, dead or alive (server-fn.c:221-236)
#
# Isolated: USERPROFILE, PSMUX_DATA_DIR and PSMUX_CONFIG_FILE point into a
# fresh temp dir and every command uses -L. PSMUX_EXE picks the binary
# (defaults to psmux on PATH).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$TempHome = Join-Path $env:TEMP ("psmux_roe_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "empty.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_TARGET*' -or $_.Name -like 'TMUX*' -or $_.Name -eq 'PSMUX_SESSION' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
$NS = "roe_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function P { & $PSMUX -L $NS @args 2>&1 }
function Check($name, $got, $want) {
    if ("$got" -eq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}
function CheckLike($name, $got, $pattern) {
    if ("$got" -match $pattern) { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected /$pattern/, got '$got'" }
}
function Windows { (P list-windows -t s -F '#{window_name}') -join ' ' }
function LastLine($t) { @((P capture-pane -p -t $t) | ForEach-Object { "$_" } | Where-Object { $_.Trim() -ne '' }) | Select-Object -Last 1 }
function WaitDead($t) {
    for ($i = 0; $i -lt 60; $i++) {
        if (((P display -p -t $t '#{pane_dead}') -join '') -eq '1') { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}
function WaitGone($name) {
    for ($i = 0; $i -lt 60; $i++) {
        if (-not ((Windows) -split ' ' -contains $name)) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}
# One line on the session's port the way the attached client sends a key it
# did not bind (client.rs: `send-key <name>`).
function TypeKey($key) {
    $port = (Get-Content (Join-Path $TempHome "${NS}__s.port") -Raw).Trim()
    $auth = (Get-Content (Join-Path $TempHome "${NS}__s.key") -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port)
    try {
        $st = $tcp.GetStream()
        $w = [IO.StreamWriter]::new($st); $r = [IO.StreamReader]::new($st)
        $w.Write("AUTH $auth`n"); $w.Flush(); $null = $r.ReadLine()
        $w.Write("send-key $key`n"); $w.Flush()
        Start-Sleep -Milliseconds 300
    } finally {
        $tcp.Close()
    }
}
function Exits($code) { "pwsh -NoProfile -Command exit $code" }
# A pane command that needs quotes goes through a script file: quotes inside
# one argument do not survive the trip to the server intact.
function Script($name, $body) {
    $path = Join-Path $TempHome "$name.ps1"
    Set-Content -Path $path -Value $body
    "pwsh -NoProfile -File $path"
}
$DeadLine = '^Pane is dead \(status {0}, \w{{3}} \w{{3}} [ \d]\d \d\d:\d\d:\d\d \d{{4}}\)$'

try {
    P new-session -d -s s -x 100 -y 15 | Out-Null
    Start-Sleep -Seconds 2
    P has-session -t s | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Fail "session did not start"; exit 1 }

    Write-Host "`n[1] failed" -ForegroundColor Yellow
    P set -g remain-on-exit failed | Out-Null
    P new-window -d -n f0 (Exits 0) | Out-Null
    Check "an exit 0 closes the pane" (WaitGone 'f0') 'True'
    P new-window -d -n f3 (Exits 3) | Out-Null
    Check "an exit 3 stays" (WaitDead 's:f3') 'True'
    Start-Sleep -Milliseconds 500
    CheckLike "its last line" (LastLine 's:f3') ($DeadLine -f 3)
    Check "pane_dead_status" ((P display -p -t s:f3 '#{pane_dead_status}') -join '') '3'
    Check "pane_dead_signal" ((P display -p -t s:f3 '[#{pane_dead_signal}]') -join '') '[]'
    CheckLike "pane_dead_time" ((P display -p -t s:f3 '#{pane_dead_time}') -join '') '^\d{10}$'

    Write-Host "`n[2] key" -ForegroundColor Yellow
    P set -g remain-on-exit key | Out-Null
    P new-window -n k (Exits 5) | Out-Null
    Check "the pane stays" (WaitDead 's:k') 'True'
    Start-Sleep -Milliseconds 500
    CheckLike "its last line" (LastLine 's:k') ($DeadLine -f 5)
    P send-keys -t s:k a | Out-Null
    Start-Sleep -Milliseconds 800
    Check "send-keys leaves it, as tmux's does" ((Windows) -split ' ' -contains 'k') 'True'
    # In copy mode the key is the mode's (server-client.c:1531-1538,
    # :1684-1688) and never reaches the dead pane check.
    P copy-mode -t s:k | Out-Null
    Start-Sleep -Milliseconds 500
    TypeKey 'a'
    Start-Sleep -Milliseconds 500
    Check "a key typed in copy mode leaves it" ((Windows) -split ' ' -contains 'k') 'True'
    P send-keys -t s:k -X cancel | Out-Null
    Start-Sleep -Milliseconds 500
    TypeKey 'a'
    Check "a typed key closes it" (WaitGone 'k') 'True'

    Write-Host "`n[2b] failed-key" -ForegroundColor Yellow
    P set -g remain-on-exit failed-key | Out-Null
    Check "show -gv" ((P show -gv remain-on-exit) -join '') 'failed-key'
    P new-window -d -n fk0 (Exits 0) | Out-Null
    Check "an exit 0 closes the pane" (WaitGone 'fk0') 'True'
    P new-window -n fk3 (Exits 3) | Out-Null
    Check "an exit 3 stays" (WaitDead 's:fk3') 'True'
    Start-Sleep -Milliseconds 500
    CheckLike "its last line" (LastLine 's:fk3') ($DeadLine -f 3)
    P send-keys -t s:fk3 a | Out-Null
    Start-Sleep -Milliseconds 800
    Check "send-keys leaves it" ((Windows) -split ' ' -contains 'fk3') 'True'
    TypeKey 'a'
    Check "a typed key closes it" (WaitGone 'fk3') 'True'

    Write-Host "`n[3] remain-on-exit-format" -ForegroundColor Yellow
    P set -g remain-on-exit on | Out-Null
    P set -g remain-on-exit-format 'bye #{pane_dead_status}' | Out-Null
    P new-window -d -n c (Exits 7) | Out-Null
    $null = WaitDead 's:c'; Start-Sleep -Milliseconds 500
    Check "a custom text" (LastLine 's:c') 'bye 7'
    P set -g remain-on-exit-format '' | Out-Null
    P new-window -d -n e (Script 'hello' "Write-Output hello; exit 2") | Out-Null
    $null = WaitDead 's:e'; Start-Sleep -Milliseconds 500
    Check "an empty one writes nothing" (LastLine 's:e') 'hello'
    P set -gu remain-on-exit-format | Out-Null
    P new-window -d -n p (Script 'late' "Start-Sleep 3; exit 4") | Out-Null
    P set -p -t s:p remain-on-exit-format 'PANE #{pane_dead_status}' | Out-Null
    $null = WaitDead 's:p'; Start-Sleep -Milliseconds 500
    Check "a pane's own wins" (LastLine 's:p') 'PANE 4'
    # A window's own text (tmux 3.8: set -w -t keeps it on that window, show
    # -w lists it, the global and the other windows keep the default).
    P new-window -d -n wf (Script 'wlate' "Start-Sleep 3; exit 3") | Out-Null
    P new-window -d -n wg (Script 'glate' "Start-Sleep 3; exit 5") | Out-Null
    P set -w -t s:wf remain-on-exit-format 'WIN #{pane_dead_status}' | Out-Null
    Check "set -w is the window's own" ((P show -wv -t s:wf remain-on-exit-format) -join '') 'WIN #{pane_dead_status}'
    CheckLike "show -w lists it" ((P show -w -t s:wf) -join '|') '(^|\|)remain-on-exit-format "?WIN '
    CheckLike "the global keeps the default" ((P show -gv remain-on-exit-format) -join '') '^Pane is dead \('
    $null = WaitDead 's:wf'; $null = WaitDead 's:wg'; Start-Sleep -Milliseconds 500
    Check "the window's own text on its dead pane" (LastLine 's:wf') 'WIN 3'
    CheckLike "another window keeps the default text" (LastLine 's:wg') ($DeadLine -f 5)

    Write-Host "`n[4] split-window -k and -m" -ForegroundColor Yellow
    P set -g remain-on-exit off | Out-Null
    P split-window -d -k -t s:p (Exits 6) | Out-Null
    P split-window -d -m 'bye #{pane_dead_status}' -t s:p (Exits 8) | Out-Null
    Start-Sleep -Seconds 4
    $panes = @(P list-panes -t s:p -F '#{pane_id} #{pane_dead} #{pane_dead_status}')
    $k = $panes | Where-Object { $_ -match ' 1 6$' } | ForEach-Object { ($_ -split ' ')[0] }
    $m = $panes | Where-Object { $_ -match ' 1 8$' } | ForEach-Object { ($_ -split ' ')[0] }
    Check "the -k pane stays dead with status 6" ([bool]$k) 'True'
    Check "the -m pane stays dead with status 8" ([bool]$m) 'True'
    if ($k) {
        Check "-k sets the pane's remain-on-exit" ((P show -p -t $k remain-on-exit) -join '') 'remain-on-exit key'
        CheckLike "the -k pane's last line" (LastLine $k) ($DeadLine -f 6)
    }
    if ($m) {
        Check "-m sets key" ((P show -pv -t $m remain-on-exit) -join '') 'key'
        Check "-m sets the format" ((P show -pv -t $m remain-on-exit-format) -join '') 'bye #{pane_dead_status}'
        Check "the -m pane's last line" (LastLine $m) 'bye 8'
    }

    Write-Host "`n[5] respawn-pane" -ForegroundColor Yellow
    P respawn-pane -t s:f3 'pwsh -NoProfile' | Out-Null
    Start-Sleep -Seconds 1
    Check "pane_dead and the rest after respawn" ((P display -p -t s:f3 'dead=#{pane_dead} status=[#{pane_dead_status}] time=[#{pane_dead_time}]') -join '') 'dead=0 status=[] time=[]'

    Write-Host "`n[6] values tmux refuses" -ForegroundColor Yellow
    foreach ($bad in '1', 'true', 'bogus') {
        $out = (P set -g remain-on-exit $bad) -join ' | '
        $rc = $LASTEXITCODE
        if ($rc -ne 0 -and $out -match "unknown value: $bad") { Write-Pass "set remain-on-exit $bad is refused: '$out'" }
        else { Write-Fail "set remain-on-exit $bad rc=$rc '$out'" }
    }
    Check "and the value stays" ((P show -gv remain-on-exit) -join '') 'off'

    Write-Host "`n[7] kill-pane on a window's last pane" -ForegroundColor Yellow
    P set -g remain-on-exit on | Out-Null
    P new-window -d -n kd (Exits 0) | Out-Null
    Check "the pane is dead first" (WaitDead 's:kd') 'True'
    P kill-pane -t s:kd | Out-Null
    Check "kill-pane closes the window of a dead last pane" (WaitGone 'kd') 'True'
    P new-window -d -n kl 'pwsh -NoProfile' | Out-Null
    Start-Sleep -Seconds 2
    P kill-pane -t s:kl | Out-Null
    Check "kill-pane closes the window of a live last pane under on" (WaitGone 'kl') 'True'
    P set -g remain-on-exit failed-key | Out-Null
    P new-window -d -n kf (Exits 3) | Out-Null
    $null = WaitDead 's:kf'
    P kill-pane -t s:kf | Out-Null
    Check "kill-pane closes the window of a dead failed-key pane" (WaitGone 'kf') 'True'
    P set -g remain-on-exit off | Out-Null
}
finally {
    P kill-server | Out-Null
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
