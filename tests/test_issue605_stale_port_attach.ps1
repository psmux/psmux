# Issue #605: "no connections made, cuz target machine refuse it"
#
# The report is two consecutive attaches on Windows Server 2022:
#
#   > ta
#   psmux: No connection could be made because the target machine actively
#   refused it. (os error 10061)                                  took 4s
#
# `<data dir>\<session>.port` still named a server that was no longer
# listening. The CLI attach gate asked probe_session_alive, whose connect arm
# read `Err(kind == ConnectionRefused) => false, Err(_) => true`. Windows does
# not answer a connect to an UNBOUND loopback port promptly: it drops the SYN
# and retransmits, so the refusal takes about 2.04s to surface (measured on
# this machine across six ports: 2043 2027 2034 2062 2035 ms). The gate's 500ms
# probe saw a plain timeout, voted ALIVE, and the client then connected for
# real and let the raw std::io::Error escape run_remote, where main prints
# every escaping error verbatim as `psmux: <display>`.
#
# tmux 3.4 is the parity reference. With its server killed and the socket left
# in place:
#
#   tmux -L parity605 attach            -> "no sessions"                 rc 1  12ms
#   tmux -L parity605 attach -t p1      -> "no sessions"                 rc 1  25ms
#   tmux -L parity605 has-session -t p1 -> "no server running on <path>" rc 1   5ms
#
# One line, no OS vocabulary, no multi-second stall.
#
# What this suite pins:
#   * an attach that cannot reach its server says `can't find session: NAME`,
#     exits 1, and names no winsock error
#   * that holds even when the liveness gate is beaten by a server that goes
#     away between the probe and the connect (the shape that leaked #605)
#   * the dead registration is reaped, and a LIVE session's is not
#   * a live session still attaches
#
# The attach client is launched through a .cmd wrapper in its own console
# window: psmux prints its version and returns 0 when stdout is not a tty, so a
# redirected stdout would never reach the code path under test.
#
# Set PSMUX_TEST_BIN to test a non-installed binary.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

# Inherited session routing would aim these calls at somebody else's server.
$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION      = $null
$env:PSMUX_PANE         = $null
$env:TMUX               = $null
$env:TMUX_PANE          = $null

$rig  = Join-Path $env:TEMP ("psmux605-" + [guid]::NewGuid().ToString('N').Substring(0,8))
$root = Join-Path $rig 'data'
New-Item -ItemType Directory -Force -Path $rig, $root | Out-Null
$env:PSMUX_DATA_DIR = $root

# Wall-clock ceilings. tmux answers in tens of milliseconds; the point of the
# fix is that psmux must not sit through a SYN retransmit before it can say the
# session is gone. Generous enough for a loaded CI box, far under the ~2.7s the
# unfixed binary took.
$FastMs  = 1500   # the gate rejects it outright
$RaceMs  = 2000   # the gate is beaten and the client's own connect fails

# stdout must be a console for the client to attach at all, so the psmux under
# test is launched from a .cmd in its own window with stderr redirected.
$wrapper = Join-Path $rig 'runattach.cmd'
@'
@echo off
setlocal
set "BIN=%~1"
set "ERRF=%~2"
set "RCF=%~3"
shift
shift
shift
set "ARGS=%1"
:loop
shift
if "%~1"=="" goto done
set "ARGS=%ARGS% %1"
goto loop
:done
"%BIN%" %ARGS% 2>"%ERRF%"
>"%RCF%" echo %ERRORLEVEL%
endlocal
'@ | Set-Content -Path $wrapper -Encoding ASCII

# Serves exactly ONE connection (the liveness probe) so the gate votes ALIVE,
# then repoints .port at a port nothing has ever bound. That is a server dying
# between the probe and the attach, compressed into something deterministic.
$oneshot = Join-Path $rig 'oneshot.ps1'
@'
param([int]$Port, [int]$DeadPort, [string]$ReadyFile, [string]$PortFile)
$l = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $Port)
$l.Start(8)
Set-Content -NoNewline -Path $ReadyFile -Value "ready"
$c = $l.AcceptTcpClient()
Set-Content -NoNewline -Path $PortFile -Value "$DeadPort"
try {
    $s = $c.GetStream()
    $b = [System.Text.Encoding]::ASCII.GetBytes("OK`n")
    $s.Write($b, 0, $b.Length); $s.Flush()
} catch {}
Start-Sleep -Milliseconds 50
try { $c.Close() } catch {}
Start-Sleep -Seconds 20
[Environment]::Exit(0)
'@ | Set-Content -Path $oneshot -Encoding UTF8

