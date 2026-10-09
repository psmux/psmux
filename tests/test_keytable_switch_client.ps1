# switch-client -T must route the NEXT key, whichever route set the table.
#
#   bind -n F6 switch-client -T mytbl
#   bind -T mytbl x set -g @hit x
#
# Before the fix only a table latched by a PREFIX binding worked. A root (-n)
# binding, the command prompt (prefix : switch-client -T mytbl) and the CLI
# (psmux switch-client -T mytbl) all made #{client_key_table} read mytbl, but
# the attached client kept looking the next key up in root: x was typed into
# the shell, @hit never changed, and the table stayed mytbl forever.
#
# tmux has one c->keytable that every route writes (cmd-switch-client.c:96)
# and server_client_handle_key reads; the next key is looked up there, then
# the client drops back to root (server-client.c:1572) unless the binding
# re-arms it.
#
# Proof is a side effect read over the CLI: the bound command sets @hit.
# Keys are real console input (tests/injector.cs, WriteConsoleInput) into an
# attached client. Runs in its own -L namespace; never touches the default one.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$dataDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$TMP = Join-Path $env:TEMP "psmux_keytable_sc"
New-Item -ItemType Directory -Force -Path $TMP | Out-Null
$SOCK = if ($env:KT_SOCK) { $env:KT_SOCK } else { "keytable_sc$PID" }
$S = "kt"
$ROUNDS = if ($env:KT_ROUNDS) { [int]$env:KT_ROUNDS } else { 3 }
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }
function Write-Ascii([string]$p, [string]$t) { [IO.File]::WriteAllText($p, $t, (New-Object System.Text.ASCIIEncoding)) }
function Q([string]$fmt) { (& $PSMUX -L $SOCK display-message -p -t $S $fmt 2>&1 | Out-String).Trim() }
function Hit { (& $PSMUX -L $SOCK show -gv '@hit' 2>&1 | Out-String).Trim() }
function Reset-Hit { & $PSMUX -L $SOCK set -g '@hit' 0 2>&1 | Out-Null }
function LastLine {
    $cap = (& $PSMUX -L $SOCK capture-pane -t $S -p 2>&1 | Out-String)
    (($cap -split "`r?`n" | Where-Object { $_ -match '\S' }) | Select-Object -Last 1)
}

Write-Host "binary:  $PSMUX" -ForegroundColor Cyan
Write-Host "socket:  $SOCK" -ForegroundColor Cyan

