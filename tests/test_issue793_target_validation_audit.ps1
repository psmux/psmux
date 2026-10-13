# Issue #793 follow up: a -t naming no pane or window must be refused on
# every route, never answered from the ACTIVE pane or silently dropped.
#
# Measured on 9e2fa78f before the fix, one session `s` (window 0 with %1 %2,
# window 1 with %3), no client attached:
#
#   route  command                          answer
#   TCP    display-message -p -t %99 ...    X%1YsZ   (the active pane's values)
#   -CC    display-message -p -t %99 ...    %end X%1YsZ
#   TCP    capture-pane -p -t %99           empty, no error
#   TCP    select-pane -t %99 / -t s:0.7    nothing at all
#   TCP    kill-pane -t %99                 nothing at all
#   CLI    copy-mode / send-prefix -t %99,  exit 0, no message (fire and
#          rotate-window / resize-window    forget, the server's ERROR was
#          -t s:9                           never read)
#   CLI    source-file -t %99 NUL           "ERROR: can't find pane" on
#                                           STDOUT at exit 0
#   TCP/-CC run-shell / source-file -t %99  refused, where tmux runs them
#          and -t s:9                       (CMD_FIND_CANFAIL)
#
# tmux 3.4 for the same targets: send-keys, capture-pane, select-pane,
# kill-pane, copy-mode, send-prefix, rotate-window, resize-window all exit 1
# with "can't find pane: %99" / "can't find window: 9"; run-shell, if-shell
# and source-file (CMD_FIND_CANFAIL) run and exit 0. display-message is also
# CANFAIL in tmux (it prints the format with EMPTY values at exit 0); psmux's
# CLI has refused it since #545 and the other routes now match the CLI.
#
# GROUND TRUTH: the CLI's exit code and stderr, the raw TCP reply read to EOF,
# a real `psmux -CC attach` driven over pipes, and a fingerprint of every pane
# (ids, pids, sizes, modes, cursors, options) that must not move on a refusal.
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

. "$PSScriptRoot\isolated_config.ps1"

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

