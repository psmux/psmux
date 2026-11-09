# Issue #1028: the progress indicator, terminal-features, Spb, the formats and what a client writes.
#
# tmux keeps OSC 9;4 per pane (screen_set_progress_bar, screen.c:344 at tag 3.8),
# shows it in #{pane_pb_state} and #{pane_pb_progress} (format.c:2612, :2623), and
# writes the active pane's through the Spb capability when it changes
# (server-client.c:2659, tty.c:3219). Spb comes from the progressbar feature
# (tty-features.c:373), which terminal-features turns on (tty-term.c:619), and
# terminal-overrides applies on top of it (tty-term.c:447).
#
# Every expected value was measured from tmux 3.8 under script(1). Two differ on
# purpose: psmux's terminal-features starts with *:progressbar as well, so a
# default config writes, and an option set while attached counts from the next
# write, where tmux reads both options at attach.
#   1. the inbox console host
#   2. OpenConsole, with PSMUX_CONPTY_DIR_FOR_TEST (a directory with conpty.dll
#      and OpenConsole.exe; skipped without it)
#
# tests/reply_order_probe.cs writes the sequences in one write from the pane.
# tests/pipe_client_progress_log.cs plays the outer terminal for a pipe route
# client and logs each OSC 9 ; 4 the client writes.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per case;
# cleanup is `-L <ns> kill-server` and the logger this suite started.
# PSMUX_TEST_BIN or PSMUX_EXE picks the binary (defaults to
# target\release\psmux.exe).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }
$ConptyDir = $env:PSMUX_CONPTY_DIR_FOR_TEST
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
# The panes run pwsh commands; SHELL would make psmux start them in bash.
Remove-Item Env:SHELL -EA SilentlyContinue
# Patterns are matched against the client's TERM, which each case sets itself.
# A pipe route client with no TERM takes xterm-256color (main.rs, for crossterm),
# so an unset TERM, which a console route client matches as empty, is left to
# tests-rs/test_progress_bar_features.rs.
Remove-Item Env:TERM -EA SilentlyContinue
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_pbf_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$probe = Join-Path $TempHome 'reply_order_probe.exe'
$TermExe = Join-Path $TempHome 'pipe_client_progress_log.exe'
if ($csc) {
    & $csc.FullName /nologo /optimize /out:$probe (Join-Path $PSScriptRoot 'reply_order_probe.cs') 2>&1 | Out-Null
    & $csc.FullName /nologo /optimize /out:$TermExe (Join-Path $PSScriptRoot 'pipe_client_progress_log.cs') 2>&1 | Out-Null
}
if (-not (Test-Path $probe) -or -not (Test-Path $TermExe)) {
    Write-Fail "build tests\reply_order_probe.cs and pipe_client_progress_log.cs (csc.exe unavailable)"
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
    exit $script:TestsFailed
}

$ST = '\e\'
$BEL = '\a'
$script:NS = ""
$script:Term = $null
$script:Log = ""
$script:run = 0
function P { & $PSMUX -L $script:NS @args 2>&1 }

