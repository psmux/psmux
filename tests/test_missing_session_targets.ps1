# A -t that names no session is tmux's error, never "this session".
#
# tmux 3.4 (verified live, sessions A and B):
#   set -t NOPE @u x / set-environment -t NOPE / show-environment -t NOPE
#       -> "no such session: NOPE", exit 1, nothing set
#   list-windows -t NOPE / has-session -t NOPE / lock-session -t NOPE
#       -> "can't find session: NOPE", exit 1
#   from a hook in A: nothing changes in A; `set -t <a window of A>` sets A
#   -t A and -t B act on A and B.
#
# psmux before the fix: over raw TCP to A every one of these acted on A and
# answered OK (list-windows -t NOPE listed A, set -t NOPE set A's option,
# set-environment -t B set it in A); from a hook set and set-environment -t
# NOPE changed A; the CLI's set -t NOPE was a warning at exit 0 and
# lock-session -t NOPE exit 0.

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
$root = Join-Path $env:TEMP "psmux_missing_target_${PID}_$(Get-Random)"
$script:Ns = $null

function P { & $Exe -L $script:Ns @args 2>&1 }
function Names { @(P ls -F '#{session_name}' | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) -join ',' }
function WaitFor([scriptblock]$cond, [int]$ms = 3000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return [bool](& $cond)
}
function Fresh {
    if ($script:Ns) { Down }
    $script:Ns = "mst$(Get-Random)"
    $env:PSMUX_DATA_DIR = Join-Path $root $script:Ns
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
    P new-session -d -s A -x 100 -y 30 -n mywin | Out-Null
    P new-session -d -s B -x 100 -y 30 | Out-Null
    $up = WaitFor { (Names) -eq 'A,B' } 15000
    if ($up) {
        foreach ($s in 'A', 'B') { P set -t $s -g display-time 60000 | Out-Null }
        # The hooks fire on renaming window 1, so `mywin` keeps its name.
        P new-window -d -t A: -n trig | Out-Null
    }
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
function Flat($s) { "$s" -replace "`r?`n", '\n' }
# The CLI's dump-state leaves the status message out; the raw reply has it.
function Status($s) { $d = Tcp $s 'dump-state'; if ($d -match '"status_message":"((?:[^"\\]|\\.)*)"') { $Matches[1] } else { '' } }
function Opt($s, $name) { ((P show -t $s -v $name) -join '').Trim() }
function Envv($s, $name) { ((P show-environment -t $s $name) -join '').Trim() }
function Fire($cmd) {
    P set-hook -t A after-rename-window $cmd | Out-Null
    P rename-window -t A:1 "fire$(Get-Random)" | Out-Null
}

Write-Host "`n=== a -t naming no session is tmux's error (CLI, TCP, hook) ===" -ForegroundColor Cyan
try {
    # ---- raw TCP to A ----
    if (Fresh) {
        $cases = @(
            @("set -t NOPE @u tcp", "ERROR: no such session: NOPE"),
            @("set -t NOPE status-left ZZtcp", "ERROR: no such session: NOPE"),
            @("list-windows -t NOPE", "ERROR: can't find session: NOPE"),
            @("set-environment -t NOPE EV tcp", "ERROR: no such session: NOPE"),
            @("show-environment -t NOPE EV", "ERROR: no such session: NOPE"),
            @("has-session -t NOPE", "ERROR: can't find session: NOPE"),
            @("lock-session -t NOPE", "ERROR: can't find session: NOPE")
        )
        foreach ($c in $cases) {
            $r = Tcp A $c[0]
            Check ($r -match [regex]::Escape($c[1])) "TCP $($c[0]) answers $($c[1])" "'$(Flat $r)'"
        }
        Start-Sleep -Milliseconds 400
        Check ((Opt A '@u') -notmatch '^tcp$' -and (Opt A status-left) -ne 'ZZtcp') "and A's options are untouched" "@u='$(Opt A '@u')' sl='$(Opt A status-left)'"
        Check ((Envv A EV) -notmatch 'EV=tcp') "and A's environment is untouched" "'$(Envv A EV)'"
        # own and other sessions still work
        $r = Tcp A "list-windows -t A -F '#{session_name}:#{window_name}'"
        Check ($r -match 'A:mywin' -and $r -notmatch 'ERROR') "TCP list-windows -t A lists A" "'$(Flat $r)'"
        $r = Tcp A 'has-session -t A'
        Check ($r -notmatch 'ERROR') "TCP has-session -t A is a yes" "'$(Flat $r)'"
        $r = Tcp A 'has-session -t B'
        Check ($r -notmatch 'ERROR') "TCP has-session -t B (another live session) is a yes" "'$(Flat $r)'"
        $r = Tcp A 'set-environment -t A EV ownA'
        Check ($r -notmatch 'ERROR' -and (WaitFor { (Envv A EV) -eq 'EV=ownA' })) "TCP set-environment -t A sets it in A" "'$(Flat $r)' A=$(Envv A EV)"
        $r = Tcp A 'set -t B @u fromA'
        Check ($r -notmatch 'ERROR' -and (WaitFor { (Opt B '@u') -eq 'fromA' })) "TCP set -t B on A's server still sets B (#1002)" "'$(Flat $r)' B.@u=$(Opt B '@u')"
        $r = Tcp A 'set-environment -t B EV fromA'
        Start-Sleep -Milliseconds 400
        Check ((Envv A EV) -eq 'EV=ownA') "TCP set-environment -t B never lands in A" "'$(Flat $r)' A=$(Envv A EV)"
    } else { Check $false "TCP setup" "sessions did not start" }

    # ---- hook in A ----
    if (Fresh) {
        $hooks = @(
            @('set -t NOPE @h hook', 'no such session: NOPE'),
            @('set-environment -t NOPE HV hook', 'no such session: NOPE'),
            @('list-windows -t NOPE', "can't find session: NOPE"),
            @('lock-session -t NOPE', "can't find session: NOPE"),
            @('has-session -t NOPE', "can't find session: NOPE")
        )
        foreach ($h in $hooks) {
            Fire $h[0]
            Start-Sleep -Milliseconds 900
            $sm = Status A
            Check ($sm -eq $h[1]) "hook $($h[0]): A's status line says $($h[1])" "'$sm'"
        }
        Check ((Opt A '@h') -notmatch 'hook') "and the hook's set -t NOPE did not set A" "'$(Opt A '@h')'"
        Check ((Envv A HV) -notmatch 'HV=hook') "and its set-environment -t NOPE did not set A" "'$(Envv A HV)'"
        Fire 'set -t mywin @w yes'
        Check (WaitFor { (Opt A '@w') -eq 'yes' }) "hook set -t <a window of A> still sets A" "'$(Opt A '@w')'"
        Fire 'set -t B @b yes'
        Check (WaitFor { (Opt B '@b') -eq 'yes' }) "hook set -t B still sets B (#1002)" "'$(Opt B '@b')'"
        Fire 'set-environment -t B HB yes'
        Check (WaitFor { (Envv B HB) -eq 'HB=yes' }) "hook set-environment -t B sets it in B (#938)" "'$(Envv B HB)'"
    } else { Check $false "hook setup" "sessions did not start" }

    # ---- CLI ----
    if (Fresh) {
        $out = (P set -t NOPE '@u' cli | Out-String).Trim(); $rc = $LASTEXITCODE
        Check ($rc -eq 1 -and $out -match 'no such session: NOPE') "CLI set -t NOPE fails with no such session" "rc=$rc '$out'"
        $out = (P lock-session -t NOPE | Out-String).Trim(); $rc = $LASTEXITCODE
        Check ($rc -eq 1 -and $out -match "can't find session: NOPE") "CLI lock-session -t NOPE fails with can't find session" "rc=$rc '$out'"
        P lock-session -t A | Out-Null
        Check ($LASTEXITCODE -eq 0) "CLI lock-session -t A succeeds" "rc=$LASTEXITCODE"
        P set -t A '@u' cliA | Out-Null
        Check ($LASTEXITCODE -eq 0 -and (Opt A '@u') -eq 'cliA') "CLI set -t A sets A" "rc=$LASTEXITCODE @u=$(Opt A '@u')"
        P set -t B '@u' cliB | Out-Null
        Check ($LASTEXITCODE -eq 0 -and (Opt B '@u') -eq 'cliB') "CLI set -t B sets B" "rc=$LASTEXITCODE @u=$(Opt B '@u')"
        P list-windows -t NOPE | Out-Null
        Check ($LASTEXITCODE -eq 1) "CLI list-windows -t NOPE fails" "rc=$LASTEXITCODE"
        P set-environment -t NOPE EV x | Out-Null
        Check ($LASTEXITCODE -eq 1) "CLI set-environment -t NOPE fails" "rc=$LASTEXITCODE"
    } else { Check $false "CLI setup" "sessions did not start" }
} finally {
    if ($script:Ns) { Down }
    if ($null -eq $savedData) { Remove-Item Env:PSMUX_DATA_DIR -EA SilentlyContinue } else { $env:PSMUX_DATA_DIR = $savedData }
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

$defaultAfter = (& $Exe ls -F '#{session_name} #{session_created}' 2>&1 | Out-String)
Check ($defaultBefore -eq $defaultAfter) "the default namespace is unchanged" ""

Write-Host "`n=== Results: $($script:Passed) passed, $($script:Failed) failed ===" -ForegroundColor $(if ($script:Failed) { 'Red' } else { 'Green' })
exit $(if ($script:Failed) { 1 } else { 0 })