$conf = Join-Path $TMP "kt.conf"
Write-Ascii $conf @"
set -g @hit 0
bind -n F6 switch-client -T mytbl
bind -T mytbl x set -g @hit x
bind -T mytbl F9 set -g @hit F9
bind -T mytbl s set -g @hit s \; switch-client -T mytbl
bind q switch-client -T mytbl
"@

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$inj = Join-Path $TMP "keys_kt.exe"
Remove-Item $inj -Force -EA SilentlyContinue
& $csc /nologo /optimize /out:$inj (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
if (-not (Test-Path $inj)) { Write-Fail "could not compile tests/injector.cs"; exit 1 }

$launch = Join-Path $TMP "launch_kt.cmd"
Write-Ascii $launch @"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set PSMUX_NO_WARM=1
set PSMUX_DATA_DIR=$dataDir
"$PSMUX" -L $SOCK -f "$conf" new-session -s $S -x 120 -y 30 cmd
"@
$lp = Start-Process -FilePath $launch -PassThru
for ($i = 0; $i -lt 100; $i++) { if (Test-Path "$dataDir\${SOCK}__$S.port") { break }; Start-Sleep -Milliseconds 250 }
Start-Sleep -Seconds 3
$cpid = 0
for ($k = 0; $k -lt 12; $k++) {
    $cli = Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -match "$SOCK\b.*new-session -s\s+$S\b" } | Select-Object -First 1
    if ($cli) { $cpid = [int]$cli.ProcessId; break }
    Start-Sleep -Milliseconds 600
}
if ($cpid -eq 0) {
    Write-Fail "attached client did not start"
} else {
    Write-Info "attached client pid=$cpid"

    # Part A: the four routes that set a table, each followed by a char key and
    # a function key bound in that table.
    Write-Host "`n=== Part A: next key fires in the table, by route ===" -ForegroundColor Cyan
    $cases = @(
        @{ n = "root -n F6, then x";              arm = "{F6}"; key = "x";    want = "x" },
        @{ n = "root -n F6, then F9";             arm = "{F6}"; key = "{F9}"; want = "F9" },
        @{ n = "prefix q, then x";                arm = "^b{SLEEP:300}q"; key = "x"; want = "x" },
        @{ n = "command prompt, then x";          arm = "^b{SLEEP:300}:{SLEEP:400}switch-client -T mytbl{ENTER}"; key = "x"; want = "x" },
        @{ n = "CLI switch-client -T, then x";    arm = "CLI"; key = "x";    want = "x" },
        @{ n = "CLI switch-client -T, then F9";   arm = "CLI"; key = "{F9}"; want = "F9" }
    )
    foreach ($c in $cases) {
        $ok = 0; $detail = ""
        for ($r = 0; $r -lt $ROUNDS; $r++) {
            Reset-Hit
            if ($c.arm -eq "CLI") { & $PSMUX -L $SOCK switch-client -T mytbl 2>&1 | Out-Null }
            else { & $inj $cpid $c.arm 2>&1 | Out-Null }
            Start-Sleep -Milliseconds 900
            $t1 = Q '#{client_key_table}'
            & $inj $cpid $c.key 2>&1 | Out-Null
            Start-Sleep -Milliseconds 1100
            $h = Hit; $t2 = Q '#{client_key_table}'
            if ($t1 -eq "mytbl" -and $h -eq $c.want -and $t2 -eq "root") { $ok++ }
            else { $detail = "table after arm=$t1, @hit=$h, table after key=$t2" }
        }
        if ($ok -eq $ROUNDS) { Write-Pass "$($c.n): fired $ok/$ROUNDS and returned to root" }
        else { Write-Fail "$($c.n): fired $ok/$ROUNDS ($detail)" }
    }

    # Part B: F6 and x in one burst, faster than a state round trip. The frame
    # reporting mytbl lands after x consumed the latch; it must not re-arm it.
    Write-Host "`n=== Part B: burst typing ===" -ForegroundColor Cyan
    $ok = 0; $detail = ""
    for ($r = 0; $r -lt $ROUNDS; $r++) {
        Reset-Hit
        & $inj $cpid "{F6}x" 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1200
        $h = Hit; $t = Q '#{client_key_table}'
        if ($h -eq "x" -and $t -eq "root") { $ok++ } else { $detail = "@hit=$h table=$t" }
    }
    if ($ok -eq $ROUNDS) { Write-Pass "F6 x in one burst fired x and left root ($ok/$ROUNDS)" }
    else { Write-Fail "F6 x burst: $ok/$ROUNDS ($detail)" }

    # Part C: a sticky table entered from a root binding stays latched.
    Write-Host "`n=== Part C: sticky table from a root binding ===" -ForegroundColor Cyan
    Reset-Hit
    & $inj $cpid "{F6}{SLEEP:500}s{SLEEP:500}" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    $t = Q '#{client_key_table}'
    if ((Hit) -eq "s" -and $t -eq "mytbl") { Write-Pass "s fired and re-armed mytbl" }
    else { Write-Fail "sticky s: @hit=$(Hit) table=$t" }
    & $inj $cpid "x" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $t = Q '#{client_key_table}'
    if ((Hit) -eq "x" -and $t -eq "root") { Write-Pass "the key after the sticky one fired in mytbl, then root" }
    else { Write-Fail "after sticky: @hit=$(Hit) table=$t" }

    # Part D: an unbound key in a CLI-set table is swallowed, then root.
    Write-Host "`n=== Part D: unbound key and explicit reset ===" -ForegroundColor Cyan
    & $PSMUX -L $SOCK send-keys -t $S "cls" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    & $PSMUX -L $SOCK switch-client -T mytbl 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    & $inj $cpid "Z" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $ll = LastLine; $t = Q '#{client_key_table}'
    if ($ll -notmatch 'Z\s*$') { Write-Pass "unbound Z in the CLI-set table was swallowed" }
    else { Write-Fail "Z reached the shell: '$ll'" }
    if ($t -eq "root") { Write-Pass "client returned to root after the unbound key" }
    else { Write-Fail "client stuck in '$t'" }
    # switch-client -T root from the CLI disarms the client too.
    Reset-Hit
    & $PSMUX -L $SOCK switch-client -T mytbl 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    & $PSMUX -L $SOCK switch-client -T root 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    & $inj $cpid "x" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 1000
    $ll = LastLine
    if ((Hit) -eq "0" -and $ll -match 'x\s*$') { Write-Pass "after switch-client -T root, x went to the shell, not to mytbl" }
    else { Write-Fail "after -T root: @hit=$(Hit) lastline='$ll'" }
    & $inj $cpid "{ESC}" 2>&1 | Out-Null

    # Part E: the session stays functional through the TUI (CLI-driven checks).
    Write-Host "`n=== Part E: TUI sanity ===" -ForegroundColor Cyan
    & $PSMUX -L $SOCK split-window -t $S 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    $np = Q '#{window_panes}'
    if ($np -eq "2") { Write-Pass "split-window on the attached session: 2 panes" } else { Write-Fail "panes=$np" }
    $att = Q '#{session_attached}'
    if ($att -ge 1) { Write-Pass "client still attached ($att)" } else { Write-Fail "session_attached=$att" }

    try { Stop-Process -Id $cpid -Force -EA SilentlyContinue } catch {}
}
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
if ($lp -and -not $lp.HasExited) { try { Stop-Process -Id $lp.Id -Force -EA SilentlyContinue } catch {} }

Write-Host "`n=== keytable switch-client results: $script:Pass passed, $script:Fail failed ===" -ForegroundColor Cyan
if ($script:Fail -gt 0) { exit 1 }
exit 0
