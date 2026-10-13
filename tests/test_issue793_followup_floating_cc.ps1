# Issue #793 follow up, items 2 to 5 of the target audit.
#
# Measured on 618b633f + the audit fix (f2010509) before this change:
#
#   5. `new-pane -P -F '#{pane_id}'` and `new-pane -P -- cmd.exe /q /k`
#      printed an id and left NO floating pane: -F was not parsed (its value
#      became the command) and the `--` argv went to `pwsh -Command` as the
#      string "-- cmd.exe /q /k". `-F 'id=#{pane_id}'` printed `%10`, unformatted.
#   4. On a floating pane %f: send-keys -t %f typed nothing into it (rc 0),
#      resize-pane -t %f did nothing (rc 0), respawn-pane -k -t %f and
#      swap-pane -s %f were "can't find pane", select-pane -t %f did nothing
#      (rc 0) and kill-pane -t %f did nothing (rc 0).
#   2. if-shell -t %99 -F '#{pane_id}' took the TRUE branch; tmux 3.4 takes
#      the false one (CMD_FIND_CANFAIL leaves the target empty).
#   3. psmux -CC answered "unknown command" for run-shell, if-shell,
#      swap-window and join-pane.
#
# GROUND TRUTH: list-panes (pane ids, floating flag, pids, sizes, active),
# capture-pane of the floating pane, the CLI's exit code, the raw TCP reply,
# and a real `psmux -CC attach` driven over pipes.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH. PSMUX_DATA_DIR is
# honoured.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0; $script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }
# A script error is a failure, never a silent exit 0.
trap { Write-Fail "unexpected error: $_"; continue }

