# Issue #1018: a palette index the pane has no entry for is asked of the outer
# terminal.
#
# tmux's input_osc_4 (input.c:2945-2958 at tag 3.8) answers `OSC 4 ; N ; ?`
# from the pane's palette, and when the pane has no entry asks the outer
# terminal of the most recently active client showing the pane
# (input_add_request, input.c:3584), holding the pane's later replies until
# the answer comes or 500 ms pass (input_reply, input_request_reply,
# input_request_timer_callback). psmux answered such an index with nothing.
# The sixteen ANSI colours stay answered from the values the client read at
# startup, so the cases ask 20, 21 and 196.
#
# tests/fake_outer_terminal.cs plays the outer terminal: it runs a client on
# the pipe route (stdin and stdout on pipes), logs each OSC 4 query the client
# writes, and answers `rgb:NN/NN/NN` with NN the index in hex, or stays silent.
# tests/reply_order_probe.cs runs in the pane with `raw` and records the
# replies as they came. XTVERSION (`CSI > q`) shows the order, as both console
# hosts pass it to psmux (the inbox conhost answers DA1 itself).
# tests/inject_console_reply.cs writes a reply into the input of a client on the
# console route, in the shape a terminal's reply has there, which must not be
# typed into the pane (tty_keys_palette, tty-keys.c:1830).
#   1. the inbox console host
#   2. OpenConsole, with PSMUX_CONPTY_DIR_FOR_TEST (a directory with
#      conpty.dll and OpenConsole.exe; skipped without it)
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per route;
# cleanup is `-L <ns> kill-server` and the fake terminal this suite started.
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_pfw_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$probe = Join-Path $TempHome 'reply_order_probe.exe'
$TermExe = Join-Path $TempHome 'fake_outer_terminal.exe'
$InjectExe = Join-Path $TempHome 'inject_console_reply.exe'
if ($csc) {
    & $csc.FullName /nologo /optimize /out:$InjectExe (Join-Path $PSScriptRoot 'inject_console_reply.cs') 2>&1 | Out-Null
    & $csc.FullName /nologo /optimize /out:$probe (Join-Path $PSScriptRoot 'reply_order_probe.cs') 2>&1 | Out-Null
    & $csc.FullName /nologo /optimize /out:$TermExe (Join-Path $PSScriptRoot 'fake_outer_terminal.cs') 2>&1 | Out-Null
}
if (-not (Test-Path $probe) -or -not (Test-Path $TermExe) -or -not (Test-Path $InjectExe)) {
    Write-Fail "build tests\reply_order_probe.cs, fake_outer_terminal.cs and inject_console_reply.cs (csc.exe unavailable)"
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
    exit $script:TestsFailed
}

$E = '\e'; $A = '\a'; $ST = '\e\'
$C1 = 'rgb:c5c5/0f0f/1f1f'
$script:NS = ""
$script:Term = $null
$script:Log = ""
$script:run = 0
function P { & $PSMUX -L $script:NS @args 2>&1 }

function Start-Term($mode) {
    $script:Log = Join-Path $TempHome "outer_$($script:NS)_$mode.txt"
    $script:Term = Start-Process -FilePath $TermExe -PassThru -WindowStyle Hidden -ArgumentList @(
        $script:Log, '300', $mode, $PSMUX, '-L', $script:NS, 'attach', '-t', 's')
    for ($k = 0; $k -lt 50; $k++) {
        if ("$(P list-clients)" -match 'attached') { break }
        Start-Sleep -Milliseconds 200
    }
    Start-Sleep -Milliseconds 800
}
function Stop-Term {
    if ($script:Term -and -not $script:Term.HasExited) { Stop-Process -Id $script:Term.Id -Force -EA SilentlyContinue }
    $script:Term = $null
    Start-Sleep -Milliseconds 800
}
function Outer { if ($script:Log -and (Test-Path $script:Log)) { @(Get-Content $script:Log | Where-Object { $_ }) } else { @() } }

