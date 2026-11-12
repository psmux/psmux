# psmux: CLI replies survive a loopback filter that aborts client half-closes,
# and a connection that ends with no reply is an error, never empty output.
#
# Some loopback stream filters (seen with the WFP network driver of an ad
# blocker, 8.0 series) silently abort a TCP connection when the side that
# connected calls shutdown(SD_SEND): the client then reads zero bytes and a
# clean EOF. psmux's CLI half-closed after every request, so on such a machine
# display-message, list-panes, capture-pane and friends printed NOTHING at
# exit 0, and new-session waited 15 s and reported a failure.
#
# This suite reproduces the mechanism without the filter: a fake session
# server that behaves like it (sees the client's FIN before its reply, drops
# the reply and closes), and one that closes with no reply at all. A real
# server in a private namespace then checks the ordinary paths end to end.
#
# Run: powershell -NoProfile -ExecutionPolicy Bypass -File tests\test_halfclose_filter_reply.ps1
#      ($env:PSMUX_TEST_BIN picks the binary; default target\release\psmux.exe)

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass { param($msg) Write-Host "[PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail { param($msg) Write-Host "[FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info { param($msg) Write-Host "[INFO] $msg" -ForegroundColor Cyan }

$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -ErrorAction SilentlyContinue).Path }
if (-not $PSMUX -or -not (Test-Path $PSMUX)) { Write-Error "psmux binary not found"; exit 1 }
Write-Info "Using: $PSMUX"

# Own namespace and data dir: nothing here touches the default sessions.
$NS = "hcf" + [guid]::NewGuid().ToString('N').Substring(0, 8)
$DATA = Join-Path ([IO.Path]::GetTempPath()) "psmux_$NS"
New-Item -ItemType Directory -Force $DATA | Out-Null
$env:PSMUX_DATA_DIR = $DATA
Write-Info "Namespace $NS, data dir $DATA"

# ---------------------------------------------------------------------------
# Fake session server, run on a background runspace.
#   mode 'filter'  : like the filter. If the client half-closes after its
#                    request, the reply is lost: close with zero bytes sent.
#                    Otherwise answer "OK\n<marker>\n" and close gracefully.
#   mode 'silent'  : read the request, then close with zero bytes sent.
#   mode 'ackonly' : the AUTH ack, then a clean close with no reply.
#   mode 'ackreset': the AUTH ack, then a reset.
# ---------------------------------------------------------------------------
$serverScript = {
    param($listener, $mode, $marker, $log)
    while ($true) {
        try { $c = $listener.AcceptTcpClient() } catch { return }
        try {
            $s = $c.Client
            $buf = New-Object byte[] 65536
            $s.ReceiveTimeout = 2000
            $req = ''
            $sawFin = $false
            # Read the request: lines until a short quiet gap. A FIN from the
            # client (a zero byte read) is what the filter reacts to.
            $deadline = [DateTime]::UtcNow.AddMilliseconds(1500)
            while ([DateTime]::UtcNow -lt $deadline) {
                if ($s.Poll(150000, [Net.Sockets.SelectMode]::SelectRead)) {
                    $n = 0
                    try { $n = $s.Receive($buf) } catch { break }
                    if ($n -eq 0) { $sawFin = $true; break }
                    $req += [Text.Encoding]::UTF8.GetString($buf, 0, $n)
                } elseif ($req.Contains("`n") -and $req.Split("`n").Count -ge 3) {
                    break
                }
            }
            [void]$log.Add("mode=$mode fin=$sawFin req=" + ($req -replace "`n", '\n'))
            if ($mode -eq 'filter' -and -not $sawFin) {
                $out = [Text.Encoding]::UTF8.GetBytes("OK`n$marker`n")
                [void]$s.Send($out)
                $s.Shutdown([Net.Sockets.SocketShutdown]::Send)
                # Drain what is left of the client's input, then close.
                $s.ReceiveTimeout = 200
                try { while ($s.Receive($buf) -gt 0) { } } catch { }
            } elseif ($mode -eq 'ackonly') {
                # The ack arrives, the reply does not, the close is clean.
                [void]$s.Send([Text.Encoding]::UTF8.GetBytes("OK`n"))
                $s.Shutdown([Net.Sockets.SocketShutdown]::Send)
                $s.ReceiveTimeout = 200
                try { while ($s.Receive($buf) -gt 0) { } } catch { }
            } elseif ($mode -eq 'ackreset') {
                # The ack arrives, then the connection is reset.
                [void]$s.Send([Text.Encoding]::UTF8.GetBytes("OK`n"))
                Start-Sleep -Milliseconds 50
                $s.LingerState = New-Object Net.Sockets.LingerOption($true, 0)
            }
            $c.Close()
        } catch { try { $c.Close() } catch { } }
    }
}

