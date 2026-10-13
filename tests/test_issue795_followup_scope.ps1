# #795 follow up: user option scope and display-message unknown flags.
#
# 1. `setw @k v` / `set -w @k v` stored the value in the SESSION store, so
#    `show-options -w` listed nothing, every window saw the value, and
#    `set -gw @k` collided with `set -g @k`. tmux keeps user options in the
#    table the flags select and resolves `#{@k}` pane > window > global window
#    > session > global (format.c format_find).
# 2. `display -p -x hello` printed `-x hello`; tmux refuses "unknown flag -x".
#
# Every expectation below was measured on tmux 3.4. Routes: direct CLI argv,
# raw TCP command lines, and a source-file config.
# Isolation: unique -L namespace, private PSMUX_DATA_DIR (honoured when set),
# cleanup only via `<binary> -L <ns> kill-server`. Exit code = failure count.
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "t795s_$PID"
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP "psmux_t795s_$PID"
}
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = "1"
Remove-Item Env:SHELL -ErrorAction SilentlyContinue
$S = "s795s"

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }

function Px([string[]]$A) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new($PSMUX)
    foreach ($x in @('-L', $NS) + $A) { $psi.ArgumentList.Add($x) }
    $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true; $psi.UseShellExecute = $false
    $p = [System.Diagnostics.Process]::Start($psi)
    $o = $p.StandardOutput.ReadToEnd(); $e = $p.StandardError.ReadToEnd(); $p.WaitForExit()
    [pscustomobject]@{ rc = $p.ExitCode; out = $o.Trim(); err = $e.Trim() }
}
function Check($label, $actual, $expected) {
    if ($actual -ceq $expected) { Write-Pass "$label = [$expected]" } else { Write-Fail "$label = [$actual], want [$expected]" }
}
function W0 { "${S}:0" }
function W1 { "${S}:1" }
function Fmt($t, $f) { (Px @('display-message', '-p', '-t', $t, $f)).out }

$null = Px @('new-session', '-d', '-s', $S)
$ok = $false
for ($i = 0; $i -lt 50; $i++) { if ((Px @('has-session', '-t', $S)).rc -eq 0) { $ok = $true; break }; Start-Sleep -Milliseconds 100 }
if (-not $ok) { Write-Fail "session creation failed"; exit 1 }
$null = Px @('new-window', '-t', $S)
Start-Sleep -Milliseconds 500

Write-Host "`n=== CLI: setw @w lands on the target window ===" -ForegroundColor Cyan
$r = Px @('setw', '-t', (W0), '@w', 'win0')
Check "setw rc" $r.rc 0
Check "show -w -t :0" (Px @('show-options', '-w', '-t', (W0))).out '@w win0'
Check "show -wv -t :0 @w" (Px @('show-options', '-wv', '-t', (W0), '@w')).out 'win0'
Check "show -wqv -t :1 @w" (Px @('show-options', '-wqv', '-t', (W1), '@w')).out ''
Check "show -qv -t S @w (session)" (Px @('show-options', '-qv', '-t', $S, '@w')).out ''
Check "#{@w} in :0" (Fmt (W0) '#{@w}') 'win0'
Check "#{@w} in :1" (Fmt (W1) '#{@w}') ''

Write-Host "`n=== CLI: lookup order pane > window > global window > session ===" -ForegroundColor Cyan
$null = Px @('set', '-t', $S, '@w', 'sess')
Check "session set: :0 keeps window" (Fmt (W0) '#{@w}') 'win0'
Check "session set: :1 sees session" (Fmt (W1) '#{@w}') 'sess'
$null = Px @('set', '-g', '@w', 'glob')
$null = Px @('set', '-gw', '@w', 'gwin')
Check "global window beats session" (Fmt (W1) '#{@w}') 'gwin'
Check "show -gw @w" (Px @('show-options', '-gw', '@w')).out '@w gwin'
$null = Px @('set', '-u', '-t', $S, '@w')
Check "after set -u session" (Fmt (W1) '#{@w}') 'gwin'
$null = Px @('set', '-p', '-t', (W1), '@w', 'pane1')
Check "pane beats all" (Fmt (W1) '#{@w}') 'pane1'
$null = Px @('set', '-w', '-t', (W1), '@w', 'win1')
Check "pane still beats window" (Fmt (W1) '#{@w}') 'pane1'
$null = Px @('set', '-u', '-p', '-t', (W1), '@w')
Check "after set -u -p" (Fmt (W1) '#{@w}') 'win1'
$null = Px @('set', '-u', '-w', '-t', (W0), '@w')
Check "set -u -w: show -w empty" (Px @('show-options', '-w', '-t', (W0))).out ''
Check "set -u -w: inherits global window" (Fmt (W0) '#{@w}') 'gwin'
Check "show -w -t :1" (Px @('show-options', '-w', '-t', (W1))).out '@w win1'
$wa = (Px @('show-options', '-wA', '-t', (W0))).out
if ($wa -notmatch '@w') { Write-Pass "show -wA lists no inherited @w (tmux lists only own user options)" } else { Write-Fail "show -wA -t :0 = [$wa]" }
$null = Px @('set', '-gwu', '@w')
Check "set -gwu removes the global window value" (Px @('show-options', '-gwqv', '@w')).out ''

