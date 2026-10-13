# Issue #795: set-option / set-environment literal values that look like
# flags were rewritten or refused.
#
#   set @k -x=foo   stored "-x foo"   (normalize_flag_equals split it, rc 0)
#   set @k -tfoo    rc 1 "empty value" (normalize_attached_target_flag split it)
#   set @k --       rc 1 "empty value" (parse_set_option_args ate the --)
#
# tmux 3.4 stores all of them verbatim at rc 0: its args_parse stops at the
# first operand. The normalizers now rewrite only a command's flag region.
# Every route is checked: direct CLI argv, raw TCP command lines, and a
# source-file config, plus the commands that carry a free value operand
# (display-message, rename-window, rename-session, set-buffer, new-window -n).
#
# Isolation: unique -L namespace, private PSMUX_DATA_DIR (honoured when set),
# cleanup only via `<binary> -L <ns> kill-server`. Exit code = failure count.
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "t795_$PID"
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP "psmux_t795_$PID"
}
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = "1"
Remove-Item Env:SHELL -ErrorAction SilentlyContinue
$S = "s795"

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }

# Exact argv through ProcessStartInfo.ArgumentList: PowerShell's own native
# argument passing would drop or requote some of these tokens.
function Px([string[]]$A) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new($PSMUX)
    foreach ($x in @('-L', $NS) + $A) { $psi.ArgumentList.Add($x) }
    $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true; $psi.UseShellExecute = $false
    $p = [System.Diagnostics.Process]::Start($psi)
    $o = $p.StandardOutput.ReadToEnd(); $e = $p.StandardError.ReadToEnd(); $p.WaitForExit()
    [pscustomobject]@{ rc = $p.ExitCode; out = $o.Trim(); err = $e.Trim() }
}
function Opt($name) { (Px @('show-options', '-qv', '-t', $S, $name)).out }
function Env1($name) { (Px @('show-environment', '-t', $S, $name)).out }
function Fmt($f) { (Px @('display-message', '-t', $S, '-p', $f)).out }

$null = Px @('new-session', '-d', '-s', $S)
$ok = $false
for ($i = 0; $i -lt 50; $i++) { if ((Px @('has-session', '-t', $S)).rc -eq 0) { $ok = $true; break }; Start-Sleep -Milliseconds 100 }
if (-not $ok) { Write-Fail "session creation failed"; exit 1 }

$values = @('-x=foo', '-u=1', '-tfoo', '-t=foo', '-t x', '--', '-target', '-t')

Write-Host "`n=== CLI: set-option -t S @k VALUE ===" -ForegroundColor Cyan
foreach ($v in $values) {
    $r = Px @('set-option', '-t', $S, '@k', $v)
    $got = Opt '@k'
    if ($r.rc -eq 0 -and $got -eq $v) { Write-Pass "[$v] stored verbatim" }
    else { Write-Fail "[$v] rc=$($r.rc) stored=[$got] err=[$($r.err)]" }
    $null = Px @('set-option', '-u', '-t', $S, '@k')
}

Write-Host "`n=== CLI: set-option -t S -- @k VALUE ===" -ForegroundColor Cyan
foreach ($v in @('-x=foo', '-tfoo', '--')) {
    $r = Px @('set-option', '-t', $S, '--', '@k', $v)
    $got = Opt '@k'
    if ($r.rc -eq 0 -and $got -eq $v) { Write-Pass "[-- @k $v] stored verbatim" }
    else { Write-Fail "[-- @k $v] rc=$($r.rc) stored=[$got] err=[$($r.err)]" }
    $null = Px @('set-option', '-u', '-t', $S, '@k')
}

Write-Host "`n=== CLI: set-environment / setenv ===" -ForegroundColor Cyan
foreach ($cmd in @('set-environment', 'setenv')) {
    foreach ($v in @('-x=foo', '-tfoo', '-t=foo', '-t x', '--')) {
        $r = Px @($cmd, '-t', $S, 'E795', $v)
        $got = Env1 'E795'
        if ($r.rc -eq 0 -and $got -eq "E795=$v") { Write-Pass "$cmd [$v] stored verbatim" }
        else { Write-Fail "$cmd [$v] rc=$($r.rc) got=[$got] err=[$($r.err)]" }
        $null = Px @('set-environment', '-u', '-t', $S, 'E795')
    }
}

Write-Host "`n=== CLI: other free value operands ===" -ForegroundColor Cyan
$r = Px @('setw', '-t', $S, '@w', '-x=foo')
$got = Fmt '#{@w}'
if ($r.rc -eq 0 -and $got -eq '-x=foo') { Write-Pass "setw @w -x=foo stored verbatim" } else { Write-Fail "setw @w -x=foo rc=$($r.rc) got=[$got]" }