function Start-FakeServer($mode, $marker) {
    $listener = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $log = [Collections.ArrayList]::Synchronized((New-Object Collections.ArrayList))
    $ps = [PowerShell]::Create()
    [void]$ps.AddScript($serverScript).AddArgument($listener).AddArgument($mode).AddArgument($marker).AddArgument($log)
    $handle = $ps.BeginInvoke()
    [pscustomobject]@{ Listener = $listener; PS = $ps; Handle = $handle; Port = $listener.LocalEndpoint.Port; Log = $log }
}

function Stop-FakeServer($fs) {
    try { $fs.Listener.Stop() } catch { }
    try { [void]$fs.PS.EndInvoke($fs.Handle) } catch { }
    $fs.PS.Dispose()
}

# Register a fake session in the private namespace: port, key and a pid file
# naming this PowerShell process (alive), as a real server's registry would.
function Register-FakeSession($name, $port) {
    $base = "${NS}__$name"
    $me = Get-Process -Id $PID
    $ft = $me.StartTime.ToFileTimeUtc()
    Set-Content -NoNewline -Path (Join-Path $DATA "$base.port") -Value "$port"
    Set-Content -NoNewline -Path (Join-Path $DATA "$base.key") -Value "fakekey0123456789"
    Set-Content -NoNewline -Path (Join-Path $DATA "$base.pid") -Value "${PID}:$ft"
}
function Unregister-FakeSession($name) {
    Get-ChildItem $DATA -Filter "${NS}__$name.*" | Remove-Item -Force -ErrorAction SilentlyContinue
}

function Invoke-Psmux {
    param([string[]]$CliArgs)
    # stderr lines come back as ErrorRecords under Windows PowerShell 5.1:
    # keep their text only, so the messages read the same in pwsh and 5.1.
    $all = & $PSMUX @CliArgs 2>&1
    $rc = $LASTEXITCODE
    $out = @($all | Where-Object { $_ -isnot [Management.Automation.ErrorRecord] }) -join "`n"
    $err = @($all | Where-Object { $_ -is [Management.Automation.ErrorRecord] } | ForEach-Object { $_.Exception.Message }) -join "`n"
    [pscustomobject]@{ Out = $out.Trim(); Err = $err.Trim(); Rc = $rc }
}

# === Test 1: a reply the filter would drop on a client half-close ==========
Write-Host "`n=== Test 1: reply survives a filter that aborts client half-close ===" -ForegroundColor Cyan
$marker = "hcf-reply-" + (Get-Random)
$fs = Start-FakeServer 'filter' $marker
Register-FakeSession 'filt' $fs.Port
$r = Invoke-Psmux @('-L', $NS, 'display-message', '-p', '-t', 'filt', '#{session_name}')
Write-Info ("rc={0} out=[{1}] err=[{2}]" -f $r.Rc, $r.Out, $r.Err)
foreach ($l in $fs.Log) { Write-Info "fake server: $l" }
if ($r.Rc -eq 0 -and $r.Out -eq $marker) { Write-Pass "display-message printed the server's reply" }
else { Write-Fail "display-message lost the reply (rc=$($r.Rc) out=[$($r.Out)])" }
$finSeen = @($fs.Log | Where-Object { $_ -match 'fin=True' }).Count
if ($finSeen -eq 0) { Write-Pass "the CLI did not half-close its request" }
else { Write-Fail "the CLI half-closed $finSeen request(s)" }
Stop-FakeServer $fs
Unregister-FakeSession 'filt'

