# Issue #804: run-shell children got no session environment. $TMUX was
# unset, so a helper script could not tell it was inside a session:
#
#   psmux run-shell "pwsh -NoProfile -Command Set-Content $HOME/env.txt \"[$env:TMUX][$env:TMUX_PANE]\""
#
# wrote `[][]`, while a pane child has both.
#
# What tmux gives a job (the spec):
#   job.c:92        job_run: env = environ_for_session(s, ...)
#   environ.c:253   environ_for_session: global environment, the session
#                   environment over it, then TMUX=<socket>,<pid>,<session id>
#                   (environ.c:282)
#   spawn.c:394     TMUX_PANE is set ONLY for a pane (spawn_pane), never for a
#                   job, so a run-shell / if-shell / #() / popup child has TMUX
#                   but no TMUX_PANE. The pane is reachable as #{pane_id}.
#   cmd-run-shell.c:209, cmd-if-shell.c:121, format.c:431 and popup.c:648 all
#   go through job_run. cmd-pipe-pane.c:126 forks directly (no TMUX).
#
# GROUND TRUTH: the environment the child actually saw, written to a file by a
# script the child runs, compared with what a pane child sees.
#
# Routes: CLI run-shell (plain, -b, with a format), the reporter's exact
# command, if-shell (CLI), raw TCP, a hook, source-file, the startup config
# file (-f), #() in display-message, and on an attached client a bound key
# run-shell, a bound key if-shell, the command prompt, display-popup and the
# status line #() job.
#
# Isolation: a unique -L namespace only; cleanup is `-L <ns> kill-server` and
# the client window this script opened, by PID.
#
# Set PSMUX_TEST_BIN to test a binary that is not on PATH.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$script:TestsPassed = 0; $script:TestsFailed = 0; $script:Skipped = 0
$script:Opened = @()

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow; $script:Skipped++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }
function Write-Head($msg) { Write-Host "`n--- $msg ---" -ForegroundColor Yellow }
function Check($ok, $pass, $fail) { if ($ok) { Write-Pass $pass } else { Write-Fail $fail } }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

# This script must not itself look like it runs inside a session.
foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX_TARGET_SESSION','PSMUX_TARGET_FULL') {
    Remove-Item "Env:$v" -EA SilentlyContinue
}

$NS   = "a804_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$SESS = "s804"
$TMP  = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_804_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TMP | Out-Null

function P { & $PSMUX -L $NS @args 2>&1 }
function DT($t, $f) { ((P display-message -t $t -p $f) -join '').Trim() }
function F($name) { Join-Path $TMP $name }

# The child: records its environment, and asks a bare `psmux` (no -L, no -t)
# which session it is in. The psmux it finds is the one the server put on its
# PATH (#660), so it is the binary under test.
$DUMP = F 'dump.ps1'
@'
param([string]$Out, [string]$Ask = '1')
$who = ''
$who2 = ''
$o = [ordered]@{
    TMUX  = "$env:TMUX"
    PANE  = "$env:TMUX_PANE"
    PSESS = "$env:PSMUX_SESSION"
    PTGT  = "$env:PSMUX_TARGET_SESSION"
    G     = "$env:I804_G"
    S     = "$env:I804_S"
}
if ($Ask -eq '1') {
    $who = ((& psmux display-message -p '#{session_name}' 2>&1) -join '').Trim()
    # Again with $TMUX as the only clue, the way a tmux helper finds its server.
    Remove-Item Env:PSMUX_TARGET_SESSION -EA SilentlyContinue
    $who2 = ((& psmux display-message -p '#{session_name}' 2>&1) -join '').Trim()
}
$o.WHO = $who
$o.WHO2 = $who2
[IO.File]::WriteAllText($Out, ($o | ConvertTo-Json -Compress))
exit 0
'@ | Set-Content -Path $DUMP -Encoding UTF8

function Child($out, $ask = '1') { "pwsh -NoProfile -File $DUMP $out $ask" }
function ReadEnv($p) {
    for ($i = 0; $i -lt 60; $i++) {
        if (Test-Path $p) {
            try { return (Get-Content $p -Raw | ConvertFrom-Json) } catch {}
        }
        Start-Sleep -Milliseconds 250
    }
    return $null
}

