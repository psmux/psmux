# Issue #897 (shared server, #798 step 2): rare cases, against the real binary.
#
#   A. a stale namespace marker: the next server replaces it, and new-session
#      runs there, also with several CLIs starting the namespace at once
#      (the race itself, a reader deleting the NEW server's marker, is covered
#      by tests-rs/test_issue894_897_shared_guards.rs, because its order cannot
#      be forced from outside);
#   B. every command line of a connection is checked, not only the first:
#      a refused command on a later line, after a first line over 16 KiB, or
#      behind a command alias does not run and gets the unknown command reply;
#      ordinary batches still work, and an attach is not held 2 s at the gate;
#   C. the namespace lock never collides with a per session session's lock:
#      shared `-L x__y` starts next to session `y@shared-server` in `-L x`.
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

$ROOT = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_897_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $ROOT | Out-Null
$script:Namespaces = @()

function Default-Snapshot {
    $d = Join-Path $env:USERPROFILE ".psmux"
    (Get-ChildItem $d -Filter *.port -Name -EA SilentlyContinue | Where-Object { $_ -notmatch '__' } | Sort-Object) -join ','
}
$DEFAULT_BEFORE = Default-Snapshot

Write-Host "`n=== Issue #897: shared server rare cases ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"

function New-Ns($tag) {
    $ns = "s897" + $tag + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $dir = Join-Path $ROOT $ns
    New-Item -ItemType Directory -Force $dir | Out-Null
    $script:Namespaces += , @($ns, $dir)
    return @{ Ns = $ns; Dir = $dir }
}

# Run the binary; '<unset>' runs it without PSMUX_SERVER_MODE.
function Run($ctx, $mode, [string[]]$a) {
    $env:PSMUX_DATA_DIR = $ctx.Dir
    if ($mode -eq '<unset>') { Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue } else { $env:PSMUX_SERVER_MODE = $mode }
    $out = & $PSMUX -L $ctx.Ns @a 2>&1
    $code = $LASTEXITCODE
    Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue
    return @{ Code = $code; Out = (($out | ForEach-Object { "$_" }) -join "`n").Trim() }
}
function S($ctx, [string[]]$a) { Run $ctx '<unset>' $a }

function Server-Procs($ns) {
    @(Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" -EA SilentlyContinue |
        Where-Object { $_.ExecutablePath -eq $PSMUX -and $_.CommandLine -match ' server ' -and $_.CommandLine -match "-L $([regex]::Escape($ns))(\s|$)" })
}
function Shared-Procs($ns) { @(Server-Procs $ns | Where-Object { $_.CommandLine -match '--shared-server' }) }

# A raw one shot request on a session's listener: AUTH, an optional pause,
# then the lines, a half close, and the reply read to EOF.
function Tcp-Raw($ctx, $session, [string[]]$lines, $pauseMs = 0, $timeoutMs = 8000) {
    $port = [int](Get-Content (Join-Path $ctx.Dir "$($ctx.Ns)__$session.port")).Trim()
    $key = (Get-Content (Join-Path $ctx.Dir "$($ctx.Ns)__$session.key")).Trim()
    $t = New-Object Net.Sockets.TcpClient; $t.Connect('127.0.0.1', $port)
    $s = $t.GetStream(); $s.ReadTimeout = $timeoutMs
    $b = [Text.Encoding]::UTF8.GetBytes("AUTH $key`n"); $s.Write($b, 0, $b.Length); $s.Flush()
    if ($pauseMs -gt 0) { Start-Sleep -Milliseconds $pauseMs }
    $b = [Text.Encoding]::UTF8.GetBytes((($lines | ForEach-Object { "$_`n" }) -join '')); $s.Write($b, 0, $b.Length); $s.Flush()
    $t.Client.Shutdown([Net.Sockets.SocketShutdown]::Send)
    $ms = New-Object IO.MemoryStream; $buf = New-Object byte[] 4096
    try { while (($n = $s.Read($buf, 0, $buf.Length)) -gt 0) { $ms.Write($buf, 0, $n) } } catch {}
    $t.Close()
    return [Text.Encoding]::UTF8.GetString($ms.ToArray())
}
function Show($text) { ($text -replace "`r", '\r') -replace "`n", '\n' }

function Wait-Until([scriptblock]$cond, $ms = 5000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return (& $cond)
}

