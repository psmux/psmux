# Issue #1043: cursor colour, OSC 12 ; colour, OSC 112 and OSC 12 ; ? from a pane, the
# cursor-colour option, #{cursor_colour}, and what the client writes to its
# terminal, as tmux handles them (input_osc_12 and input_osc_112, input.c:3164
# and 3189; screen_set_default_cursor, screen.c:206; format_cb_cursor_colour,
# format.c:1296; tty_force_cursor_colour, tty.c:770, and tty_stop_tty, tty.c:495,
# all at tag 3.8). psmux ignored all of them: a query got no reply, the
# option was kept as text, and the terminal never heard of a colour.
#
# Every expectation is what tmux 3.8 did, measured with the same bytes, except
# where a comment says otherwise. The inbox conhost passes on the colour a
# program sets but drops OSC 112 and the query (pipe-pane -o), so there a
# program's colour is not taken, as it would outlive the program; the option
# works on both console hosts.
#
# tests/reply_order_probe.cs runs in the pane with `raw`: it writes a case's
# bytes in one write and records the replies. tests/pipe_client_cursor_colour_log.cs
# plays the client's terminal on the pipe route and logs every OSC 12 and
# OSC 112 the client writes.
#   1. the inbox conhost.
#   2. OpenConsole, when PSMUX_TEST_CONPTY_DIR or PSMUX_CONPTY_DIR_FOR_TEST
#      names a folder holding conpty.dll, skipped otherwise.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per route;
# cleanup is `-L <ns> kill-server` and the logger this suite started.
# PSMUX_TEST_BIN or PSMUX_EXE picks the binary (defaults to
# target\release\psmux.exe).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }
$ConptyDir = if ($env:PSMUX_TEST_CONPTY_DIR) { $env:PSMUX_TEST_CONPTY_DIR } elseif ($env:PSMUX_CONPTY_DIR_FOR_TEST) { $env:PSMUX_CONPTY_DIR_FOR_TEST } else { '' }
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
# The panes run pwsh commands; SHELL would make psmux start them in bash.
Remove-Item Env:SHELL -EA SilentlyContinue
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_ccol_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$probe = Join-Path $TempHome 'reply_order_probe.exe'
$TermExe = Join-Path $TempHome 'pipe_client_cursor_colour_log.exe'
if ($csc) {
    & $csc.FullName /nologo /optimize /out:$probe (Join-Path $PSScriptRoot 'reply_order_probe.cs') 2>&1 | Out-Null
    & $csc.FullName /nologo /optimize /out:$TermExe (Join-Path $PSScriptRoot 'pipe_client_cursor_colour_log.cs') 2>&1 | Out-Null
}
if (-not (Test-Path $probe) -or -not (Test-Path $TermExe)) {
    Write-Fail "build tests\reply_order_probe.cs and pipe_client_cursor_colour_log.cs (csc.exe unavailable)"
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
    exit $script:TestsFailed
}