# Run the probe with $queries in a new window; returns the replies as they
# came, and sets $script:Asked to the OSC 4 queries the outer terminal saw
# meanwhile.
function Probe($queries) {
    $script:run++
    $before = @(Outer).Count
    P new-window -d -t s -n "w$($script:run)" | Out-Null
    Start-Sleep -Milliseconds 2500
    $out = Join-Path $TempHome "r_$($script:NS)_$($script:run).txt"
    P send-keys -t "s:w$($script:run)" "& '$probe' '$out' '$queries' raw" Enter | Out-Null
    for ($k = 0; $k -lt 60 -and -not (Test-Path $out); $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 300
    $script:Asked = (@(Outer) | Select-Object -Skip $before) -join ' '
    if (Test-Path $out) { (Get-Content $out -Raw).Trim() } else { '(no reply file)' }
}

function Run-Cases($route) {
    $script:NS = "pfw_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500

    Write-Host "  -- the outer terminal answers" -ForegroundColor DarkCyan
    Start-Term 'answer'
    Check "${route}: client attached" ("$(P list-clients)" -match 'attached') $true
    $got = Probe "$E]4;20;?$A"
    Check "${route}: index 20 with BEL is answered by the outer terminal" $got "$E]4;20;rgb:1414/1414/1414$A"
    Check "${route}: and the outer terminal was asked for 20" $script:Asked "$E]4;20;?$ST"
    $got = Probe "$E]4;196;?$ST"
    Check "${route}: index 196 with ST" $got "$E]4;196;rgb:c4c4/c4c4/c4c4$ST"
    $got = Probe "$E]4;20;?;21;?$A"
    Check "${route}: two indexes in one OSC 4, in order" $got "$E]4;20;rgb:1414/1414/1414$A$E]4;21;rgb:1515/1515/1515$A"
    $got = Probe "$E]4;20;?$A$E[>q"
    Check "${route}: XTVERSION asked after it comes after the colour" ($got -match '^\\e\]4;20;rgb:1414/1414/1414\\a\\eP>\|') $true
    $got = Probe "$E]4;20;#112233$A$E]4;20;?$A"
    Check "${route}: an index the pane set is answered from the pane" $got "$E]4;20;rgb:1111/2222/3333$A"
    Check "${route}: and the outer terminal is not asked" $script:Asked ''
    $got = Probe "$E]4;1;?$A"
    Check "${route}: index 1 is answered from the startup values" $got "$E]4;1;$C1$A"
    Check "${route}: and the outer terminal is not asked for it" $script:Asked ''
    Stop-Term

    Write-Host "  -- the outer terminal stays silent" -ForegroundColor DarkCyan
    Start-Term 'silent'
    $got = Probe "$E]4;20;?$A$E[>q"
    Check "${route}: no colour, and XTVERSION still comes" ($got -match '^\\eP>\|') $true
    Check "${route}: the outer terminal was asked" $script:Asked "$E]4;20;?$ST"
    Stop-Term

    Write-Host "  -- no client" -ForegroundColor DarkCyan
    $got = Probe "$E]4;20;?$A$E[>q"
    Check "${route}: no colour and no wait for XTVERSION" ($got -match '^\\eP>\|') $true

    Write-Host "  -- a reply in a console client's input" -ForegroundColor DarkCyan
    $client = Start-Process -FilePath $PSMUX -ArgumentList @('-L', $script:NS, 'attach', '-t', 's') -PassThru -WindowStyle Minimized
    Start-Sleep -Seconds 3
    P new-window -t s -n typed | Out-Null
    Start-Sleep -Milliseconds 2500
    & $InjectExe $client.Id "$E]4;20;rgb:0000/0000/d7d7$ST"
    Start-Sleep -Milliseconds 300
    & $InjectExe $client.Id 'ok'
    Start-Sleep -Seconds 2
    $screen = (P capture-pane -p -t s:typed | ForEach-Object { "$_".TrimEnd() } | Where-Object { $_ }) -join ' | '
    Check "${route}: a reply is not typed into the pane" ($screen -match 'rgb:') $false
    Check "${route}: and what follows it is" ($screen -match '> ok$') $true
    if (-not $client.HasExited) { Stop-Process -Id $client.Id -Force -EA SilentlyContinue }
    Start-Sleep -Milliseconds 500

    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== palette questions for the outer terminal ===" -ForegroundColor Cyan
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
    Stop-Term
    if ($script:NS) { P kill-server | Out-Null }
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