# === Test 2: no reply at all is an error, not empty output at exit 0 =======
Write-Host "`n=== Test 2: a connection closed with no reply is an error ===" -ForegroundColor Cyan
$fs = Start-FakeServer 'silent' ''
Register-FakeSession 'mute' $fs.Port
foreach ($cmd in @(
        @('display-message', '-p', '-t', 'mute', '#{session_name}'),
        @('list-panes', '-t', 'mute', '-F', '#{pane_id}'),
        @('capture-pane', '-p', '-t', 'mute'))) {
    $r = Invoke-Psmux (@('-L', $NS) + $cmd)
    Write-Info ("{0}: rc={1} out=[{2}] err=[{3}]" -f $cmd[0], $r.Rc, $r.Out, $r.Err)
    if ($r.Rc -ne 0 -and $r.Err -match 'no response') { Write-Pass "$($cmd[0]) reports no response and exits non-zero" }
    else { Write-Fail "$($cmd[0]) failed silently (rc=$($r.Rc) err=[$($r.Err)])" }
}
Stop-FakeServer $fs
Unregister-FakeSession 'mute'

# === Test 2b: the ack arrives but the reply does not ========================
Write-Host "`n=== Test 2b: ack then no reply (clean close, reset) ===" -ForegroundColor Cyan
foreach ($mode in 'ackonly', 'ackreset') {
    $fs = Start-FakeServer $mode ''
    Register-FakeSession 'ack' $fs.Port
    foreach ($cmd in @(
            @('display-message', '-p', '-t', 'ack', '#{session_name}'),
            @('list-panes', '-t', 'ack', '-F', '#{pane_id}'),
            @('capture-pane', '-p', '-t', 'ack'))) {
        $r = Invoke-Psmux (@('-L', $NS) + $cmd)
        Write-Info ("{0} {1}: rc={2} out=[{3}] err=[{4}]" -f $mode, $cmd[0], $r.Rc, $r.Out, $r.Err)
        if ($r.Rc -ne 0 -and $r.Err -match 'no re') { Write-Pass "$mode $($cmd[0]): lost reply is an error" }
        else { Write-Fail "$mode $($cmd[0]): lost reply passed as empty (rc=$($r.Rc))" }
    }
    if ($mode -eq 'ackonly') {
        # A command that succeeds silently must stay silent at exit 0.
        $r = Invoke-Psmux @('-L', $NS, 'set-option', '-t', 'ack', 'status', 'off')
        Write-Info ("ackonly set-option: rc={0} out=[{1}] err=[{2}]" -f $r.Rc, $r.Out, $r.Err)
        if ($r.Rc -eq 0 -and -not $r.Out -and -not $r.Err) { Write-Pass "set-option with an empty reply stays silent at exit 0" }
        else { Write-Fail "set-option with an empty reply: rc=$($r.Rc) err=[$($r.Err)]" }
    }
    Stop-FakeServer $fs
    Unregister-FakeSession 'ack'
}

# === Test 3: a real server, end to end ======================================
Write-Host "`n=== Test 3: real server in a private namespace ===" -ForegroundColor Cyan
$sw = [Diagnostics.Stopwatch]::StartNew()
$r = Invoke-Psmux @('-L', $NS, 'new-session', '-d', '-s', 'real', '-x', '80', '-y', '20')
$ms = $sw.ElapsedMilliseconds
Write-Info ("new-session rc={0} {1} ms err=[{2}]" -f $r.Rc, $ms, $r.Err)
if ($r.Rc -eq 0 -and $ms -lt 10000) { Write-Pass "new-session acknowledged in $ms ms" }
else { Write-Fail "new-session rc=$($r.Rc) after $ms ms" }

