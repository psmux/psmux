# A session name or -L namespace with a character Windows cannot keep in a
# file name (< > : " / \ | ? * and the control characters) broke psmux
# silently. `psmux -L ns new-session -d -s a:b` returned 0, but the registry
# was written as NTFS alternate data streams (b.port, b.key, b.pid, b.sid) of
# a file named `ns__a`: `ls` said no server was running, `has-session` failed,
# and the server and its __warm__ standby were orphaned. `-L x:y` did the same.
#
# psmux now refuses such a name up front, the way tmux refuses a name it
# cannot take (`invalid session name: NAME`, cmd-new-session.c and
# cmd-rename-session.c), at exit 1, with no file written and no server started.
#
# Isolation: a private PSMUX_DATA_DIR and unique -L namespaces. Cleanup is
# `psmux -L <ns> kill-server` plus Stop-Process of the PIDs whose command line
# carries one of this run's namespaces, never anything by name.

$ErrorActionPreference = 'Continue'
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

Write-Host "psmux: $PSMUX ($(& $PSMUX -V))"
$tag = [guid]::NewGuid().ToString('N').Substring(0, 8)
$ns = "unst_$tag"
$nsShared = "unsh_$tag"
$dataDir = Join-Path $env:TEMP "psmux_unst_$tag"
New-Item -ItemType Directory -Path $dataDir -Force | Out-Null
$savedData = $env:PSMUX_DATA_DIR
$savedMode = $env:PSMUX_SERVER_MODE
$env:PSMUX_DATA_DIR = $dataDir
Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue

