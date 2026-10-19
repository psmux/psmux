# Issue #798 step 1: the server mode switch.
#
# PSMUX_SERVER_MODE (or `set -s server-mode` in the config) picks the kind of
# server a namespace start creates. `per-session`, unset and empty run exactly
# the code psmux always ran; `shared` enters src/server/shared, which in this
# step starts nothing and says so at exit 1; anything else is an error at exit 1.
#
# Part 1 proves the default path is untouched: the same scenario (new-session,
# new-window, ls, list-windows, kill-session, registry files, server processes
# per session, the warm standby) gives one identical signature with the switch
# unset, `per-session` and empty. With PSMUX_BASELINE_EXE pointing at a master
# build, the signature must also equal master's.
#
# Every step runs under its own -L namespace in a PSMUX_DATA_DIR of its own,
# with an isolated config, and cleans up only with `-L <ns> kill-server`.
# PSMUX_TEST_EXE picks the binary (default: psmux on PATH).

$ErrorActionPreference = "Continue"
$script:Passed = 0; $script:Failed = 0
function Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }
function Check($ok, $m, $detail = "") { if ($ok) { Pass $m } else { Fail "$m $detail" } }
function Head($m) { Write-Host "`n--- $m ---" -ForegroundColor Yellow }

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $c = Get-Command psmux -EA SilentlyContinue; if ($c) { $PSMUX = $c.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
$PSMUX = (Resolve-Path $PSMUX).Path
$BASE = $env:PSMUX_BASELINE_EXE
if ($BASE) { $BASE = (Resolve-Path $BASE).Path }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION',
    'PSMUX_TARGET_FULL', 'TMUX', 'TMUX_PANE', 'PSMUX_SERVER_MODE', 'PSMUX_NO_WARM', 'PSMUX_REMOTE_ATTACH') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
. "$PSScriptRoot\isolated_config.ps1" -Lines @("set -g default-shell $POWERSHELL")
$CFG_DEFAULT = $env:PSMUX_CONFIG_FILE

$ROOT = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_798_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $ROOT | Out-Null
$script:Namespaces = @()

Write-Host "`n=== Issue #798 step 1: PSMUX_SERVER_MODE / server-mode switch ===" -ForegroundColor Cyan
Write-Host "  Binary:   $PSMUX"
if ($BASE) { Write-Host "  Baseline: $BASE" }
Write-Host "  Root:     $ROOT"

# A fresh namespace and data dir for one step.
function New-Ns($tag) {
    $ns = "s798" + $tag + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $dir = Join-Path $ROOT $ns
    New-Item -ItemType Directory -Force $dir | Out-Null
    $script:Namespaces += , @($ns, $dir)
    return @{ Ns = $ns; Dir = $dir }
}

# Run the binary with a switch value. '<unset>' removes the variable.
function Run($exe, $ctx, $mode, [string[]]$a) {
    $env:PSMUX_DATA_DIR = $ctx.Dir
    if ($mode -eq '<unset>') { Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue } else { $env:PSMUX_SERVER_MODE = $mode }
    $out = & $exe -L $ctx.Ns @a 2>&1
    $code = $LASTEXITCODE
    Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue
    return @{ Code = $code; Out = (($out | ForEach-Object { "$_" }) -join "`n").Trim() }
}

function Server-Procs($exe, $ns) {
    Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" -EA SilentlyContinue |
        Where-Object { $_.ExecutablePath -eq $exe -and $_.CommandLine -match " server " -and $_.CommandLine -match "-L $ns(\s|$)" }
}

function Proc-Summary($exe, $ns) {
    $names = @(Server-Procs $exe $ns | ForEach-Object {
            if ($_.CommandLine -match ' -s (\S+)') { $Matches[1] } else { '?' } })
    ($names | Group-Object | Sort-Object Name | ForEach-Object { "$($_.Name)=$($_.Count)" }) -join ','
}

