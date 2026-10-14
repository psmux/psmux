# Issue #803: session options must not share the global store.
#
#   psmux set -g -t pr prefix C-a      # global=C-a  session=C-a
#   psmux set    -t pr prefix C-x      # global=C-x  session=C-x   <- global changed
#   psmux set -u -t pr prefix          # global=C-b  session=C-b   <- default, not C-a
#
# tmux keeps a table per session whose parent is the global session table
# (options.c options_get walks up to the parent): `set` without -g writes the
# session table only, `set -u` removes that entry (options_remove_or_default
# only falls back to the table default on a GLOBAL table) so the global shows
# through again, `show -A` marks inherited values with `*`
# (cmd-show-options.c:197) and `-o` at session scope asks the session table.
#
# psmux runs one server per session, so "global" is global to the session's
# server; a session in another server is never touched by either scope.
#
# Part I types into an attached client (tests/injector.cs, WriteConsoleInput)
# and drives the nested session F12 toggle end to end.
# Runs in its own -L namespace with a private PSMUX_DATA_DIR.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP ("psmux_issue803_data_" + $PID)
    New-Item -ItemType Directory -Force -Path $env:PSMUX_DATA_DIR | Out-Null
}
$dataDir = $env:PSMUX_DATA_DIR
$TMP = Join-Path $env:TEMP "psmux_issue803"
New-Item -ItemType Directory -Force -Path $TMP | Out-Null
$SOCK = "i803_$PID"
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }
function Write-Ascii([string]$p, [string]$t) { [IO.File]::WriteAllText($p, $t, (New-Object System.Text.ASCIIEncoding)) }
function P { & $PSMUX -L $SOCK @args 2>&1 | Out-String | ForEach-Object { $_.Trim() } }
function G([string]$s, [string]$o) { P show -gv -t $s $o }
function S([string]$s, [string]$o) { P show -v -t $s $o }
# One show line; psmux may quote a string value the way tmux's args_escape can.
function CheckLine([string]$what, [string]$got, [string]$name, [string]$want) {
    if ($got -cmatch ('^' + [regex]::Escape($name) + ' "?' + [regex]::Escape($want) + '"?\r?$')) { Write-Pass "$what = '$got'" } else { Write-Fail "$what = '$got' (expected '$name $want')" }
}
function Check([string]$what, [string]$got, [string]$want) {
    if ($got -ceq $want) { Write-Pass "$what = '$want'" } else { Write-Fail "$what = '$got' (expected '$want')" }
}
function Wait-Port([string]$sess) {
    for ($i = 0; $i -lt 80; $i++) { if (Test-Path "$dataDir\${SOCK}__$sess.port") { return }; Start-Sleep -Milliseconds 250 }
}

Write-Host "binary:  $PSMUX" -ForegroundColor Cyan
Write-Host "socket:  $SOCK" -ForegroundColor Cyan
$plain = Join-Path $TMP "plain.conf"
Write-Ascii $plain "set -g @plain 1`n"

# ---------------------------------------------------------------- Part A
Write-Host "`n=== Part A: the issue's prefix sequence ===" -ForegroundColor Cyan
& $PSMUX -L $SOCK -f $plain new-session -d -s pr -x 100 -y 30 2>&1 | Out-Null
Wait-Port pr; Start-Sleep -Milliseconds 800
P set -g -t pr prefix C-a | Out-Null
Check "after set -g prefix C-a: global" (G pr prefix) "C-a"
Check "after set -g prefix C-a: session" (S pr prefix) "C-a"
P set -t pr prefix C-x | Out-Null
Check "after set prefix C-x: global" (G pr prefix) "C-a"
Check "after set prefix C-x: session" (S pr prefix) "C-x"
P set -u -t pr prefix | Out-Null
Check "after set -u prefix: global" (G pr prefix) "C-a"
Check "after set -u prefix: session falls back to global" (S pr prefix) "C-a"