Write-Host "`n=== TCP: same writes on raw command lines ===" -ForegroundColor Cyan
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
    $null = Tcp "setw -t $(W1) @t tcpwin"
    Check "TCP setw: show -wv :1" (Px @('show-options', '-wv', '-t', (W1), '@t')).out 'tcpwin'
    Check "TCP setw: #{@t} in :0" (Fmt (W0) '#{@t}') ''
    $null = Tcp "set -gw @tg tcpg"
    Check "TCP set -gw: show -gwv" (Px @('show-options', '-gwv', '@tg')).out 'tcpg'
    Check "TCP set -gw: #{@tg} in :0" (Fmt (W0) '#{@tg}') 'tcpg'
    Check "TCP show -gw @tg" (Tcp 'show-options -gw @tg') '@tg tcpg'
    $resp = Tcp "display-message -p -x hello"
    if ($resp -match 'unknown flag -x') { Write-Pass "TCP display -p -x refused: [$resp]" } else { Write-Fail "TCP display -p -x hello = [$resp]" }
    Check "TCP display -p -- -x" (Tcp "display-message -p -- -x") '-x'
} else { Write-Fail "no port file at $portFile" }

Write-Host "`n=== source-file ===" -ForegroundColor Cyan
$conf = Join-Path $env:PSMUX_DATA_DIR "t795s_$PID.conf"
@"
setw -t $(W1) @c cwin
set -gw @cg cglob
"@ | Set-Content -Path $conf -Encoding ASCII
$null = Px @('source-file', $conf)
Check "conf setw: show -wv :1" (Px @('show-options', '-wv', '-t', (W1), '@c')).out 'cwin'
Check "conf setw: #{@c} in :0" (Fmt (W0) '#{@c}') ''
Check "conf set -gw: show -gwv" (Px @('show-options', '-gwv', '@cg')).out 'cglob'
Check "conf set -gw: #{@cg} in :0" (Fmt (W0) '#{@cg}') 'cglob'
Remove-Item $conf -ErrorAction SilentlyContinue

Write-Host "`n=== display-message unknown flags ===" -ForegroundColor Cyan
$r = Px @('display-message', '-p', '-x=foo')
if ($r.rc -eq 1 -and $r.err -match 'unknown flag -x') { Write-Pass "display -p -x=foo refused rc 1" } else { Write-Fail "display -p -x=foo rc=$($r.rc) out=[$($r.out)] err=[$($r.err)]" }
$r = Px @('display-message', '-p', '-x', 'hello')
if ($r.rc -eq 1 -and $r.err -match 'unknown flag -x') { Write-Pass "display -p -x hello refused rc 1" } else { Write-Fail "display -p -x hello rc=$($r.rc) out=[$($r.out)] err=[$($r.err)]" }
$r = Px @('display-message', '-t', $S, '-p', '--', '-x=foo')
if ($r.rc -eq 0 -and $r.out -eq '-x=foo') { Write-Pass "display -p -- -x=foo prints it" } else { Write-Fail "display -p -- -x=foo rc=$($r.rc) out=[$($r.out)]" }
$r = Px @('display-message', '-t', $S, '-p', 'a -x b')
if ($r.rc -eq 0 -and $r.out -eq 'a -x b') { Write-Pass "a message containing -x is data" } else { Write-Fail "display 'a -x b' rc=$($r.rc) out=[$($r.out)]" }
$r = Px @('display-message', '-p', '-t', $S, '#S')
if ($r.rc -eq 0 -and $r.out -eq $S) { Write-Pass "known flags still work" } else { Write-Fail "display -p -t S #S rc=$($r.rc) out=[$($r.out)]" }

