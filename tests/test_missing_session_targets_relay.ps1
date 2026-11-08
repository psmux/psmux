# Session targets, part 2: the CLI says tmux's words for a -t naming no
# session, and a raw TCP line naming ANOTHER live session runs there.
#
# tmux 3.4 (verified live, sessions A and B):
#   list-windows -t NOPE     -> "can't find session: NOPE", exit 1
#   set-environment -t NOPE  -> "no such session: NOPE", exit 1
#   show-environment -t NOPE -> "no such session: NOPE", exit 1
#   show -t NOPE             -> "no such session: NOPE", exit 1
#   has-session -t NOPE      -> "can't find session: NOPE", exit 1
#   show -t B / list-windows -t B / show-environment -t B -> B's values
#   from inside A, `-t <a window of A>` is A
#
# psmux before: the CLI said "no server running on session '<ns>__NOPE'" and
# has-session said nothing; over TCP to A, `show -t NOPE` showed A's options,
# and list-windows / show-environment / set-environment -t B were refused
# with "can't find session: B" (show -t B showed A's).

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
$root = Join-Path $env:TEMP "psmux_target_relay_${PID}_$(Get-Random)"
$script:Ns = "mtr$(Get-Random)"
$env:PSMUX_DATA_DIR = Join-Path $root $script:Ns
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null

function P { & $Exe -L $script:Ns @args 2>&1 }
function Names { @(P ls -F '#{session_name}' | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) -join ',' }
function WaitFor([scriptblock]$cond, [int]$ms = 3000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 100 }
    return [bool](& $cond)
}
function RunCli {
    $err = Join-Path $root 'stderr.txt'
    $out = & $Exe -L $script:Ns @args 2> $err
    $rc = $LASTEXITCODE
    $e = if (Test-Path $err) { (Get-Content $err -Raw) } else { '' }
    @{ rc = $rc; out = (($out | ForEach-Object { "$_" }) -join "`n").Trim(); err = "$e".Trim() }
}
function Tcp($session, $line) {
    $base = Join-Path $env:PSMUX_DATA_DIR "$($script:Ns)__$session"
    if (-not (Test-Path "$base.port")) { return "(no $session.port)" }
    $port = [int]((Get-Content "$base.port" -Raw).Trim())
    $key = (Get-Content "$base.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', $port)
    $stream = $tcp.GetStream(); $stream.ReadTimeout = 6000
    $bytes = [Text.Encoding]::UTF8.GetBytes("AUTH $key`n$line`n")
    $stream.Write($bytes, 0, $bytes.Length)
    $tcp.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send)
    $buf = New-Object byte[] 65536; $ms = New-Object System.IO.MemoryStream
    try { while (($n = $stream.Read($buf, 0, $buf.Length)) -gt 0) { $ms.Write($buf, 0, $n) } } catch {}
    $tcp.Close()
    [Text.Encoding]::UTF8.GetString($ms.ToArray())
}
function Flat($s) { "$s" -replace "`r?`n", '\n' }
function Envv($s, $name) { ((P show-environment -t $s $name) -join '').Trim() }