function Ours {
    # Every process this run may have started: its command line names one of
    # the run's namespaces (`-L unst_<tag>`, `-L x:y_<tag>`, ...).
    @(Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" -EA SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine.Contains($tag) })
}
function Reap([string]$why) {
    $p = Ours
    foreach ($x in $p) {
        Write-Host "    reaping orphan PID $($x.ProcessId) ($why): $($x.CommandLine)" -ForegroundColor DarkYellow
        Stop-Process -Id $x.ProcessId -Force -EA SilentlyContinue
    }
    if ($p.Count) { Start-Sleep -Milliseconds 500 }
}
function Streams {
    # Every alternate data stream under the data dir: the bug's footprint.
    @(Get-ChildItem $dataDir -Recurse -Force -File -EA SilentlyContinue | ForEach-Object {
        Get-Item -LiteralPath $_.FullName -Stream * -EA SilentlyContinue |
            Where-Object { $_.Stream -ne ':$DATA' } | ForEach-Object { "$($_.FileName):$($_.Stream)" }
    })
}
function Scrub {
    # What a broken create left behind: the registry files of this run and
    # every file carrying an alternate data stream (the `ns__a` of `a:b`).
    Get-ChildItem $dataDir -Force -File -EA SilentlyContinue | Where-Object {
        $_.Name -like "*$tag*" -or @(Get-Item -LiteralPath $_.FullName -Stream * -EA SilentlyContinue).Count -gt 1
    } | ForEach-Object { [System.IO.File]::Delete($_.FullName) }
}
function RegistryFiles {
    @(Get-ChildItem $dataDir -Force -File -EA SilentlyContinue | Where-Object { $_.Name -like "*$tag*" } | ForEach-Object Name)
}
function Run {
    # Run SOCKET ARGS...: no declared parameter, so a `-s` or `-d` in ARGS is
    # never taken for one of this function's own.
    $a = @('-L') + $args
    $out = (& $PSMUX @a 2>&1 | ForEach-Object { "$_" }) -join ' / '
    return @{ rc = $LASTEXITCODE; out = $out }
}

# A refused create: exit 1, the error text, no process, no file, no stream.
function ExpectRefused([string]$label, [string]$sock, [string]$want, [string[]]$cmd) {
    $before = (RegistryFiles).Count
    $r = Run $sock @cmd
    Start-Sleep -Milliseconds 1500
    $procs = Ours
    $streams = Streams
    $files = (RegistryFiles).Count - $before
    $ok = ($r.rc -eq 1) -and $r.out.Contains($want) -and ($procs.Count -eq 0) -and ($streams.Count -eq 0) -and ($files -le 0)
    if ($ok) {
        Write-Pass "$label -> rc 1, '$($r.out)'"
    } else {
        Write-Fail ("$label -> rc=$($r.rc) out='$($r.out)' (want rc 1 and '$want'); " +
            "processes=$($procs.Count) streams=[$($streams -join ', ')] new files=$files")
        Reap $label
        Scrub
    }
}

$env:PSMUX_DATA_DIR = $savedData
$realBefore = (& $PSMUX ls 2>&1 | Out-String)
$env:PSMUX_DATA_DIR = $dataDir

try {
    Write-Host "`n[1] the reported case: new-session -s 'a:b'" -ForegroundColor Yellow
    $r = Run $ns new-session -d -s 'a:b'
    Start-Sleep -Milliseconds 2000
    $streams = Streams
    $ls = Run $ns ls
    $has = Run $ns has-session -t 'a:b'
    $procs = Ours
    Write-Host "    rc=$($r.rc) out='$($r.out)' streams=[$($streams -join ', ')] ls rc=$($ls.rc) '$($ls.out)' has rc=$($has.rc) processes=$($procs.Count)"
    if ($r.rc -eq 0 -and $procs.Count -gt 0 -and $ls.rc -ne 0) {
        Write-Fail "BUG: rc 0 but no server ls can reach, $($procs.Count) orphaned process(es), streams [$($streams -join ', ')]"
        Reap 'reported case'
        Scrub
    } elseif ($r.rc -eq 1 -and $r.out.Contains('invalid session name: a:b') -and $procs.Count -eq 0 -and $streams.Count -eq 0) {
        Write-Pass "refused: '$($r.out)', nothing written, nothing started"
    } else {
        Write-Fail "unexpected: rc=$($r.rc) out='$($r.out)'"
        Reap 'reported case'
    }

    Write-Host "`n[2] every character Windows cannot keep in a file name" -ForegroundColor Yellow
    $bad = @('<', '>', '"', '/', '\', '|', '?', '*', "`t", [string][char]27, [string][char]1)
    foreach ($c in $bad) {
        $n = "x${c}y"
        $shown = ($n -replace "`t", '\t' -replace [char]27, '\033' -replace [char]1, '\001')
        $want = 'invalid session name: '
        ExpectRefused "new-session -s '$shown'" $ns $want @('new-session', '-d', '-s', $n)
    }
    ExpectRefused "new-session -A -d -s 'a:b'" $ns 'invalid session name: a:b' @('new-session', '-A', '-d', '-s', 'a:b')
    ExpectRefused "new -s 'a:b' (alias)" $ns 'invalid session name: a:b' @('new', '-d', '-s', 'a:b')

    Write-Host "`n[3] -L names" -ForegroundColor Yellow
    ExpectRefused "-L 'x:y_$tag' new-session" "x:y_$tag" "invalid socket name: x:y_$tag" @('new-session', '-d', '-s', 'ok')
    ExpectRefused "-L 'x|y_$tag' new-session" "x|y_$tag" "invalid socket name: x|y_$tag" @('new-session', '-d', '-s', 'ok')
    ExpectRefused "-L 'x/y_$tag' new-session" "x/y_$tag" "invalid socket name: x/y_$tag" @('new-session', '-d', '-s', 'ok')
    ExpectRefused "-L 'x:y_$tag' ls" "x:y_$tag" "invalid socket name: x:y_$tag" @('ls')

    Write-Host "`n[4] names that work today still do" -ForegroundColor Yellow
    $good = @('plain', 'my.session', 'with space', 'café', 'x-y_z', '日本', 'CON')
    foreach ($n in $good) {
        $r = Run $ns new-session -d -s $n
        if ($r.rc -eq 0) { Write-Pass "new-session -s '$n'" } else { Write-Fail "new-session -s '$n' -> rc=$($r.rc) '$($r.out)'" }
    }
    Start-Sleep -Milliseconds 1000
    $names = @(& $PSMUX -L $ns ls -F '#{session_name}' 2>&1 | ForEach-Object { "$_" })
    foreach ($n in $good) {
        if ($names -contains $n) { Write-Pass "ls lists '$n'" } else { Write-Fail "ls is missing '$n' (got: $($names -join ', '))" }
        $h = Run $ns has-session -t "=$n"
        if ($h.rc -eq 0) { Write-Pass "has-session -t '=$n'" } else { Write-Fail "has-session -t '=$n' -> rc=$($h.rc) '$($h.out)'" }
    }
    $r = Run $ns rename-session -t 'x-y_z' 'renamed.ok'
    Start-Sleep -Milliseconds 800
    $h = Run $ns has-session -t '=renamed.ok'
    if ($r.rc -eq 0 -and $h.rc -eq 0) { Write-Pass "rename-session x-y_z -> renamed.ok" } else { Write-Fail "rename to renamed.ok -> rc=$($r.rc) '$($r.out)', has rc=$($h.rc)" }
    if ((Streams).Count -eq 0) { Write-Pass "no alternate data streams after the good names" } else { Write-Fail "streams: $((Streams) -join ', ')" }

    Write-Host "`n[5] rename-session to an unstorable name" -ForegroundColor Yellow
    $r = Run $ns rename-session -t plain 'r:s'
    Start-Sleep -Milliseconds 800
    $h = Run $ns has-session -t '=plain'
    $st = Streams
    if ($r.rc -eq 1 -and $r.out.Contains('invalid session name: r:s') -and $h.rc -eq 0 -and $st.Count -eq 0) {
        Write-Pass "CLI rename refused: '$($r.out)', 'plain' kept"
    } else { Write-Fail "CLI rename -> rc=$($r.rc) '$($r.out)', plain has rc=$($h.rc), streams [$($st -join ', ')]" }

    # Inside the server: a sourced file runs its commands in the server.
    $src = Join-Path $dataDir "cmds_$tag.conf"
    Set-Content -LiteralPath $src -Value "rename-session -t plain 'p|q'" -Encoding UTF8
    $r = Run $ns source-file $src
    Start-Sleep -Milliseconds 800
    $h = Run $ns has-session -t '=plain'
    $names = @(& $PSMUX -L $ns ls -F '#{session_name}' 2>&1 | ForEach-Object { "$_" })
    if ($h.rc -eq 0 -and -not ($names -contains 'p|q') -and (Streams).Count -eq 0) {
        Write-Pass "server rename (source-file) refused, 'plain' kept (out '$($r.out)')"
    } else { Write-Fail "server rename -> rc=$($r.rc) '$($r.out)', plain has rc=$($h.rc), ls: $($names -join ', ')" }

    Write-Host "`n[6] new-session from inside a server" -ForegroundColor Yellow
    $before = (Ours).Count
    Set-Content -LiteralPath $src -Value "new-session -d -s 'c:d'" -Encoding UTF8
    $r = Run $ns source-file $src
    Start-Sleep -Milliseconds 2000
    $after = (Ours).Count
    $st = Streams
    if ($after -le $before -and $st.Count -eq 0) {
        Write-Pass "server new-session (source-file) refused: no process, no stream (out '$($r.out)')"
    } else { Write-Fail "server new-session -> rc=$($r.rc) '$($r.out)' processes $before -> $after streams [$($st -join ', ')]" }
    $r = Run $ns if-shell true "new-session -d -s 'e:f'"
    Start-Sleep -Milliseconds 2000
    $after2 = (Ours).Count
    $st = Streams
    if ($after2 -le $before -and $st.Count -eq 0 -and $r.out.Contains('invalid session name: e:f')) {
        Write-Pass "if-shell new-session refused: '$($r.out)'"
    } else { Write-Fail "if-shell new-session -> rc=$($r.rc) '$($r.out)' processes $before -> $after2 streams [$($st -join ', ')]" }

    Write-Host "`n[7] the shared server switch" -ForegroundColor Yellow
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1500
    Reap 'the per session servers of [4] to [6]'
    $env:PSMUX_SERVER_MODE = 'shared'
    ExpectRefused "PSMUX_SERVER_MODE=shared new-session -s 'a:b'" $nsShared 'invalid session name: a:b' @('new-session', '-d', '-s', 'a:b')
    Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue
} finally {
    Remove-Item Env:PSMUX_SERVER_MODE -EA SilentlyContinue
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    & $PSMUX -L $nsShared kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1500
    Reap 'final cleanup'
    $env:PSMUX_DATA_DIR = $savedData
    $realAfter = (& $PSMUX ls 2>&1 | Out-String)
    if ($realAfter -ne $realBefore) { Write-Fail "the user's default session list changed:`n$realBefore`n->`n$realAfter" }
    if ($savedMode) { $env:PSMUX_SERVER_MODE = $savedMode }
    Remove-Item -LiteralPath $dataDir -Recurse -Force -EA SilentlyContinue
}

Write-Host "`nResults: $script:TestsPassed passed, $script:TestsFailed failed"
exit $(if ($script:TestsFailed -eq 0) { 0 } else { 1 })