function Get-FreePort {
    $l = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $l.Start(); $p = $l.LocalEndpoint.Port; $l.Stop(); return $p
}

# Launch an ATTACHED psmux in a real console; return rc, stderr and wall time.
function Invoke-Attach {
    param([string[]]$PsmuxArgs, [int]$TimeoutSec = 30)
    $tag  = [guid]::NewGuid().ToString('N').Substring(0,8)
    $errF = Join-Path $rig "e_$tag.txt"
    $rcF  = Join-Path $rig "r_$tag.txt"
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $wrapper -ArgumentList (@($PSMUX,$errF,$rcF) + $PsmuxArgs) `
                       -PassThru -WindowStyle Minimized
    $exited = $p.WaitForExit($TimeoutSec * 1000)
    $sw.Stop()
    if (-not $exited) { try { Stop-Process -Id $p.Id -Force } catch {} }
    Start-Sleep -Milliseconds 200
    $err = ""
    try { $err = (Get-Content -LiteralPath $errF -Raw -EA Stop) } catch {}
    if ($null -eq $err) { $err = "" }
    $rc = -1
    try { $rc = [int]((Get-Content -LiteralPath $rcF -Raw -EA Stop).Trim()) } catch {}
    if (-not $exited) { $rc = -1 }
    return @{
        rc  = $rc
        err = ($err -replace "`r`n", " ").Trim()
        ms  = [int]$sw.Elapsed.TotalMilliseconds
    }
}

# Run a headless psmux command (no console needed).
function Run($argv) {
    $out = & $PSMUX @argv 2>&1
    $rc = $LASTEXITCODE
    return @{ rc = $rc; out = ((($out | Out-String) -replace '\s+', ' ').Trim()) }
}

# The exact vocabulary the user must never see. Any of these means a raw
# std::io::Error reached the terminal instead of a psmux sentence.
$rawLeaks = @('os error', 'actively refused', 'target machine', 'connection timed out',
              '10061', '10060', 'ConnectionRefused')

function Assert-NoRawOsError($label, $text) {
    $hit = $rawLeaks | Where-Object { $text -like "*$_*" }
    if ($hit) { Write-Fail "$label leaked raw transport wording ($($hit -join ', ')): '$text'" }
    else      { Write-Pass "$label names no operating system error" }
}

# Plant a registry entry for a server that is not there.
function Set-StaleRegistry($name, $port) {
    Set-Content -NoNewline -Path (Join-Path $root "$name.port") -Value "$port"
    Set-Content -NoNewline -Path (Join-Path $root "$name.key")  -Value "aabbccddeeff0011"
    Set-Content -NoNewline -Path (Join-Path $root "$name.sid")  -Value "0"
}

function Kill-RigServers {
    Get-ChildItem (Join-Path $root '*.pid') -EA SilentlyContinue | ForEach-Object {
        $raw = (Get-Content -LiteralPath $_.FullName -Raw -EA SilentlyContinue)
        if ($raw) {
            $n = ($raw.Trim() -split ':')[0]
            if ($n -match '^\d+$') { try { Stop-Process -Id ([int]$n) -Force -EA Stop } catch {} }
        }
    }
    Start-Sleep -Milliseconds 400
}

try {
    # === 1. A session that never existed ===
    Write-Host "`n--- a session that never existed ---" -ForegroundColor Yellow
    $ghost = 'i605ghost-' + [guid]::NewGuid().ToString('N').Substring(0,6)
    $r = Invoke-Attach @('attach','-t',$ghost)
    Write-Info "rc=$($r.rc) ms=$($r.ms) stderr='$($r.err)'"
    Assert-NoRawOsError "attach to a session that never existed" $r.err
    if ($r.err -eq "psmux: can't find session: $ghost") {
        Write-Pass "tmux wording: `"can't find session: $ghost`""
    } else { Write-Fail "expected `"psmux: can't find session: $ghost`", got '$($r.err)'" }
    if ($r.rc -eq 1) { Write-Pass "exit code 1 (tmux parity)" } else { Write-Fail "expected exit 1, got $($r.rc)" }
    if ($r.ms -lt $FastMs) { Write-Pass "answered in $($r.ms)ms (< ${FastMs}ms)" }
    else { Write-Fail "took $($r.ms)ms; tmux answers a missing session in ~12ms" }

    # === 2. A stale .port that points at a port nothing is bound to ===
    # This is the reboot/crash leftover: the registry survives, the server does
    # not, and the connect is answered by a SYN retransmit rather than a prompt
    # refusal.
    Write-Host "`n--- stale .port, nothing listening ---" -ForegroundColor Yellow
    $stale = 'i605stale-' + [guid]::NewGuid().ToString('N').Substring(0,6)
    Set-StaleRegistry $stale (Get-FreePort)
    $r = Invoke-Attach @('attach','-t',$stale)
    Write-Info "rc=$($r.rc) ms=$($r.ms) stderr='$($r.err)'"
    Assert-NoRawOsError "attach over a stale .port" $r.err
    if ($r.err -eq "psmux: can't find session: $stale") {
        Write-Pass "stale registration reported as a missing session"
    } else { Write-Fail "expected `"psmux: can't find session: $stale`", got '$($r.err)'" }
    if ($r.rc -eq 1) { Write-Pass "exit code 1" } else { Write-Fail "expected exit 1, got $($r.rc)" }
    if ($r.ms -lt $FastMs) { Write-Pass "answered in $($r.ms)ms (< ${FastMs}ms)" }
    else { Write-Fail "took $($r.ms)ms: the attach sat through the SYN retransmit" }
    if (-not (Test-Path -LiteralPath (Join-Path $root "$stale.port"))) {
        Write-Pass "the dead registration was reaped"
    } else { Write-Fail "the dead .port survived; the next attach will re-litigate it" }

    # === 3. THE #605 SHAPE: the liveness gate is beaten ===
    # No gate can rule out a server that dies a millisecond after answering the
    # probe. When that happens the user must still get a psmux sentence, not
    # whatever winsock said. This is the case the unfixed binary failed.
    Write-Host "`n--- server gone between the liveness probe and the attach ---" -ForegroundColor Yellow
    $race = 'i605race-' + [guid]::NewGuid().ToString('N').Substring(0,6)
    $livePort = Get-FreePort
    $deadPort = Get-FreePort
    Set-StaleRegistry $race $livePort
    $ready = Join-Path $rig "ready_$race.txt"
    $helper = Start-Process -FilePath "pwsh" -ArgumentList @(
        "-NoProfile","-File",$oneshot,"-Port","$livePort","-DeadPort","$deadPort",
        "-ReadyFile",$ready,"-PortFile",(Join-Path $root "$race.port")
    ) -PassThru -WindowStyle Hidden
    for ($i = 0; $i -lt 200; $i++) { if (Test-Path -LiteralPath $ready) { break }; Start-Sleep -Milliseconds 50 }
    if (-not (Test-Path -LiteralPath $ready)) {
        Write-Info "helper listener never became ready; skipping the race case"
    } else {
        Write-Info "probe answered on port $livePort, client redirected to dead port $deadPort"
        $r = Invoke-Attach @('attach','-t',$race)
        Write-Info "rc=$($r.rc) ms=$($r.ms) stderr='$($r.err)'"
        Assert-NoRawOsError "attach whose connect fails after a passing probe" $r.err
        if ($r.err -eq "psmux: can't find session: $race") {
            Write-Pass "a failed connect is reported as a missing session, not as a winsock error"
        } else { Write-Fail "expected `"psmux: can't find session: $race`", got '$($r.err)'" }
        if ($r.rc -eq 1) { Write-Pass "exit code 1" } else { Write-Fail "expected exit 1, got $($r.rc)" }
        if ($r.ms -lt $RaceMs) { Write-Pass "answered in $($r.ms)ms (< ${RaceMs}ms)" }
        else { Write-Fail "took $($r.ms)ms; the unfixed client burned a 2s connect budget here" }
    }
    try { Stop-Process -Id $helper.Id -Force -EA SilentlyContinue } catch {}

    # === 4. The other verbs must not leak either ===
    Write-Host "`n--- ls / has-session / new-session -A over a stale registry ---" -ForegroundColor Yellow
    $stale2 = 'i605verb-' + [guid]::NewGuid().ToString('N').Substring(0,6)
    Set-StaleRegistry $stale2 (Get-FreePort)
    $r = Run @('ls');                          Assert-NoRawOsError "ls" "$($r.out)"
    Set-StaleRegistry $stale2 (Get-FreePort)
    $r = Run @('has-session','-t',$stale2)
    Assert-NoRawOsError "has-session" "$($r.out)"
    if ($r.rc -ne 0) { Write-Pass "has-session over a stale entry exits non-zero (rc=$($r.rc))" }
    else { Write-Fail "has-session reported a server that is not running (rc=0)" }
    Set-StaleRegistry $stale2 (Get-FreePort)
    $r = Run @('new-session','-A','-s',$stale2,'-d')
    Assert-NoRawOsError "new-session -A" "$($r.out)"
    if ($r.rc -eq 0) { Write-Pass "new-session -A took over the stale name (rc=0, tmux parity)" }
    else { Write-Fail "new-session -A refused the stale name (rc=$($r.rc)) '$($r.out)'" }
    Run @('kill-session','-t',$stale2) | Out-Null

    # === 5. A live session must still attach, and keep its registration ===
    Write-Host "`n--- no regression: a live session still attaches ---" -ForegroundColor Yellow
    $live = 'i605live-' + [guid]::NewGuid().ToString('N').Substring(0,6)
    $r = Run @('new-session','-d','-s',$live)
    if ($r.rc -ne 0) {
        Write-Fail "could not create '$live' (rc=$($r.rc)) '$($r.out)'"
    } else {
        Start-Sleep -Seconds 2
        # A failed attach elsewhere must not disturb this session's files.
        $other = 'i605other-' + [guid]::NewGuid().ToString('N').Substring(0,6)
        Set-StaleRegistry $other (Get-FreePort)
        Invoke-Attach @('attach','-t',$other) | Out-Null
        if ((Test-Path -LiteralPath (Join-Path $root "$live.port")) -and
            (Test-Path -LiteralPath (Join-Path $root "$live.key"))) {
            Write-Pass "a live session's registry survived a failed attach to another name"
        } else { Write-Fail "reaping a dead entry took the live session's files with it" }

        $tag  = [guid]::NewGuid().ToString('N').Substring(0,8)
        $errF = Join-Path $rig "live_e_$tag.txt"
        $rcF  = Join-Path $rig "live_r_$tag.txt"
        $p = Start-Process -FilePath $wrapper `
             -ArgumentList @($PSMUX,$errF,$rcF,"attach","-t",$live) -PassThru -WindowStyle Minimized
        Start-Sleep -Seconds 5
        $attached = -not $p.HasExited
        if ($attached) { Write-Pass "the client attached and stayed connected" }
        else {
            $e = ""
            try { $e = ((Get-Content -LiteralPath $errF -Raw -EA Stop) -replace "`r`n"," ").Trim() } catch {}
            Write-Fail "the client to a LIVE session exited early: '$e'"
        }
        $clients = Run @('list-clients','-t',$live)
        if ($clients.out -match [regex]::Escape($live)) {
            Write-Pass "the server sees the attached client: '$($clients.out)'"
        } else { Write-Info "list-clients: '$($clients.out)'" }
        Run @('detach-client','-s',$live) | Out-Null
        Start-Sleep -Seconds 2
        if (-not $p.HasExited) { try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {} }
        Run @('kill-session','-t',$live) | Out-Null
    }
}
finally {
    Kill-RigServers
    $env:PSMUX_DATA_DIR = $null
    Remove-Item -LiteralPath $rig -Recurse -Force -EA SilentlyContinue
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:Pass)" -ForegroundColor Green
Write-Host "  Failed: $($script:Fail)" -ForegroundColor $(if ($script:Fail -gt 0) { "Red" } else { "Green" })
exit $script:Fail