# One whole dump-state reply.
function Dump {
    $portFile = Get-ChildItem $TempHome -Filter "$($script:NS)*.port" -Recurse -EA SilentlyContinue | Select-Object -First 1
    if (-not $portFile) { return '' }
    $port = (Get-Content $portFile.FullName -Raw).Trim()
    $key = (Get-Content ($portFile.FullName -replace '\.port$', '.key') -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $stream = $tcp.GetStream(); $stream.ReadTimeout = 3000
    $w = [IO.StreamWriter]::new($stream); $r = [IO.StreamReader]::new($stream)
    $w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
    $w.Write("dump-state`n"); $w.Flush()
    $line = ''
    try { while ($true) { $line = $r.ReadLine(); if ($null -eq $line -or $line.EndsWith('}')) { break } } } catch {}
    $tcp.Close()
    $line
}

# host_progress in dump-state, or '(none)'.
function Held { if ((Dump) -match '"host_progress"\s*:\s*"([^"]*)"') { $Matches[1] } else { '(none)' } }

# Everything the client has written to its terminal, joined.
function Logged {
    if ($script:Log -and (Test-Path $script:Log)) { (@(Get-Content $script:Log | Where-Object { $_ }) -join ' ') } else { '' }
}
function Bar($pair, $end = $ST) { "\e]9;4;$pair$end" }
function Pane($target, $format = '#{pane_pb_state} #{pane_pb_progress}') { "$(P display-message -p -t $target $format)" }

# A server with this config, and a client on the pipe route with this TERM.
function Up($config = @(), $term = $null) {
    $script:NS = "pbf_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ($config -join "`n")
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500
    $script:Log = Join-Path $TempHome "outer_$($script:NS).txt"
    if ($term) { $env:TERM = $term }
    $script:Term = Start-Process -FilePath $TermExe -PassThru -WindowStyle Hidden -ArgumentList @($script:Log, $PSMUX, '-L', $script:NS, 'attach', '-t', 's')
    Remove-Item Env:TERM -EA SilentlyContinue
    for ($k = 0; $k -lt 50; $k++) {
        if ("$(P list-clients)" -match 'attached') { break }
        Start-Sleep -Milliseconds 200
    }
    Start-Sleep -Milliseconds 800
}
function Down {
    if ($script:Term -and -not $script:Term.HasExited) { & taskkill /PID $script:Term.Id /T /F 2>&1 | Out-Null }
    $script:Term = $null
    if ($script:NS) { P kill-server | Out-Null }
    $script:NS = ""
    Start-Sleep -Milliseconds 500
}

# Write $queries from the pane, and let a frame carry the result. $wait is run
# by the pane's shell first, for a pane that goes on to do something else.
function Emit($target, $queries, $wait = '') {
    $script:run++
    $out = Join-Path $TempHome "r_$($script:NS)_$($script:run).txt"
    P send-keys -t $target "$wait& '$probe' '$out' '$queries' raw" Enter | Out-Null
    if ($wait) { return }
    for ($k = 0; $k -lt 60 -and -not (Test-Path $out); $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 700
}
function Set-Bar($target, $pair) { Emit $target "\e]9;4;$pair$BEL" }
function New-Pane($how) { P @how | Out-Null; Start-Sleep -Milliseconds 2200 }

# A config, the client's TERM, what 1;50 from the pane writes, and whether
# #{client_termfeatures} lists the feature.
function Option-Case($route, $name, $config, $term, $written, $listed) {
    Up $config $term
    Set-Bar s '1;50'
    Check "${route}: $name, held" (Held) '1;50'
    Check "${route}: $name, written" (Logged) $written
    $features = "$(P display-message -p '#{client_termfeatures}')"
    Check "${route}: $name, listed" ($features -match '(^|,)progressbar($|,)') $listed
    Down
}

function Run-Cases($route) {
    $wrote = Bar '1;50'
    $wroteBel = Bar '1;50' $BEL

    Write-Host "  -- terminal-features" -ForegroundColor DarkCyan
    Option-Case $route 'the default' @() $null $wrote $true
    Option-Case $route 'the default, another TERM' @() 'vt100' $wrote $true
    Option-Case $route 'removed' @("set -as terminal-features '*:progressbar@'") $null '' $false
    Option-Case $route 'removed, then added' @("set -as terminal-features '*:progressbar@'", "set -as terminal-features '*:progressbar'") $null '' $false
    Option-Case $route 'the whole list replaced' @("set -g terminal-features 'xterm*:title'") $null '' $false
    Option-Case $route 'a pattern matching the client' @("set -g terminal-features 'xterm*:progressbar'") 'xterm-256color' $wrote $true
    Option-Case $route 'a pattern not matching the client' @("set -g terminal-features 'xterm*:progressbar'") 'vt100' '' $false
    Option-Case $route 'a name in capitals' @("set -g terminal-features '*:PROGRESSBAR'") $null $wrote $true
    Option-Case $route 'after a name tmux does not know' @("set -g terminal-features '*:bogus:progressbar'") $null '' $false
    Option-Case $route 'before a name tmux does not know' @("set -g terminal-features '*:progressbar:bogus'") $null $wrote $true

    Write-Host "  -- terminal-overrides" -ForegroundColor DarkCyan
    Option-Case $route 'Spb@' @("set -as terminal-overrides '*:Spb@'") $null '' $true
    Option-Case $route 'Spb= with nothing' @("set -as terminal-overrides '*:Spb='") $null '' $true
    Option-Case $route 'Spb@ for another TERM' @("set -as terminal-overrides 'nomatch*:Spb@'") $null $wrote $true
    Option-Case $route 'Spb ending in BEL' @("set -as terminal-overrides '*:Spb=\E]9;4;%p1%d;%p2%d\a'") $null $wroteBel $true
    Option-Case $route 'Spb with the feature removed' @("set -as terminal-features '*:progressbar@'", "set -as terminal-overrides '*:Spb=\E]9;4;%p1%d;%p2%d\a'") $null $wroteBel $false
    Option-Case $route 'Spb with no second parameter' @("set -as terminal-overrides '*:Spb=\E]9;4;%p1%d\a'") $null '' $true

    Write-Host "  -- attaching" -ForegroundColor DarkCyan
    Up
    Check "${route}: nothing written at attach" (Logged) ''
    Check "${route}: a new pane reads as hidden" (Pane s) 'hidden 0'
    Check "${route}: display -a lists the value" ((P display-message -p -a) -match '^pane_pb_progress=0$') 'pane_pb_progress=0'
    Check "${route}: display -a lists the state" ((P display-message -p -a) -match '^pane_pb_state=hidden$') 'pane_pb_state=hidden'
    Down

    Write-Host "  -- the formats follow the pane" -ForegroundColor DarkCyan
    Up
    foreach ($case in @(
        @('0;5', 'hidden 5', '0;5'), @('1;15', 'normal 15', '1;15'), @('2;25', 'error 25', '2;25'),
        @('3;35', 'indeterminate 25', '3;25'), @('4;45', 'paused 45', '4;45')
    )) {
        Set-Bar s $case[0]
        Check "${route}: $($case[0]) in the formats" (Pane s) $case[1]
        Check "${route}: $($case[0]) as held" (Held) $case[2]
    }
    Check "${route}: every change written" (Logged) (@('0;5', '1;15', '2;25', '3;25', '4;45' | ForEach-Object { Bar $_ }) -join ' ')
    Down

    Write-Host "  -- switching panes and windows" -ForegroundColor DarkCyan
    Up
    New-Pane @('split-window', '-t', 's')
    New-Pane @('new-window', '-t', 's')
    P select-window -t s:0 | Out-Null
    Start-Sleep -Milliseconds 500
    Set-Bar s:0.0 '1;50'
    Check "${route}: a bar in an inactive pane is not written" (Logged) ''
    Check "${route}: that pane holds it" (Pane s:0.0) 'normal 50'
    Check "${route}: the active pane does not" (Pane s:0.1) 'hidden 0'
    $seen = @()
    foreach ($step in @(
        @('select-pane', 's:0.0', '1;50', 'select the pane'),
        @('select-pane', 's:0.1', '0;0', 'select a pane with none'),
        @('select-pane', 's:0.0', '1;50', 'select it again'),
        @('select-window', 's:1', '0;0', 'select a window with none'),
        @('select-window', 's:0', '1;50', 'select the window again')
    )) {
        P $step[0] -t $step[1] | Out-Null
        Start-Sleep -Milliseconds 900
        $seen += (Bar $step[2])
        Check "${route}: $($step[3])" (Logged) ($seen -join ' ')
    }
    Set-Bar s:0.0 '0'
    $seen += (Bar '0;50')
    Check "${route}: a hide with no value keeps it" (Logged) ($seen -join ' ')
    Down

    Write-Host "  -- the status line" -ForegroundColor DarkCyan
    Up @("set -g window-status-format 'WSF#{window_index}[#{pane_pb_state}:#{pane_pb_progress}]'")
    New-Pane @('new-window', '-t', 's')
    Set-Bar s:0.0 '1;33'
    $tab = if ((Dump) -match 'WSF0\[[^\]]*\]') { $Matches[0] } else { '(none)' }
    Check "${route}: a bar in another window reaches its status" $tab 'WSF0[normal:33]'
    Check "${route}: and is not written" (Logged) ''
    Down

    Write-Host "  -- copy mode" -ForegroundColor DarkCyan
    Up
    Emit s "\e]9;4;2;60$BEL" 'Start-Sleep 2; '
    Start-Sleep -Milliseconds 600
    P copy-mode -t s | Out-Null
    Start-Sleep -Milliseconds 2800
    Check "${route}: in copy mode" (Pane s '#{pane_in_mode}') '1'
    Check "${route}: a bar set meanwhile reaches the formats" (Pane s) 'error 60'
    Check "${route}: and the terminal" (Logged) (Bar '2;60')
    Down

    Write-Host "  -- a dead pane" -ForegroundColor DarkCyan
    Up @('set -g remain-on-exit on')
    Set-Bar s '2;70'
    P send-keys -t s exit Enter | Out-Null
    Start-Sleep -Milliseconds 1800
    Check "${route}: it keeps the bar" (Pane s '#{pane_dead} #{pane_pb_state} #{pane_pb_progress}') '1 error 70'
    Check "${route}: and writes nothing new" (Logged) (Bar '2;70')
    P respawn-pane -t s | Out-Null
    Start-Sleep -Milliseconds 2500
    Check "${route}: a respawn clears it" (Pane s '#{pane_dead} #{pane_pb_state} #{pane_pb_progress}') '0 hidden 0'
    Check "${route}: and hides it" (Logged) "$(Bar '2;70') $(Bar '0;0')"
    Down

    Write-Host "  -- detaching" -ForegroundColor DarkCyan
    Up
    Set-Bar s '1;50'
    P detach-client -s s | Out-Null
    Start-Sleep -Milliseconds 900
    Check "${route}: a detach writes nothing" (Logged) (Bar '1;50')
    Down

    Write-Host "  -- set while attached, which counts from the next write" -ForegroundColor DarkCyan
    Up
    Set-Bar s '1;10'
    P set -as terminal-features '*:progressbar@' | Out-Null
    Start-Sleep -Milliseconds 400
    Set-Bar s '1;20'
    Check "${route}: removed" (Logged) (Bar '1;10')
    Check "${route}: and listed so" ("$(P display-message -p '#{client_termfeatures}')" -match '(^|,)progressbar($|,)') $false
    P set -su terminal-features | Out-Null
    Start-Sleep -Milliseconds 400
    Check "${route}: the default back, with no change to write" (Logged) (Bar '1;10')
    Set-Bar s '1;30'
    Check "${route}: the next change is written" (Logged) "$(Bar '1;10') $(Bar '1;30')"
    Down
}

Write-Host "`n=== The progress indicator ===" -ForegroundColor Cyan
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
        Write-Info "skipped: set PSMUX_CONPTY_DIR_FOR_TEST to a folder holding conpty.dll and OpenConsole.exe"
    }
}
catch {
    Write-Fail "exception: $_"
}
finally {
    Down
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