$script:WantTmux = ''
# A job child: TMUX exactly as a pane child has it, no TMUX_PANE (tmux
# spawn.c:394 sets that for panes only), PSMUX_TARGET_SESSION, the
# set-environment values, and a bare `psmux` reaching this session.
# PSMUX_SESSION is psmux's pane marker (the nested session guard reads it), so
# only a pane has it, and a popup, which has always had it.
function Assert-JobEnv($label, $e, [switch]$NoAsk, [switch]$NoSessionEnv, [string]$PSess = '') {
    if ($null -eq $e) { Write-Fail "${label}: the child never wrote its environment"; return }
    Check ($e.TMUX -ne '' -and $e.TMUX -eq $script:WantTmux) "${label}: TMUX = $($e.TMUX)" "${label}: TMUX = '$($e.TMUX)', pane child has '$($script:WantTmux)'"
    Check ($e.PANE -eq '') "${label}: no TMUX_PANE (tmux sets it for panes only)" "${label}: TMUX_PANE = '$($e.PANE)'"
    Check ($e.PSESS -eq $PSess) "${label}: PSMUX_SESSION = '$PSess'" "${label}: PSMUX_SESSION = '$($e.PSESS)', want '$PSess'"
    Check ($e.PTGT -eq "${NS}__$SESS") "${label}: PSMUX_TARGET_SESSION kept" "${label}: PSMUX_TARGET_SESSION = '$($e.PTGT)'"
    Check ($e.G -eq 'gval') "${label}: set-environment -g value present" "${label}: I804_G = '$($e.G)'"
    if (-not $NoSessionEnv) {
        Check ($e.S -eq 'sval') "${label}: session set-environment value present" "${label}: I804_S = '$($e.S)'"
    }
    if (-not $NoAsk) {
        Check ($e.WHO -eq $SESS) "${label}: bare psmux in the child answers $SESS" "${label}: bare psmux answered '$($e.WHO)'"
        Check ($e.WHO2 -eq $SESS) "${label}: bare psmux routed by TMUX alone answers $SESS" "${label}: bare psmux by TMUX alone answered '$($e.WHO2)'"
    }
}

$defaultBefore = ((& $PSMUX ls 2>&1) -join "`n")