function Registry($ctx) {
    (Get-ChildItem $ctx.Dir -File -Name -EA SilentlyContinue | Sort-Object |
        ForEach-Object { $_.Replace($ctx.Ns, '<ns>') }) -join ','
}

function Wait-Warm($exe, $ctx) {
    for ($i = 0; $i -lt 50; $i++) {
        if ((Proc-Summary $exe $ctx.Ns) -match '__warm__=1') { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

function Stop-Ns($exe, $ctx) {
    $env:PSMUX_DATA_DIR = $ctx.Dir
    & $exe -L $ctx.Ns kill-server 2>&1 | Out-Null
    for ($i = 0; $i -lt 30 -and @(Server-Procs $exe $ctx.Ns).Count -gt 0; $i++) { Start-Sleep -Milliseconds 100 }
}

# The default path scenario. Returns a signature string that must not depend on
# the switch value when that value means per session.
function Scenario($exe, $mode) {
    $ctx = New-Ns "d"
    $sig = @()
    $r = Run $exe $ctx $mode @('new-session', '-d', '-s', 'A', '-x', '100', '-y', '30'); $sig += "newA=$($r.Code):$($r.Out)"
    $sig += "warm=$(Wait-Warm $exe $ctx)"
    $sig += "procs1=$(Proc-Summary $exe $ctx.Ns)"
    $r = Run $exe $ctx $mode @('new-session', '-d', '-s', 'B', '-x', '100', '-y', '30'); $sig += "newB=$($r.Code):$($r.Out)"
    $sig += "warm2=$(Wait-Warm $exe $ctx)"
    $r = Run $exe $ctx $mode @('new-window', '-t', 'A'); $sig += "neww=$($r.Code):$($r.Out)"
    $r = Run $exe $ctx $mode @('ls', '-F', '#{session_name}:#{session_windows}'); $sig += "ls=$($r.Code):$($r.Out -replace "`n", ' ')"
    $r = Run $exe $ctx $mode @('list-windows', '-t', 'A', '-F', '#{window_index}:#{window_panes}'); $sig += "lsw=$($r.Code):$($r.Out -replace "`n", ' ')"
    $r = Run $exe $ctx $mode @('display-message', '-p', '-t', 'A', '#{session_name}'); $sig += "disp=$($r.Code):$($r.Out)"
    $sig += "procs2=$(Proc-Summary $exe $ctx.Ns)"
    $sig += "reg2=$(Registry $ctx)"
    $r = Run $exe $ctx $mode @('kill-session', '-t', 'B'); $sig += "kill=$($r.Code):$($r.Out)"
    Start-Sleep -Milliseconds 700
    $r = Run $exe $ctx $mode @('ls', '-F', '#{session_name}'); $sig += "ls2=$($r.Code):$($r.Out -replace "`n", ' ')"
    $sig += "procs3=$(Proc-Summary $exe $ctx.Ns)"
    $sig += "reg3=$(Registry $ctx)"
    Stop-Ns $exe $ctx
    $sig += "procs4=$(Proc-Summary $exe $ctx.Ns)"
    return ($sig -join "`n")
}

try {
    Head "Part 1: the default path is the same with the switch unset, per-session and empty"
    $sigs = @{}
    foreach ($m in '<unset>', 'per-session', '') {
        $sigs[$m] = Scenario $PSMUX $m
    }
    Write-Host ($sigs['<unset>'] -split "`n" | ForEach-Object { "      $_" } | Out-String)
    Check ($sigs['<unset>'] -match 'newA=0:' -and $sigs['<unset>'] -match 'ls=0:A:2 B:1') "unset: sessions A and B made, A has 2 windows" $sigs['<unset>']
    Check ($sigs['<unset>'] -match 'procs2=(__warm__=1,)?A=1,B=1') "unset: one server process per session" ($sigs['<unset>'] -split "`n" | Select-String procs2)
    Check ($sigs['<unset>'] -match 'reg2=[^\n]*<ns>__A\.key,<ns>__A\.pid,<ns>__A\.port,<ns>__A\.sid,<ns>__B\.key,<ns>__B\.pid,<ns>__B\.port,<ns>__B\.sid') "unset: four registry files per session" ($sigs['<unset>'] -split "`n" | Select-String reg2)
    Check ($sigs['per-session'] -eq $sigs['<unset>']) "PSMUX_SERVER_MODE=per-session gives the identical signature" ("`n" + $sigs['per-session'])
    Check ($sigs[''] -eq $sigs['<unset>']) "PSMUX_SERVER_MODE= (empty) gives the identical signature" ("`n" + $sigs[''])
    if ($BASE) {
        $b = Scenario $BASE '<unset>'
        Check ($b -eq $sigs['<unset>']) "the signature equals the baseline binary's" ("`nbaseline:`n$b")
    }

    Head "Part 2: shared starts no server and says why"
    $ctx = New-Ns "sh"
    $r = Run $PSMUX $ctx 'shared' @('new-session', '-d', '-s', 'A')
    Check ($r.Code -eq 1) "new-session under shared exits 1" "(got $($r.Code))"
    Check ($r.Out -match 'shared server mode is not available in this build yet \(step 1 of #798\)') "new-session under shared names the reason" $r.Out
    $r = Run $PSMUX $ctx ' SHARED ' @('new-session', '-d', '-s', 'A')
    Check ($r.Code -eq 1 -and $r.Out -match 'not available') "case and spaces are ignored (' SHARED ')" $r.Out
    $r = Run $PSMUX $ctx 'shared' @('new', '-d')
    Check ($r.Code -eq 1) "the new alias is gated too"
    $r = Run $PSMUX $ctx 'shared' @('start-server')
    Check ($r.Code -eq 1 -and $r.Out -match 'not available') "start-server under shared exits 1" $r.Out
    $env:PSMUX_DATA_DIR = $ctx.Dir; $env:PSMUX_SERVER_MODE = 'shared'
    $cc = ('' | & $PSMUX -L $ctx.Ns -CC 2>&1 | Out-String).Trim(); $ccCode = $LASTEXITCODE
    Remove-Item Env:PSMUX_SERVER_MODE
    Check ($ccCode -eq 1 -and $cc -match 'not available') "bare -CC (control mode start) under shared exits 1" "($ccCode) $cc"
    $env:PSMUX_SERVER_MODE = 'shared'
    $srv = Start-Process -FilePath $PSMUX -ArgumentList @('server', '-s', 'X', '-L', $ctx.Ns) -WindowStyle Hidden -PassThru
    Remove-Item Env:PSMUX_SERVER_MODE
    # A binary without the gate runs that server for ever: bound the wait and
    # stop only the process this step started.
    $exited = $srv.WaitForExit(10000)
    if (-not $exited) { Stop-Process -Id $srv.Id -Force -EA SilentlyContinue; $srv.WaitForExit(3000) | Out-Null }
    Check ($exited -and $srv.ExitCode -eq 1) "the internal server process refuses shared too" "(exited=$exited code=$($srv.ExitCode))"
    Start-Sleep -Milliseconds 300
    Check ((Proc-Summary $PSMUX $ctx.Ns) -eq '') "no server process exists in the namespace" (Proc-Summary $PSMUX $ctx.Ns)
    $reg = Registry $ctx
    Check ($reg -notmatch '\.port|\.key|\.pid|\.sid') "no registry file was written" $reg
    $r = Run $PSMUX $ctx '<unset>' @('ls')
    Check ($r.Code -eq 1 -and $r.Out -match 'no server running') "ls afterwards: no server running" $r.Out

    Head "Part 3: an unknown value is an error, never a fallback"
    $ctx = New-Ns "bg"
    foreach ($bad in 'bogus', 'per_session', 'on') {
        $r = Run $PSMUX $ctx $bad @('new-session', '-d', '-s', 'A')
        Check ($r.Code -eq 1 -and $r.Out -eq "psmux: PSMUX_SERVER_MODE: unknown value '$bad' (expected per-session or shared)") "PSMUX_SERVER_MODE=${bad}: exact error at exit 1" "($($r.Code)) $($r.Out)"
    }
    $r = Run $PSMUX $ctx 'bogus' @('start-server')
    Check ($r.Code -eq 1 -and $r.Out -match 'unknown value') "start-server with a bad value exits 1" $r.Out
    Start-Sleep -Milliseconds 300
    Check ((Proc-Summary $PSMUX $ctx.Ns) -eq '') "no server process after the bad values" (Proc-Summary $PSMUX $ctx.Ns)

    Head "Part 4: the config switch (set -s server-mode)"
    $cfgShared = Join-Path $ROOT "shared.conf"; Set-Content $cfgShared "set -g default-shell $POWERSHELL`nset -s server-mode shared" -Encoding ASCII
    $cfgBad = Join-Path $ROOT "bad.conf"; Set-Content $cfgBad "set -s server-mode sharde" -Encoding ASCII
    $cfgPer = Join-Path $ROOT "per.conf"; Set-Content $cfgPer "set -g default-shell $POWERSHELL`nset -s server-mode per-session" -Encoding ASCII
    $ctx = New-Ns "cf"
    $env:PSMUX_CONFIG_FILE = $cfgShared
    $r = Run $PSMUX $ctx '<unset>' @('new-session', '-d', '-s', 'A')
    Check ($r.Code -eq 1 -and $r.Out -match 'not available') "config server-mode shared: new-session exits 1" $r.Out
    $r = Run $PSMUX $ctx 'per-session' @('new-session', '-d', '-s', 'A')
    Check ($r.Code -eq 0) "PSMUX_SERVER_MODE=per-session outranks the config" "($($r.Code)) $($r.Out)"
    Stop-Ns $PSMUX $ctx
    $env:PSMUX_CONFIG_FILE = $cfgBad
    $r = Run $PSMUX $ctx '<unset>' @('new-session', '-d', '-s', 'A')
    Check ($r.Code -eq 1 -and $r.Out -match "server-mode: unknown value 'sharde' in .*bad\.conf") "a bad config value names itself and the file" $r.Out
    $env:PSMUX_CONFIG_FILE = $cfgPer
    $r = Run $PSMUX $ctx '<unset>' @('new-session', '-d', '-s', 'P')
    Check ($r.Code -eq 0 -and $r.Out -eq '') "config server-mode per-session starts quietly" "($($r.Code)) $($r.Out)"
    $r = Run $PSMUX $ctx '<unset>' @('show-messages')
    Check ($r.Out -notmatch 'server-mode') "no config warning about server-mode" $r.Out
    $r = Run $PSMUX $ctx '<unset>' @('source-file', $cfgPer)
    Check ($r.Code -eq 0 -and $r.Out -eq '') "re-sourcing that config is silent" "($($r.Code)) $($r.Out)"
    Stop-Ns $PSMUX $ctx
    $env:PSMUX_CONFIG_FILE = $CFG_DEFAULT

    Head "Part 5: show and set on a running per session server (CLI, TCP)"
    $ctx = New-Ns "sw"
    $r = Run $PSMUX $ctx '<unset>' @('new-session', '-d', '-s', 'A'); Check ($r.Code -eq 0) "per session server up"
    $r = Run $PSMUX $ctx '<unset>' @('show', '-s', 'server-mode'); Check ($r.Code -eq 0 -and $r.Out -eq 'server-mode per-session') "show -s server-mode" $r.Out
    $r = Run $PSMUX $ctx '<unset>' @('show', '-sv', 'server-mode'); Check ($r.Out -eq 'per-session') "show -sv server-mode" $r.Out
    $r = Run $PSMUX $ctx '<unset>' @('display-message', '-p', '#{server-mode}'); Check ($r.Out -eq 'per-session') "#{server-mode} format" $r.Out
    $r = Run $PSMUX $ctx 'shared' @('show', '-sv', 'server-mode'); Check ($r.Out -eq 'per-session') "a shell with shared still sees this server's real mode" $r.Out
    $r = Run $PSMUX $ctx '<unset>' @('show', '-s'); $listing = $r.Out
    Check ($listing -notmatch 'server-mode') "bare show -s listing does not gain a line"
    $r = Run $PSMUX $ctx '<unset>' @('show', '-g'); Check ($r.Out -notmatch 'server-mode') "show -g listing does not gain a line"
    $r = Run $PSMUX $ctx '<unset>' @('set', '-s', 'server-mode', 'per-session'); Check ($r.Code -eq 0 -and $r.Out -eq '') "set -s server-mode per-session is a silent no-op" "($($r.Code)) $($r.Out)"
    $r = Run $PSMUX $ctx '<unset>' @('set', '-s', 'server-mode', 'shared')
    Check ($r.Code -eq 1 -and $r.Out -match 'cannot change on a running server') "set -s server-mode shared on a running server is refused" "($($r.Code)) $($r.Out)"
    $r = Run $PSMUX $ctx '<unset>' @('set', '-s', 'server-mode', 'bogus')
    Check ($r.Code -eq 1 -and $r.Out -match 'unknown value') "set -s server-mode bogus is refused" "($($r.Code)) $($r.Out)"
    $r = Run $PSMUX $ctx '<unset>' @('show', '-sv', 'server-mode'); Check ($r.Out -eq 'per-session') "mode unchanged after the refused sets" $r.Out
    # Raw TCP, the path a plugin or control client takes.
    $port = [int](Get-Content (Join-Path $ctx.Dir "$($ctx.Ns)__A.port")).Trim()
    $key = (Get-Content (Join-Path $ctx.Dir "$($ctx.Ns)__A.key")).Trim()
    $tcp = New-Object Net.Sockets.TcpClient; $tcp.Connect('127.0.0.1', $port)
    $st = $tcp.GetStream(); $w = New-Object IO.StreamWriter($st); $w.NewLine = "`n"
    $w.WriteLine("AUTH $key"); $w.WriteLine("show-options -sv server-mode"); $w.Flush()
    $rd = New-Object IO.StreamReader($st); $st.ReadTimeout = 2000
    $tcpOut = @(); try { while ($true) { $l = $rd.ReadLine(); if ($null -eq $l) { break }; $tcpOut += $l } } catch {}
    $tcp.Close()
    Check (($tcpOut -join '|') -match 'per-session') "TCP show-options -sv server-mode answers per-session" ($tcpOut -join '|')
    # A warm standby exists here (Part 1). A shared request must not claim it.
    $warmBefore = Wait-Warm $PSMUX $ctx
    $before = Proc-Summary $PSMUX $ctx.Ns
    $r = Run $PSMUX $ctx 'shared' @('new-session', '-d', '-s', 'C')
    Start-Sleep -Milliseconds 300
    $after = Proc-Summary $PSMUX $ctx.Ns
    Check ($r.Code -eq 1 -and $before -eq $after) "shared new-session leaves the warm standby and sessions alone (warm=$warmBefore)" "before=$before after=$after"
    $r = Run $PSMUX $ctx '<unset>' @('ls', '-F', '#{session_name}'); Check ($r.Out -eq 'A') "session list unchanged" $r.Out
    $r = Run $PSMUX $ctx 'shared' @('kill-session', '-t', 'A'); Check ($r.Code -eq 0) "commands that start no server still work under shared (kill-session)"

    Head "Part 6: starts a running server makes for the user honour the switch"
    # The config is flipped to shared while a per session server runs. A start
    # the server makes itself (hook, binding, command prompt: commands.rs; a
    # TCP new-session: connection.rs) must not quietly make a per session one.
    $cfgLive = Join-Path $ROOT "live.conf"; Set-Content $cfgLive "set -g default-shell $POWERSHELL" -Encoding ASCII
    $env:PSMUX_CONFIG_FILE = $cfgLive
    $ctx = New-Ns "rt"
    $r = Run $PSMUX $ctx '<unset>' @('new-session', '-d', '-s', 'A'); Check ($r.Code -eq 0) "per session server up"
    Run $PSMUX $ctx '<unset>' @('set-hook', '-g', 'after-new-window', 'new-session -d -s K') | Out-Null
    $warm = Wait-Warm $PSMUX $ctx
    Set-Content $cfgLive "set -g default-shell $POWERSHELL`nset -s server-mode shared" -Encoding ASCII
    Run $PSMUX $ctx '<unset>' @('new-window', '-t', 'A') | Out-Null
    Start-Sleep -Milliseconds 1500
    $r = Run $PSMUX $ctx '<unset>' @('ls', '-F', '#{session_name}')
    Check ($r.Out -eq 'A') "hook new-session (execute_command_string) starts nothing under shared (warm=$warm)" $r.Out
    function Tcp-Cmd($ctx, $line) {
        $port = [int](Get-Content (Join-Path $ctx.Dir "$($ctx.Ns)__A.port")).Trim()
        $key = (Get-Content (Join-Path $ctx.Dir "$($ctx.Ns)__A.key")).Trim()
        $t = New-Object Net.Sockets.TcpClient; $t.Connect('127.0.0.1', $port)
        $s = $t.GetStream(); $w = New-Object IO.StreamWriter($s); $w.NewLine = "`n"
        $w.WriteLine("AUTH $key"); $w.WriteLine($line); $w.Flush()
        $rd = New-Object IO.StreamReader($s); $s.ReadTimeout = 8000
        $o = @(); try { while ($true) { $l = $rd.ReadLine(); if ($null -eq $l) { break }; $o += $l } } catch {}
        $t.Close(); return ($o -join '|')
    }
    $o = Tcp-Cmd $ctx "new-session -s T1"
    Check ($o -match 'ERROR: shared server mode is not available') "TCP new-session (server spawn route) answers the error" $o
    $o = Tcp-Cmd $ctx "new-session -d -s T2"
    Check ($o -match 'ERROR: shared server mode is not available') "TCP new-session -d (CLI route) answers the error" $o
    Start-Sleep -Milliseconds 500
    $r = Run $PSMUX $ctx '<unset>' @('ls', '-F', '#{session_name}')
    Check ($r.Out -eq 'A') "no session was made by the TCP routes" $r.Out
    Set-Content $cfgLive "set -g default-shell $POWERSHELL" -Encoding ASCII
    Run $PSMUX $ctx '<unset>' @('new-window', '-t', 'A') | Out-Null
    $ok = $false; for ($i = 0; $i -lt 50 -and -not $ok; $i++) { Start-Sleep -Milliseconds 100; $ok = ((Run $PSMUX $ctx '<unset>' @('ls', '-F', '#{session_name}')).Out -match 'K') }
    Check $ok "with the config back to per session the same hook makes K"
    $env:PSMUX_CONFIG_FILE = $CFG_DEFAULT
} finally {
    Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue
    foreach ($pair in $script:Namespaces) {
        $env:PSMUX_DATA_DIR = $pair[1]
        & $PSMUX -L $pair[0] kill-server 2>&1 | Out-Null
    }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $ROOT -EA SilentlyContinue
    if (Get-Command Remove-PsmuxIsolatedConfig -EA SilentlyContinue) { Remove-PsmuxIsolatedConfig }
}

Write-Host "`n=== Results: $script:Passed passed, $script:Failed failed ===" -ForegroundColor Cyan
if ($script:Failed -gt 0) { exit 1 } else { exit 0 }
