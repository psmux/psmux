# Issue #798 step 2: the shared server.
#
# PSMUX_SERVER_MODE=shared (or `set -s server-mode shared` in the config) runs
# ONE server process per namespace that holds every session of the namespace.
# This suite proves, against the real binary:
#
#   * two and three sessions in one namespace live in exactly one server
#     process (counted by PID and command line under this suite's data dir),
#     with today's per session registry files, and window and pane ids unique
#     across the server;
#   * new-window, split-window, send-keys and capture-pane act on each session
#     independently; list-sessions and has-session see every session;
#   * a pane gets ITS session's PSMUX_TARGET_SESSION and start directory, never
#     another session's, and PSMUX_SERVER_MODE in a pane is what the launching
#     shell had (set, or unset when the config chose the mode);
#   * `set -g`, `setenv -g` reach every session (tmux: one global table per
#     server), including a session created later, while a session option stays
#     in its session;
#   * kill-session of one session leaves the others; the server exits with its
#     last session;
#   * a real visible client attaches to each session;
#   * two namespaces side by side each get their own single shared server;
#   * a command the shared server does not run yet is answered byte for byte in
#     the per session server's unknown command shape, with the same CLI exit
#     code;
#   * a shared request is never served by the per session warm standby.
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

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION',
    'PSMUX_TARGET_FULL', 'TMUX', 'TMUX_PANE', 'PSMUX_SERVER_MODE', 'PSMUX_NO_WARM', 'PSMUX_REMOTE_ATTACH') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}
$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
. "$PSScriptRoot\isolated_config.ps1" -Lines @("set -g default-shell $POWERSHELL")
$CFG_DEFAULT = $env:PSMUX_CONFIG_FILE

$ROOT = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_798s_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $ROOT | Out-Null
$script:Namespaces = @()
$script:Windows = @()

# The user's default namespace, read passively: its registry file names only.
function Default-Snapshot {
    $d = Join-Path $env:USERPROFILE ".psmux"
    (Get-ChildItem $d -Filter *.port -Name -EA SilentlyContinue | Where-Object { $_ -notmatch '__' } | Sort-Object) -join ','
}
$DEFAULT_BEFORE = Default-Snapshot

Write-Host "`n=== Issue #798 step 2: the shared server ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  Root:   $ROOT"

function New-Ns($tag) {
    $ns = "s798" + $tag + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $dir = Join-Path $ROOT $ns
    New-Item -ItemType Directory -Force $dir | Out-Null
    $script:Namespaces += , @($ns, $dir)
    return @{ Ns = $ns; Dir = $dir }
}

# Run the binary with a switch value. '<unset>' removes the variable. The
# variable is only ever set for the one command and removed right after.
function Run($ctx, $mode, [string[]]$a) {
    $env:PSMUX_DATA_DIR = $ctx.Dir
    if ($mode -eq '<unset>') { Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue } else { $env:PSMUX_SERVER_MODE = $mode }
    $out = & $PSMUX -L $ctx.Ns @a 2>&1
    $code = $LASTEXITCODE
    Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue
    return @{ Code = $code; Out = (($out | ForEach-Object { "$_" }) -join "`n").Trim() }
}
function S($ctx, [string[]]$a) { Run $ctx '<unset>' $a }

# Every psmux server process of a namespace started from this binary.
function Server-Procs($ns) {
    @(Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" -EA SilentlyContinue |
        Where-Object { $_.ExecutablePath -eq $PSMUX -and $_.CommandLine -match ' server ' -and $_.CommandLine -match "-L $ns(\s|$)" })
}
function Shared-Procs($ns) { @(Server-Procs $ns | Where-Object { $_.CommandLine -match '--shared-server' }) }

function Pid-Of($ctx, $session) {
    $f = Join-Path $ctx.Dir "$($ctx.Ns)__$session.pid"
    if (-not (Test-Path $f)) { return $null }
    return [int](((Get-Content $f -Raw).Trim() -split ':')[0])
}

# Raw request on a session's own listener; returns the exact reply text.
function Tcp-Raw($ctx, $session, [string[]]$lines, $timeoutMs = 4000) {
    $port = [int](Get-Content (Join-Path $ctx.Dir "$($ctx.Ns)__$session.port")).Trim()
    $key = (Get-Content (Join-Path $ctx.Dir "$($ctx.Ns)__$session.key")).Trim()
    $t = New-Object Net.Sockets.TcpClient; $t.Connect('127.0.0.1', $port)
    $s = $t.GetStream(); $s.ReadTimeout = $timeoutMs
    $payload = "AUTH $key`n" + (($lines | ForEach-Object { "$_`n" }) -join '')
    $b = [Text.Encoding]::UTF8.GetBytes($payload); $s.Write($b, 0, $b.Length); $s.Flush()
    $ms = New-Object IO.MemoryStream; $buf = New-Object byte[] 4096
    try { while (($n = $s.Read($buf, 0, $buf.Length)) -gt 0) { $ms.Write($buf, 0, $n) } } catch {}
    $t.Close()
    return [Text.Encoding]::UTF8.GetString($ms.ToArray())
}

