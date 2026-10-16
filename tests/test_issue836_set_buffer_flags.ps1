# Issue #836: set-buffer -n renames a buffer, a bundled -ab NAME keeps its -a,
# and -t names a client, on every route, as in tmux.
#
# tmux's template is `ab:t:n:w` (cmd-set-buffer.c:36, tag 3.7c), read by
# args_parse_flags, so flags bundle (`-ab NAME`, `-bNAME`, `-nNEW`). `-n NEW`
# renames the -b buffer, or the newest automatic one, and keeps its data
# (:87 to :104, paste_rename in paste.c:203). `-t` is a client that may be
# missing (CMD_CLIENT_TFLAG|CMD_CLIENT_CANFAIL, :40). No data at all is
# "no data specified" (:107).
#
# psmux read `-n` only in control mode (#813). Elsewhere `-n new` was skipped
# as an unknown flag and `new` became the data, so `set-buffer -b old -n new`
# overwrote the buffer `old` with the text "new".
#
# Every expected value below is what tmux 3.7c printed for the same commands
# on a fresh server. Only the CLI is used, under a namespace of its own.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$env:PSMUX_SESSION_NAME = $null
$env:PSMUX_SESSION      = $null
$env:PSMUX_PANE         = $null
$env:TMUX               = $null
$env:TMUX_PANE          = $null

$NS = "bsetb-" + [guid]::NewGuid().ToString('N').Substring(0, 6)

function P { & $PSMUX -L $NS @args 2>&1 }
# Output (stdout and stderr) and exit status of one command.
function PX { $o = (& $PSMUX -L $NS @args 2>&1 | ForEach-Object { "$_" }) -join ' '; return @{ out = $o.Trim(); rc = $LASTEXITCODE } }
# Every buffer as name=sample, sorted, one string.
function Buffers {
    Start-Sleep -Milliseconds 300
    return (@(P list-buffers -F '#{buffer_name}=#{buffer_sample}' | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) | Sort-Object) -join ' '
}
function Fresh {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    P new-session -d -s s -x 80 -y 20 | Out-Null
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 150
        & $PSMUX -L $NS has-session -t s 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) { break }
    }
}
# One case: the command's exit status and output, then the buffers, against tmux.
function Expect($label, $r, [int]$rc, [string]$out, [string]$list) {
    $got = Buffers
    $outOk = if ($out) { $r.out -match [regex]::Escape($out) } else { -not $r.out }
    if ($r.rc -eq $rc -and $outOk -and $got -ceq $list) {
        Write-Pass "$label -> rc=$rc$(if ($out) { " '$out'" }) [$got]"
    } else {
        Write-Fail "$label -> rc=$($r.rc) out=[$($r.out)] buffers [$got]; tmux: rc=$rc$(if ($out) { " '$out'" }) [$list]"
    }
}

