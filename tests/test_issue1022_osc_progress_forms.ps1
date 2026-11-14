# Issue #1022: OSC 9;4 with no value, and the forms tmux ignores.
#
# tmux's input_osc_9 (input.c:3030 at tag 3.8) takes `OSC 9 ; 4 ; state`, with or
# without a `;` after it, and keeps the last value (screen_set_progress_bar,
# screen.c:344, which also keeps it for the indeterminate state whatever is
# given). A state past 4, a value past 100 and anything else misshapen leave the
# indicator as it was. psmux took only the four field form, and turned a bad
# state or value into a state of 4, a value of 100 or a value of 0.
#
# Both console hosts pass every form on as written, so each case checks what the
# server holds (host_progress in dump-state) and what a client writes to its
# terminal. Every expected pair was measured from tmux 3.8.
#   1. the inbox console host
#   2. OpenConsole, with PSMUX_CONPTY_DIR_FOR_TEST (a directory with conpty.dll
#      and OpenConsole.exe; skipped without it)
#
# tests/reply_order_probe.cs writes the sequences in one write from the pane.
# tests/pipe_client_progress_log.cs plays the outer terminal for a pipe route
# client and logs each OSC 9 ; 4 the client writes.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per console
# host; cleanup is `-L <ns> kill-server` and the logger this suite started.
# PSMUX_TEST_BIN or PSMUX_EXE picks the binary (defaults to
# target\release\psmux.exe).

. "$PSScriptRoot\helpers\stop_own_tree.ps1"
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }
$ConptyDir = $env:PSMUX_CONPTY_DIR_FOR_TEST
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_pb_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
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

$A = '\a'; $ST = '\e\'
$Start = "\e]9;4;1;50$A"
$script:NS = ""
$script:Term = $null
$script:Log = ""
$script:run = 0
function P { & $PSMUX -L $script:NS @args 2>&1 }

# host_progress in dump-state, or '(none)'.
function Held {
    $portFile = Get-ChildItem $TempHome -Filter "$($script:NS)*.port" -Recurse -EA SilentlyContinue | Select-Object -First 1
    if (-not $portFile) { return '(no port file)' }
    $port = (Get-Content $portFile.FullName -Raw).Trim()
    $key = (Get-Content ($portFile.FullName -replace '\.port$', '.key') -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true
    $st = $tcp.GetStream(); $st.ReadTimeout = 3000
    $w = [IO.StreamWriter]::new($st); $r = [IO.StreamReader]::new($st)
    $w.Write("AUTH $key`n"); $w.Flush(); $null = $r.ReadLine()
    $w.Write("dump-state`n"); $w.Flush()
    $line = ''
    try { while ($true) { $line = $r.ReadLine(); if ($null -eq $line -or $line.EndsWith('}')) { break } } } catch {}
    $tcp.Close()
    if ($line -match '"host_progress"\s*:\s*"([^"]*)"') { $Matches[1] } else { '(none)' }
}

function Logged { if ($script:Log -and (Test-Path $script:Log)) { @(Get-Content $script:Log | Where-Object { $_ }) } else { @() } }

# Write $queries from a new window, the active one, and check what the server
# holds and the last OSC 9;4 the client wrote since. $want is "state;value".
function Case($route, $name, $queries, $want) {
    $script:run++
    $window = "w$($script:run)"
    P new-window -t s -n $window | Out-Null
    Start-Sleep -Milliseconds 2200
    $before = @(Logged).Count
    $out = Join-Path $TempHome "r_$($script:NS)_$($script:run).txt"
    P send-keys -t "s:$window" "& '$probe' '$out' '$queries' raw" Enter | Out-Null
    for ($k = 0; $k -lt 60 -and -not (Test-Path $out); $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 300
    Check "${route}: $name, held" (Held) $want
    $new = @(Logged | Select-Object -Skip $before)
    $last = if ($new.Count) { $new[$new.Count - 1] } else { '(nothing)' }
    Check "${route}: $name, written to the terminal" $last "\e]9;4;$want$ST"
}

function Run-Cases($route) {
    $script:NS = "pb_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500
    $script:Log = Join-Path $TempHome "outer_$($script:NS).txt"
    $script:Term = Start-Process -FilePath $TermExe -PassThru -WindowStyle Hidden -ArgumentList @($script:Log, $PSMUX, '-L', $script:NS, 'attach', '-t', 's')
    for ($k = 0; $k -lt 50; $k++) {
        if ("$(P list-clients)" -match 'attached') { break }
        Start-Sleep -Milliseconds 200
    }
    Start-Sleep -Milliseconds 800
    Check "${route}: client attached" ("$(P list-clients)" -match 'attached') $true

    Write-Host "  -- no value: the state changes and the value stays" -ForegroundColor DarkCyan
    Case $route 'indeterminate' "$Start\e]9;4;3$A" '3;50'
    Case $route 'indeterminate with ST' "$Start\e]9;4;3$ST" '3;50'
    Case $route 'indeterminate with a ;' "$Start\e]9;4;3;$A" '3;50'
    Case $route 'hide' "$Start\e]9;4;0$A" '0;50'
    Case $route 'hide with ST' "$Start\e]9;4;0$ST" '0;50'
    Case $route 'hide with a ;' "$Start\e]9;4;0;$A" '0;50'
    Case $route 'default with a ;' "$Start\e]9;4;1;$A" '1;50'
    Case $route 'error' "$Start\e]9;4;2$A" '2;50'
    Case $route 'warning' "$Start\e]9;4;4$A" '4;50'
    Case $route 'indeterminate on a new pane' "\e]9;4;3$A" '3;0'

    Write-Host "  -- a value" -ForegroundColor DarkCyan
    Case $route 'indeterminate does not take it' "$Start\e]9;4;3;70$A" '3;50'
    Case $route 'hide takes it' "$Start\e]9;4;0;70$A" '0;70'
    Case $route 'default takes it' "$Start\e]9;4;1;70$A" '1;70'
    Case $route 'leading zeros' "$Start\e]9;4;1;007$A" '1;7'

    Write-Host "  -- ignored" -ForegroundColor DarkCyan
    Case $route 'no state' "$Start\e]9;4$A" '1;50'
    Case $route 'a state past 4' "$Start\e]9;4;9;70$A" '1;50'
    Case $route 'a state of two digits' "$Start\e]9;4;12;70$A" '1;50'
    Case $route 'a value past 100' "$Start\e]9;4;1;101$A" '1;50'
    Case $route 'a value past 255' "$Start\e]9;4;1;300$A" '1;50'
    Case $route 'a value that is no number' "$Start\e]9;4;1;x$A" '1;50'
    Case $route 'a field too many' "$Start\e]9;4;1;70;$A" '1;50'

    Write-Host "  -- RIS" -ForegroundColor DarkCyan
    Case $route 'the value survives it' "$Start\ec\e]9;4;3$A" '3;50'

    if ($script:Term -and -not $script:Term.HasExited) { Stop-OwnTree -RootPid $script:Term.Id }
    $script:Term = $null
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== OSC 9;4 forms ===" -ForegroundColor Cyan
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
    if ($script:Term -and -not $script:Term.HasExited) { Stop-OwnTree -RootPid $script:Term.Id }
    if ($script:NS) { P kill-server | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
