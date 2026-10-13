# Issue #793: `send-keys C-c` and `respawn-pane -k` sent over the raw TCP
# protocol were answered OK and did nothing, while the CLI worked.
#
# ROOT CAUSE: the reporter's Send-Tcp read the FIRST `<ns>*.port` file, and the
# namespace's warm standby (`<ns>____warm__.port`) sorts before the session's
# own `<ns>__t.port`. So the commands reached the standby, a server that does
# not hold session t, and it ignored the `t:` part of `-t t:@1.%1`, ran them
# on ITS OWN %1 and answered OK. The transport was never the problem: the
# same lines on the session's own port work (section 2). tmux answers a
# target naming a session it does not have with "can't find session: t".
#
# GROUND TRUTH: a detached session under its own -L namespace, a `ping -t`
# child found by parent pid (CIM), `#{pane_pid}` before and after, and the
# reply text read to EOF after a half close (one send per TCP reply).
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH. PSMUX_DATA_DIR is
# honoured: the port and key files are read from it when it is set.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

. "$PSScriptRoot\isolated_config.ps1"

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$NS = "i793-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$DATA = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { Join-Path $env:USERPROFILE ".psmux" }
function P { & $PSMUX -L $NS @args 2>&1 }

# One connection: AUTH, the payload, half close, read the reply to EOF.
function Send-Tcp([string]$PortFile, [string]$Payload) {
    $pf = Join-Path $DATA $PortFile
    if (-not (Test-Path $pf)) { return "<no port file $PortFile>" }
    $key = (Get-Content ($pf -replace '\.port$', '.key') -Raw).Trim()
    $c = [Net.Sockets.TcpClient]::new('127.0.0.1', [int]((Get-Content $pf -Raw).Trim()))
    try {
        $s = $c.GetStream()
        $b = [Text.Encoding]::UTF8.GetBytes("AUTH $key`n$Payload")
        $s.Write($b, 0, $b.Length)
        $c.Client.Shutdown([Net.Sockets.SocketShutdown]::Send)
        $s.ReadTimeout = 4000
        $resp = ''; $buf = New-Object byte[] 4096
        try { while (($n = $s.Read($buf, 0, $buf.Length)) -gt 0) { $resp += [Text.Encoding]::UTF8.GetString($buf, 0, $n) } } catch {}
        return $resp
    } finally { $c.Close() }
}
function Flat($s) { ($s -replace "`r?`n", ' | ').Trim() }

# ping.exe children of a pane shell, by parent pid. Only these pids are ever
# stopped by this suite.
function PingUnder([int]$panePid) {
    @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$panePid" -EA SilentlyContinue |
        Where-Object Name -eq 'PING.EXE' | ForEach-Object { [int]$_.ProcessId })
}
function StopPings([int]$panePid) {
    foreach ($p in (PingUnder $panePid)) { Stop-Process -Id $p -Force -EA SilentlyContinue }
}
function PanePid { [int](((P display-message -p -t "t:" '#{pane_pid}') -join '').Trim()) }
function StartPing {
    $pp = PanePid
    P send-keys -t "t:" 'ping -t localhost' Enter | Out-Null
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep -Milliseconds 200
        if ((PingUnder $pp).Count -gt 0) { break }
    }
    return $pp
}

P new-session -d -s t -x 120 -y 30 -- pwsh -NoLogo -NoProfile | Out-Null
for ($i = 0; $i -lt 40; $i++) { Start-Sleep -Milliseconds 250; P has-session -t t | Out-Null; if ($LASTEXITCODE -eq 0) { break } }
Start-Sleep -Seconds 2
$TARGET = ((P list-panes -a -F '#{session_name}:#{window_id}.#{pane_id}') -join '').Trim()
$OWN = "${NS}__t.port"
$WARM = "${NS}____warm__.port"
Write-Info "target $TARGET"
if ($TARGET -notmatch '^t:@\d+\.%\d+$') {
    Write-Fail "session t did not come up (list-panes gave [$TARGET])"
    P kill-server | Out-Null
    exit 1
}

