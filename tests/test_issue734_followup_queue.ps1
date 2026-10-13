# Issue #734 follow up (GiorgoLazaridis, 2026-10-09): the rest of an if-shell
# branch after a new-session, the unspaced separator, the forwarding timeout
# with its local fallback, and source-file answering before it ran.
#
# Reported on ea4c950c, still present on master 44dae87e:
#
#   tmux -f NUL -S $s start-server ';' set-option -g exit-empty off
#   tmux -S $s if-shell 'exit 0 # #{pid}' \
#       "new-session -d -P -F '<fmt>' -s probe ; display-message -p CREATED" \
#       'display-message -p DENIED'
#
# printed only the creation record (6/6 runs here); CREATED never ran. The
# one-shot detached new-session arm ended the connection with `break`
# whatever was still queued. tmux runs every command of a list in order and
# stops only at one that fails (cmd-queue.c).
#
# `-s probe; display-message -p CREATED` (no space before `;`) made a session
# named `probe;` (6/6). Both tmux grammars end a command at a token ending in
# an unescaped `;`: cmd_parse_from_arguments (cmd-parse.y:1098) and the string
# lexer (cmd-parse.y:1314, :1675).
#
# A formatted if-shell or run-shell goes to the server whole; the CLI waited
# 3 s for the answer and then ran the command AGAIN locally, unexpanded,
# while the server was still on the first: `duplicate session`, a run-shell
# body run twice (6/6 with a 4 s condition). tmux's client waits for its
# command however long it runs.
#
# side findings 2B: a raw TCP `source-file` answered at once and the server
# then ran the file (its if-shell conditions are shells), so the next query
# queued behind the rest of the file and timed out on a slow machine with
# `psmux: no response from server (timed out)` (6/6 here with 800 ms
# conditions). tmux's source-file item waits for the inserted commands
# (cmd-source-file.c:250 CMD_RETURN_WAIT, :131 cmdq_continue).
#
# Isolation: a private PSMUX_DATA_DIR and private -S sockets; cleanup is
# `-S <socket> kill-server` and leftover scratch servers by PID from the
# scratch data dir. Nothing is ever stopped by name.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0; $script:Skipped = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow; $script:Skipped++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX_CONFIG_FILE','PSMUX_TARGET_SESSION','PSMUX_TARGET_FULL') {
    Set-Item -Path "env:$v" -Value $null
}

$TAG  = [guid]::NewGuid().ToString('N').Substring(0, 6)
$root = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_734q_" + $TAG)
$dataDir = Join-Path $root 'data'
New-Item -ItemType Directory -Force $dataDir | Out-Null
$savedDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = $dataDir
$script:Sockets = @()

