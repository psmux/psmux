# rename-session from a hook, the command prompt or a raw TCP line reads -t as
# a session, as tmux does (CMD_FIND_SESSION), and the rename moves the
# session's registry files from its CURRENT name.
#
# tmux 3.4 (verified live, hook after-rename-window on A, sessions A and B):
#   rename-session -t NOPE X  -> "can't find session: NOPE", ls A,B
#   rename-session X          -> ls B,X
#   rename-session B          -> "duplicate session: B", ls A,B
#   rename-session -t B X     -> ls A,X
#
# psmux before the fix: the hook's -t NOPE renamed the hook's OWN session, every
# hook rename left the old .port behind (ls listed X twice), and a hook rename
# onto B wrote A's port over B's registry (ls B,B, the real B orphaned). Over raw
# TCP, -t NOPE renamed this server's session.

param(
    [string]$Exe = $(if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source })
)

$ErrorActionPreference = "Continue"
$script:Passed = 0
$script:Failed = 0
function Check([bool]$ok, [string]$what, [string]$detail) {
    if ($ok) { Write-Host "  [PASS] $what" -ForegroundColor Green; $script:Passed++ }
    else { Write-Host "  [FAIL] $what ($detail)" -ForegroundColor Red; $script:Failed++ }
}

Remove-Item Env:PSMUX_SERVER_MODE -ErrorAction SilentlyContinue
foreach ($v in 'TMUX', 'TMUX_PANE', 'PSMUX_SESSION', 'PSMUX_TARGET_SESSION', 'PSMUX_SESSION_NAME') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedData = $env:PSMUX_DATA_DIR
$env:PSMUX_NO_WARM = '1'
$defaultBefore = (& $Exe ls -F '#{session_name} #{session_created}' 2>&1 | Out-String)
$root = Join-Path $env:TEMP "psmux_hook_rename_${PID}_$(Get-Random)"
$script:Ns = $null

function P { & $Exe -L $script:Ns @args 2>&1 }
function Names { @(P ls -F '#{session_name}' | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) -join ',' }
function Ports { @(Get-ChildItem $env:PSMUX_DATA_DIR -File -Filter '*.port' | ForEach-Object { $_.BaseName -replace '^.*__', '' } | Sort-Object) -join ',' }
function Status($s) {
    # The CLI's dump-state leaves the status message out; the raw reply has it.
    $d = Tcp $s 'dump-state'
    if ($d -match '"status_message":"((?:[^"\\]|\\.)*)"') { $Matches[1] } else { '' }
}
function WaitFor([scriptblock]$cond, [int]$ms = 3000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return [bool](& $cond)
}
function Fresh {
    if ($script:Ns) { Down }
    $script:Ns = "hrn$(Get-Random)"
    $env:PSMUX_DATA_DIR = Join-Path $root $script:Ns
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
    P new-session -d -s A -x 100 -y 30 | Out-Null
    P new-session -d -s B -x 100 -y 30 | Out-Null
    $up = WaitFor { (Names) -eq 'A,B' } 15000
    # One server per session: each needs the long display-time, or A's
    # message is gone before it is read.
    if ($up) { foreach ($s in 'A', 'B') { P set -t $s -g display-time 60000 | Out-Null } }
    return $up
}
function Down {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 300
    foreach ($p in @(Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -match "-L $($script:Ns)(\s|$)" })) {
        taskkill /PID $p.ProcessId /T /F 2>&1 | Out-Null
    }
    $script:Ns = $null
}
function Fire($cmd) {
    P set-hook -t A after-rename-window $cmd | Out-Null
    P rename-window -t A:0 "fire$(Get-Random)" | Out-Null
}
function Tcp($session, $line) {
    $base = Join-Path $env:PSMUX_DATA_DIR "$($script:Ns)__$session"
    if (-not (Test-Path "$base.port")) { return "(no $session.port)" }
    $port = [int]((Get-Content "$base.port" -Raw).Trim())
    $key = (Get-Content "$base.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', $port)
    $stream = $tcp.GetStream(); $stream.ReadTimeout = 3000
    $bytes = [Text.Encoding]::UTF8.GetBytes("AUTH $key`n$line`n")
    $stream.Write($bytes, 0, $bytes.Length)
    $tcp.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send)
    $buf = New-Object byte[] 65536; $ms = New-Object System.IO.MemoryStream
    try { while (($n = $stream.Read($buf, 0, $buf.Length)) -gt 0) { $ms.Write($buf, 0, $n) } } catch {}
    $tcp.Close()
    [Text.Encoding]::UTF8.GetString($ms.ToArray())
}