try {

# ── 1. the reporter's script reaches the warm standby ──

Write-Head "1. a session target sent to the standby is can't find session"
for ($w = 0; $w -lt 40 -and -not (Test-Path (Join-Path $DATA $WARM)); $w++) { Start-Sleep -Milliseconds 250 }
$first = (Get-ChildItem $DATA -Filter "$NS*.port" | Select-Object -First 1).Name
Write-Info "first port file: $first"
if (-not (Test-Path (Join-Path $DATA $WARM))) {
    Write-Info "no warm standby in this namespace (warm pool disabled?), section 1 skipped"
} else {
    Check ($first -eq $WARM) "the first <ns>*.port file is the standby, as in the report" "the first is $first"
    $warmPid0 = (Flat (Send-Tcp $WARM "display-message -p '#{pane_pid}'`n")) -replace '^OK \| ', '' -replace ' \|$', ''
    Write-Info "standby pane pid $warmPid0"

    $pp = StartPing
    $before = PingUnder $pp
    $r = Flat (Send-Tcp $WARM "send-keys -t $TARGET C-c`n")
    Write-Info "send-keys reply [$r]"
    Check ($r -match "ERROR: can't find session: t") "send-keys C-c to the standby is an ERROR, not a silent OK" "the reply was [$r]"
    Start-Sleep -Seconds 1
    StopPings $pp

    $r = Flat (Send-Tcp $WARM "respawn-pane -k -t $TARGET -- pwsh -NoLogo -NoProfile`n")
    Write-Info "respawn-pane reply [$r]"
    Check ($r -match "ERROR: can't find session: t") "respawn-pane -k to the standby is an ERROR" "the reply was [$r]"

    $r = Flat (Send-Tcp $WARM "respawn-window -k -t t:@1`n")
    Check ($r -match "ERROR: can't find session: t") "respawn-window -k to the standby is an ERROR" "the reply was [$r]"

    $r = Flat (Send-Tcp $WARM "TARGET $TARGET`nsend-keys C-c`n")
    Check ($r -match "ERROR: can't find session: t") "the TARGET line form is checked too" "the reply was [$r]"

    $r = Flat (Send-Tcp $WARM "kill-window -t t:@1`n")
    Check ($r -match "ERROR: can't find session: t") "kill-window -t t:@1 to the standby is an ERROR" "the reply was [$r]"

    $r = Flat (Send-Tcp $WARM "select-pane -t ${TARGET}`n")
    Check ($r -match "ERROR: can't find session: t") "select-pane (a focus command) to the standby is an ERROR" "the reply was [$r]"

    Start-Sleep -Seconds 2
    $warmPid1 = (Flat (Send-Tcp $WARM "display-message -p '#{pane_pid}'`n")) -replace '^OK \| ', '' -replace ' \|$', ''
    Check ($warmPid0 -ne '' -and $warmPid1 -eq $warmPid0) "the standby's own pane was left alone ($warmPid0)" "standby pane pid $warmPid0 -> $warmPid1"
}

# ── 2. the same lines on the session's own port ──

Write-Head "2. on the session's own port the commands work"
for ($run = 1; $run -le 3; $run++) {
    $pp = StartPing
    $before = PingUnder $pp
    $r = Flat (Send-Tcp $OWN "send-keys -t $TARGET C-c`n")
    $after = @()
    for ($i = 0; $i -lt 20; $i++) { Start-Sleep -Milliseconds 200; $after = PingUnder $pp; if ($after.Count -eq 0) { break } }
    Check ($before.Count -gt 0 -and $after.Count -eq 0 -and $r -notmatch 'ERROR') "run ${run}: send-keys C-c stopped ping (before [$before], reply [$r])" "run ${run}: before [$before] after [$after] reply [$r]"
    StopPings $pp
}
for ($run = 1; $run -le 3; $run++) {
    $p0 = PanePid
    $r = Flat (Send-Tcp $OWN "respawn-pane -k -t $TARGET -- pwsh -NoLogo -NoProfile`n")
    Start-Sleep -Seconds 2
    $p1 = PanePid
    $oldAlive = [bool](Get-Process -Id $p0 -EA SilentlyContinue)
    Check ($p1 -ne $p0 -and -not $oldAlive -and $r -notmatch 'ERROR') "run ${run}: respawn-pane -k $p0 -> $p1, old shell gone" "run ${run}: $p0 -> $p1 oldAlive=$oldAlive reply [$r]"
}
$p0 = PanePid
$r = Flat (Send-Tcp $OWN "respawn-window -k -t t:@1`n")
Start-Sleep -Seconds 2
$p1 = PanePid
Check ($p1 -ne $p0 -and $r -notmatch 'ERROR') "respawn-window -k $p0 -> $p1" "respawn-window: $p0 -> $p1 reply [$r]"

# ── 3. every spelling of the session's own name is accepted ──

Write-Head "3. own session spellings resolve"
$sid = ((P display-message -p -t "t:" '#{session_id}') -join '').Trim()
foreach ($spec in @("=$TARGET", "${sid}:@1.%1", "${NS}__t:@1.%1", "t:")) {
    $r = Flat (Send-Tcp $OWN "display-message -p -t $spec '#{session_name}'`n")
    Check ($r -match '\| t \|' -and $r -notmatch 'ERROR') "[$spec] resolves on the own port" "[$spec] reply [$r]"
}
$r = Flat (Send-Tcp $OWN "send-keys -t bogus:@1.%1 x`n")
Check ($r -match "ERROR: can't find session: bogus") "a bogus session on the own port is an ERROR" "reply [$r]"

# ── 4. the CLI is unchanged ──

Write-Head "4. the CLI path"
$pp = StartPing
$before = PingUnder $pp
P send-keys -t $TARGET C-c | Out-Null
$rc = $LASTEXITCODE
$after = @()
for ($i = 0; $i -lt 20; $i++) { Start-Sleep -Milliseconds 200; $after = PingUnder $pp; if ($after.Count -eq 0) { break } }
Check ($before.Count -gt 0 -and $after.Count -eq 0 -and $rc -eq 0) "CLI send-keys C-c stopped ping" "before [$before] after [$after] rc $rc"
StopPings $pp
$p0 = PanePid
P respawn-pane -k -t $TARGET -- pwsh -NoLogo -NoProfile | Out-Null
$rc = $LASTEXITCODE
Start-Sleep -Seconds 2
$p1 = PanePid
Check ($p1 -ne $p0 -and $rc -eq 0) "CLI respawn-pane -k $p0 -> $p1" "$p0 -> $p1 rc $rc"

} finally {
    try { StopPings (PanePid) } catch {}
    P kill-server | Out-Null
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed" -ForegroundColor Cyan
exit $script:TestsFailed
