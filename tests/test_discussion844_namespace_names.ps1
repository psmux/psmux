# Discussion #844: a session name with `__` in it and an -L namespace name.
#
# psmux names a session's registry files after a base: `<ns>__<session>` under
# -L <ns>, and the session name alone in the default namespace. Two things went
# wrong with names that contain `__`:
#
# 1. Under -L nsq a session named `nsq__x` has the base `nsq__nsq__x`, but
#    `-t nsq__x` looked up `nsq__x`, because the lookup skipped the prefix when
#    the name already started with `nsq__`. has-session, rename-session and
#    every other -t on such a session failed.
#
# 2. A default namespace session named `nsq__bb` has the base `nsq__bb`, the
#    same as session `bb` under -L nsq. Each blocked the other, and the default
#    one was listed, found and killed as namespace nsq.
#
# Every expected value below is what tmux 3.4 and 3.7c gave for the same
# commands with a socket per -L name (TMUX_TMPDIR of its own), except one rule
# tmux does not have: the default namespace refuses a session name with `__`
# in it (`invalid session: NAME`, exit 1). The registry file names are read by
# plugins and other tools and stay as they are, so such a name cannot be told
# apart from a session of an -L namespace; refusing it is the only way to keep
# the two from being confused.
#
# The suite runs in a PSMUX_DATA_DIR of its own, which holds the default
# namespace's registry too, and checks that before it creates a default
# namespace session. Set PSMUX_TEST_EXE to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

$POWERSHELL = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
. "$PSScriptRoot\isolated_config.ps1" -Lines @("set -g default-shell $POWERSHELL")

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $PSMUX = $env:PSMUX_TEST_BIN }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'PSMUX_TARGET_SESSION', 'PSMUX_TARGET_FULL', 'TMUX', 'TMUX_PANE') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}
$env:PSMUX_NO_WARM = "1"

$REAL_DIR = Join-Path $env:USERPROFILE ".psmux"
$TMP = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_d844_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null
$env:PSMUX_DATA_DIR = $TMP
# Two namespaces of this run's own. $NS is the one the names collide with.
$NS = "nsq" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$NS2 = "nsr" + [guid]::NewGuid().ToString('N').Substring(0, 6)

Write-Host "`n=== Discussion #844: namespace and session names with __ ===" -ForegroundColor Cyan
Write-Host "  Binary:     $PSMUX"
Write-Host "  Data dir:   $TMP"
Write-Host "  Namespaces: $NS $NS2"