$r = Px @('display-message', '-t', $S, '-p', '--', '-tfoo')
if ($r.rc -eq 0 -and $r.out -eq '-tfoo') { Write-Pass "display -p -- -tfoo prints -tfoo" } else { Write-Fail "display -p -- -tfoo rc=$($r.rc) out=[$($r.out)]" }
$r = Px @('display-message', '-t', $S, '-p', '--', '-x=foo')
if ($r.rc -eq 0 -and $r.out -eq '-x=foo') { Write-Pass "display -p -- -x=foo prints -x=foo" } else { Write-Fail "display -p -- -x=foo rc=$($r.rc) out=[$($r.out)]" }

$null = Px @('rename-window', '-t', $S, '--', '-tfoo')
$got = Fmt '#{window_name}'
if ($got -eq '-tfoo') { Write-Pass "rename-window -- -tfoo" } else { Write-Fail "rename-window -- -tfoo got=[$got]" }
$null = Px @('rename-window', '-t', $S, '--', '-x=foo')
$got = Fmt '#{window_name}'
if ($got -eq '-x=foo') { Write-Pass "rename-window -- -x=foo" } else { Write-Fail "rename-window -- -x=foo got=[$got]" }

$null = Px @('new-window', '-t', $S, '-n', '-x=foo')
$got = Fmt '#{window_name}'
if ($got -eq '-x=foo') { Write-Pass "new-window -n -x=foo names the window -x=foo" } else { Write-Fail "new-window -n -x=foo got=[$got]" }
$null = Px @('new-window', '-t', $S, '-n', '-tfoo')
$got = Fmt '#{window_name}'
if ($got -eq '-tfoo') { Write-Pass "new-window -n -tfoo names the window -tfoo" } else { Write-Fail "new-window -n -tfoo got=[$got]" }

$null = Px @('set-buffer', '-b', 'b795', '--', '-x=foo')
$got = (Px @('show-buffer', '-b', 'b795')).out
if ($got -eq '-x=foo') { Write-Pass "set-buffer -- -x=foo" } else { Write-Fail "set-buffer -- -x=foo got=[$got]" }
$null = Px @('set-buffer', '-b', 'b795', '--', '--')
$got = (Px @('show-buffer', '-b', 'b795')).out
if ($got -eq '--') { Write-Pass "set-buffer -- --" } else { Write-Fail "set-buffer -- -- got=[$got]" }

Write-Host "`n=== Nested commands: bind-key and if-shell ===" -ForegroundColor Cyan
$null = Px @('bind-key', '-n', 'F9', 'set-option', '@kb', '-x=foo')
$null = Px @('bind-key', '-n', 'F10', 'set-option', '@kc', '-tfoo')
$keys = (Px @('list-keys', '-T', 'root')).out
if ($keys -match 'F9 set-option @kb -x=foo') { Write-Pass "bind-key stores set-option @kb -x=foo verbatim" } else { Write-Fail "bind-key F9 stored: [$(($keys -split "`n") -match 'F9')]" }
if ($keys -match 'F10 set-option @kc -tfoo') { Write-Pass "bind-key stores set-option @kc -tfoo verbatim" } else { Write-Fail "bind-key F10 stored: [$(($keys -split "`n") -match 'F10')]" }
$null = Px @('unbind-key', '-n', 'F9'); $null = Px @('unbind-key', '-n', 'F10')

$null = Px @('if-shell', '-t', $S, 'true', 'set-option @ki -x=foo')
$got = ''; for ($i = 0; $i -lt 20; $i++) { $got = Opt '@ki'; if ($got) { break }; Start-Sleep -Milliseconds 100 }
if ($got -eq '-x=foo') { Write-Pass "if-shell branch set @ki -x=foo stored verbatim" } else { Write-Fail "if-shell branch @ki got=[$got]" }
$null = Px @('if-shell', '-t', $S, 'true', "set-option -t$S @kl -tv")
$got = ''; for ($i = 0; $i -lt 20; $i++) { $got = Opt '@kl'; if ($got) { break }; Start-Sleep -Milliseconds 100 }
if ($got -eq '-tv') { Write-Pass "if-shell branch set -tS @kl -tv: attached target, value kept" } else { Write-Fail "if-shell branch @kl got=[$got]" }

Write-Host "`n=== CLI: real flags still split (#196, #571) ===" -ForegroundColor Cyan
$r = Px @('has-session', "-t=$S")
if ($r.rc -eq 0) { Write-Pass "has-session -t=$S rc 0" } else { Write-Fail "has-session -t=$S rc=$($r.rc)" }
$r = Px @('has-session', '-t=nosuch795')
if ($r.rc -ne 0) { Write-Pass "has-session -t=nosuch795 rc $($r.rc)" } else { Write-Fail "has-session -t=nosuch795 rc 0" }
$r = Px @('has-session', "-t$S")
if ($r.rc -eq 0) { Write-Pass "has-session -t$S rc 0" } else { Write-Fail "has-session -t$S rc=$($r.rc)" }
$r = Px @('set-option', "-t=$S", '@k2', '-t=v')
$got = Opt '@k2'
if ($r.rc -eq 0 -and $got -eq '-t=v') { Write-Pass "set -t=S @k2 -t=v: flag split, value kept" } else { Write-Fail "set -t=S @k2 -t=v rc=$($r.rc) got=[$got] err=[$($r.err)]" }