Write-Host "`n=== rename-session -t and the registry files (hook, TCP, CLI) ===" -ForegroundColor Cyan
try {
    # H1: the hook's -t names no session.
    if (Fresh) {
        Fire 'rename-session -t NOPE X'
        Start-Sleep -Milliseconds 1200
        # Read before ls: listing the sessions clears a status message.
        $sm = Status A
        Check ((Names) -eq 'A,B') "hook rename-session -t NOPE X renames nothing" "ls=$(Names)"
        Check ($sm -match "can't find session: NOPE") "and A's status line says can't find session: NOPE" "'$sm'"
    } else { Check $false "H1 setup" "sessions did not start" }

    # H1b: the `rename` alias is the same command.
    if (Fresh) {
        Fire 'rename -t NOPE X'
        Start-Sleep -Milliseconds 1200
        $sm = Status A
        Check ((Names) -eq 'A,B') "hook rename -t NOPE X (alias) renames nothing" "ls=$(Names)"
        Check ($sm -match "can't find session: NOPE") "and A's status line says can't find session: NOPE" "'$sm'"
    } else { Check $false "H1b setup" "sessions did not start" }

    # H2: a plain hook rename moves the files: one row, no old .port.
    if (Fresh) {
        Fire 'rename-session X'
        $ok = WaitFor { (Names) -eq 'B,X' }
        Check $ok "hook rename-session X lists B,X once each" "ls=$(Names)"
        Check ((Ports) -eq 'B,X') "and A's .port is gone" "ports=$(Ports)"
        P has-session -t A 2>&1 | Out-Null
        Check ($LASTEXITCODE -ne 0) "and has-session -t A fails" "rc=$LASTEXITCODE"
        $fmt = ((P display -p -t X '#{session_name}') -join '').Trim()
        Check ($fmt -eq 'X') "and X answers with its new name" "'$fmt'"
    } else { Check $false "H2 setup" "sessions did not start" }

    # H3: a hook rename onto a live session is refused and B keeps its files.
    if (Fresh) {
        $bPort = (Get-Content (Join-Path $env:PSMUX_DATA_DIR "$($script:Ns)__B.port") -Raw).Trim()
        $bPid = ((P display -p -t B '#{pid}') -join '').Trim()
        Fire 'rename-session B'
        Start-Sleep -Milliseconds 1200
        $sm = Status A
        Check ((Names) -eq 'A,B') "hook rename-session B onto a live B renames nothing" "ls=$(Names)"
        $now = (Get-Content (Join-Path $env:PSMUX_DATA_DIR "$($script:Ns)__B.port") -Raw).Trim()
        Check ($now -eq $bPort) "and B's .port still names B's server" "before=$bPort now=$now"
        $pidNow = ((P display -p -t B '#{pid}') -join '').Trim()
        Check ($pidNow -eq $bPid) "and B is still the same server" "before=$bPid now=$pidNow"
        Check ($sm -match 'duplicate session: B') "and A's status line says duplicate session: B" "'$sm'"
    } else { Check $false "H3 setup" "sessions did not start" }

    # H4: -t naming the other live session still renames that one (#938).
    if (Fresh) {
        Fire 'rename-session -t B X'
        $ok = WaitFor { (Names) -eq 'A,X' }
        Check $ok "hook rename-session -t B X renames B" "ls=$(Names)"
    } else { Check $false "H4 setup" "sessions did not start" }

    # T1: raw TCP to A, -t names no session.
    if (Fresh) {
        $reply = Tcp A 'rename-session -t NOPE X'
        Start-Sleep -Milliseconds 800
        Check ($reply -match "ERROR: can't find session: NOPE") "TCP rename-session -t NOPE X replies can't find session" "'$($reply -replace "`r?`n", '\n')'"
        Check ((Names) -eq 'A,B') "and renames nothing" "ls=$(Names)"
    } else { Check $false "T1 setup" "sessions did not start" }

    if (Fresh) {
        # T2: the own session by name still renames.
        $reply = Tcp A 'rename-session -t A Y'
        $ok = WaitFor { (Names) -eq 'B,Y' }
        Check $ok "TCP rename-session -t A Y renames A" "ls=$(Names) reply='$($reply -replace "`r?`n", '\n')'"
        # T3: no -t renames this server's session.
        $reply = Tcp B 'rename-session Z'
        $ok = WaitFor { (Names) -eq 'Y,Z' }
        Check $ok "TCP rename-session Z renames B" "ls=$(Names)"
    } else { Check $false "T2 setup" "sessions did not start" }

    # C1: the CLI.
    if (Fresh) {
        $out = (P rename-session -t NOPE X | Out-String).Trim()
        $rc = $LASTEXITCODE
        Check ($rc -ne 0 -and (Names) -eq 'A,B') "CLI rename-session -t NOPE X fails and renames nothing" "rc=$rc out='$out' ls=$(Names)"
        P rename-session -t A X | Out-Null
        $ok = WaitFor { (Names) -eq 'B,X' }
        Check $ok "CLI rename-session -t A X renames A" "ls=$(Names)"
        Check ((Ports) -eq 'B,X') "and A's .port is gone" "ports=$(Ports)"
    } else { Check $false "C1 setup" "sessions did not start" }
} finally {
    if ($script:Ns) { Down }
    if ($null -eq $savedData) { Remove-Item Env:PSMUX_DATA_DIR -EA SilentlyContinue } else { $env:PSMUX_DATA_DIR = $savedData }
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

$defaultAfter = (& $Exe ls -F '#{session_name} #{session_created}' 2>&1 | Out-String)
Check ($defaultBefore -eq $defaultAfter) "the default namespace is unchanged" ""

Write-Host "`n=== Results: $($script:Passed) passed, $($script:Failed) failed ===" -ForegroundColor $(if ($script:Failed) { 'Red' } else { 'Green' })
exit $(if ($script:Failed) { 1 } else { 0 })