# Run psmux with these arguments: exit code, stdout and stderr.
# Only this process is waited for: new-session leaves a server behind, and
# Start-Process -Wait would wait for it too.
function Run([string[]]$argv) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new($PSMUX)
    foreach ($a in $argv) { $psi.ArgumentList.Add($a) }
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    $outTask = $p.StandardOutput.ReadToEndAsync(); $errTask = $p.StandardError.ReadToEndAsync()
    if (-not $p.WaitForExit(20000)) { try { $p.Kill() } catch {} }
    # The server inherits nothing it keeps open, but do not hang on it if it does.
    [void]$outTask.Wait(2000); [void]$errTask.Wait(2000)
    return @{
        Code = $p.ExitCode
        Out  = $(if ($outTask.IsCompleted) { $outTask.Result.Trim() } else { "" })
        Err  = $(if ($errTask.IsCompleted) { $errTask.Result.Trim() } else { "" })
    }
}
# Session names `ls` prints, in order.
function Names([string[]]$prefix) {
    $r = Run (@($prefix) + @("ls", "-F", "#{session_name}"))
    if ($r.Code -ne 0) { return @() }
    return @($r.Out -split "`r?`n" | Where-Object { $_ })
}
function RealPorts { @(Get-ChildItem $REAL_DIR -Filter *.port -EA SilentlyContinue | ForEach-Object Name | Sort-Object) -join "," }
function WaitPort($base) {
    for ($i = 0; $i -lt 50; $i++) { if (Test-Path (Join-Path $TMP "$base.port")) { return $true }; Start-Sleep -Milliseconds 100 }
    return $false
}
# Stop every session this run made, one by one, in each namespace it used.
# Never a bare kill-server: that is a default namespace sweep.
function Cleanup {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    & $PSMUX -L $NS2 kill-server 2>&1 | Out-Null
    foreach ($n in (Names @())) { & $PSMUX kill-session -t "=$n" 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 400
    # A server the old binary could not find by name still has a pid file in
    # this run's own data dir: stop that PID if it is this binary.
    foreach ($f in (Get-ChildItem $TMP -Filter *.pid -EA SilentlyContinue)) {
        $id = ((Get-Content $f.FullName -Raw -EA SilentlyContinue) + "") -replace ':.*', ''
        if ($id -match '^\d+$') {
            $proc = Get-CimInstance Win32_Process -Filter "ProcessId=$id" -EA SilentlyContinue
            if ($proc -and $proc.ExecutablePath -eq $PSMUX) { Stop-Process -Id ([int]$id) -Force -EA SilentlyContinue }
        }
        Remove-Item ($f.FullName -replace '\.pid$', '.*') -Force -EA SilentlyContinue
    }
}

$realBefore = RealPorts

try {
    Write-Head "The private data dir holds the default namespace's registry"
    $r = Run @("new-session", "-d", "-s", "probe844")
    $inTmp = WaitPort "probe844"
    $inReal = Test-Path (Join-Path $REAL_DIR "probe844.port")
    if (-not $inTmp -or $inReal) {
        Write-Fail "default namespace is not isolated (private: $inTmp, real: $inReal); stopping"
        & $PSMUX kill-session -t probe844 2>&1 | Out-Null
        exit 99
    }
    Write-Pass "default namespace session probe844 lives in the private data dir only"
    & $PSMUX kill-session -t probe844 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400

    # ---------------------------------------------------------------- 1
    Write-Head "1. A name that starts with the namespace, under -L $NS"
    $X = "${NS}__x"; $Z = "${NS}__z"
    $r = Run @("-L", $NS, "new-session", "-d", "-s", $X)
    Check ($r.Code -eq 0) "-L $NS new-session -s $X exits 0 (tmux)" "exit $($r.Code) [$($r.Err)]"
    WaitPort "${NS}__$X" | Out-Null
    Check ((Names @("-L", $NS)) -contains $X) "-L $NS ls shows $X (tmux)" "ls: $((Names @('-L', $NS)) -join ',')"
    $r = Run @("-L", $NS, "has-session", "-t", $X)
    Check ($r.Code -eq 0) "-L $NS has-session -t $X exits 0 (tmux)" "exit $($r.Code) [$($r.Err)]"
    $r = Run @("-L", $NS, "display-message", "-p", "-t", $X, "#{session_name}")
    Check (($r.Code -eq 0) -and ($r.Out -eq $X)) "-L $NS display-message -t $X names it (tmux)" "exit $($r.Code) out [$($r.Out)] [$($r.Err)]"
    $r = Run @("-L", $NS, "rename-session", "-t", $X, $Z)
    Check ($r.Code -eq 0) "-L $NS rename-session -t $X $Z exits 0 (tmux)" "exit $($r.Code) [$($r.Err)]"
    Start-Sleep -Milliseconds 300
    $r = Run @("-L", $NS, "has-session", "-t", $Z)
    Check ($r.Code -eq 0) "-L $NS has-session -t $Z exits 0 after the rename (tmux)" "exit $($r.Code) [$($r.Err)]"
    $r = Run @("-L", $NS, "new-session", "-d", "-s", "y")
    WaitPort "${NS}__y" | Out-Null
    $r = Run @("-L", $NS, "rename-session", "-t", "y", "${NS}__w")
    Start-Sleep -Milliseconds 300
    $r = Run @("-L", $NS, "has-session", "-t", "${NS}__w")
    Check ($r.Code -eq 0) "-L $NS rename y to ${NS}__w, then has-session -t ${NS}__w exits 0 (tmux)" "exit $($r.Code) [$($r.Err)]"
    $r = Run @("-L", $NS, "kill-session", "-t", $Z)
    Check ($r.Code -eq 0) "-L $NS kill-session -t $Z exits 0 (tmux)" "exit $($r.Code) [$($r.Err)]"
    Start-Sleep -Milliseconds 400
    Check (-not ((Names @("-L", $NS)) -contains $Z)) "$Z is gone after kill-session (tmux)" "ls: $((Names @('-L', $NS)) -join ',')"
    # A name that only looks like a namespace prefix is an ordinary name.
    $r = Run @("-L", $NS, "new-session", "-d", "-s", "${NS2}__v")
    WaitPort "${NS}__${NS2}__v" | Out-Null
    $r = Run @("-L", $NS, "has-session", "-t", "${NS2}__v")
    Check ($r.Code -eq 0) "-L $NS has-session -t ${NS2}__v exits 0 (tmux)" "exit $($r.Code) [$($r.Err)]"
    Check ((Names @("-L", $NS2)).Count -eq 0) "-L $NS2 ls does not show $NS's ${NS2}__v (tmux)" "ls: $((Names @('-L', $NS2)) -join ',')"
    Cleanup

    # ---------------------------------------------------------------- 2
    # tmux accepts nsq__bb in the default namespace (its own socket). psmux
    # cannot store it apart from -L nsq's bb without renaming the registry
    # files every tool reads, so it refuses the name the way tmux refuses a
    # name it cannot store: `invalid session: NAME`, exit 1, nothing made.
    Write-Head "2. The default namespace refuses ${NS}__bb, so -L $NS's bb is free"
    $DB = "${NS}__bb"
    $r = Run @("new-session", "-d", "-s", $DB)
    Check (($r.Code -eq 1) -and ($r.Err -match "^invalid session: $([regex]::Escape($DB))")) `
        "new-session -s $DB is refused: [$($r.Err)]" "exit $($r.Code) [$($r.Err)]"
    Start-Sleep -Milliseconds 500
    Check (-not (Test-Path (Join-Path $TMP "$DB.port"))) "no registry file $DB.port was written" "the session was made"
    Check ((Names @("-L", $NS)).Count -eq 0) "-L $NS ls shows nothing (tmux)" "ls: [$((Names @('-L', $NS)) -join ',')]"
    $r = Run @("-L", $NS, "has-session", "-t", "bb")
    Check ($r.Code -ne 0) "-L $NS has-session -t bb fails (tmux)" "exit $($r.Code)"
    $r = Run @("-L", $NS, "new-session", "-d", "-s", "bb")
    Check ($r.Code -eq 0) "-L $NS new-session -s bb exits 0 (tmux)" "exit $($r.Code) [$($r.Err)]"
    WaitPort $DB | Out-Null
    Check ((Names @("-L", $NS)) -contains "bb") "-L $NS ls shows bb (tmux)" "ls: [$((Names @('-L', $NS)) -join ',')]"
    Check ((Names @()).Count -eq 0) "ls in the default namespace does not show it (tmux)" "ls: [$((Names @()) -join ',')]"
    $r = Run @("new-session", "-d", "-s", $DB)
    Check (($r.Code -eq 1) -and ($r.Err -match "^invalid session: ")) "new-session -s $DB is still refused, not a duplicate of -L $NS's bb" "exit $($r.Code) [$($r.Err)]"
    $r = Run @("new-session", "-d", "-A", "-s", $DB)
    Check (($r.Code -eq 1) -and ($r.Err -match "^invalid session: ")) "new-session -A -s $DB does not attach to -L $NS's bb" "exit $($r.Code) [$($r.Err)]"
    Check ((Names @("-L", $NS)) -contains "bb") "-L $NS's bb is untouched" "ls: [$((Names @('-L', $NS)) -join ',')]"
    # Other names are not affected.
    foreach ($ok in @("a_b", "c_", "_d")) {
        $r = Run @("new-session", "-d", "-s", $ok)
        WaitPort $ok | Out-Null
        Check (($r.Code -eq 0) -and ((Names @()) -contains $ok)) "new-session -s $ok exits 0 and is listed (tmux)" "exit $($r.Code) [$($r.Err)] ls [$((Names @()) -join ',')]"
    }
    Cleanup

    # ---------------------------------------------------------------- 3
    Write-Head "3. Renames and the server routes refuse it too"
    $r = Run @("-L", $NS, "new-session", "-d", "-s", "bb")
    WaitPort "${NS}__bb" | Out-Null
    $r = Run @("new-session", "-d", "-s", "plain")
    WaitPort "plain" | Out-Null
    $r = Run @("rename-session", "-t", "plain", $DB)
    Check (($r.Code -eq 1) -and ($r.Err -match "^invalid session: ")) "rename-session -t plain $DB is refused: [$($r.Err)]" "exit $($r.Code) [$($r.Err)]"
    $r = Run @("rename-session", "-t", "plain", "q__r")
    Check (($r.Code -eq 1) -and ($r.Err -match "^invalid session: q__r")) "rename-session -t plain q__r is refused" "exit $($r.Code) [$($r.Err)]"
    Check ((Names @()) -contains "plain") "plain keeps its name" "ls: [$((Names @()) -join ',')]"
    Check ((Names @("-L", $NS)) -contains "bb") "-L $NS ls still shows bb" "ls: [$((Names @('-L', $NS)) -join ',')]"

    # The server's own command line (source-file runs it there) and the raw
    # TCP command channel go through the server's new-session and rename.
    $conf = Join-Path $TMP "case.conf"
    "new-session -d -s srcx__y" | Set-Content -Path $conf -Encoding ASCII
    $r = Run @("source-file", $conf)
    Start-Sleep -Milliseconds 800
    Check (-not (Test-Path (Join-Path $TMP "srcx__y.port"))) "source-file new-session -s srcx__y makes nothing" "srcx__y was made"
    $port = (Get-Content (Join-Path $TMP "plain.port") -Raw).Trim()
    $key = (Get-Content (Join-Path $TMP "plain.key") -Raw).Trim()
    function Tcp($line) {
        $c = [Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port)
        $s = $c.GetStream(); $s.ReadTimeout = 3000
        $w = [IO.StreamWriter]::new($s); $w.Write("AUTH $key`n$line`n"); $w.Flush()
        $rd = [IO.StreamReader]::new($s); $out = @()
        try { while ($null -ne ($l = $rd.ReadLine())) { $out += $l } } catch {}
        $c.Close(); return ($out -join "`n")
    }
    $reply = Tcp "new-session -s tcpx__y"
    Start-Sleep -Milliseconds 800
    Check (($reply -match "invalid session: tcpx__y") -and -not (Test-Path (Join-Path $TMP "tcpx__y.port"))) `
        "TCP new-session -s tcpx__y is refused with invalid session" "reply [$reply]"
    $reply = Tcp "rename-session tcpz__w"
    Start-Sleep -Milliseconds 500
    Check (((Names @()) -contains "plain") -and -not (Test-Path (Join-Path $TMP "tcpz__w.port"))) `
        "TCP rename-session tcpz__w leaves plain as it is" "ls: [$((Names @()) -join ',')]"

    $r = Run @("-L", $NS, "kill-server")
    Start-Sleep -Milliseconds 400
    Check ((Names @()) -contains "plain") "-L $NS kill-server leaves the default namespace running (tmux)" "ls: [$((Names @()) -join ',')]"
    Cleanup
}
finally {
    Cleanup
    $realAfter = RealPorts
    if ($realAfter -ne $realBefore) { Write-Fail "the real registry changed: before [$realBefore] after [$realAfter]" }
    Remove-PsmuxIsolatedConfig -EA SilentlyContinue
    Remove-Item Env:PSMUX_DATA_DIR, Env:PSMUX_NO_WARM -EA SilentlyContinue
    Remove-Item $TMP -Recurse -Force -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