Write-Host "`n=== session targets part 2: CLI wording, TCP relay to another session ===" -ForegroundColor Cyan
try {
    P new-session -d -s A -x 100 -y 30 -n mywin | Out-Null
    P new-session -d -s B -x 100 -y 30 -n bwin | Out-Null
    if (-not (WaitFor { (Names) -eq 'A,B' } 15000)) { Check $false "setup" "sessions did not start: $(Names)" }
    else {
        P set -t A status-left AAA | Out-Null
        P set -t B status-left BBB | Out-Null
        P set-environment -t B EVB inB | Out-Null

        # ---- CLI wording ----
        foreach ($c in @(
            @(@('list-windows', '-t', 'NOPE'), "can't find session: NOPE"),
            @(@('set-environment', '-t', 'NOPE', 'EV', 'x'), 'no such session: NOPE'),
            @(@('show-environment', '-t', 'NOPE', 'EV'), 'no such session: NOPE'),
            @(@('show', '-t', 'NOPE', '-v', 'status-left'), 'no such session: NOPE'),
            @(@('has-session', '-t', 'NOPE'), "can't find session: NOPE"),
            @(@('has-session', '-t', '=NOPE'), "can't find session: NOPE")
        )) {
            $words = $c[0]
            $r = RunCli @words
            Check ($r.rc -eq 1 -and $r.err -match [regex]::Escape($c[1]) -and $r.err -notmatch 'no server running') "CLI $($c[0] -join ' ') says $($c[1]) at exit 1" "rc=$($r.rc) err='$($r.err)'"
        }
        $r = RunCli has-session -t A
        Check ($r.rc -eq 0 -and $r.err -eq '') "CLI has-session -t A is a silent yes" "rc=$($r.rc) err='$($r.err)'"
        $r = RunCli show -t B -v status-left
        Check ($r.rc -eq 0 -and $r.out -eq 'BBB') "CLI show -t B shows B's" "rc=$($r.rc) out='$($r.out)'"

        # ---- TCP to A ----
        $t = Tcp A 'show -t NOPE -v status-left'
        Check ($t -match 'ERROR: no such session: NOPE' -and $t -notmatch 'AAA') "TCP show -t NOPE is no such session" "'$(Flat $t)'"
        $t = Tcp A 'show -t mywin -v status-left'
        Check ($t -match 'AAA' -and $t -notmatch 'ERROR') "TCP show -t <a window of A> shows A's" "'$(Flat $t)'"
        $t = Tcp A 'show -t A -v status-left'
        Check ($t -match 'AAA' -and $t -notmatch 'ERROR') "TCP show -t A shows A's" "'$(Flat $t)'"
        $t = Tcp A 'show -t B -v status-left'
        Check ($t -match 'BBB' -and $t -notmatch 'AAA|ERROR') "TCP show -t B shows B's" "'$(Flat $t)'"
        $t = Tcp A "list-windows -t B -F '#{session_name}:#{window_name}'"
        Check ($t -match 'B:bwin' -and $t -notmatch 'A:mywin|ERROR') "TCP list-windows -t B lists B" "'$(Flat $t)'"
        $t = Tcp A 'show-environment -t B EVB'
        Check ($t -match 'EVB=inB' -and $t -notmatch 'ERROR') "TCP show-environment -t B shows B's" "'$(Flat $t)'"
        $t = Tcp A 'set-environment -t B EV2 fromA'
        Check ($t -notmatch 'ERROR' -and (WaitFor { (Envv B EV2) -eq 'EV2=fromA' })) "TCP set-environment -t B sets it in B" "'$(Flat $t)' B=$(Envv B EV2)"
        Check ((Envv A EV2) -notmatch 'fromA') "and not in A" "A=$(Envv A EV2)"
        $t = Tcp A 'lock-session -t B'
        Check ($t -notmatch 'ERROR') "TCP lock-session -t B is a yes" "'$(Flat $t)'"
        $t = Tcp A "list-windows -t A -F '#{session_name}:#{window_name}'"
        Check ($t -match 'A:mywin' -and $t -notmatch 'ERROR') "TCP list-windows -t A still lists A" "'$(Flat $t)'"
        $t = Tcp A 'list-windows -t NOPE'
        Check ($t -match "ERROR: can't find session: NOPE") "TCP list-windows -t NOPE is still can't find session" "'$(Flat $t)'"
    }
} finally {
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 300
    foreach ($p in @(Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -match "-L $($script:Ns)(\s|$)" })) {
        taskkill /PID $p.ProcessId /T /F 2>&1 | Out-Null
    }
    if ($null -eq $savedData) { Remove-Item Env:PSMUX_DATA_DIR -EA SilentlyContinue } else { $env:PSMUX_DATA_DIR = $savedData }
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

$defaultAfter = (& $Exe ls -F '#{session_name} #{session_created}' 2>&1 | Out-String)
Check ($defaultBefore -eq $defaultAfter) "the default namespace is unchanged" ""

Write-Host "`n=== Results: $($script:Passed) passed, $($script:Failed) failed ===" -ForegroundColor $(if ($script:Failed) { 'Red' } else { 'Green' })
exit $(if ($script:Failed) { 1 } else { 0 })