# ---------------------------------------------------------------- Part B
Write-Host "`n=== Part B: the issue's status-left sequence ===" -ForegroundColor Cyan
P set -g -t pr status-left GLOBAL | Out-Null
P set -t pr status-left SESS | Out-Null
Check "status-left global" (G pr status-left) "GLOBAL"
Check "status-left session" (S pr status-left) "SESS"
Check "#{status-left} uses the session value" (P display-message -p -t pr '#{status-left}') "SESS"
P set -u -t pr status-left | Out-Null
Check "after set -u status-left: session" (S pr status-left) "GLOBAL"
Check "after set -u status-left: global" (G pr status-left) "GLOBAL"

# ---------------------------------------------------------------- Part C
Write-Host "`n=== Part C: -g under a session override, -gu, -u without an override ===" -ForegroundColor Cyan
P set -t pr status-left SESS | Out-Null
P set -g -t pr status-left G2 | Out-Null
Check "set -g under an override: session keeps" (S pr status-left) "SESS"
Check "set -g under an override: global takes" (G pr status-left) "G2"
P set -ga -t pr status-left "+A" | Out-Null
Check "set -ga under an override appends to the global" (G pr status-left) "G2+A"
P set -a -t pr status-left "+S" | Out-Null
Check "set -a appends to the session value" (S pr status-left) "SESS+S"
P set -gu -t pr status-left | Out-Null
Check "set -gu under an override: global back to default" (G pr status-left) "[#S]"
Check "set -gu under an override: session keeps" (S pr status-left) "SESS+S"
P set -u -t pr status-left | Out-Null
Check "set -u after -gu: session shows the default global" (S pr status-left) "[#S]"
P set -g -t pr status-left KEEP | Out-Null
P set -u -t pr status-left | Out-Null
Check "set -u with no session entry leaves the global alone" (G pr status-left) "KEEP"
Check "set -u with no session entry: session still inherits" (S pr status-left) "KEEP"
# A boolean with no value toggles at the scope named.
P set -g -t pr mouse on | Out-Null
P set -t pr mouse | Out-Null
Check "set mouse (toggle) at session scope: session" (S pr mouse) "off"
Check "set mouse (toggle) at session scope: global" (G pr mouse) "on"
P set -u -t pr mouse | Out-Null
Check "set -u mouse: session back to the global" (S pr mouse) "on"

# ---------------------------------------------------------------- Part D
Write-Host "`n=== Part D: show, show -g, show -A ===" -ForegroundColor Cyan
P set -g -t pr status-left GL | Out-Null
P set -t pr status-left SL | Out-Null
$all = P show -t pr
$allG = P show -g -t pr
$allA = P show -A -t pr
if ($all -cmatch '(?m)^status-left "?SL"?\r?$') { Write-Pass "show lists the session value" } else { Write-Fail "show status-left line: $(($all -split "`n" | Select-String '^status-left ') -join ' | ')" }
if ($allG -cmatch '(?m)^status-left "?GL"?\r?$') { Write-Pass "show -g lists the global value" } else { Write-Fail "show -g status-left line: $(($allG -split "`n" | Select-String '^status-left ') -join ' | ')" }
if ($allA -cmatch '(?m)^status-left "?SL"?\r?$') { Write-Pass "show -A: a session value is unmarked" } else { Write-Fail "show -A status-left line: $(($allA -split "`n" | Select-String '^status-left') -join ' | ')" }
if ($allA -cmatch '(?m)^prefix\* C-a\r?$') { Write-Pass "show -A: inherited prefix is marked prefix*" } else { Write-Fail "show -A prefix line: $(($allA -split "`n" | Select-String '^prefix') -join ' | ')" }
if ($allG -notmatch '(?m)^\S+\*') { Write-Pass "show -g marks nothing" } else { Write-Fail "show -g has a * marker" }
CheckLine "show -A status-left" (P show -A -t pr status-left) "status-left" "SL"
CheckLine "show -A prefix" (P show -A -t pr prefix) "prefix*" "C-a"
CheckLine "show -gA status-left (global table owns it)" (P show -gA -t pr status-left) "status-left" "GL"
CheckLine "show -g status-left" (P show -g -t pr status-left) "status-left" "GL"
P set -u -t pr status-left | Out-Null

