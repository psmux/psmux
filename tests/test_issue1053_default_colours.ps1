# Issue #1053: default colours. OSC 10 ; colour, OSC 11 ; colour, OSC 110,
# OSC 111 and OSC 10/11 ; ? from a pane, and how window-style and
# window-active-style take part, as tmux handles them (input_osc_10,
# input_osc_110, input_osc_11 and input_osc_111, input.c:3070-3160;
# tty_default_colours, tty.c:3164; window_pane_get_bg, window.c:2699; all at
# tag 3.8). psmux ignored a program's colours, answered every query with the
# outer terminal's colour, and drew a pane's default cells without the
# program's colours.
#
# Every expectation is what tmux 3.8 did, measured with the same bytes, except
# where a comment says otherwise. The inbox conhost passes on the colours a
# program sets but drops a bare OSC 110 and OSC 111 (PSMUX_PANE_RAW), so
# there a program's colours are not taken, as they would outlive the program.
# What it does with a query differs by build: Windows 11's (26300) drops it,
# so no reply comes, and Windows 10's (19045) passes it on when it next
# draws, so psmux answers with the window styles or the outer terminal's
# colour. Each case's bytes end with text so that the host draws.
#
# tests/reply_order_probe.cs runs in the pane with `raw`: it writes a case's
# bytes in one write and records the replies. tests/pipe_client_output_log.cs
# plays the client's terminal on the pipe route and keeps everything the
# client draws, so a case can look at what was drawn after it started.
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_dcol_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$probe = Join-Path $TempHome 'reply_order_probe.exe'
$TermExe = Join-Path $TempHome 'pipe_client_output_log.exe'
if ($csc) {
    & $csc.FullName /nologo /optimize /out:$probe (Join-Path $PSScriptRoot 'reply_order_probe.cs') 2>&1 | Out-Null
    & $csc.FullName /nologo /optimize /out:$TermExe (Join-Path $PSScriptRoot 'pipe_client_output_log.cs') 2>&1 | Out-Null
}
if (-not (Test-Path $probe) -or -not (Test-Path $TermExe)) {
    Write-Fail "build tests\reply_order_probe.cs and pipe_client_output_log.cs (csc.exe unavailable)"
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
    exit $script:TestsFailed
}