try {
    Head "A: a stale namespace marker is replaced by the next server"
    $a = New-Ns "a"
    $markerDir = Join-Path $a.Dir "shared"
    New-Item -ItemType Directory -Force $markerDir | Out-Null
    $marker = Join-Path $markerDir "L-$($a.Ns).server"
    # This test process, with a creation time it never had: a dead server.
    [IO.File]::WriteAllText($marker, "mode=shared`npid=$PID`ncreation=1`nport=1`nkey=stale`n")
    $env:PSMUX_DATA_DIR = $a.Dir
    $env:PSMUX_SERVER_MODE = 'shared'
    $procs = @()
    foreach ($s in 'P1', 'P2', 'P3', 'P4') {
        $procs += Start-Process -FilePath $PSMUX -ArgumentList @('-L', $a.Ns, 'new-session', '-d', '-s', $s) -NoNewWindow -PassThru
    }
    Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue
    $codes = @()
    foreach ($p in $procs) { if (-not $p.WaitForExit(30000)) { $codes += 'hung' } else { $codes += $p.ExitCode } }
    Check (@($codes | Where-Object { $_ -ne 0 }).Count -eq 0) "four CLIs starting the namespace at once over a stale marker all succeed" ($codes -join ',')
    $text = Get-Content $marker -Raw -EA SilentlyContinue
    $live = @(Shared-Procs $a.Ns)
    Check ($live.Count -eq 1) "one shared server holds the namespace" "($($live.Count))"
    Check ($live.Count -eq 1 -and $text -match "(?m)^pid=$($live[0].ProcessId)\s*$") "the marker names that live server" (Show $text)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $r = Run $a 'shared' @('new-session', '-d', '-s', 'P5')
    Check ($r.Code -eq 0 -and $sw.ElapsedMilliseconds -lt 10000) "a later new-session joins it ($($sw.ElapsedMilliseconds) ms)" $r.Out
    $ls = (S $a @('ls', '-F', '#{session_name}')).Out -split "`n" | Sort-Object
    Check (($ls -join ',') -eq 'P1,P2,P3,P4,P5') "every session is listed" ($ls -join ',')
    Check (@(Shared-Procs $a.Ns).Count -eq 1) "still one shared server"

    Head "B: every command line of a connection is checked"
    $b = New-Ns "b"
    foreach ($s in 'A', 'B') { Run $b 'shared' @('new-session', '-d', '-s', $s, '-x', '120', '-y', '30') | Out-Null }
    S $b @('split-window', '-t', 'A') | Out-Null
    $zoom = { (S $b @('display-message', '-p', '-t', 'A', '#{window_zoomed_flag}')).Out }
    Check ((& $zoom) -eq '0' -and (S $b @('list-panes', '-t', 'A')).Out.Split("`n").Count -eq 2) "A has two panes, not zoomed"
    $refusal = "OK`nERROR: unknown command: resize-pane`n"

    $r = Tcp-Raw $b 'A' @('resize-pane -Z')
    Check ($r -ceq $refusal -and (& $zoom) -eq '0') "control: resize-pane -Z alone is refused" (Show $r)

    $r = Tcp-Raw $b 'A' @('send-keys -t A -l z', 'resize-pane -Z', 'session-info')
    Check ((& $zoom) -eq '0') "a. resize-pane -Z on the second line does not run" "zoom=$(& $zoom)"
    Check ($r.EndsWith("ERROR: unknown command: resize-pane`n") -and $r -notmatch 'windows \(created') "a. it gets the unknown command reply, and the line after it never runs" (Show $r)

    $r = Tcp-Raw $b 'A' @((' ' * 17000) + 'resize-pane -Z')
    Check ($r -ceq $refusal -and (& $zoom) -eq '0') "b. behind a first line over 16 KiB it is refused" (Show $r)

    # c. A client that sends AUTH alone and waits is taken for one waiting
    # for OK (an attach) and goes straight to the handler, whose own 2 s wait
    # for the next line then applies as in a per session server: a command
    # 2.5 s after AUTH is dropped there too (no reply, nothing runs). The peek
    # used to spend those 2 s first, so the handler's wait started late and
    # the command ran.
    $r = Tcp-Raw $b 'A' @('resize-pane -Z') 2500
    Check ($r -notmatch 'resize-pane|windows' -and (& $zoom) -eq '0') "c. sent 2.5 s after AUTH it does not run, as per session" (Show $r)

    $r = S $b @('set', '-s', 'command-alias', 'list-tree=resize-pane -Z')
    Check ($r.Code -eq 0) "d. an alias list-tree=resize-pane -Z is set" $r.Out
    $r = Tcp-Raw $b 'A' @('list-tree')
    Check ($r -ceq $refusal -and (& $zoom) -eq '0') "d. the alias does not carry resize-pane in" (Show $r)

    $r = Tcp-Raw $b 'A' @('send-keys -t A -l q', 'session-info')
    Check ($r -match '(?m)^A: \d+ windows' -and $r -notmatch 'ERROR') "an ordinary batch with the session-info barrier still runs" (Show $r)
    $r = Tcp-Raw $b 'A' @('send-keys -t A -l q', 'set -t A @later897 yes', 'session-info')
    Check ($r -match '(?m)^A: \d+ windows' -and (S $b @('show', '-v', '-t', 'A', '@later897')).Out -eq 'yes') "a supported command on a later line still runs" (Show $r)
    # As a per session server: a one shot connection stops after display-message -p.
    $r = Tcp-Raw $b 'A' @('display-message -p first897', 'display-message -p second897')
    Check ($r -ceq "OK`nfirst897`n") "display-message -p ends the batch as a per session server does" (Show $r)
    $r = S $b @('send-keys', '-t', 'A', 'echo ok897', 'Enter')
    Check ($r.Code -eq 0) "CLI send-keys still works" $r.Out
    $r = S $b @('resize-pane', '-Z', '-t', 'A')
    Check ((& $zoom) -eq '0') "CLI resize-pane is still refused" $r.Out
    Check (@(Shared-Procs $b.Ns).Count -eq 1) "the shared server is still up"

    Head "B2: an attach is not held at the gate"
    # The attaching client sends AUTH and waits for OK before PERSISTENT; the
    # gate used to wait its whole 2 s peek deadline for that second line.
    $times = @()
    foreach ($i in 1..3) {
        $env:PSMUX_DATA_DIR = $b.Dir
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $p = Start-Process -FilePath $PSMUX -ArgumentList @('-L', $b.Ns, 'attach-session', '-t', 'B') -PassThru
        $ok = Wait-Until { (S $b @('display-message', '-p', '-t', 'B', '#{session_attached}')).Out -eq '1' } 10000
        $times += if ($ok) { $sw.ElapsedMilliseconds } else { 99999 }
        Stop-Process -Id $p.Id -Force -EA SilentlyContinue
        $p.WaitForExit(5000) | Out-Null
        Wait-Until { (S $b @('display-message', '-p', '-t', 'B', '#{session_attached}')).Out -eq '0' } 10000 | Out-Null
    }
    $best = ($times | Measure-Object -Minimum).Minimum
    Check ($best -lt 1500) "a client attaches to a shared session in well under the old 2 s (best $best ms of $($times -join ', '))"

    Head "C: the namespace lock never collides with a per session lock"
    $base = "s897c" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $cdir = Join-Path $ROOT $base
    New-Item -ItemType Directory -Force $cdir | Out-Null
    $c1 = @{ Ns = $base; Dir = $cdir }; $c2 = @{ Ns = "${base}__x"; Dir = $cdir }
    $script:Namespaces += , @($c1.Ns, $cdir); $script:Namespaces += , @($c2.Ns, $cdir)
    $r = Run $c1 '<unset>' @('new-session', '-d', '-s', 'x@shared-server')
    Check ($r.Code -eq 0) "per session: session x@shared-server in -L $base" $r.Out
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $r = Run $c2 'shared' @('new-session', '-d', '-s', 's1')
    Check ($r.Code -eq 0 -and $sw.ElapsedMilliseconds -lt 10000) "shared: -L ${base}__x starts next to it ($($sw.ElapsedMilliseconds) ms)" $r.Out
    Check (@(Shared-Procs $c2.Ns).Count -eq 1) "with its own shared server"
    Check ((S $c1 @('has-session', '-t', 'x@shared-server')).Code -eq 0) "and the per session session is untouched"
} finally {
    Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue
    foreach ($pair in $script:Namespaces) {
        $env:PSMUX_DATA_DIR = $pair[1]
        & $PSMUX -L $pair[0] kill-server 2>&1 | Out-Null
    }
    Remove-Item Env:PSMUX_DATA_DIR -EA SilentlyContinue
    foreach ($pair in $script:Namespaces) {
        Wait-Until { @(Server-Procs $pair[0]).Count -eq 0 } 5000 | Out-Null
    }
    Remove-PsmuxIsolatedConfig
}

$DEFAULT_AFTER = Default-Snapshot
Check ($DEFAULT_AFTER -eq $DEFAULT_BEFORE) "the default namespace was never touched" "before [$DEFAULT_BEFORE] after [$DEFAULT_AFTER]"
Remove-Item -Recurse -Force $ROOT -EA SilentlyContinue

Write-Host "`n=== Results: $($script:Passed) passed, $($script:Failed) failed ===" -ForegroundColor Cyan
exit $script:Failed