. "$PSScriptRoot\isolated_config.ps1"

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$NS = "i793f-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$DATA = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { Join-Path $env:USERPROFILE ".psmux" }
$WORK = Join-Path ([IO.Path]::GetTempPath()) ("psmux_i793f_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $WORK | Out-Null
function P { & $PSMUX -L $NS @args 2>&1 }

function Run-Tcp([string]$line) {
    $pf = Join-Path $DATA "${NS}__s.port"
    $key = (Get-Content ($pf -replace '\.port$', '.key') -Raw).Trim()
    $c = [Net.Sockets.TcpClient]::new('127.0.0.1', [int]((Get-Content $pf -Raw).Trim()))
    try {
        $s = $c.GetStream()
        $b = [Text.Encoding]::UTF8.GetBytes("AUTH $key`n$line`n")
        $s.Write($b, 0, $b.Length); $c.Client.Shutdown([Net.Sockets.SocketShutdown]::Send)
        $s.ReadTimeout = 6000; $resp = ''; $buf = New-Object byte[] 8192
        try { while (($n = $s.Read($buf, 0, $buf.Length)) -gt 0) { $resp += [Text.Encoding]::UTF8.GetString($buf, 0, $n) } } catch {}
    } finally { $c.Close() }
    $resp = ($resp -replace '^OK\r?\n', '').Trim()
    return @{ ok = ($resp -notmatch '^ERROR'); text = $resp }
}
$script:cc = $null; $script:ccN = 0
function Cc-Start {
    $script:ccN++
    $script:ccOut = Join-Path $WORK "cc$($script:ccN).out"
    $psi = [Diagnostics.ProcessStartInfo]::new($PSMUX, "-L $NS -CC attach -t s")
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
    $script:cc = [Diagnostics.Process]::Start($psi)
    $script:ccFo = [IO.FileStream]::new($script:ccOut, 'Create', 'Write', 'ReadWrite', 1)
    $script:ccT = $script:cc.StandardOutput.BaseStream.CopyToAsync($script:ccFo, 1)
    Start-Sleep -Milliseconds 1500
}
function Cc-Text {
    $fs = [IO.FileStream]::new($script:ccOut, 'Open', 'Read', 'ReadWrite')
    try { return [IO.StreamReader]::new($fs).ReadToEnd() } finally { $fs.Close() }
}
function Cc-Stop {
    if ($script:cc) {
        if (-not $script:cc.HasExited) {
            try { $script:cc.StandardInput.Close() } catch {}
            if (-not $script:cc.WaitForExit(3000)) { Stop-Process -Id $script:cc.Id -Force -EA SilentlyContinue }
        }
        try { $script:ccT.Wait(2000) | Out-Null } catch {}
        $script:ccFo.Close()
    }
    $script:cc = $null
}
function Run-Cc([string]$line) {
    if (-not $script:cc -or $script:cc.HasExited) { Cc-Start }
    $start = (Cc-Text).Length
    $script:cc.StandardInput.Write("$line`n"); $script:cc.StandardInput.Flush()
    $deadline = (Get-Date).AddSeconds(8)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 100
        $t = (Cc-Text).Substring($start)
        if ($t -match '(?s)%begin[^\n]*\n(.*?)\r?\n?(%end|%error)') {
            return @{ ok = ($Matches[2] -eq '%end'); text = $Matches[1].Trim() }
        }
    }
    return @{ ok = $false; text = '<no reply>' }
}
function PaneRow([string]$id, [string]$fmt) {
    $row = @(P list-panes -a -F "#{pane_id}|$fmt") | Where-Object { "$_" -like "$id|*" } | Select-Object -First 1
    if ($row) { return ("$row" -replace '^[^|]*\|', '') } else { return $null }
}
function Floats { @(P list-panes -a -F '#{pane_id} #{pane_floating_flag}' | Where-Object { "$_" -match ' 1$' } | ForEach-Object { ("$_" -split ' ')[0] }) }
function Fresh {
    Cc-Stop
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 400
    P new-session -d -s s -x 140 -y 40 | Out-Null
    for ($i = 0; $i -lt 40; $i++) { Start-Sleep -Milliseconds 250; P has-session -t s | Out-Null; if ($LASTEXITCODE -eq 0) { break } }
    Start-Sleep -Seconds 2
}
function NewFloat {
    $f = ((P new-pane -d -P) -join '').Trim()
    for ($i = 0; $i -lt 20 -and -not ((Floats) -contains $f); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Seconds 2
    return $f
}

try {

# ── 5. new-pane -P with -F and with a command ──

Write-Head "5. new-pane -P leaves the floating pane it prints"
Fresh
$cases = @(
    @{ a = @('new-pane', '-d', '-P', '-F', '#{pane_id}'); want = '^%\d+$'; cmd = $null },
    @{ a = @('new-pane', '-d', '-P', '--', 'cmd.exe', '/q', '/k'); want = '^%\d+$'; cmd = 'cmd' },
    @{ a = @('new-pane', '-d', '-P', '-F', 'id=#{pane_id}', '--', 'cmd.exe', '/q', '/k'); want = '^id=%\d+$'; cmd = 'cmd' },
    @{ a = @('new-pane', '-d', '-P', 'cmd.exe /q /k'); want = '^%\d+$'; cmd = 'cmd' }
)
foreach ($c in $cases) {
    $out = ((P @($c.a)) -join '').Trim()
    $id = if ($out -match '(%\d+)') { $Matches[1] } else { '' }
    $exists = $false
    for ($i = 0; $i -lt 16; $i++) { Start-Sleep -Milliseconds 250; if ((Floats) -contains $id) { $exists = $true } }
    Start-Sleep -Milliseconds 1500
    $still = (Floats) -contains $id
    $curcmd = PaneRow $id '#{pane_current_command}'
    $cmdOk = (-not $c.cmd) -or ("$curcmd" -match $c.cmd)
    Check ($out -match $c.want -and $exists -and $still -and $cmdOk) "[$($c.a -join ' ')] printed [$out], floating pane $id lives ($curcmd)" "[$($c.a -join ' ')] printed [$out] exists=$exists still=$still cmd=[$curcmd]"
}

# ── 4. by-id pane commands on a floating pane ──

foreach ($route in 'cli', 'tcp') {
    Write-Head "4.$route by-id commands on a floating pane"
    Fresh
    $f = NewFloat
    Write-Info "floating pane $f"
    function Rt([string]$line) {
        if ($route -eq 'tcp') { return Run-Tcp $line }
        $a = $line -split ' '
        $o = (P @a | ForEach-Object { "$_" }) -join "`n"
        return @{ ok = ($LASTEXITCODE -eq 0); text = $o }
    }
    $r = Rt "send-keys -t $f echo FLOAT793$route Enter"
    Start-Sleep -Seconds 2
    $capF = (P capture-pane -p -t $f) -join "`n"
    $capT = (P capture-pane -p -t '%1') -join "`n"
    Check ($r.ok -and $capF -match "(?m)^FLOAT793$route" -and $capT -notmatch "FLOAT793$route") "send-keys -t $f types into the floating pane only" "ok=$($r.ok) [$($r.text)] float has it: $($capF -match "FLOAT793$route") tiled has it: $($capT -match "FLOAT793$route")"
    $r = Rt "resize-pane -t $f -x 30"
    Start-Sleep -Milliseconds 800
    $w = PaneRow $f '#{pane_width}'
    Check ($r.ok -and $w -eq '28') "resize-pane -t $f -x 30 sizes the float (inner width $w)" "ok=$($r.ok) [$($r.text)] width=$w"
    $p0 = PaneRow $f '#{pane_pid}'
    $r = Rt "respawn-pane -k -t $f"
    Start-Sleep -Seconds 3
    $p1 = PaneRow $f '#{pane_pid}'
    $oldAlive = [bool](Get-Process -Id ([int]$p0) -EA SilentlyContinue)
    Check ($r.ok -and $p1 -and $p1 -ne $p0 -and -not $oldAlive) "respawn-pane -k -t $f gives it a new process ($p0 -> $p1)" "ok=$($r.ok) [$($r.text)] $p0 -> $p1 oldAlive=$oldAlive"
    $r = Rt "select-pane -t $f"
    Start-Sleep -Milliseconds 600
    $act = ((P display-message -p '#{pane_id}') -join '').Trim()
    Check ($r.ok -and $act -eq $f) "select-pane -t $f focuses it (active pane $act)" "ok=$($r.ok) [$($r.text)] active=$act"
    # With the float focused, a request aimed at the tiled %1 must reach %1.
    $r = Rt "send-keys -t %1 echo TILED793$route Enter"
    Start-Sleep -Seconds 2
    $capF = (P capture-pane -p -t $f) -join "`n"
    $capT = (P capture-pane -p -t '%1') -join "`n"
    Check ($capT -match "(?m)^TILED793$route" -and $capF -notmatch "TILED793$route") "send-keys -t %1 while $f is focused types into %1" "tiled has it: $($capT -match "TILED793$route") float has it: $($capF -match "TILED793$route")"
    $act = ((P display-message -p '#{pane_id}') -join '').Trim()
    Check ($act -eq $f) "and the focus stays on $f afterwards" "active=$act"
    $r = Rt "select-pane -t %1"
    Start-Sleep -Milliseconds 600
    $act = ((P display-message -p '#{pane_id}') -join '').Trim()
    Check ($r.ok -and $act -eq '%1') "select-pane -t %1 takes the focus back from the float" "ok=$($r.ok) active=$act"
    $r = Rt "swap-pane -s $f -t %1"
    Start-Sleep -Milliseconds 800
    $fl = Floats
    Check ($r.ok -and ($fl -contains '%1') -and -not ($fl -contains $f)) "swap-pane -s $f -t %1 trades places (floating now: $($fl -join ','))" "ok=$($r.ok) [$($r.text)] floating=$($fl -join ',')"
    $r = Rt "swap-pane -s %1 -t $f"
    Start-Sleep -Milliseconds 800
    $fl = Floats
    Check ($r.ok -and ($fl -contains $f) -and -not ($fl -contains '%1')) "and back (floating now: $($fl -join ','))" "ok=$($r.ok) [$($r.text)] floating=$($fl -join ',')"
    $r = Rt "kill-pane -t $f"
    Start-Sleep -Seconds 1
    $fl = Floats
    $tiled = PaneRow '%1' '#{pane_id}'
    Check ($r.ok -and -not ($fl -contains $f) -and $tiled -eq '%1') "kill-pane -t $f closes the float and leaves %1" "ok=$($r.ok) [$($r.text)] floating=$($fl -join ',') tiled=$tiled"
}

# ── 2. if-shell -t <nothing> -F: empty target, false branch ──

Write-Head "2. if-shell CANFAIL format"
Fresh
foreach ($route in 'cli', 'tcp', 'cc') {
    $cases = @(
        @("if-shell -t %99 -F '#{pane_id}' 'display -p yes793' 'display -p no793'", 'no793'),
        @("if-shell -t %99 -F '#{pid}' 'display -p yes793' 'display -p no793'", 'yes793'),
        @("if-shell -t %1 -F '#{pane_id}' 'display -p yes793' 'display -p no793'", 'yes793')
    )
    foreach ($c in $cases) {
        $res = switch ($route) {
            'cli' {
                $a = @('if-shell'); $toks = [regex]::Matches($c[0].Substring(9), "'[^']*'|\S+") | ForEach-Object { $_.Value.Trim("'") }
                $o = (& $PSMUX -L $NS @($a + $toks) 2>&1 | ForEach-Object { "$_" }) -join "`n"
                @{ ok = ($LASTEXITCODE -eq 0); text = $o }
            }
            'tcp' { Run-Tcp $c[0] }
            'cc' { Run-Cc $c[0] }
        }
        Check ($res.ok -and $res.text -match $c[1] -and $res.text -notmatch ($(if ($c[1] -eq 'yes793') { 'no793' } else { 'yes793' }))) "$route [$($c[0])] -> $($c[1])" "$route [$($c[0])] ok=$($res.ok) [$($res.text)]"
    }
}

# ── 3. control mode runs run-shell, if-shell, swap-window, join-pane ──

Write-Head "3. -CC run-shell, if-shell, swap-window, join-pane"
Fresh
P new-window -t s | Out-Null
Start-Sleep -Seconds 2
$res = Run-Cc "run-shell 'echo cc793run'"
Check ($res.ok -and $res.text -match 'cc793run') "run-shell prints its output in the block" "ok=$($res.ok) [$($res.text)]"
$res = Run-Cc "run-shell -t %99 'echo cc793canfail'"
Check ($res.ok -and $res.text -match 'cc793canfail') "run-shell -t %99 runs (CANFAIL)" "ok=$($res.ok) [$($res.text)]"
$res = Run-Cc "if-shell -F 1 'display -p yes793' 'display -p no793'"
Check ($res.ok -and $res.text -eq 'yes793') "if-shell -F 1 runs its true branch" "ok=$($res.ok) [$($res.text)]"
$res = Run-Cc "if-shell 'exit 1' 'display -p yes793' 'display -p no793'"
Check ($res.ok -and $res.text -eq 'no793') "if-shell with a failing shell condition runs its else branch" "ok=$($res.ok) [$($res.text)]"
$res = Run-Cc "if-shell -F 1 'display -p -t %99 x'"
Check ((-not $res.ok) -and $res.text -match "can't find pane: %99") "a branch that fails ends the block with %error" "ok=$($res.ok) [$($res.text)]"
$before = (P list-windows -t s -F '#{window_index}:#{window_id}') -join ' '
$res = Run-Cc "swap-window -s s:0 -t s:1"
Start-Sleep -Milliseconds 600
$after = (P list-windows -t s -F '#{window_index}:#{window_id}') -join ' '
Check ($res.ok -and $after -ne $before) "swap-window -s s:0 -t s:1 swaps them ($before -> $after)" "ok=$($res.ok) [$($res.text)] $before -> $after"
$res = Run-Cc "swap-window -t s:9"
Check ((-not $res.ok) -and $res.text -match "can't find window") "swap-window -t s:9 is %error" "ok=$($res.ok) [$($res.text)]"
$n0 = @(P list-panes -s -t s -F '#{pane_id}').Count
$w0 = @(P list-windows -t s).Count
$res = Run-Cc "join-pane -s s:1 -t s:0"
Start-Sleep -Milliseconds 800
$w1 = @(P list-windows -t s).Count
$inW0 = @(P list-panes -t s:0 -F '#{pane_id}').Count
Check ($res.ok -and $w1 -eq ($w0 - 1) -and $inW0 -eq 2) "join-pane -s s:1 -t s:0 moves the pane ($w0 windows -> $w1, window 0 now $inW0 panes)" "ok=$($res.ok) [$($res.text)] windows $w0 -> $w1, window 0 panes $inW0"
$res = Run-Cc "join-pane -s %99 -t s:0"
Check ((-not $res.ok) -and $res.text -match "can't find pane") "join-pane -s %99 is %error" "ok=$($res.ok) [$($res.text)]"

# ── 6. if-shell keeps the dash tokens inside a { ... } block ──
#
# tmux 3.4 (source-file of the same lines): @b1=v1, @b3=3 (`set -gF` kept).
# Over TCP and -CC psmux dropped every `-` token inside the block before
# this fix: `{ display -p x }` ran `display x` (no output) and
# `{ set -gF @o '#{e|+:1,2}' }` stored the literal format.

Write-Head "6. if-shell brace blocks keep their flags"
Fresh
$res = Run-Tcp "if-shell true { display -p tcpbrace793 }"
Check ($res.ok -and $res.text -match 'tcpbrace793') "TCP { display -p x } prints x" "ok=$($res.ok) [$($res.text)]"
Run-Tcp "if-shell true { set -gF @t2 '#{e|+:1,2}' }" | Out-Null
$v = ((P show -gv '@t2') -join '').Trim()
Check ($v -eq '3') "TCP { set -gF @o '#{e|+:1,2}' } stores 3" "stored [$v]"
Run-Tcp "if-shell -F 0 { set -g @t3 yes } { set -g @t3 no }" | Out-Null
$v = ((P show -gv '@t3') -join '').Trim()
Check ($v -eq 'no') "TCP -F 0 { yes } { no } runs the else block" "stored [$v]"
$res = Run-Cc "if-shell true { display -p ccbrace793 }"
Check ($res.ok -and $res.text -eq 'ccbrace793') "-CC { display -p x } prints x in the block" "ok=$($res.ok) [$($res.text)]"
$res = Run-Cc "if-shell true { set -gF @c2 '#{e|+:2,2}' }"
$v = ((P show -gv '@c2') -join '').Trim()
Check ($res.ok -and $v -eq '4') "-CC { set -gF ... } stores 4" "ok=$($res.ok) [$($res.text)] stored [$v]"

# ── 7. new-pane -e/-s/-S/-R applied, flags a float cannot honour refused ──

Write-Head "7. new-pane extras"
Fresh
$fid = ((P new-pane -d -P -e FOO793=bar793 -- cmd.exe /q /k 'echo %FOO793%') -join '').Trim()
Start-Sleep -Seconds 2
$cap = (P capture-pane -p -t $fid) -join "`n"
Check ($fid -match '^%\d+$' -and $cap -match '(?m)^bar793') "-e FOO793=bar793 reaches the float's process ($fid)" "id [$fid] capture [$cap]"
P kill-pane -t $fid | Out-Null
$fid = ((P new-pane -d -P -s bg=blue -S fg=red -R fg=green) -join '').Trim()
Start-Sleep -Seconds 1
$dump = (P dump-state) -join ''
Check ($dump -match '"style":"bg=blue"' -and $dump -match '"astyle":"fg=red"' -and $dump -match '"bstyle":"fg=green"') "-s/-S/-R reach the float the client draws" "floats json did not carry them"
P kill-pane -t $fid | Out-Null
foreach ($flag in @(@('-k'), @('-m', 'bye'), @('-I'), @('-O'), @('-M'), @('-W'), @('-L'))) {
    $before = @(Floats).Count
    $o = (P @(@('new-pane', '-d') + $flag) | ForEach-Object { "$_" }) -join ' '
    $rc = $LASTEXITCODE
    Start-Sleep -Milliseconds 500
    $after = @(Floats).Count
    Check ($rc -eq 1 -and $o -match 'not supported' -and $after -eq $before) "CLI new-pane $($flag -join ' ') is refused: $o" "rc=$rc out=[$o] floats $before -> $after"
}
$res = Run-Tcp "new-pane -d -k"
Check ((-not $res.ok) -and $res.text -match 'not supported') "TCP new-pane -k is ERROR" "[$($res.text)]"
$res = Run-Cc "new-pane -d -k"
Check ((-not $res.ok) -and $res.text -match 'not supported') "-CC new-pane -k is %error" "ok=$($res.ok) [$($res.text)]"
$before = @(Floats).Count
$o = ((P new-pane -d -P -l 50% -h -b -f -Z) -join '').Trim()
Start-Sleep -Seconds 1
Check ($o -match '^%\d+$' -and @(Floats).Count -eq $before + 1) "-l/-h/-b/-f/-Z (tiled only in tmux) still make the float" "out=[$o]"

# ── 8. a long run-shell opens its -CC block at once ──
#
# tmux writes %begin when the item starts and %end when the job ends. psmux
# wrote both after the job: nothing for 7 s, then the whole block.

Write-Head "8. -CC %begin goes out before a long job"
Fresh
Cc-Start
$start = (Cc-Text).Length
$sw = [Diagnostics.Stopwatch]::StartNew()
$script:cc.StandardInput.Write("run-shell 'Start-Sleep -Seconds 6; echo late793'`n"); $script:cc.StandardInput.Flush()
$beginAt = $null; $endAt = $null
while ($sw.Elapsed.TotalSeconds -lt 20) {
    Start-Sleep -Milliseconds 100
    $x = (Cc-Text).Substring($start)
    if (-not $beginAt -and $x -match '%begin') { $beginAt = $sw.Elapsed.TotalSeconds }
    if ($x -match '(?s)%begin.*late793.*%end') { $endAt = $sw.Elapsed.TotalSeconds; break }
}
Check ($beginAt -ne $null -and $beginAt -lt 2) "%begin at $([math]::Round([double]$beginAt, 1)) s, before the job ends" "%begin at $beginAt"
Check ($endAt -ne $null -and $endAt -ge 5.5) "%end with the output at $([math]::Round([double]$endAt, 1)) s, after the job (no 5 s timeout)" "%end at $endAt"

} finally {
    Cc-Stop
    P kill-server | Out-Null
    Remove-Item $WORK -Recurse -Force -EA SilentlyContinue
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed" -ForegroundColor Cyan
exit $script:TestsFailed