$NS = "i793a-" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$DATA = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { Join-Path $env:USERPROFILE ".psmux" }
$WORK = Join-Path ([IO.Path]::GetTempPath()) ("psmux_i793a_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $WORK | Out-Null
function P { & $PSMUX -L $NS @args 2>&1 }

# ── transports ──

function Tok([string]$line) {
    $out = @(); $cur = ''; $inq = $false; $has = $false
    foreach ($ch in $line.ToCharArray()) {
        if ($ch -eq "'") { $inq = -not $inq; $has = $true; continue }
        if ($ch -eq ' ' -and -not $inq) { if ($has) { $out += $cur; $cur = ''; $has = $false }; continue }
        $cur += $ch; $has = $true
    }
    if ($has) { $out += $cur }
    return , $out
}
# Each returns @{ ok = <no error>; text = <reply or error text> }.
function Run-Cli([string]$line) {
    $a = Tok $line
    $o = (& $PSMUX -L $NS @a 2>&1 | ForEach-Object { "$_" }) -join "`n"
    return @{ ok = ($LASTEXITCODE -eq 0 -and $o -notmatch 'ERROR'); text = $o; rc = $LASTEXITCODE }
}
function Run-Tcp([string]$line) {
    $pf = Join-Path $DATA "${NS}__s.port"
    $key = (Get-Content ($pf -replace '\.port$', '.key') -Raw).Trim()
    $c = [Net.Sockets.TcpClient]::new('127.0.0.1', [int]((Get-Content $pf -Raw).Trim()))
    try {
        $s = $c.GetStream()
        $b = [Text.Encoding]::UTF8.GetBytes("AUTH $key`n$line`n")
        $s.Write($b, 0, $b.Length); $c.Client.Shutdown([Net.Sockets.SocketShutdown]::Send)
        $s.ReadTimeout = 4000; $resp = ''; $buf = New-Object byte[] 8192
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
    $deadline = (Get-Date).AddSeconds(6)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 100
        $t = (Cc-Text).Substring($start)
        if ($t -match '(?s)%begin[^\n]*\n(.*?)\r?\n?(%end|%error)') {
            return @{ ok = ($Matches[2] -eq '%end'); text = $Matches[1].Trim() }
        }
    }
    return @{ ok = $false; text = '<no reply>' }
}
function Run([string]$route, [string]$line) {
    switch ($route) { 'cli' { Run-Cli $line } 'tcp' { Run-Tcp $line } 'cc' { Run-Cc $line } }
}

$FP = '#{window_id}.#{pane_id}:#{pane_pid}:#{window_name}:#{pane_active}#{window_active}:#{pane_width}x#{pane_height}:#{pane_in_mode}:#{pane_pipe}:#{cursor_x},#{cursor_y}:#{@x}:#{window_layout}'
function Finger { ((P list-panes -s -t s -F $FP) -join ';') }
function Fresh {
    Cc-Stop
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 400
    P new-session -d -s s -x 120 -y 30 -- cmd.exe /q /k | Out-Null
    for ($i = 0; $i -lt 40; $i++) { Start-Sleep -Milliseconds 250; P has-session -t s | Out-Null; if ($LASTEXITCODE -eq 0) { break } }
    P split-window -t s:0 -- cmd.exe /q /k | Out-Null
    P new-window -t s -- cmd.exe /q /k | Out-Null
    P select-window -t s:0 | Out-Null
    P select-pane -t s:0.0 | Out-Null
    Start-Sleep -Milliseconds 1200
}

try {
Fresh
$p0 = ((P display-message -p -t s:0.0 '#{pane_id}') -join '').Trim()
$p1 = ((P display-message -p -t s:0.1 '#{pane_id}') -join '').Trim()
$p2 = ((P display-message -p -t s:1.0 '#{pane_id}') -join '').Trim()
Write-Info "panes: s:0.0=$p0 s:0.1=$p1 s:1.0=$p2"
if ($p0 -notmatch '^%\d+$' -or $p2 -notmatch '^%\d+$') {
    Write-Fail "the session did not come up ($p0 $p2)"
    exit 1
}

# ── 1. a target naming nothing is refused, on every route, changing nothing ──

$refusals = @(
    @("display-message -p -t %99 'X#{pane_id}Y'", "can't find pane: %99"),
    @("display-message -p -t s:0.7 'X#{pane_id}Y'", "can't find pane: 7"),
    @("display-message -p -t s:9 'X#{pane_id}Y'", "can't find window: 9"),
    @("capture-pane -p -t %99", "can't find pane: %99"),
    @("select-pane -t %99", "can't find pane: %99"),
    @("select-pane -t s:0.7", "can't find pane: 7"),
    @("kill-pane -t %99", "can't find pane: %99"),
    @("copy-mode -t %99", "can't find pane: %99"),
    @("send-prefix -t %99", "can't find pane: %99"),
    @("send-keys -t %99 ZQ", "can't find pane: %99"),
    @("respawn-pane -k -t %99", "can't find pane: %99"),
    @("rotate-window -t s:9", "can't find window: 9"),
    @("resize-window -t s:9 -x 50", "can't find window: 9")
)
foreach ($route in 'cli', 'tcp', 'cc') {
    Write-Head "1.$route refusals"
    foreach ($r in $refusals) {
        $f0 = Finger
        $res = Run $route $r[0]
        Start-Sleep -Milliseconds 300
        $f1 = Finger
        $msg = $r[1]
        $refused = (-not $res.ok) -and ($res.text -match [regex]::Escape($msg))
        if ($route -eq 'cli') { $refused = $refused -and $res.rc -eq 1 }
        Check ($refused -and $f0 -eq $f1) "[$($r[0])] refused with '$msg', nothing moved" "[$($r[0])] ok=$($res.ok) text=[$($res.text)] moved=$($f0 -ne $f1)"
        if ($f0 -ne $f1) { Fresh }
    }
}

# ── 2. valid targets and format only uses still work ──

foreach ($route in 'cli', 'tcp', 'cc') {
    Write-Head "2.$route valid targets"
    $res = Run $route "display-message -p 'X#{pane_id}Y'"
    Check ($res.ok -and $res.text -eq "X${p0}Y") "display-message -p with no -t is the active pane" "got ok=$($res.ok) [$($res.text)]"
    $res = Run $route "display-message -p -t $p2 '#{pane_id}'"
    Check ($res.ok -and $res.text -eq $p2) "display-message -t $p2 (another window) answers $p2" "got ok=$($res.ok) [$($res.text)]"
    $res = Run $route "display-message -p -t s:0.1 '#{pane_id}'"
    Check ($res.ok -and $res.text -eq $p1) "display-message -t s:0.1 answers $p1" "got ok=$($res.ok) [$($res.text)]"
    $res = Run $route "display-message -p -t s:1 '#{window_index}'"
    Check ($res.ok -and $res.text -eq '1') "display-message -t s:1 answers window 1" "got ok=$($res.ok) [$($res.text)]"
    $res = Run $route "capture-pane -p -t $p2"
    Check ($res.ok) "capture-pane -p -t $p2 succeeds" "got [$($res.text)]"
    $res = Run $route "select-pane -t $p1"
    Start-Sleep -Milliseconds 300
    $act = ((P display-message -p -t s:0 '#{pane_id}') -join '').Trim()
    Check ($res.ok -and $act -eq $p1) "select-pane -t $p1 selects it" "ok=$($res.ok) [$($res.text)] active=$act"
    P select-pane -t $p0 | Out-Null
}

# ── 3. CMD_FIND_CANFAIL commands run without a target, as in tmux ──
#
# Control mode has no run-shell / if-shell (a separate gap), so only the CLI
# and TCP routes are measured for those two; source-file runs on all three.

foreach ($route in 'cli', 'tcp') {
    Write-Head "3.$route CANFAIL"
    $res = Run $route "run-shell -t %99 'echo canfail793'"
    Check ($res.ok -and $res.text -match 'canfail793') "run-shell -t %99 runs" "got ok=$($res.ok) [$($res.text)]"
    $res = Run $route "run-shell -t s:9 'echo canfail793'"
    Check ($res.ok -and $res.text -match 'canfail793') "run-shell -t s:9 runs" "got ok=$($res.ok) [$($res.text)]"
    $res = Run $route "if-shell -t %99 -F 1 'display -p yes793' 'display -p no793'"
    Check ($res.ok -and $res.text -match 'yes793') "if-shell -t %99 -F 1 takes its branch" "got ok=$($res.ok) [$($res.text)]"
    $res = Run $route "run-shell -t $p2 'echo valid793'"
    Check ($res.ok -and $res.text -match 'valid793') "run-shell -t $p2 runs" "got ok=$($res.ok) [$($res.text)]"
}
foreach ($route in 'cli', 'tcp', 'cc') {
    $res = Run $route "source-file -t %99 NUL"
    Check ($res.ok -and $res.text -notmatch 'ERROR') "$route source-file -t %99 NUL runs quietly" "got ok=$($res.ok) [$($res.text)]"
}

# ── 4. a floating pane passes the existence check ──

Write-Head "4. floating pane"
# The default shell: `new-pane -P -- cmd.exe /q /k` prints an id but leaves no
# floating pane (seen on 9e2fa78f as well, a separate defect).
$fl = Run-Tcp "new-pane -P"
$fid = if ($fl.text -match '(%\d+)') { $Matches[1] } else { '' }
Write-Info "new-pane -P answered [$($fl.text)]"
if ($fid -eq '' -or $fid -in @($p0, $p1, $p2)) {
    Write-Info "no floating pane id to test with, section skipped"
} else {
    Start-Sleep -Milliseconds 800
    foreach ($route in 'tcp', 'cc') {
        $res = Run $route "display-message -p -t $fid '#{pane_id}'"
        Check ($res.ok -and $res.text -eq $fid) "$route display-message -t $fid (floating) answers $fid" "got ok=$($res.ok) [$($res.text)]"
        $res = Run $route "capture-pane -p -t $fid"
        Check ($res.ok) "$route capture-pane -t $fid (floating) succeeds" "got [$($res.text)]"
    }
    $res = Run-Tcp "select-pane -t $fid"
    Check ($res.ok) "select-pane -t $fid (floating) is not refused" "got [$($res.text)]"
    # kill-pane -t <floating %id> is not refused. It does not kill the float
    # either, on 9e2fa78f as well (KillPaneById walks the tiled tree only), a
    # separate defect, so only the absence of a refusal is asserted here.
    $res = Run-Tcp "kill-pane -t $fid"
    Check ($res.ok) "kill-pane -t $fid (floating) is not refused" "got [$($res.text)]"
}

} finally {
    Cc-Stop
    P kill-server | Out-Null
    Remove-Item $WORK -Recurse -Force -EA SilentlyContinue
}

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed" -ForegroundColor Cyan
exit $script:TestsFailed