$r = Invoke-Psmux @('-L', $NS, 'display-message', '-p', '-t', 'real', '#{session_name}')
if ($r.Rc -eq 0 -and $r.Out -eq 'real') { Write-Pass "display-message -p prints the session name" }
else { Write-Fail "display-message rc=$($r.Rc) out=[$($r.Out)] err=[$($r.Err)]" }

$r = Invoke-Psmux @('-L', $NS, 'list-panes', '-t', 'real', '-F', '#{pane_id}')
if ($r.Rc -eq 0 -and $r.Out -match '^%\d+$') { Write-Pass "list-panes -F prints the pane id ($($r.Out))" }
else { Write-Fail "list-panes rc=$($r.Rc) out=[$($r.Out)]" }

$tok = "HCF" + (Get-Random)
& $PSMUX -L $NS send-keys -t real "echo $tok" Enter 2>$null
$seen = $false
for ($i = 0; $i -lt 40 -and -not $seen; $i++) {
    Start-Sleep -Milliseconds 250
    $r = Invoke-Psmux @('-L', $NS, 'capture-pane', '-p', '-t', 'real')
    if ($r.Out -match $tok) { $seen = $true }
}
if ($seen) { Write-Pass "capture-pane shows what send-keys typed" } else { Write-Fail "capture-pane never showed $tok" }

# A launcher's readiness probe: a cmd.exe session, then its pane pid polled.
$r = Invoke-Psmux @('-L', $NS, 'new-session', '-d', '-s', 'p', '-x', '80', '-y', '10', '--', 'cmd.exe', '/d', '/k')
$panePid = $null
$sw = [Diagnostics.Stopwatch]::StartNew()
while (-not $panePid -and $sw.ElapsedMilliseconds -lt 8000) {
    $d = Invoke-Psmux @('-L', $NS, 'display-message', '-p', '-t', 'p:', '#{pane_pid}')
    if ($d.Rc -eq 0 -and $d.Out -match '^\d+$') { $panePid = $d.Out } else { Start-Sleep -Milliseconds 200 }
}
if ($r.Rc -eq 0 -and $panePid) { Write-Pass "cmd.exe session answers #{pane_pid} ($panePid) after $($sw.ElapsedMilliseconds) ms" }
else { Write-Fail "cmd.exe session probe: new-session rc=$($r.Rc), no pane pid in 8 s" }
$r = Invoke-Psmux @('-L', $NS, 'new-session', '-d', '-P', '-F', '#{session_name}:#{window_index}', '-s', 'job', '-x', '80', '-y', '10', '--', 'cmd.exe', '/d', '/k')
if ($r.Rc -eq 0 -and $r.Out -eq 'job:0') { Write-Pass "new-session -P -F prints the window it made ($($r.Out))" }
else { Write-Fail "new-session -P -F rc=$($r.Rc) out=[$($r.Out)] err=[$($r.Err)]" }

# Latency: one-shot replies must not wait on a timer for their EOF.
$times = foreach ($i in 1..15) {
    $t = [Diagnostics.Stopwatch]::StartNew()
    & $PSMUX -L $NS display-message -p -t real '#{session_name}' 2>$null | Out-Null
    $t.Elapsed.TotalMilliseconds
}
$sorted = $times | Sort-Object
$median = $sorted[[int][Math]::Floor($sorted.Count / 2)]
Write-Info ("display-message median {0:N1} ms (min {1:N1}, max {2:N1})" -f $median, $sorted[0], $sorted[-1])
if ($median -lt 500) { Write-Pass "display-message median under 500 ms" } else { Write-Fail "display-message median $median ms" }

& $PSMUX -L $NS kill-server 2>$null
Start-Sleep -Milliseconds 500
Remove-Item -Recurse -Force $DATA -ErrorAction SilentlyContinue

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor White
exit ([int]($script:TestsFailed -gt 0))