# Run the binary with an argument array; stdout lines, stderr text, rc, ms.
function Run([string[]]$argv) {
    $errf = Join-Path $root ("err_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $out = & $PSMUX @argv 2>$errf
    $rc = $LASTEXITCODE
    $ms = $sw.ElapsedMilliseconds
    $err = ''
    if (Test-Path $errf) { $err = ([IO.File]::ReadAllText($errf)).Trim(); Remove-Item $errf -EA SilentlyContinue }
    [pscustomobject]@{ Lines = @($out | ForEach-Object { "$_" }); Err = $err; Rc = $rc; Ms = $ms }
}

function New-HeldServer([string]$name) {
    $s = Join-Path $root "$name.sock"
    $script:Sockets += $s
    $r = Run @('-f', 'NUL', '-S', $s, 'start-server', ';', 'set-option', '-g', 'exit-empty', 'off')
    if ($r.Rc -ne 0) { Write-Info "start-server rc=$($r.Rc) err='$($r.Err)'" }
    return $s
}

function Sessions([string]$s) { @((Run @('-S', $s, 'list-sessions', '-F', '#{session_name}')).Lines) }

$FMT = "#S:#{window_index}`t#{pane_id}`t#{socket_path}`t#{pid}"

try {
    # ==================================================================
    Write-Head "A: the reported branch keeps its marker after new-session -P"
    # ==================================================================
    foreach ($cond in @('exit 0 # #{pid}', 'exit 0')) {
        for ($i = 1; $i -le 3; $i++) {
            $label = "cond '$cond' run $i"
            $s = New-HeldServer "a$i$($cond.Length)"
            $serverPid = ((Run @('-S', $s, 'display-message', '-p', '#{pid}')).Lines -join '').Trim()
            $yes = "new-session -d -P -F '$FMT' -s probe ; display-message -p CREATED"
            $r = Run @('-S', $s, 'if-shell', $cond, $yes, 'display-message -p DENIED')
            $rec = @($r.Lines | Where-Object { $_ -like 'probe:0*' })
            $recIdx = [array]::IndexOf($r.Lines, ($rec | Select-Object -First 1))
            $markIdx = [array]::IndexOf($r.Lines, 'CREATED')
            Check ($rec.Count -eq 1) "${label}: one creation record" "${label}: record missing, out=[$($r.Lines -join ' | ')]"
            if ($rec.Count -eq 1) {
                $f = $rec[0] -split "`t"
                Check ($f.Count -eq 4 -and $f[1] -match '^%\d+$' -and $f[2] -eq $s -and $f[3] -eq $serverPid) "${label}: record has 4 fields, the private socket and the held server pid" "${label}: record fields wrong: [$($rec[0] -replace "`t", '<TAB>')] want socket $s pid $serverPid"
            }
            Check ($markIdx -ge 0 -and $markIdx -gt $recIdx) "${label}: CREATED follows the record" "${label}: CREATED missing or out of order, out=[$($r.Lines -join ' | ')]"
            Check ($r.Lines -notcontains 'DENIED' -and $r.Lines -notcontains 'OK') "${label}: nothing else printed" "${label}: extra output [$($r.Lines -join ' | ')]"
            Check ($r.Rc -eq 0 -and $r.Err -eq '') "${label}: rc 0, stderr empty" "${label}: rc=$($r.Rc) err='$($r.Err)'"
            $ls = Sessions $s
            Check (($ls -join ',') -eq 'probe') "${label}: the session is probe" "${label}: sessions [$($ls -join ',')]"
        }
    }

    # ==================================================================
    Write-Head "B: an unspaced separator ends the command (tmux cmd-parse.y)"
    # ==================================================================
    foreach ($cond in @('exit 0 # #{pid}', 'exit 0')) {
        $label = "cond '$cond'"
        $s = New-HeldServer "b$($cond.Length)"
        $yes = "new-session -d -P -F '$FMT' -s probe; display-message -p CREATED"
        $r = Run @('-S', $s, 'if-shell', $cond, $yes, 'display-message -p DENIED')
        $ls = Sessions $s
        Check (($ls -join ',') -eq 'probe') "${label}: session named probe, not 'probe;'" "${label}: sessions [$($ls -join ',')]"
        Check ($r.Lines -contains 'CREATED') "${label}: marker printed" "${label}: out=[$($r.Lines -join ' | ')]"
        Check ($r.Err -notmatch 'timed out|without format expansion') "${label}: no timeout or local fallback" "${label}: err='$($r.Err)'"
    }
    $s = New-HeldServer "b2"
    $null = Run @('-S', $s, 'new-session', '-d', '-s', 'main')
    $r = Run @('-S', $s, 'if-shell', 'exit 0', 'display-message -p a; display-message -p b')
    Check (($r.Lines -join ',') -eq 'a,b') "display-message -p a; display-message -p b prints a then b" "out=[$($r.Lines -join ' | ')]"
    $r = Run @('-S', $s, 'if-shell', 'exit 0', "set -g @q1 'x;'; set -g @q2 y")
    $q1 = ((Run @('-S', $s, 'show-options', '-gqv', '@q1')).Lines -join '')
    $q2 = ((Run @('-S', $s, 'show-options', '-gqv', '@q2')).Lines -join '')
    Check ($q1 -eq 'x;' -and $q2 -eq 'y') "a quoted 'x;' stays data, the bare ; after it separates" "@q1='$q1' @q2='$q2'"
    $null = Run @('-S', $s, 'set', '-g', '@argv', 'v;')
    $av = ((Run @('-S', $s, 'show-options', '-gqv', '@argv')).Lines -join '')
    Check ($av -eq 'v;') "a CLI argv value ending in ; is still the value" "@argv='$av'"
    $null = Run @('-S', $s, 'bind-key', '-n', 'M-;', 'display-message', 'semi')
    $lk = ((Run @('-S', $s, 'list-keys', '-T', 'root')).Lines | Where-Object { $_ -match 'M-;' }) -join ''
    Check ($lk -match 'display-message') "bind-key -n M-; still binds the key" "list-keys root: '$lk'"

    # ==================================================================
    Write-Head "C: a branch's commands run before the rest of the list"
    # ==================================================================
    $r = Run @('-S', $s, 'if-shell', 'exit 0', 'display-message -p a ; display-message -p b', ';', 'display-message', '-p', 'c')
    Check (($r.Lines -join ',') -eq 'a,b,c') "CLI list: a, b, c" "out=[$($r.Lines -join ' | ')]"
    $r = Run @('-S', $s, 'if-shell', 'exit 0 # #{pid}', 'display-message -p a ; display-message -p b', ';', 'display-message', '-p', 'c')
    Check (($r.Lines -join ',') -eq 'a,b,c') "CLI list with a formatted condition: a, b, c" "out=[$($r.Lines -join ' | ')]"
    $base = Get-ChildItem $dataDir -Filter '*main.port' | Select-Object -First 1
    if ($base) {
        $b = $base.BaseName
        $port = (Get-Content (Join-Path $dataDir "$b.port") -Raw).Trim()
        $key  = (Get-Content (Join-Path $dataDir "$b.key") -Raw).Trim()
        $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port)
        $st = $tcp.GetStream(); $w = [IO.StreamWriter]::new($st); $rd = [IO.StreamReader]::new($st)
        $w.Write("AUTH $key`nif-shell true 'display-message -p a ; display-message -p b' ; display-message -p c`n"); $w.Flush()
        $tcp.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send)
        $st.ReadTimeout = 10000
        $raw = ''; try { $raw = $rd.ReadToEnd() } catch {}
        $tcp.Close()
        $got = @($raw -split "`r?`n" | Where-Object { $_ -ne '' -and $_ -ne 'OK' }) -join ','
        Check ($got -eq 'a,b,c') "raw TCP list: a, b, c" "raw reply [$($raw -replace "`n", ' | ')]"
    } else { Write-Skip "raw TCP list: no port file for main" }

    # ==================================================================
    Write-Head "D: a failing new-session stops the list; a plain one prints nothing"
    # ==================================================================
    $r = Run @('-S', $s, 'if-shell', 'exit 0', 'new-session -d -s q2 ; display-message -p after')
    Check (($r.Lines -join ',') -eq 'after') "new-session -d ; display prints only 'after'" "out=[$($r.Lines -join ' | ')]"
    $r = Run @('-S', $s, 'if-shell', 'exit 0', 'new-session -d -s q2 ; display-message -p SHOULD_NOT')
    Check ($r.Lines -notcontains 'SHOULD_NOT' -and (($r.Lines + $r.Err) -join ' ') -match 'duplicate session: q2') "duplicate session reported, the marker did not run" "out=[$($r.Lines -join ' | ')] err='$($r.Err)'"

    # ==================================================================
    Write-Head "E: a slow formatted command is waited for, never run twice"
    # ==================================================================
    $s = New-HeldServer "e"
    $r = Run @('-S', $s, 'if-shell', 'Start-Sleep 4 # #{pid}', "new-session -d -P -F '#S' -s slow ; display-message -p CREATED", 'display-message -p DENIED')
    Check ($r.Err -notmatch 'timed out|without format expansion') "if-shell: no timeout, no local fallback" "err='$($r.Err)'"
    Check (($r.Lines -join ',') -eq 'slow,CREATED' -and $r.Rc -eq 0) "if-shell: record and marker once, rc 0 ($($r.Ms) ms)" "rc=$($r.Rc) out=[$($r.Lines -join ' | ')] err='$($r.Err)'"
    Check ((($r.Lines + $r.Err) -join ' ') -notmatch 'duplicate') "if-shell: no duplicate session" "out=[$($r.Lines -join ' | ')] err='$($r.Err)'"
    $cnt = Join-Path $root 'runshell.count'
    $r = Run @('-S', $s, 'run-shell', "Start-Sleep 4; Add-Content -Path '$cnt' -Value 'ran #{pid}'")
    Start-Sleep -Milliseconds 1500
    $lines = @(Get-Content $cnt -EA SilentlyContinue)
    Check ($lines.Count -eq 1 -and $lines[0] -match '^ran \d+$') "run-shell: the body ran once, expanded" "ran $($lines.Count)x [$($lines -join ' ; ')] err='$($r.Err)'"

    # ==================================================================
    Write-Head "F: source-file answers once the file has run (side findings 2B)"
    # ==================================================================
    $conf = Join-Path $root 'slow.conf'
    @(
        "if-shell 'Start-Sleep -Milliseconds 800; exit 0' { set -g @a yes } { set -g @a no }",
        "if-shell 'Start-Sleep -Milliseconds 800; exit 1' { set -g @b yes } { set -g @b no }",
        "if-shell 'Start-Sleep -Milliseconds 800; exit 0' {", "  set -g @c yes", "}",
        "set -g @after reached"
    ) | Set-Content -Path $conf -Encoding ASCII
    $s = New-HeldServer "f"
    $null = Run @('-S', $s, 'new-session', '-d', '-s', 'f1')
    $pf = Get-ChildItem $dataDir -Filter '*f1.port' | Select-Object -First 1
    if ($pf) {
        $b = $pf.BaseName
        $port = (Get-Content (Join-Path $dataDir "$b.port") -Raw).Trim()
        $key  = (Get-Content (Join-Path $dataDir "$b.key") -Raw).Trim()
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $tcp = [System.Net.Sockets.TcpClient]::new('127.0.0.1', [int]$port)
        $st = $tcp.GetStream(); $w = [IO.StreamWriter]::new($st); $rd = [IO.StreamReader]::new($st)
        $w.Write("AUTH $key`nsource-file '$conf'`n"); $w.Flush()
        $tcp.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send)
        $st.ReadTimeout = 30000
        try { $null = $rd.ReadToEnd() } catch {}
        $tcp.Close()
        $tSrc = $sw.ElapsedMilliseconds
        $sw.Restart()
        $a = ((Run @('-S', $s, 'show-options', '-gqv', '@a')).Lines -join '').Trim()
        $tQ = $sw.ElapsedMilliseconds
        Check ($tSrc -ge 2000) "raw TCP source-file answered after the file ran ($tSrc ms)" "answered after $tSrc ms, before its 3 x 800 ms conditions"
        Check ($a -eq 'yes' -and $tQ -lt 2000) "the first query right after it: @a = yes in $tQ ms" "@a='$a' after $tQ ms"
        foreach ($k in @{ '@b' = 'no'; '@c' = 'yes'; '@after' = 'reached' }.GetEnumerator()) {
            $v = ((Run @('-S', $s, 'show-options', '-gqv', $k.Key)).Lines -join '').Trim()
            Check ($v -eq $k.Value) "TCP source-file: $($k.Key) = $v" "TCP source-file: $($k.Key) = '$v', want '$($k.Value)'"
        }
        foreach ($k in '@a','@b','@c','@after') { $null = Run @('-S', $s, 'set', '-gu', $k) }
        $r = Run @('-S', $s, 'source-file', $conf)
        $a = ((Run @('-S', $s, 'show-options', '-gqv', '@a')).Lines -join '').Trim()
        $after = ((Run @('-S', $s, 'show-options', '-gqv', '@after')).Lines -join '').Trim()
        Check ($r.Ms -ge 2000 -and $r.Rc -eq 0 -and $a -eq 'yes' -and $after -eq 'reached') "CLI source-file returns after the file ran ($($r.Ms) ms), options set" "rc=$($r.Rc) $($r.Ms) ms @a='$a' @after='$after' err='$($r.Err)'"
    } else { Write-Skip "source-file: no port file for f1" }
}
finally {
    foreach ($s in $script:Sockets) { & $PSMUX -S $s kill-server 2>&1 | Out-Null }
    Start-Sleep -Milliseconds 500
    # Anything still registered in the scratch data dir was started by this
    # test: stop exactly those pids, never anything by name.
    Get-ChildItem $dataDir -Filter *.pid -EA SilentlyContinue | ForEach-Object {
        $first = (Get-Content $_.FullName -EA SilentlyContinue | Select-Object -First 1)
        $p = 0
        if ($first -and [int]::TryParse(($first -split ':')[0], [ref]$p) -and $p -gt 0) {
            $proc = Get-Process -Id $p -EA SilentlyContinue
            if ($proc -and $proc.Path -and ((Split-Path $proc.Path -Leaf) -in @('psmux.exe','tmux.exe','pmux.exe'))) {
                Write-Info "stopping leftover scratch server $p ($($_.Name))"
                Stop-Process -Id $p -Force -EA SilentlyContinue
            }
        }
    }
    $env:PSMUX_DATA_DIR = $savedDataDir
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

Write-Host "`nResults: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:Skipped) skipped"
exit $script:TestsFailed