# ---------------------------------------------------------------- Part E
Write-Host "`n=== Part E: -o asks the scope it writes ===" -ForegroundColor Cyan
P set -g -t pr status-right GR | Out-Null
$e = P set -o -t pr status-right SR1
Check "set -o with no session entry applies" (S pr status-right) "SR1"
if ($e -eq "") { Write-Pass "set -o with no session entry printed nothing" } else { Write-Fail "set -o said '$e'" }
$e = P set -o -t pr status-right SR2
if ($e -match 'already set: status-right') { Write-Pass "set -o over a session entry: already set" } else { Write-Fail "set -o over a session entry said '$e'" }
Check "set -o refused: session unchanged" (S pr status-right) "SR1"
Check "set -o: global untouched" (G pr status-right) "GR"
P set -u -t pr status-right | Out-Null

# ---------------------------------------------------------------- Part F
Write-Host "`n=== Part F: two sessions in one namespace ===" -ForegroundColor Cyan
& $PSMUX -L $SOCK -f $plain new-session -d -s p2 -x 100 -y 30 2>&1 | Out-Null
Wait-Port p2; Start-Sleep -Milliseconds 800
$p2Before = S p2 status-left
P set -t pr status-left PRONLY | Out-Null
Check "pr session value" (S pr status-left) "PRONLY"
Check "p2 does not see pr's session value" (S p2 status-left) $p2Before
P set -t p2 prefix C-q | Out-Null
Check "p2 session prefix" (S p2 prefix) "C-q"
Check "pr prefix unaffected by p2" (S pr prefix) "C-a"
P set -g -t pr status-right PRGLOBAL | Out-Null
$p2r = G p2 status-right
if ($p2r -cne "PRGLOBAL") { Write-Pass "set -g on pr stays in pr's server (p2 global status-right '$p2r')" } else { Write-Fail "set -g on pr reached p2" }
P set -u -t pr status-left | Out-Null
P kill-server | Out-Null
Start-Sleep -Milliseconds 800