Write-Host "`n=== TCP: raw command lines ===" -ForegroundColor Cyan
$portFile = Join-Path $env:PSMUX_DATA_DIR "${NS}__$S.port"
if (Test-Path $portFile) {
    $port = [int](Get-Content $portFile -Raw).Trim()
    $key = (Get-Content ($portFile -replace '\.port$', '.key') -Raw).Trim()
    function Tcp([string]$line) {
        $tcp = New-Object System.Net.Sockets.TcpClient
        $tcp.Connect('127.0.0.1', $port); $tcp.ReceiveTimeout = 5000
        $st = $tcp.GetStream()
        $wr = New-Object System.IO.StreamWriter($st); $wr.AutoFlush = $true; $wr.NewLine = "`n"
        $rd = New-Object System.IO.StreamReader($st)
        $wr.WriteLine("AUTH $key"); $null = $rd.ReadLine()
        $wr.WriteLine($line)
        $out = try { $rd.ReadToEnd() } catch { '<timeout>' }
        $tcp.Close(); return $out.Trim()
    }
    foreach ($v in @('-x=foo', '-u=1', '-tfoo', '-t=foo', '-t x', '--', '-target')) {
        $resp = Tcp "set-option -t $S @k '$v'"
        $got = Opt '@k'
        if ($got -eq $v -and $resp -notmatch 'ERROR') { Write-Pass "TCP [$v] stored verbatim" }
        else { Write-Fail "TCP [$v] stored=[$got] resp=[$resp]" }
        $null = Px @('set-option', '-u', '-t', $S, '@k')
    }
    $resp = Tcp "set-option -t $S -- @k '-x=foo'"
    $got = Opt '@k'
    if ($got -eq '-x=foo') { Write-Pass "TCP [-- @k -x=foo] stored verbatim" } else { Write-Fail "TCP [-- @k -x=foo] got=[$got] resp=[$resp]" }
    $null = Px @('set-option', '-u', '-t', $S, '@k')
    foreach ($v in @('-x=foo', '-tfoo')) {
        $resp = Tcp "set-environment -t $S E795 '$v'"
        $got = Env1 'E795'
        if ($got -eq "E795=$v") { Write-Pass "TCP setenv [$v]" } else { Write-Fail "TCP setenv [$v] got=[$got] resp=[$resp]" }
        $null = Px @('set-environment', '-u', '-t', $S, 'E795')
    }
    $null = Tcp "set-buffer -b b795 -- '-x=foo'"
    $got = (Px @('show-buffer', '-b', 'b795')).out
    if ($got -eq '-x=foo') { Write-Pass "TCP set-buffer -- -x=foo" } else { Write-Fail "TCP set-buffer -- -x=foo got=[$got]" }
    $resp = Tcp "set-option -t=$S @k3 '-t=v'"
    $got = (Px @('show-options', '-qv', '-t', $S, '@k3')).out
    if ($got -eq '-t=v' -and $resp -notmatch 'ERROR') { Write-Pass "TCP set -t=S @k3 -t=v: flag split, value kept" }
    else { Write-Fail "TCP set -t=S @k3 -t=v got=[$got] resp=[$resp]" }
} else { Write-Fail "no port file at $portFile" }

Write-Host "`n=== source-file ===" -ForegroundColor Cyan
$conf = Join-Path $env:PSMUX_DATA_DIR "t795_$PID.conf"
@"
set-option -g @c1 '-x=foo'
set-option -g @c2 '-tfoo'
set-option -g @c3 '--'
set-environment -g CE795 '-x=foo'
"@ | Set-Content -Path $conf -Encoding ASCII
$null = Px @('source-file', $conf)
foreach ($pair in @(@('@c1', '-x=foo'), @('@c2', '-tfoo'), @('@c3', '--'))) {
    $got = (Px @('show-options', '-gqv', $pair[0])).out
    if ($got -eq $pair[1]) { Write-Pass "conf $($pair[0]) = $($pair[1])" } else { Write-Fail "conf $($pair[0]) got=[$got]" }
}
$got = (Px @('show-environment', '-g', 'CE795')).out
if ($got -eq 'CE795=-x=foo') { Write-Pass "conf set-environment -x=foo" } else { Write-Fail "conf set-environment got=[$got]" }
Remove-Item $conf -ErrorAction SilentlyContinue

$null = Px @('kill-server')

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