try {
    Write-Head "1. -n renames and keeps the data (CLI)"
    Fresh; P set-buffer -b old OLDDATA | Out-Null
    Expect "-b old -n new" (PX set-buffer -b old -n new) 0 '' 'new=OLDDATA'
    Fresh; P set-buffer AUTO1 | Out-Null
    Expect "-n renamed (newest automatic buffer)" (PX set-buffer -n renamed) 0 '' 'renamed=AUTO1'
    Fresh; P set-buffer -b a A | Out-Null; P set-buffer -b b B | Out-Null
    Expect "-b a -n b (replaces b)" (PX set-buffer -b a -n b) 0 '' 'b=A'
    Fresh; P set-buffer AUTO1 | Out-Null
    Expect "-nrenamed (bundled value)" (PX set-buffer -nrenamed) 0 '' 'renamed=AUTO1'
    Fresh; P set-buffer AUTO1 | Out-Null
    Expect "-n renamed DATA (data ignored)" (PX set-buffer -n renamed DATA) 0 '' 'renamed=AUTO1'
    # -n returns before the data and -a are read (cmd-set-buffer.c:103).
    Fresh; P set-buffer AAA | Out-Null; P set-buffer BBB | Out-Null
    Expect "-b buffer1 -n BF1 -a add (rename only)" (PX set-buffer -b buffer1 -n BF1 -a add) 0 '' 'BF1=BBB buffer0=AAA'

    Write-Head "2. -n errors (CLI)"
    Fresh
    Expect "-b nosuch -n y" (PX set-buffer -b nosuch -n y) 1 'unknown buffer: nosuch' ''
    Fresh
    Expect "-n z with no buffer" (PX set-buffer -n z) 1 'no buffer' ''
    Fresh; P set-buffer AUTO1 | Out-Null
    Expect "-n ''" (PX set-buffer -n '') 1 'new name is empty' 'buffer0=AUTO1'

    Write-Head "3. bundled -a and -b (CLI)"
    Fresh; P set-buffer AUTO1 | Out-Null
    Expect "-ab buffer0 TAIL" (PX set-buffer -ab buffer0 TAIL) 0 '' 'buffer0=AUTO1TAIL'
    Fresh; P set-buffer AUTO1 | Out-Null
    Expect "-bbuffer0 -a GLUED" (PX set-buffer -bbuffer0 -a GLUED) 0 '' 'buffer0=AUTO1GLUED'
    Fresh
    Expect "-a -b nosuch NEW" (PX set-buffer -a -b nosuch NEW) 0 '' 'nosuch=NEW'

    Write-Head "4. -t names a client (CLI)"
    Fresh
    Expect "-t nosuchclient TDATA" (PX set-buffer -t nosuchclient TDATA) 0 '' 'buffer0=TDATA'
    Fresh
    Expect "-t nosuchclient -b tb TB" (PX set-buffer -t nosuchclient -b tb TB) 0 '' 'tb=TB'
    # psmux keeps buffers per session (one server each), so a -t that names a
    # session still picks that session's buffers, as before. tmux has one
    # buffer store, so there is no tmux value to compare with here.
    Fresh; P new-session -d -s s2 -x 80 -y 20 | Out-Null; Start-Sleep -Milliseconds 1500
    $r = PX set-buffer -t s2 S2DATA
    Start-Sleep -Milliseconds 300
    $s2 = (@(P list-buffers -t s2 -F '#{buffer_name}=#{buffer_sample}' | ForEach-Object { "$_".Trim() } | Where-Object { $_ })) -join ' '
    $s1 = (@(P list-buffers -t s -F '#{buffer_name}=#{buffer_sample}' | ForEach-Object { "$_".Trim() } | Where-Object { $_ })) -join ' '
    if ($r.rc -eq 0 -and $s2 -ceq 'buffer0=S2DATA' -and -not $s1) { Write-Pass "-t s2 (a session) -> s2 has [$s2], s has none" }
    else { Write-Fail "-t s2 (a session) -> rc=$($r.rc) out=[$($r.out)] s2 [$s2] s [$s1]" }

    Write-Head "4b. inside a server, -t naming another session is refused"
    # A server cannot reach another session's buffers, so a command run
    # inside one (the command prompt, a key binding, if-shell, a hook) is
    # refused like the cross-session window commands, never written to this
    # session's buffers. if-shell and a hook stand in for the prompt here.
    function Both { $a = (@(P list-buffers -t s -F '#{buffer_name}=#{buffer_sample}') | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) -join ' '; $b = (@(P list-buffers -t s2 -F '#{buffer_name}=#{buffer_sample}') | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) -join ' '; "s [$a] s2 [$b]" }
    Fresh; P new-session -d -s s2 -x 80 -y 20 | Out-Null; Start-Sleep -Milliseconds 1500
    $r = PX if-shell -t s true 'set-buffer -t s2 IFX'
    Start-Sleep -Milliseconds 300
    $both = Both
    if ($r.out -match 'set-buffer: s2 is another session' -and $both -eq 's [] s2 []') { Write-Pass "if-shell in s: set-buffer -t s2 -> refused, $both" }
    else { Write-Fail "if-shell in s: set-buffer -t s2 -> rc=$($r.rc) out=[$($r.out)] $both" }
    P set-hook -t s -g after-rename-window 'set-buffer -t s2 HKX' | Out-Null
    $r = PX rename-window -t s: rn
    Start-Sleep -Milliseconds 500
    $both = Both
    if ($both -eq 's [] s2 []') { Write-Pass "hook in s: set-buffer -t s2 -> nothing written, $both" }
    else { Write-Fail "hook in s: set-buffer -t s2 -> $both" }
    $r = PX if-shell -t s true 'set-buffer -t s IFOWN'
    Start-Sleep -Milliseconds 300
    $both = Both
    if ($both -eq 's [buffer0=IFOWN] s2 []') { Write-Pass "if-shell in s: set-buffer -t s (its own session) -> $both" }
    else { Write-Fail "if-shell in s: set-buffer -t s (its own session) -> rc=$($r.rc) out=[$($r.out)] $both" }

    Write-Head "5. data (CLI)"
    Fresh
    Expect "no data" (PX set-buffer) 1 'no data specified' ''
    Fresh
    Expect "empty data" (PX set-buffer '') 0 '' ''

    Write-Head "6. the server's dispatch (if-shell)"
    Fresh; P set-buffer -b old OLDDATA | Out-Null
    Expect "if-shell: -b old -n new" (PX if-shell true 'set-buffer -b old -n new') 0 '' 'new=OLDDATA'
    Fresh; P set-buffer AUTO1 | Out-Null
    Expect "if-shell: -ab buffer0 TAIL" (PX if-shell true 'set-buffer -ab buffer0 TAIL') 0 '' 'buffer0=AUTO1TAIL'

    Write-Head "7. a hook (execute_command_string)"
    Fresh; P set-buffer -b old OLDDATA | Out-Null
    P set-hook -g after-rename-window 'set-buffer -b old -n new' | Out-Null
    Expect "hook: -b old -n new" (PX rename-window rn) 0 '' 'new=OLDDATA'
    Fresh
    P set-hook -g after-rename-window 'set-buffer -t nosuchclient TDATA' | Out-Null
    Expect "hook: -t nosuchclient TDATA" (PX rename-window rn) 0 '' 'buffer0=TDATA'
    Fresh; P set-buffer AUTO1 | Out-Null
    P set-hook -g after-rename-window 'set-buffer -ab buffer0 TAIL' | Out-Null
    Expect "hook: -ab buffer0 TAIL" (PX rename-window rn) 0 '' 'buffer0=AUTO1TAIL'
}
finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
}

Write-Host ""
Write-Host "passed: $script:TestsPassed  failed: $script:TestsFailed" -ForegroundColor Cyan
exit $script:TestsFailed