# ---------------------------------------------------------------- Part G / H
Write-Host "`n=== Part G: startup config set without -g; Part H: source-file ===" -ForegroundColor Cyan
$confG = Join-Path $TMP "g.conf"
Write-Ascii $confG "set status-left CONFSL`nset prefix C-a`n"
& $PSMUX -L $SOCK -f $confG new-session -d -s cf -x 100 -y 30 2>&1 | Out-Null
Wait-Port cf; Start-Sleep -Milliseconds 800
Check "startup 'set status-left' lands in the global table" (G cf status-left) "CONFSL"
Check "startup 'set status-left': session inherits it" (S cf status-left) "CONFSL"
Check "startup 'set prefix C-a': global" (G cf prefix) "C-a"
$allA = P show -A -t cf
if ($allA -cmatch '(?m)^status-left\* "?CONFSL"?\r?$') { Write-Pass "show -A marks the startup value inherited" } else { Write-Fail "show -A after startup set: $(($allA -split "`n" | Select-String '^status-left') -join ' | ')" }
P set -u -t cf status-left | Out-Null
Check "set -u keeps the startup value (nothing at session scope)" (S cf status-left) "CONFSL"

$confH = Join-Path $TMP "h.conf"
Write-Ascii $confH "set status-left SRC`nset -g status-right SRCG`n"
P source-file -t cf $confH | Out-Null
Start-Sleep -Milliseconds 300
Check "runtime source-file 'set status-left': session" (S cf status-left) "SRC"
Check "runtime source-file 'set status-left': global kept" (G cf status-left) "CONFSL"
Check "runtime source-file 'set -g status-right': global" (G cf status-right) "SRCG"
P set -u -t cf status-left | Out-Null
Check "set -u after source-file: back to the config global" (S cf status-left) "CONFSL"
P kill-server | Out-Null
Start-Sleep -Milliseconds 800

# ---------------------------------------------------------------- Part I
Write-Host "`n=== Part I: nested F12 toggle with an attached client ===" -ForegroundColor Cyan
$conf = Join-Path $TMP "toggle.conf"
Write-Ascii $conf @"
set -g prefix C-a
set -g status-left "GL"
bind -T root F12 set prefix None \; set key-table off \; set status-left "OFF"
bind -T off  F12 set -u prefix \; set -u key-table \; set -u status-left
"@
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$inj = Join-Path $TMP "keys_803.exe"
Remove-Item $inj -Force -EA SilentlyContinue
& $csc /nologo /optimize /out:$inj (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
if (-not (Test-Path $inj)) { Write-Fail "could not compile tests/injector.cs"; exit 1 }
$S = "tg"
$launch = Join-Path $TMP "launch_803.cmd"
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
Wait-Port $S
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
    function Inject([string]$k, [int]$ms = 1200) { & $inj $cpid $k 2>&1 | Out-Null; Start-Sleep -Milliseconds $ms }
    function Wins { [int](P display-message -p -t $S '#{session_windows}') }
    for ($round = 1; $round -le 2; $round++) {
        Inject "{F12}"
        Check "round $round F12 off: session prefix" (S $S prefix) "None"
        Check "round $round F12 off: session status-left" (S $S status-left) "OFF"
        Check "round $round F12 off: global prefix untouched" (G $S prefix) "C-a"
        Check "round $round F12 off: global status-left untouched" (G $S status-left) "GL"
        Check "round $round F12 off: client table" (P display-message -p -t $S '#{client_key_table}') "off"
        $w0 = Wins
        Inject "^a{SLEEP:300}c"
        $w1 = Wins
        if ($w1 -eq $w0) { Write-Pass "round $round outer off: C-a c made no window" } else { Write-Fail "round $round outer off: C-a c made a window ($w0 -> $w1)" }
        Inject "{ESC}" 300
        Inject "{F12}"
        Check "round $round F12 on: prefix restored from the global" (S $S prefix) "C-a"
        Check "round $round F12 on: status-left restored from the global" (S $S status-left) "GL"
        Check "round $round F12 on: client table" (P display-message -p -t $S '#{client_key_table}') "root"
        $w2 = Wins
        Inject "^a{SLEEP:300}c"
        $w3 = Wins
        if ($w3 -eq $w2 + 1) { Write-Pass "round $round outer on: C-a c made a window" } else { Write-Fail "round $round outer on: C-a c windows $w2 -> $w3" }
    }
    # A -g write while the session overrides the prefix must not re-arm it.
    P set -t $S prefix C-x | Out-Null
    P set -g -t $S prefix C-a | Out-Null
    Start-Sleep -Milliseconds 800
    $w4 = Wins
    Inject "^a{SLEEP:300}c"
    $w5 = Wins
    if ($w5 -eq $w4) { Write-Pass "global C-a under a session C-x: C-a c made no window" } else { Write-Fail "global C-a under session C-x: C-a c made a window" }
    Inject "{ESC}" 300
    Inject "^x{SLEEP:300}c"
    $w6 = Wins
    if ($w6 -eq $w5 + 1) { Write-Pass "session prefix C-x: C-x c made a window" } else { Write-Fail "session prefix C-x: windows $w5 -> $w6" }

    try { Stop-Process -Id $cpid -Force -EA SilentlyContinue } catch {}
}
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
if ($lp -and -not $lp.HasExited) { try { Stop-Process -Id $lp.Id -Force -EA SilentlyContinue } catch {} }

Write-Host "`n=== issue803 results: $script:Pass passed, $script:Fail failed ===" -ForegroundColor Cyan
if ($script:Fail -gt 0) { exit 1 }
exit 0