# ======================================================================
Write-Head "Part A: startup config file (-f) and CLI routes"
# ======================================================================
$conf = F 'start.conf'
@(
    "set-environment -g I804_G gval",
    "run-shell `"$(Child (F 'a_conf_run.txt') '0')`"",
    "if-shell `"$(Child (F 'a_conf_if.txt') '0')`" `"set -g @c804 yes`" `"set -g @c804 no`""
) | Set-Content -Path $conf -Encoding ASCII

P -f $conf new-session -d -s $SESS | Out-Null
$up = $false
for ($i = 0; $i -lt 60; $i++) { Start-Sleep -Milliseconds 250; if ((DT $SESS '#{session_name}') -eq $SESS) { $up = $true; break } }
if (-not $up) { Write-Fail "session never came up"; exit 1 }
P set-environment -t $SESS I804_S sval | Out-Null

# What a pane child gets: the reference.
$fp = F 'a_pane.txt'
P send-keys -t $SESS "pwsh -NoProfile -File $DUMP $fp 0" Enter | Out-Null
$pane = ReadEnv $fp
if ($null -eq $pane) { Write-Fail "pane child never wrote its environment"; exit 1 }
$script:WantTmux = $pane.TMUX
Write-Info "pane child: TMUX=$($pane.TMUX) TMUX_PANE=$($pane.PANE)"
Check ($pane.TMUX -match ',\d+,\d+$' -and $pane.PANE -match '^%\d+$') "pane child has TMUX and TMUX_PANE (reference)" "pane child env unexpected: $($pane | ConvertTo-Json -Compress)"

# Startup config lines run before the session environment exists.
Assert-JobEnv "startup config run-shell" (ReadEnv (F 'a_conf_run.txt')) -NoAsk -NoSessionEnv
Assert-JobEnv "startup config if-shell" (ReadEnv (F 'a_conf_if.txt')) -NoAsk -NoSessionEnv

$f = F 'a_cli.txt'
P run-shell (Child $f) | Out-Null
Assert-JobEnv "CLI run-shell" (ReadEnv $f)

$f = F 'a_cli_t.txt'
P run-shell -t $SESS (Child $f) | Out-Null
Assert-JobEnv "CLI run-shell -t" (ReadEnv $f)

$f = F 'a_cli_b.txt'
P run-shell -b (Child $f) | Out-Null
Assert-JobEnv "CLI run-shell -b" (ReadEnv $f)

# A format in the command sends it to the server (main.rs), another route.
$f = F 'a_cli_fmt.txt'
P run-shell "$(Child $f) #{session_name}" | Out-Null
Assert-JobEnv "CLI run-shell with a format" (ReadEnv $f)

$f = F 'a_cli_if.txt'
$r = ((P if-shell (Child $f) 'display-message -p T' 'display-message -p F') -join '').Trim()
Check ($r -eq 'T') "CLI if-shell took the true branch" "CLI if-shell gave '$r'"
Assert-JobEnv "CLI if-shell" (ReadEnv $f)


# The reporter's exact command.
$f = F 'a_report.txt'
P run-shell "pwsh -NoProfile -Command Set-Content $f '[`$env:TMUX][`$env:TMUX_PANE]'" | Out-Null
for ($i = 0; $i -lt 40 -and -not (Test-Path $f); $i++) { Start-Sleep -Milliseconds 250 }
$rep = if (Test-Path $f) { (Get-Content $f -Raw).Trim() } else { '<missing>' }
Check ($rep -eq "[$($script:WantTmux)][]") "reporter's command writes '$rep'" "reporter's command wrote '$rep'"

# #{pane_id} is how a job learns its pane (tmux has no TMUX_PANE for it).
$wantPane = DT $SESS '#{pane_id}'
$r = ((P run-shell -t $SESS "Write-Output 'pane=#{pane_id}'") -join '').Trim()
Check ($r -eq "pane=$wantPane") "run-shell expands #{pane_id} to $wantPane" "run-shell #{pane_id} gave '$r'"

# ======================================================================
Write-Head "Part B: server routes (TCP, hook, source-file, #())"
# ======================================================================
function Send-Tcp($cmd) {
    $port = (Get-Content "$psmuxDir\${NS}__${SESS}.port" -Raw).Trim()
    $key  = (Get-Content "$psmuxDir\${NS}__${SESS}.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $s = $tcp.GetStream()
    $w = [System.IO.StreamWriter]::new($s); $rd = [System.IO.StreamReader]::new($s)
    $w.Write("AUTH $key`n"); $w.Flush()
    $null = $rd.ReadLine()
    $w.Write("$cmd`n"); $w.Flush()
    $s.ReadTimeout = 20000
    $out = ''
    try { $out = $rd.ReadToEnd() } catch {}
    $tcp.Close()
    return $out.Trim()
}
$f = F 'b_tcp.txt'
$null = Send-Tcp ("run-shell `"" + (Child $f).Replace('\', '\\') + "`"")
Assert-JobEnv "TCP run-shell" (ReadEnv $f)

$f = F 'b_tcp_if.txt'
$null = Send-Tcp ("if-shell `"" + (Child $f).Replace('\', '\\') + "`" `"display-message -p T`"")
Assert-JobEnv "TCP if-shell" (ReadEnv $f)

$f = F 'b_hook.txt'
P set-hook -g after-new-window "run-shell '$(Child $f)'" | Out-Null
P new-window -d -t $SESS | Out-Null
Assert-JobEnv "hook run-shell" (ReadEnv $f)
P set-hook -gu after-new-window | Out-Null

$fs1 = F 'b_src_run.txt'; $fs2 = F 'b_src_if.txt'
$sconf = F 'src.conf'
# The children do not call back into the server: a config if-shell condition
# is waited for on the server loop, so a callback would stall until timeout.
@(
    "run-shell `"$(Child $fs1 '0')`"",
    "if-shell `"$(Child $fs2 '0')`" `"set -g @s804 yes`""
) | Set-Content -Path $sconf -Encoding ASCII
P source-file -t $SESS $sconf | Out-Null
Assert-JobEnv "source-file run-shell" (ReadEnv $fs1) -NoAsk
Assert-JobEnv "source-file if-shell" (ReadEnv $fs2) -NoAsk

# #() job: tmux runs it with no session (format.c:431, s = NULL), so it has
# TMUX and the global environment.
$f = F 'b_fmt.txt'
P display-message -t $SESS -p "#($(Child $f '0'))" | Out-Null
Assert-JobEnv "#() in display-message" (ReadEnv $f) -NoAsk -NoSessionEnv

# pipe-pane forks without a job environment in tmux (cmd-pipe-pane.c:126).
$f = F 'b_pipe.txt'
P pipe-pane -t $SESS (Child $f '0') | Out-Null
$e = ReadEnv $f
if ($e) { Write-Info "pipe-pane child (informational, not a job in tmux): TMUX='$($e.TMUX)' PANE='$($e.PANE)'" }
P pipe-pane -t $SESS | Out-Null

P kill-server | Out-Null
Start-Sleep -Milliseconds 800

# ======================================================================
Write-Head "Part C: attached client (binding, prompt, popup, status #())"
# ======================================================================
$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
           Select-Object -First 1 -ExpandProperty FullName
}
$INJ = Join-Path $TMP "injector.exe"
if ($csc -and (Test-Path $csc)) { & $csc /nologo /optimize /out:$INJ (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null }

$tconf = F 'tui.conf'
@(
    "set-environment -g I804_G gval",
    "bind-key Y run-shell `"$(Child (F 'c_key_run.txt'))`"",
    "bind-key U run-shell -b `"$(Child (F 'c_key_run_b.txt'))`"",
    "bind-key Z if-shell `"$(Child (F 'c_key_if.txt'))`" `"set -g @k804 yes`""
) | Set-Content -Path $tconf -Encoding ASCII

$p = Start-Process -FilePath $PSMUX -ArgumentList "-f",$tconf,"-L",$NS,"new-session","-s",$SESS,"-x","120","-y","30" -PassThru
$script:Opened += $p.Id
$up = $false
for ($i = 0; $i -lt 80; $i++) { Start-Sleep -Milliseconds 250; if ((DT $SESS '#{session_name}') -eq $SESS) { $up = $true; break } }
Start-Sleep -Milliseconds 2000
if (-not $up) {
    Write-Fail "TUI: attached session never came up"
} else {
    P set-environment -t $SESS I804_S sval | Out-Null
    $fp = F 'c_pane.txt'
    P send-keys -t $SESS "pwsh -NoProfile -File $DUMP $fp 0" Enter | Out-Null
    $pane = ReadEnv $fp
    if ($pane) { $script:WantTmux = $pane.TMUX }

    # Status line #(): the job runs on the render path of an attached client.
    $f = F 'c_status.txt'
    P set -g status-interval 1 | Out-Null
    P set -g status-right "#($(Child $f '0'))" | Out-Null
    Assert-JobEnv "status-right #() job" (ReadEnv $f) -NoAsk -NoSessionEnv
    P set -g status-right "x" | Out-Null

    if (-not (Test-Path $INJ)) {
        Write-Skip "injector could not be built (csc.exe unavailable)"
    } else {
        & $INJ $p.Id "^b{SLEEP:300}Y" | Out-Null
        Assert-JobEnv "bound key run-shell" (ReadEnv (F 'c_key_run.txt'))
        & $INJ $p.Id "^b{SLEEP:300}U" | Out-Null
        Assert-JobEnv "bound key run-shell -b" (ReadEnv (F 'c_key_run_b.txt'))
        & $INJ $p.Id "^b{SLEEP:300}Z" | Out-Null
        Assert-JobEnv "bound key if-shell" (ReadEnv (F 'c_key_if.txt'))

        $f = F 'c_prompt.txt'
        & $INJ $p.Id "^b{SLEEP:300}:{SLEEP:500}run-shell `"$(Child $f)`"{ENTER}" | Out-Null
        Assert-JobEnv "command prompt run-shell" (ReadEnv $f)

        # display-popup: a job in tmux too (popup.c:648), so TMUX and no
        # TMUX_PANE.
        $f = F 'c_popup.txt'
        P display-popup -t $SESS -E (Child $f) | Out-Null
        Assert-JobEnv "display-popup" (ReadEnv $f) -PSess $SESS
    }
}

P kill-server | Out-Null
Start-Sleep -Milliseconds 800
foreach ($id in $script:Opened) { try { Stop-Process -Id $id -Force -EA SilentlyContinue } catch {} }
Get-ChildItem "$psmuxDir\${NS}__*" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue
Remove-Item $TMP -Recurse -Force -EA SilentlyContinue

$defaultAfter = ((& $PSMUX ls 2>&1) -join "`n")
Check ($defaultBefore -eq $defaultAfter) "default namespace session list unchanged" "default namespace changed: before '$defaultBefore' after '$defaultAfter'"

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed:  $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed:  $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $($script:Skipped)"
exit $script:TestsFailed