$A = '\a'; $ST = '\e\'
$Red = "\e]11;#ff0000$ST"
# What a reply with no colour of the pane's own is: the outer terminal's
# colour as the client reported it, unchanged by this suite.
# The client's report here is psmux's fallback, Windows Terminal's Campbell.
$HostFg = 'cccc/cccc/cccc'
$HostBg = '0c0c/0c0c/0c0c'
# The inbox conhost before Windows 11 passes OSC 10 ; ? and OSC 11 ; ? on to
# psmux (measured on 19045); the one on 26300 drops them.
$InboxForwardsQueries = [Environment]::OSVersion.Version.Build -lt 22000
function Rgb($n, $rgb, $end) { "\e]$n;rgb:$rgb$end" }
$script:NS = ""
$script:Term = $null
$script:Log = ""
$script:run = 0
function P { & $PSMUX -L $script:NS @args 2>&1 }
function LogLength { if (Test-Path $script:Log) { (Get-Item $script:Log).Length } else { 0 } }
# What the client drew from byte $from of its log on, as text.
function DrawnSince($from) {
    if (-not (Test-Path $script:Log)) { return '' }
    $fs = [System.IO.File]::Open($script:Log, 'Open', 'Read', 'ReadWrite')
    try {
        $fs.Position = [Math]::Min($from, $fs.Length)
        $buf = New-Object byte[] ($fs.Length - $fs.Position)
        [void]$fs.Read($buf, 0, $buf.Length)
        [System.Text.Encoding]::UTF8.GetString($buf)
    } finally { $fs.Close() }
}
# Write $q from a pane before the case's own query.
function Probe($target, $q) {
    $f = Join-Path $TempHome "pre_$($script:NS)_$($script:run).txt"
    P send-keys -t $target "& '$probe' '$f' '$q.' raw" Enter | Out-Null
    for ($k = 0; $k -lt 60 -and -not (Test-Path $f); $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 300
}

# Run $c.q from a new window, the active one (or from $c.pane in it), after
# $c.setup, and check the reply and, with $c.drawn, whether the client drew
# that SGR after the case started. A field given as @{ inbox = ...; oc = ... }
# differs by route, with `win10` for the inbox conhost before Windows 11 when
# it differs there; a plain value is all of them. The bytes end with a `.`.
function Case($route, $c) {
    $script:run++
    $window = "w$($script:run)"
    P new-window -t s -n $window | Out-Null
    Start-Sleep -Milliseconds 2200
    $target = if ($null -ne $c.pane) { "s:$window.$($c.pane)" } else { "s:$window" }
    if ($c.setup) { & $c.setup $window; Start-Sleep -Milliseconds 400 }
    $from = LogLength
    $out = Join-Path $TempHome "r_$($script:NS)_$($script:run).txt"
    P send-keys -t $target "& '$probe' '$out' '$($c.q).' raw" Enter | Out-Null
    for ($k = 0; $k -lt 60 -and -not (Test-Path $out); $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 800
    $key = if ($route -eq 'OpenConsole') { 'oc' } elseif ($InboxForwardsQueries) { 'win10' } else { 'inbox' }
    $pick = { param($v) if ($v -is [hashtable]) { if ($v.ContainsKey($key)) { $v[$key] } else { $v['inbox'] } } else { $v } }
    $got = if (Test-Path $out) { (Get-Content $out -Raw).Trim() } else { '(no reply file)' }
    Check "${route}: $($c.n), reply" $got (& $pick $c.reply)
    if ($c.drawn) {
        $sgr = $c.drawn
        $want = & $pick $c.shown
        Check "${route}: $($c.n), drew $sgr" ((DrawnSince $from).Contains("`e[$sgr")) $want
    }
    if ($c.cleanup) { & $c.cleanup $window; Start-Sleep -Milliseconds 400 }
}

# A reply through OpenConsole, and through the Windows 10 inbox conhost,
# which takes no colour from the program, so answers with the window styles
# or the outer terminal's colour. Windows 11's inbox conhost sends no reply.
$by = { param($oc, $win10) @{ inbox = ''; win10 = $win10; oc = $oc } }
$bg = { param($end) Rgb 11 $HostBg $end }
$cases = @(
    @{ n = 'nothing set'; q = "\e]11;?$ST"; reply = (& $by (& $bg $ST) (& $bg $ST)) }
    @{ n = 'a background set, then asked'; q = "$Red\e]11;?$ST"; reply = (& $by (Rgb 11 'ffff/0000/0000' $ST) (& $bg $ST)); drawn = '48;2;255;0;0m'; shown = @{ inbox = $false; oc = $true } }
    @{ n = 'a new window has none of it'; q = "\e]11;?$A"; reply = (& $by (& $bg $A) (& $bg $A)) }
    @{ n = 'a foreground asked with BEL'; q = "\e]10;#00ff00$A\e]10;?$A"; reply = (& $by (Rgb 10 '0000/ffff/0000' $A) (Rgb 10 $HostFg $A)); drawn = '38;2;0;255;0m'; shown = @{ inbox = $false; oc = $true } }
    @{ n = 'an rgb: colour'; q = "\e]11;rgb:12/34/56$ST\e]11;?$ST"; reply = (& $by (Rgb 11 '1212/3434/5656' $ST) (& $bg $ST)) }
    @{ n = 'reset by OSC 111'; q = "$Red\e]111$ST\e]11;?$ST"; reply = (& $by (& $bg $ST) (& $bg $ST)) }
    @{ n = 'OSC 111 with something after it'; q = "$Red\e]111;x$ST\e]11;?$ST"; reply = (& $by (Rgb 11 'ffff/0000/0000' $ST) (& $bg $ST)) }
    @{ n = 'reset by OSC 110 with an empty argument'; q = "\e]10;#ff0000$ST\e]110;$ST\e]10;?$ST"; reply = (& $by (Rgb 10 $HostFg $ST) (Rgb 10 $HostFg $ST)) }
    @{ n = 'RIS takes it back'; q = "$Red\ec\e]11;?$ST"; reply = (& $by (& $bg $ST) (& $bg $ST)) }
    @{ n = 'the alternate screen keeps it'; q = "$Red\e[?1049h\e]11;?$ST\e[?1049l"; reply = (& $by (Rgb 11 'ffff/0000/0000' $ST) (& $bg $ST)) }
    # psmux: tmux takes red; psmux has no table of X11 names.
    @{ n = 'an X11 name'; q = "\e]11;red$ST\e]11;?$ST"; reply = (& $by (& $bg $ST) (& $bg $ST)) }
    # psmux's clear-history starts the pane's screen again; tmux clears only
    # the history (cmd-capture-pane.c:424) and keeps the colours.
    @{ n = 'clear-history keeps it'; setup = { param($w) Probe "s:$w" $Red; P clear-history | Out-Null }; q = "\e]11;?$ST"; reply = (& $by (Rgb 11 'ffff/0000/0000' $ST) (& $bg $ST)) }
    @{ n = 'window-style alone'; setup = { param($w) P set -g window-style bg=green | Out-Null }; q = "\e]11;?$ST"; reply = (& $by (Rgb 11 '0000/8080/0000' $ST) (Rgb 11 '0000/8080/0000' $ST)) }
    @{ n = 'the program over window-style on the active pane'; q = "$Red\e]11;?$ST"; reply = (& $by (Rgb 11 'ffff/0000/0000' $ST) (Rgb 11 '0000/8080/0000' $ST)) }
    @{ n = 'window-style over the program on another pane'; setup = { param($w) P split-window -t "s:$w" | Out-Null; Start-Sleep -Milliseconds 2200 }; pane = 0; q = "$Red\e]11;?$ST"; reply = (& $by (Rgb 11 '0000/8080/0000' $ST) (Rgb 11 '0000/8080/0000' $ST)) }
    @{ n = 'window-active-style over the program'; setup = { param($w) P set -g window-active-style bg=blue | Out-Null }; q = "$Red\e]11;?$ST"; reply = (& $by (Rgb 11 '0000/0000/8080' $ST) (Rgb 11 '0000/0000/8080' $ST)); cleanup = { param($w) P set -gu window-active-style | Out-Null; P set -gu window-style | Out-Null } }
    @{ n = 'window-style fg'; setup = { param($w) P set -g window-style fg=colour196 | Out-Null }; q = "\e]10;?$ST"; reply = (& $by (Rgb 10 'ffff/0000/0000' $ST) (Rgb 10 'ffff/0000/0000' $ST)); cleanup = { param($w) P set -gu window-style | Out-Null } }
    @{ n = 'no style left'; q = "\e]11;?$ST"; reply = (& $by (& $bg $ST) (& $bg $ST)) }
)

function Run-Cases($route) {
    $script:NS = "dc_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    $script:run = 0
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500
    $script:Log = Join-Path $TempHome "outer_$($script:NS).bin"
    $script:Term = Start-Process -FilePath $TermExe -PassThru -WindowStyle Hidden -ArgumentList @($script:Log, $PSMUX, '-L', $script:NS, 'attach', '-t', 's')
    for ($i = 0; $i -lt 50; $i++) {
        if ("$(P list-clients)" -match 'attached') { break }
        Start-Sleep -Milliseconds 200
    }
    Start-Sleep -Milliseconds 800
    Check "${route}: client attached" ("$(P list-clients)" -match 'attached') $true

    foreach ($c in $cases) { Case $route $c }

    if ($script:Term -and -not $script:Term.HasExited) { & taskkill /PID $script:Term.Id /T /F 2>&1 | Out-Null }
    $script:Term = $null
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== Default colours ===" -ForegroundColor Cyan
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