$A = '\a'; $ST = '\e\'
$Red = "\e]12;#ff0000$ST"
$script:NS = ""
$script:Term = $null
$script:Log = ""
$script:run = 0
function P { & $PSMUX -L $script:NS @args 2>&1 }
function Logged { if ($script:Log -and (Test-Path $script:Log)) { @(Get-Content $script:Log | Where-Object { $_ }) } else { @() } }
# What the terminal was last told: the last OSC 12 or OSC 112 in the log.
function Told { $l = @(Logged | Where-Object { $_ -ne 'exit' }); if ($l.Count) { $l[$l.Count - 1] } else { '(nothing)' } }
function Tell($colour) { "\e]12;rgb:$colour\a" }
$Reset = '\e]112\a'
# Write $q from the window's pane before the case's own query.
function Probe($w, $q) {
    $f = Join-Path $TempHome "pre_$($script:NS)_$($script:run).txt"
    P send-keys -t "s:$w" "& '$probe' '$f' '$q' raw" Enter | Out-Null
    for ($k = 0; $k -lt 60 -and -not (Test-Path $f); $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 300
}

# Run $c.q from a new window, the active one, after $c.setup, and check the
# reply, #{cursor_colour} and what the terminal was last told. A field given
# as @{ inbox = ...; oc = ... } differs by route; a plain value is both.
function Case($route, $c) {
    $script:run++
    $window = "w$($script:run)"
    P new-window -t s -n $window | Out-Null
    Start-Sleep -Milliseconds 2200
    if ($c.setup) { & $c.setup $window; Start-Sleep -Milliseconds 400 }
    $out = Join-Path $TempHome "r_$($script:NS)_$($script:run).txt"
    P send-keys -t "s:$window" "& '$probe' '$out' '$($c.q)' raw" Enter | Out-Null
    for ($k = 0; $k -lt 60 -and -not (Test-Path $out); $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 500
    $key = if ($route -eq 'OpenConsole') { 'oc' } else { 'inbox' }
    $pick = { param($v) if ($v -is [hashtable]) { $v[$key] } else { $v } }
    $got = if (Test-Path $out) { (Get-Content $out -Raw).Trim() } else { '(no reply file)' }
    Check "${route}: $($c.n), reply" $got (& $pick $c.reply)
    Check "${route}: $($c.n), #{cursor_colour}" "$(P display -p -t "s:$window" '#{cursor_colour}')" (& $pick $c.fmt)
    Check "${route}: $($c.n), the terminal" (Told) (& $pick $c.told)
    if ($c.cleanup) { & $c.cleanup $window; Start-Sleep -Milliseconds 400 }
}

$oc = { param($v) @{ inbox = ''; oc = $v } }
$cases = @(
    @{ n = 'nothing set'; q = "\e]12;?$ST"; reply = ''; fmt = 'none'; told = '(nothing)' }
    @{ n = 'a colour set, then asked'; q = "$Red\e]12;?$ST"; reply = (& $oc "\e]12;rgb:ffff/0000/0000$ST"); fmt = @{ inbox = 'none'; oc = '#ff0000' }; told = @{ inbox = '(nothing)'; oc = (Tell 'ff/00/00') } }
    @{ n = 'a new window has none of it'; q = "\e]12;?$A"; reply = ''; fmt = 'none'; told = @{ inbox = '(nothing)'; oc = $Reset } }
    @{ n = 'asked with BEL'; q = "\e]12;#00ff00$A\e]12;?$A"; reply = (& $oc "\e]12;rgb:0000/ffff/0000$A"); fmt = @{ inbox = 'none'; oc = '#00ff00' }; told = @{ inbox = '(nothing)'; oc = (Tell '00/ff/00') } }
    @{ n = 'an rgb: colour'; q = "\e]12;rgb:12/34/56$ST\e]12;?$ST"; reply = (& $oc "\e]12;rgb:1212/3434/5656$ST"); fmt = @{ inbox = 'none'; oc = '#123456' }; told = @{ inbox = '(nothing)'; oc = (Tell '12/34/56') } }
    @{ n = 'reset by OSC 112'; q = "$Red\e]112$ST\e]12;?$ST"; reply = ''; fmt = 'none'; told = @{ inbox = '(nothing)'; oc = $Reset } }
    @{ n = 'OSC 112 with something after it'; q = "$Red\e]112;x$ST\e]12;?$ST"; reply = (& $oc "\e]12;rgb:ffff/0000/0000$ST"); fmt = @{ inbox = 'none'; oc = '#ff0000' }; told = @{ inbox = '(nothing)'; oc = (Tell 'ff/00/00') } }
    @{ n = 'RIS keeps it'; q = "$Red\ec\e]12;?$ST"; reply = (& $oc "\e]12;rgb:ffff/0000/0000$ST"); fmt = @{ inbox = 'none'; oc = '#ff0000' }; told = @{ inbox = '(nothing)'; oc = (Tell 'ff/00/00') } }
    # psmux: tmux answers rgb:ffff/0000/0000; psmux has no table of X11 names.
    @{ n = 'an X11 name'; q = "\e]12;red$ST\e]12;?$ST"; reply = ''; fmt = 'none'; told = @{ inbox = '(nothing)'; oc = $Reset } }
    @{ n = 'the option'; setup = { param($w) P set -g cursor-colour blue | Out-Null }; q = "\e]12;?$ST"; reply = (& $oc "\e]12;rgb:0000/0000/8080$ST"); fmt = 'blue'; told = (Tell '00/00/80') }
    @{ n = 'a program colour over the option'; q = "$Red\e]12;?$ST"; reply = (& $oc "\e]12;rgb:ffff/0000/0000$ST"); fmt = @{ inbox = 'blue'; oc = '#ff0000' }; told = @{ inbox = (Tell '00/00/80'); oc = (Tell 'ff/00/00') } }
    @{ n = 'back to the option by OSC 112'; q = "$Red\e]112$ST\e]12;?$ST"; reply = (& $oc "\e]12;rgb:0000/0000/8080$ST"); fmt = 'blue'; told = (Tell '00/00/80') }
    # psmux's clear-history starts the pane's screen again; tmux clears only
    # the history (cmd-capture-pane.c:424) and keeps both colours.
    @{ n = 'clear-history keeps the option colour'; setup = { param($w) P clear-history | Out-Null }; q = "\e]12;?$ST"; reply = (& $oc "\e]12;rgb:0000/0000/8080$ST"); fmt = 'blue'; told = (Tell '00/00/80') }
    @{ n = 'clear-history keeps the program colour'; setup = { param($w) Probe $w $Red; P clear-history | Out-Null }; q = "\e]12;?$ST"; reply = (& $oc "\e]12;rgb:ffff/0000/0000$ST"); fmt = @{ inbox = 'blue'; oc = '#ff0000' }; told = @{ inbox = (Tell '00/00/80'); oc = (Tell 'ff/00/00') } }
    @{ n = 'a number names its colour'; setup = { param($w) P set -w -t "s:$w" cursor-colour 3 | Out-Null }; q = "\e]12;?$ST"; reply = (& $oc "\e]12;rgb:8080/8080/0000$ST"); fmt = 'yellow'; told = (Tell '80/80/00') }
    @{ n = 'a pane value over the window and the global'; setup = { param($w) P set -w -t "s:$w" cursor-colour green | Out-Null; P set -p -t "s:$w" cursor-colour '#abcdef' | Out-Null }; q = "\e]12;?$ST"; reply = (& $oc "\e]12;rgb:abab/cdcd/efef$ST"); fmt = '#abcdef'; told = (Tell 'ab/cd/ef') }
    @{ n = 'the global unset'; setup = { param($w) P set -gu cursor-colour | Out-Null }; q = "\e]12;?$ST"; reply = ''; fmt = 'none'; told = $Reset }
)

function Run-Cases($route) {
    $script:NS = "cc_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $script:run = 0
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500
    $script:Log = Join-Path $TempHome "outer_$($script:NS).txt"
    $script:Term = Start-Process -FilePath $TermExe -PassThru -WindowStyle Hidden -ArgumentList @($script:Log, $PSMUX, '-L', $script:NS, 'attach', '-t', 's')
    for ($i = 0; $i -lt 50; $i++) {
        if ("$(P list-clients)" -match 'attached') { break }
        Start-Sleep -Milliseconds 200
    }
    Start-Sleep -Milliseconds 800
    Check "${route}: client attached" ("$(P list-clients)" -match 'attached') $true

    foreach ($c in $cases) { Case $route $c }

    Write-Host "  -- the option's values" -ForegroundColor DarkCyan
    # Measured: tmux refuses `bad` with `invalid colour: bad` and keeps ''.
    Check "${route}: a bad colour is refused" ("$(P set -g cursor-colour bad)" -like '*invalid colour: bad') $true
    Check "${route}: and nothing is kept" "$(P show -gv cursor-colour)" ''
    Check "${route}: an empty value is taken" "$(P set -g cursor-colour '')" ''

    Write-Host "  -- detach" -ForegroundColor DarkCyan
    P set -g cursor-colour '#102030' | Out-Null
    Start-Sleep -Milliseconds 1200
    Check "${route}: the colour before detaching" (Told) (Tell '10/20/30')
    P detach-client | Out-Null
    for ($i = 0; $i -lt 50 -and -not ((Logged) -contains 'exit'); $i++) { Start-Sleep -Milliseconds 200 }
    $l = @(Logged)
    Check "${route}: the client exited" ($l -contains 'exit') $true
    Check "${route}: the terminal's own colour back on the way out" ($l | Where-Object { $_ -ne 'exit' } | Select-Object -Last 1) $Reset

    if ($script:Term -and -not $script:Term.HasExited) { & taskkill /PID $script:Term.Id /T /F 2>&1 | Out-Null }
    $script:Term = $null
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== Issue #1043: cursor colour ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((& $PSMUX -V) -join ' ')"

try {
    Write-Host "`n[1] the inbox conhost" -ForegroundColor Yellow
    Run-Cases 'inbox conhost'

    Write-Host "`n[2] OpenConsole" -ForegroundColor Yellow
    if ($ConptyDir -and (Test-Path (Join-Path $ConptyDir 'conpty.dll'))) {
        $env:PSMUX_CONPTY_DIR = $ConptyDir
        Run-Cases 'OpenConsole'
        Remove-Item Env:PSMUX_CONPTY_DIR
    } else {
        Write-Info "skipped: set PSMUX_TEST_CONPTY_DIR to a folder holding conpty.dll and OpenConsole.exe"
    }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    if ($script:Term -and -not $script:Term.HasExited) { & taskkill /PID $script:Term.Id /T /F 2>&1 | Out-Null }
    if ($script:NS) { P kill-server | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
