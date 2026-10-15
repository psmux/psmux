# Issue #819 (PR #820 by GiorgoLazaridis): a server side if-shell condition
# must still run after send-keys C-c.
#
# A condition holding #{pid} is forwarded to the server, which expands it and
# runs the shell there. Relaying C-c to a cmd pane detaches the server from
# its console, which leaves its stdin slot holding a dead handle; a condition
# child that inherited it failed to start (os error 6) and if-shell took the
# false branch even for a guard that matches the server it runs in.
#
# This is the identity guard oh-my-claudecode uses at worker shutdown: the
# condition compares #{pid} with the server pid it recorded. The broader
# sweep over every job kind is test_issue819_job_stdin_after_ctrl_c.ps1.
#
# Runs in its own -L namespace with a private PSMUX_DATA_DIR, and checks that
# the server and pane it started are gone after its own kill-server.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP ("psmux_ifctrlc_data_" + $PID)
    New-Item -ItemType Directory -Force -Path $env:PSMUX_DATA_DIR | Out-Null
}
$dataDir = $env:PSMUX_DATA_DIR
$TMP = Join-Path $env:TEMP ("psmux_ifctrlc_" + $PID)
New-Item -ItemType Directory -Force -Path $TMP | Out-Null
$SOCK = "ifctrlc_$PID"
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }
function P { & $PSMUX -L $SOCK @args 2>&1 | Out-String | ForEach-Object { $_.Trim() } }
function Check([string]$what, [string]$got, [string]$want) {
    if ($got -ceq $want) { Write-Pass "$what = '$want'" } else { Write-Fail "$what = '$got' (expected '$want')" }
}
function Wait-Port([string]$sess) {
    for ($i = 0; $i -lt 80; $i++) { if (Test-Path "$dataDir\${SOCK}__$sess.port") { return }; Start-Sleep -Milliseconds 250 }
}

Write-Host "binary:  $PSMUX" -ForegroundColor Cyan
Write-Host "socket:  $SOCK" -ForegroundColor Cyan
$conf = Join-Path $TMP "psmux.conf"
[IO.File]::WriteAllText($conf, "set -g default-shell cmd.exe`nset -g remain-on-exit on`n")
$owned = @()

try {
    & $PSMUX -L $SOCK -f $conf new-session -d -s stdin -- cmd.exe /d /k 2>&1 | Out-Null
    Wait-Port stdin; Start-Sleep -Milliseconds 800
    foreach ($f in '#{pid}', '#{pane_pid}') {
        $id = [int](P display-message -p $f)
        $proc = Get-Process -Id $id -ErrorAction SilentlyContinue
        if ($proc) { $owned += [pscustomobject]@{ pid = $id; start = $proc.StartTime.ToUniversalTime().Ticks } }
    }
    $serverPid = $owned[0].pid
    Write-Info "server pid $serverPid, pane pid $($owned[1].pid)"

    # #{pid} makes the CLI forward the condition, so the server expands it
    # and runs the shell.
    $guard = "if ([int]'#{pid}' -eq $serverPid) { exit 0 } exit 1"
    Check "guard before C-c" (P if-shell $guard 'display-message -p ACCEPTED' 'display-message -p REJECTED') "ACCEPTED"

    P send-keys -t stdin:0.0 -l 'echo benign' | Out-Null
    P send-keys -t stdin:0.0 Enter | Out-Null
    $ready = $false
    for ($i = 0; $i -lt 40; $i++) {
        if ((P capture-pane -p -t stdin:0.0) -split "`n" | Where-Object { $_.Trim() -eq 'benign' }) { $ready = $true; break }
        Start-Sleep -Milliseconds 125
    }
    if ($ready) { Write-Pass "pane ran 'echo benign'" } else { Write-Fail "pane never printed 'benign'" }
    P send-keys -t stdin:0.0 C-c | Out-Null

    # run-shell also proves the C-c dispatch has been handled.
    Check "run-shell expands #{pid} after C-c" (P run-shell "Write-Output '#{pid}'") "$serverPid"
    Check "guard after C-c" (P if-shell $guard 'display-message -p ACCEPTED' 'display-message -p REJECTED') "ACCEPTED"
    Check "failing condition after C-c" (P if-shell 'exit 1 # #{pid}' 'display-message -p ACCEPTED' 'display-message -p REJECTED') "REJECTED"
} finally {
    & $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
    foreach ($o in $owned) {
        $same = $true
        for ($i = 0; $i -lt 100 -and $same; $i++) {
            $cur = Get-Process -Id $o.pid -ErrorAction SilentlyContinue
            $same = $null -ne $cur -and $cur.StartTime.ToUniversalTime().Ticks -eq $o.start
            if ($same) { Start-Sleep -Milliseconds 50 }
        }
        if ($same) { Write-Fail "process $($o.pid) outlived kill-server" } else { Write-Pass "process $($o.pid) gone after kill-server" }
    }
    Remove-Item -Recurse -Force $TMP -ErrorAction SilentlyContinue
}

Write-Host "`nResults: $script:Pass passed, $script:Fail failed" -ForegroundColor $(if ($script:Fail) { "Red" } else { "Green" })
exit $script:Fail