function Wait-Until([scriptblock]$cond, $ms = 5000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return (& $cond)
}

function Capture($ctx, $target) { (S $ctx @('capture-pane', '-p', '-t', $target)).Out }

try {
    Head "Part 0: the default path is untouched (owner ruling)"
    # Every commit of this work since the step 1 base touches src/ only inside
    # src/server/shared. Checked per commit, so it stays true after a merge.
    $repo = Split-Path $PSScriptRoot -Parent
    $base = if ($env:PSMUX_798_BASE) { $env:PSMUX_798_BASE } else { '0145958f' }
    $git = Get-Command git -EA SilentlyContinue
    if ($git -and (git -C $repo rev-parse --verify --quiet "$base^{commit}" 2>$null)) {
        $commits = @(git -C $repo log --format=%H "$base..HEAD" -- src/server/shared)
        $outside = @()
        foreach ($c in $commits) {
            $outside += @(git -C $repo diff-tree --no-commit-id --name-only -r $c -- src ':!src/server/shared' | ForEach-Object { "$($c.Substring(0, 8)) $_" })
        }
        Check ($commits.Count -gt 0) "found the shared server commits since $base ($($commits.Count))"
        Check ($outside.Count -eq 0) "none of them changes a file under src/ outside src/server/shared" ($outside -join ', ')
        $diff = @(git -C $repo diff --name-only $base -- src ':!src/server/shared')
        Write-Host "      src/ outside src/server/shared changed since ${base} (all commits): $(if ($diff.Count) { $diff -join ', ' } else { 'nothing' })"
    } else {
        Write-Host "      (skipped: no git or base $base not in this checkout)" -ForegroundColor DarkGray
    }

    Head "Part 1: three sessions, one server process"
    $ctx = New-Ns "a"
    $dirA = Join-Path $ROOT "dirA"; $dirB = Join-Path $ROOT "dir B"; $dirC = Join-Path $ROOT "dirC"
    foreach ($d in $dirA, $dirB, $dirC) { New-Item -ItemType Directory -Force $d | Out-Null }
    $r = Run $ctx 'shared' @('new-session', '-d', '-s', 'A', '-x', '120', '-y', '30', '-c', $dirA)
    Check ($r.Code -eq 0) "new-session A under shared exits 0" "($($r.Code)) $($r.Out)"
    Check (@(Shared-Procs $ctx.Ns).Count -eq 1) "one shared server after the first session" (@(Server-Procs $ctx.Ns).CommandLine -join ' | ')
    $r = Run $ctx 'shared' @('new-session', '-d', '-s', 'B', '-x', '120', '-y', '30', '-c', $dirB)
    Check ($r.Code -eq 0) "new-session B joins it" "($($r.Code)) $($r.Out)"
    $r = Run $ctx 'shared' @('new-session', '-d', '-s', 'C', '-x', '120', '-y', '30', '-c', $dirC)
    Check ($r.Code -eq 0) "new-session C joins it" "($($r.Code)) $($r.Out)"
    $procs = Server-Procs $ctx.Ns
    Check ($procs.Count -eq 1 -and $procs[0].CommandLine -match '--shared-server') "exactly ONE server process holds A, B and C" ($procs.CommandLine -join ' | ')
    $spid = if ($procs.Count -ge 1) { $procs[0].ProcessId } else { -1 }
    Check ((Pid-Of $ctx 'A') -eq $spid -and (Pid-Of $ctx 'B') -eq $spid -and (Pid-Of $ctx 'C') -eq $spid) "every session's .pid names that process ($spid)"
    $reg = (Get-ChildItem $ctx.Dir -File -Name | Where-Object { $_ -match '\.(port|key|pid|sid)$' } | Sort-Object) -join ','
    $expect = (@('A', 'B', 'C') | ForEach-Object { $s = $_; 'key', 'pid', 'port', 'sid' | ForEach-Object { "$($ctx.Ns)__$s.$_" } }) -join ','
    Check ($reg -eq $expect) "today's four registry files per session" $reg
    $ports = @('A', 'B', 'C') | ForEach-Object { (Get-Content (Join-Path $ctx.Dir "$($ctx.Ns)__$_.port")).Trim() }
    Check (@($ports | Select-Object -Unique).Count -eq 3) "each session has its own listener port" ($ports -join ',')
    Check (Test-Path (Join-Path $ctx.Dir "shared\L-$($ctx.Ns).server")) "the namespace marker is shared\L-<ns>.server"
    Check ((Run $ctx 'shared' @('new-session', '-d', '-s', 'B')).Out -match 'duplicate session: B') "a second B is a duplicate session"
    Check (@(Server-Procs $ctx.Ns).Count -eq 1) "still one process after the refused duplicate"

    Head "Part 2: list-sessions, has-session, ids unique across the server"
    $r = S $ctx @('ls', '-F', '#{session_name}')
    Check ($r.Code -eq 0 -and (($r.Out -split "`n" | Sort-Object) -join ',') -eq 'A,B,C') "ls lists A, B, C" $r.Out
    $r = S $ctx @('ls')
    Check ($r.Out -match '(?m)^A: 1 windows' -and $r.Out -match '(?m)^B: 1 windows' -and $r.Out -match '(?m)^C: 1 windows') "plain ls lines" $r.Out
    foreach ($s in 'A', 'B', 'C') { Check ((S $ctx @('has-session', '-t', $s)).Code -eq 0) "has-session -t $s exits 0" }
    Check ((S $ctx @('has-session', '-t', 'Z')).Code -eq 1) "has-session -t Z exits 1"
    $ids = @('A', 'B', 'C') | ForEach-Object { (S $ctx @('display-message', '-p', '-t', $_, '#{session_name} #{window_id} #{pane_id}')).Out }
    Write-Host "      $($ids -join ' | ')"
    $panes = $ids | ForEach-Object { ($_ -split ' ')[2] }; $wins = $ids | ForEach-Object { ($_ -split ' ')[1] }
    Check (@($panes | Select-Object -Unique).Count -eq 3 -and @($wins | Select-Object -Unique).Count -eq 3) "window and pane ids are unique across the server" ($ids -join ' | ')
    Check ((S $ctx @('show', '-sv', 'server-mode')).Out -eq 'shared') "show -sv server-mode answers shared"

    Head "Part 3: windows, panes, keys and captures per session"
    foreach ($s in 'A', 'B', 'C') {
        Check ((S $ctx @('new-window', '-t', $s)).Code -eq 0) "new-window -t $s"
        Check ((S $ctx @('split-window', '-t', $s)).Code -eq 0) "split-window -t $s"
    }
    foreach ($s in 'A', 'B', 'C') {
        $w = (S $ctx @('list-windows', '-t', $s, '-F', '#{window_index}:#{window_panes}')).Out -replace "`n", ' '
        Check ($w -eq '0:1 1:2') "$s has its own two windows, the second split" $w
    }
    $all = @('A', 'B', 'C') | ForEach-Object { (S $ctx @('list-panes', '-s', '-t', $_, '-F', '#{pane_id}')).Out -split "`n" } | Where-Object { $_ }
    Check ($all.Count -eq 9 -and @($all | Select-Object -Unique).Count -eq 9) "nine panes, nine distinct %ids" ($all -join ',')
    # A window made without -c and without a client directory (raw TCP, as
    # a script or plugin sends it) starts in the SESSION's directory, as a per
    # session server's does: each request runs with its own session's cwd.
    foreach ($s in 'B', 'A', 'C') { $null = Tcp-Raw $ctx $s @('new-window -d') }
    $probe = '$p=(Get-Location).Path; "CWD=$p"; "TS=$env:PSMUX_TARGET_SESSION"; "MODE=[$env:PSMUX_SERVER_MODE]"; "MARK-'
    foreach ($s in 'A', 'B', 'C') {
        foreach ($w in '1', '2') { S $ctx @('send-keys', '-t', "${s}:$w", ($probe + $s + '"'), 'Enter') | Out-Null }
    }
    foreach ($s in 'A', 'B', 'C') {
        $dir = @{ A = $dirA; B = $dirB; C = $dirC }[$s]
        $ok = Wait-Until { (Capture $ctx "${s}:1") -match "(?m)^MARK-$s" } 15000
        $cap = Capture $ctx "${s}:1"
        Check $ok "send-keys reached ${s}:1 and capture-pane shows it"
        $others = @('A', 'B', 'C') | Where-Object { $_ -ne $s } | ForEach-Object { "(?m)^MARK-$_" }
        Check (-not ($others | Where-Object { $cap -match $_ })) "${s}:1 shows none of the other sessions' output"
        Check ($cap -match "(?m)^TS=$($ctx.Ns)__$s\s*$") "a pane of $s gets ITS PSMUX_TARGET_SESSION" (($cap -split "`n" | Select-String '^TS=') -join ' ')
        Check ($cap -match '(?m)^MODE=\[shared\]') "PSMUX_SERVER_MODE in a pane is the launching shell's value (shared)"
        $null = Wait-Until { (Capture $ctx "${s}:2") -match "(?m)^MARK-$s" } 15000
        $cap2 = Capture $ctx "${s}:2"
        Check ($cap2 -match ("(?m)^CWD=" + [regex]::Escape($dir) + "\s*$")) "a window made over TCP without -c starts in $s's own directory" (($cap2 -split "`n" | Select-String '^CWD=') -join ' ')
    }

    Head "Part 4: server wide and session scoped options"
    Check ((S $ctx @('set', '-g', '-t', 'A', '@g1', 'fromA')).Code -eq 0) "set -g -t A @g1 fromA"
    $vals = @('A', 'B', 'C') | ForEach-Object { (S $ctx @('show', '-gv', '-t', $_, '@g1')).Out }
    Check (($vals -join ',') -eq 'fromA,fromA,fromA') "set -g reaches every session (tmux global table)" ($vals -join ',')
    S $ctx @('set', '-g', 'status-left', 'GLOBAL-LEFT') | Out-Null
    $vals = @('A', 'B', 'C') | ForEach-Object { (S $ctx @('show', '-gv', '-t', $_, 'status-left')).Out }
    Check (($vals -join ',') -eq 'GLOBAL-LEFT,GLOBAL-LEFT,GLOBAL-LEFT') "set -g status-left is server wide" ($vals -join ',')
    S $ctx @('set', '-t', 'B', '@s1', 'onlyB') | Out-Null
    Check ((S $ctx @('show', '-v', '-t', 'B', '@s1')).Out -eq 'onlyB') "a session option is set on B"
    Check ((S $ctx @('show', '-v', '-t', 'A', '@s1')).Out -ne 'onlyB') "and stays out of A"
    S $ctx @('setenv', '-g', 'S798_GLOBAL', 'genv') | Out-Null
    $vals = @('A', 'C') | ForEach-Object { (S $ctx @('showenv', '-g', '-t', $_, 'S798_GLOBAL')).Out }
    Check (($vals -join ',') -eq 'S798_GLOBAL=genv,S798_GLOBAL=genv') "setenv -g reaches every session" ($vals -join ',')
    $r = Run $ctx 'shared' @('new-session', '-d', '-s', 'D', '-x', '120', '-y', '30')
    Check ($r.Code -eq 0 -and (S $ctx @('show', '-gv', '-t', 'D', '@g1')).Out -eq 'fromA') "a session created later sees the earlier set -g"
    Check (@(Server-Procs $ctx.Ns).Count -eq 1) "D joined the same process"

    Head "Part 5: unknown and unsupported commands, per session shape"
    $pctx = New-Ns "p"
    $r = Run $pctx '<unset>' @('new-session', '-d', '-s', 'P', '-x', '100', '-y', '30')
    Check ($r.Code -eq 0) "a per session server for comparison"
    $perLine = Tcp-Raw $pctx 'P' @('bogus ; list-windows')
    $shLine = Tcp-Raw $ctx 'A' @('bogus ; list-windows')
    Write-Host "      per-session: $(($perLine -replace "`n", '\n'))   shared: $(($shLine -replace "`n", '\n'))"
    Check ($perLine -ceq "OK`nERROR: unknown command: bogus`n") "per session reply to an unknown command in a list" ($perLine -replace "`n", '\n')
    Check ($shLine -ceq $perLine) "shared reply is byte for byte the same" ($shLine -replace "`n", '\n')
    $sh2 = Tcp-Raw $ctx 'A' @('rotate-window')
    Check ($sh2 -ceq "OK`nERROR: unknown command: rotate-window`n") "a command step 2 does not run gets the same shape" ($sh2 -replace "`n", '\n')
    $sh3 = Tcp-Raw $ctx 'A' @('TARGET A:0', 'join-pane -s B:0')
    Check ($sh3 -ceq "OK`nERROR: unknown command: join-pane`n") "also behind a TARGET line" ($sh3 -replace "`n", '\n')
    $rp = Run $pctx '<unset>' @('bogus'); $rs = S $ctx @('bogus')
    Check ($rp.Code -eq $rs.Code -and $rp.Out -eq $rs.Out) "CLI: an unknown command exits the same in both modes ($($rp.Code))" "$($rp.Out) / $($rs.Out)"
    $rp = Run $pctx '<unset>' @('display-message', '-p', '-t', 'P', 'x', ';', 'bogus'); $rs = S $ctx @('display-message', '-p', '-t', 'A', 'x', ';', 'bogus')
    Check ($rp.Code -eq $rs.Code) "CLI: a list naming an unknown command exits the same in both modes ($($rp.Code) / $($rs.Code))" "$($rp.Out) / $($rs.Out)"
    Check ((Capture $ctx 'A:0') -ne $null) "the server still answers after the refusals"

    Head "Part 6: a real attached client on each session"
    foreach ($s in 'A', 'C') {
        $env:PSMUX_DATA_DIR = $ctx.Dir
        $p = Start-Process -FilePath $PSMUX -ArgumentList @('-L', $ctx.Ns, 'attach-session', '-t', $s) -PassThru
        $script:Windows += $p
        $ok = Wait-Until { (S $ctx @('display-message', '-p', '-t', $s, '#{session_attached}')).Out -eq '1' } 10000
        Check $ok "a visible client attached to $s (pid $($p.Id))"
        $other = if ($s -eq 'A') { 'C' } else { 'A' }
        Check ((S $ctx @('display-message', '-p', '-t', $other, '#{session_attached}')).Out -eq '0') "and not to $other"
        S $ctx @('send-keys', '-t', "${s}:0", "Write-Output ATTACHED-$s", 'Enter') | Out-Null
        Check (Wait-Until { (Capture $ctx "${s}:0") -match "ATTACHED-$s" } 10000) "keys still reach $s while attached"
        Stop-Process -Id $p.Id -Force -EA SilentlyContinue
        $p.WaitForExit(5000) | Out-Null
        Check (Wait-Until { (S $ctx @('display-message', '-p', '-t', $s, '#{session_attached}')).Out -eq '0' } 10000) "closing the client detaches it from $s"
    }

    # Protocol level: two persistent clients, one per session, as the attached
    # client speaks it. A frame of A's screen must reach A's client by push,
    # and never C's client (a per session server pushes to every client of its
    # process, which here would cross sessions).
    function Open-Persistent($session) {
        $port = [int](Get-Content (Join-Path $ctx.Dir "$($ctx.Ns)__$session.port")).Trim()
        $key = (Get-Content (Join-Path $ctx.Dir "$($ctx.Ns)__$session.key")).Trim()
        $t = New-Object Net.Sockets.TcpClient; $t.Connect('127.0.0.1', $port)
        $st = $t.GetStream(); $w = New-Object IO.StreamWriter($st); $w.NewLine = "`n"
        $w.WriteLine("AUTH $key"); $w.WriteLine("PERSISTENT"); $w.WriteLine("client-size 120 30"); $w.WriteLine("client-attach"); $w.WriteLine("dump-state"); $w.Flush()
        return @{ T = $t; W = $w; R = (New-Object IO.StreamReader($st)); S = $st; Lines = (New-Object Collections.Generic.List[string]) }
    }
    function Pump($c, $ms) {
        $c.S.ReadTimeout = 200
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt $ms) { try { $l = $c.R.ReadLine(); if ($null -eq $l) { break }; $c.Lines.Add($l) } catch { } }
    }
    $ca = Open-Persistent 'A'; $cc = Open-Persistent 'C'
    Pump $ca 1500; Pump $cc 300
    Check (@($ca.Lines | Where-Object { $_ -like '{*' }).Count -ge 1) "a persistent client of A gets its first frame" "($($ca.Lines.Count) lines)"
    $ca.Lines.Clear(); $cc.Lines.Clear()
    S $ctx @('send-keys', '-t', 'A', 'Write-Output PUSHEDONLYTOA', 'Enter') | Out-Null
    Pump $ca 3000; Pump $cc 1500
    $aGot = @($ca.Lines | Where-Object { $_ -like '{*' -and $_ -match 'PUSHEDONLYTOA' }).Count
    $cGot = @($cc.Lines | Where-Object { $_ -match 'PUSHEDONLYTOA' }).Count
    Check ($aGot -ge 1) "A's client receives the pushed frame with A's output (no dump-state asked)" "($aGot of $($ca.Lines.Count) lines)"
    Check ($cGot -eq 0) "C's client never receives A's frames" "($cGot)"
    # switch-client between two sessions of the same shared server: the
    # attached client is told to move (the SWITCH directive it follows today
    # between per session servers), and the destination is never asked over
    # TCP by the loop that serves it.
    $ca.Lines.Clear()
    $ca.W.WriteLine("switch-client -t C"); $ca.W.Flush()
    Pump $ca 2000
    Check (@($ca.Lines | Where-Object { $_ -eq "SWITCH $($ctx.Ns)__C" }).Count -eq 1) "switch-client -t C sends A's client to C" (($ca.Lines | Where-Object { $_ -notlike '{*' }) -join ' | ')
    $ca.Lines.Clear()
    $ca.W.WriteLine("switch-client -n"); $ca.W.Flush()
    Pump $ca 2000
    Check (@($ca.Lines | Where-Object { $_ -like "SWITCH $($ctx.Ns)__*" }).Count -eq 1) "switch-client -n moves it to the next session" (($ca.Lines | Where-Object { $_ -notlike '{*' }) -join ' | ')
    Check ((S $ctx @('display-message', '-p', '-t', 'C', '#{session_name}')).Out -eq 'C') "the server answers at once afterwards (no wait on itself)"
    foreach ($c in $ca, $cc) { try { $c.W.WriteLine("client-detach"); $c.W.Flush() } catch {}; $c.T.Close() }

    Head "Part 7: kill-session leaves the others; the last one ends the server"
    $r = S $ctx @('kill-session', '-t', 'B')
    Check ($r.Code -eq 0) "kill-session -t B exits 0" $r.Out
    $r = S $ctx @('ls', '-F', '#{session_name}')
    Check ((($r.Out -split "`n" | Sort-Object) -join ',') -eq 'A,C,D') "A, C and D remain" $r.Out
    Check (-not (Test-Path (Join-Path $ctx.Dir "$($ctx.Ns)__B.port"))) "B's registry files are gone"
    Check (@(Server-Procs $ctx.Ns).Count -eq 1 -and (Server-Procs $ctx.Ns)[0].ProcessId -eq $spid) "the same one process still runs"
    S $ctx @('send-keys', '-t', 'C:0', 'Write-Output AFTER-KILL', 'Enter') | Out-Null
    Check (Wait-Until { (Capture $ctx 'C:0') -match 'AFTER-KILL' } 10000) "C still works after B was killed"
    foreach ($s in 'A', 'C') { S $ctx @('kill-session', '-t', $s) | Out-Null }
    Check (@(Server-Procs $ctx.Ns).Count -eq 1) "D alone keeps the server up"
    S $ctx @('kill-session', '-t', 'D') | Out-Null
    Check (Wait-Until { @(Server-Procs $ctx.Ns).Count -eq 0 } 5000) "the server exits with its last session"
    Check (-not (Test-Path (Join-Path $ctx.Dir "shared\L-$($ctx.Ns).server"))) "and removes its namespace marker"
    Check (@(Get-ChildItem $ctx.Dir -Filter *.port -Name).Count -eq 0) "no .port file is left"
    $r = S $ctx @('ls'); Check ($r.Code -eq 1 -and $r.Out -match 'no server running') "ls: no server running" $r.Out

    Head "Part 8: two namespaces side by side, one through the config"
    $c1 = New-Ns "x"; $c2 = New-Ns "y"
    foreach ($s in 'X1', 'X2') { Run $c1 'shared' @('new-session', '-d', '-s', $s) | Out-Null }
    $cfgShared = Join-Path $ROOT "shared.conf"
    Set-Content $cfgShared "set -g default-shell $POWERSHELL`nset -s server-mode shared" -Encoding ASCII
    $env:PSMUX_CONFIG_FILE = $cfgShared
    foreach ($s in 'Y1', 'Y2', 'Y3') { $r = Run $c2 '<unset>' @('new-session', '-d', '-s', $s); Check ($r.Code -eq 0) "config shared: new-session $s" $r.Out }
    $p1 = Server-Procs $c1.Ns; $p2 = Server-Procs $c2.Ns
    Check ($p1.Count -eq 1 -and $p2.Count -eq 1 -and $p1[0].ProcessId -ne $p2[0].ProcessId) "each namespace has its own single shared server" "($($p1.Count), $($p2.Count))"
    Check ((((Run $c1 '<unset>' @('ls', '-F', '#{session_name}')).Out -split "`n" | Sort-Object) -join ',') -eq 'X1,X2') "namespace 1 lists only X1, X2"
    Check ((((Run $c2 '<unset>' @('ls', '-F', '#{session_name}')).Out -split "`n" | Sort-Object) -join ',') -eq 'Y1,Y2,Y3') "namespace 2 lists only Y1, Y2, Y3"
    S $c2 @('send-keys', '-t', 'Y2', 'Write-Output ("MODE=[" + $env:PSMUX_SERVER_MODE + "]")', 'Enter') | Out-Null
    Check (Wait-Until { (Capture $c2 'Y2') -match 'MODE=\[\]' } 15000) "a mode chosen by the config is not planted in the panes' environment" ((Capture $c2 'Y2') -split "`n" | Select-String 'MODE=')
    $env:PSMUX_CONFIG_FILE = $CFG_DEFAULT
    $r = Run $c2 '<unset>' @('kill-server')
    Check ($r.Code -eq 0 -and (Wait-Until { @(Server-Procs $c2.Ns).Count -eq 0 } 5000)) "kill-server ends namespace 2's shared server" $r.Out
    Check (@(Server-Procs $c1.Ns).Count -eq 1) "and namespace 1's keeps running"

    Head "Part 9: the per session warm standby never serves a shared request"
    $w = New-Ns "w"
    $r = Run $w '<unset>' @('new-session', '-d', '-s', 'PS')
    Check ($r.Code -eq 0) "a per session session in a namespace"
    $warm = $null
    Wait-Until { $script:warm = @(Server-Procs $w.Ns | Where-Object { $_.CommandLine -match ' -s __warm__' }); $script:warm.Count -eq 1 } 8000 | Out-Null
    $warmPid = if ($script:warm.Count -ge 1) { $script:warm[0].ProcessId } else { $null }
    Check ($null -ne $warmPid) "its warm standby is up (pid $warmPid)"
    $r = Run $w 'shared' @('new-session', '-d', '-s', 'SH')
    Check ($r.Code -eq 0) "a shared new-session in the same namespace" $r.Out
    Start-Sleep -Milliseconds 500
    $after = @(Server-Procs $w.Ns | Where-Object { $_.CommandLine -match ' -s __warm__' })
    Check ($after.Count -eq 1 -and $after[0].ProcessId -eq $warmPid) "the standby is the same process, unclaimed" (($after | ForEach-Object { "$($_.ProcessId) $($_.CommandLine)" }) -join ' | ')
    Check ((Pid-Of $w 'SH') -eq (Shared-Procs $w.Ns)[0].ProcessId) "SH lives in the shared server, not in the standby"
    Check ((((Run $w '<unset>' @('ls', '-F', '#{session_name}')).Out -split "`n" | Sort-Object) -join ',') -eq 'PS,SH') "ls sees both kinds in a mixed namespace"

    Head "Part 10: master's fixes since 0145958f, in shared mode"
    $k = New-Ns "k"
    foreach ($s in 'H', 'J') { Run $k 'shared' @('new-session', '-d', '-s', $s, '-x', '120', '-y', '30') | Out-Null }
    Check (@(Server-Procs $k.Ns).Count -eq 1) "H and J in one shared server"

    # cc32087b: a set-option run from a hook applies once.
    S $k @('set-hook', '-t', 'H', 'after-new-window', 'set -ga @hk a') | Out-Null
    S $k @('new-window', '-t', 'H') | Out-Null
    $null = Wait-Until { (S $k @('show', '-gv', '-t', 'H', '@hk')).Out -ne '' } 5000
    Start-Sleep -Milliseconds 300
    $v = (S $k @('show', '-gv', '-t', 'H', '@hk')).Out
    Check ($v -eq 'a') "set -ga from an after-new-window hook appends once (cc32087b)" "[$v]"

    # 8c3708bb: select-window -T on the current window selects the last one.
    $cur = { (S $k @('display-message', '-p', '-t', 'H', '#{window_index}')).Out }
    $before = & $cur
    S $k @('select-window', '-T', '-t', 'H:1') | Out-Null
    $after1 = & $cur
    Check ($before -eq '1' -and $after1 -eq '0') "select-window -T on the current window goes to the last window (8c3708bb)" "($before -> $after1)"
    S $k @('select-window', '-T', '-t', 'H:1') | Out-Null
    Check ((& $cur) -eq '1') "select-window -T on another window selects it"
    $r = S $k @('select-window', '-T', '-t', 'J:0')
    Check ($r.Out -match 'no last window') "select-window -T with no last window is tmux's error" "($($r.Code)) $($r.Out)"

    # 7cd8dc4e: set -w pane-base-index renumbers that window only.
    S $k @('split-window', '-t', 'H:0') | Out-Null
    S $k @('split-window', '-t', 'H:1') | Out-Null
    S $k @('set', '-w', '-t', 'H:1', 'pane-base-index', '1') | Out-Null
    $p1 = ((S $k @('list-panes', '-t', 'H:1', '-F', '#{pane_index}')).Out -split "`n") -join ' '
    $p0 = ((S $k @('list-panes', '-t', 'H:0', '-F', '#{pane_index}')).Out -split "`n") -join ' '
    Check ($p1 -eq '1 2' -and $p0 -eq '0 1') "set -w pane-base-index renumbers that window only (7cd8dc4e)" "H:1=[$p1] H:0=[$p0]"
    Check ((S $k @('select-pane', '-t', 'H:0.0')).Code -eq 0) "and H:0.0 still names a pane"

    # 063ca702: a pane's OSC 52 write reaches its session's attached client.
    function Open-Client($c, $session) {
        $port = [int](Get-Content (Join-Path $c.Dir "$($c.Ns)__$session.port")).Trim()
        $key = (Get-Content (Join-Path $c.Dir "$($c.Ns)__$session.key")).Trim()
        $t = New-Object Net.Sockets.TcpClient; $t.Connect('127.0.0.1', $port)
        $st = $t.GetStream(); $wr = New-Object IO.StreamWriter($st); $wr.NewLine = "`n"
        $wr.WriteLine("AUTH $key"); $wr.WriteLine("PERSISTENT"); $wr.WriteLine("client-size 120 30"); $wr.WriteLine("client-attach"); $wr.WriteLine("dump-state"); $wr.Flush()
        return @{ T = $t; W = $wr; R = (New-Object IO.StreamReader($st)); S = $st; Lines = (New-Object Collections.Generic.List[string]) }
    }
    function Pump-Client($c, $ms) {
        $c.S.ReadTimeout = 200
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt $ms) { try { $l = $c.R.ReadLine(); if ($null -eq $l) { break }; $c.Lines.Add($l) } catch { } }
    }
    S $k @('set', '-g', 'set-clipboard', 'on') | Out-Null
    $ch = Open-Client $k 'H'; $cj = Open-Client $k 'J'
    Pump-Client $ch 1500; Pump-Client $cj 300
    $ch.Lines.Clear(); $cj.Lines.Clear()
    $b64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes('OSCSHARED798'))
    S $k @('send-keys', '-t', 'H', ("[Console]::Write([char]27 + ']52;c;' + '" + $b64 + "' + [char]7)"), 'Enter') | Out-Null
    Pump-Client $ch 4000; Pump-Client $cj 1000
    $hasClip = @($ch.Lines | Where-Object { $_ -match ('"clipboard_osc52":"' + [regex]::Escape($b64) + '"') }).Count
    $jClip = @($cj.Lines | Where-Object { $_ -match 'clipboard_osc52' }).Count
    Check ($hasClip -ge 1) "an OSC 52 write in H reaches H's attached client as clipboard_osc52 (063ca702)" "($hasClip of $($ch.Lines.Count) lines)"
    Check ($jClip -eq 0) "and never J's client" "($jClip)"
    foreach ($c in $ch, $cj) { try { $c.W.WriteLine("client-detach"); $c.W.Flush() } catch {}; $c.T.Close() }

    # e3a0498c: C-Space reaches a pane in extended key mode 2 as CSI 27;5;32~.
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
    $REC = Join-Path $ROOT "rec860.exe"
    & $csc /nologo /platform:x64 /out:$REC "$PSScriptRoot\rec860.cs" 2>&1 | Out-Null
    if (Test-Path $REC) {
        S $k @('set', '-g', 'extended-keys', 'on') | Out-Null
        $log = Join-Path $ROOT "rec798.txt"
        S $k @('new-window', '-t', 'J', '--', $REC, 'vt', $log, '\e[>4;2m') | Out-Null
        $null = Wait-Until { (Get-Content $log -EA SilentlyContinue) -match 'READY' } 10000
        Start-Sleep -Milliseconds 700
        Check ((S $k @('display-message', '-p', '-t', 'J', '#{pane_key_mode}')).Out -eq 'Ext 2') "the recorder pane is in extended key mode 2"
        foreach ($key in '|', 'C-Space', '|') { S $k @('send-keys', '-t', 'J', $key) | Out-Null; Start-Sleep -Milliseconds 150 }
        Start-Sleep -Milliseconds 800
        $bytes = @()
        foreach ($l in (Get-Content $log -EA SilentlyContinue)) {
            if ($l -match '^BYTES .*hex=\[ ([0-9A-Fa-f ]+) \]') { $bytes += ($Matches[1].Trim() -split '\s+') }
        }
        $stream = ($bytes -join ' ').ToUpper()
        $want = '7C 1B 5B 32 37 3B 35 3B 33 32 7E 7C'
        Check ($stream -match [regex]::Escape($want)) "send-keys C-Space reaches it as CSI 27;5;32~ (e3a0498c)" "[$stream]"
    } else {
        Fail "could not build tests\rec860.cs for the extended keys check"
    }
    $r = Run $k '<unset>' @('kill-server')
    Check ($r.Code -eq 0 -and (Wait-Until { @(Server-Procs $k.Ns).Count -eq 0 } 5000)) "kill-server ends that shared server"
} finally {
    Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue
    foreach ($p in $script:Windows) { if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } }
    foreach ($pair in $script:Namespaces) {
        $env:PSMUX_DATA_DIR = $pair[1]
        & $PSMUX -L $pair[0] kill-server 2>&1 | Out-Null
    }
    Start-Sleep -Milliseconds 500
    $DEFAULT_AFTER = Default-Snapshot
    if ($DEFAULT_AFTER -ne $DEFAULT_BEFORE) { Fail "the default namespace changed: before=$DEFAULT_BEFORE after=$DEFAULT_AFTER" }
    else { Pass "the default namespace is untouched ($DEFAULT_BEFORE)" }
    Remove-Item -Recurse -Force $ROOT -EA SilentlyContinue
    if (Get-Command Remove-PsmuxIsolatedConfig -EA SilentlyContinue) { Remove-PsmuxIsolatedConfig }
}

Write-Host "`n=== Results: $script:Passed passed, $script:Failed failed ===" -ForegroundColor Cyan
exit $script:Failed