Write-Host "`n=== show-options on a user option nothing holds ===" -ForegroundColor Cyan
foreach ($flags in @(@(), @('-v'), @('-g'), @('-w'), @('-wv'), @('-gw'), @('-p'), @('-A'))) {
    $tgt = if ($flags -contains '-g' -or $flags -contains '-gw') { @() } else { @('-t', $S) }
    $r = Px (@('show-options') + $flags + $tgt + @('@unset795'))
    if ($r.rc -eq 1 -and $r.err -match 'invalid option: @unset795' -and $r.out -eq '') { Write-Pass "show $($flags -join ' ') @unset: invalid option rc 1" }
    else { Write-Fail "show $($flags -join ' ') @unset rc=$($r.rc) out=[$($r.out)] err=[$($r.err)]" }
    $q = Px (@('show-options', '-q') + $flags + $tgt + @('@unset795'))
    if ($q.rc -eq 0 -and $q.out -eq '' -and $q.err -eq '') { Write-Pass "show -q $($flags -join ' ') @unset: silent rc 0" }
    else { Write-Fail "show -q $($flags -join ' ') @unset rc=$($q.rc) out=[$($q.out)] err=[$($q.err)]" }
}
$null = Px @('set-option', '-t', $S, '@empty795', '')
Check "show @empty (set to empty)" (Px @('show-options', '-t', $S, '@empty795')).out "@empty795 ''"
$null = Px @('set-option', '-t', $S, '@k795', 'v')
Check "show @k" (Px @('show-options', '-t', $S, '@k795')).out '@k795 v'
Check "show -v @k" (Px @('show-options', '-v', '-t', $S, '@k795')).out 'v'
$list = (Px @('show-options', '-t', $S)).out
if ($list -match '(?m)^@k795 ' -and $list -match '(?m)^status ') { Write-Pass "show with no name still lists everything" } else { Write-Fail "show listing broken" }
Check "show status (table option)" (Px @('show-options', '-t', $S, 'status')).out 'status on'

Write-Host "`n=== display-message -l -v -a -N -C -F ===" -ForegroundColor Cyan
$r = Px @('display-message', '-p', '-t', $S, '-l', '#{session_name} %H')
Check "display -l prints the message unchanged" $r.out '#{session_name} %H'
$r = Px @('display-message', '-p', '-t', $S, '-v', '#{session_name}')
Check "display -v logs then prints" $r.out "# expanding format: #{session_name}`n# result is: $S`n$S"
$r = Px @('display-message', '-p', '-t', $S, '-a')
if ($r.rc -eq 0 -and $r.out -match "(?m)^session_name=$S$" -and $r.out -match '(?m)^window_index=\d+' -and $r.out -notmatch '(?m)^-a$') {
    Write-Pass "display -a lists name=value pairs ($((($r.out) -split "`n").Count) lines)"
} else { Write-Fail "display -a rc=$($r.rc) out=[$($r.out.Substring(0, [Math]::Min(200, $r.out.Length)))]" }
Check "display -N is a flag" (Px @('display-message', '-p', '-t', $S, '-N', 'n')).out 'n'
Check "display -C is a flag" (Px @('display-message', '-p', '-t', $S, '-C', 'c')).out 'c'
Check "display -F format" (Px @('display-message', '-p', '-t', $S, '-F', '#{session_name}')).out $S
$r = Px @('display-message', '-p', '-t', $S, '-F', '#S', 'extra')
if ($r.rc -eq 1 -and $r.err -match 'only one of -F or argument must be given') { Write-Pass "-F with an argument refused" } else { Write-Fail "-F + arg rc=$($r.rc) out=[$($r.out)] err=[$($r.err)]" }
$r = Px @('display-message', '-t', $S, '-N', '-d', '300', 'sticky')
Check "display -N to the status line rc" $r.rc 0

$null = Px @('kill-server')

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
