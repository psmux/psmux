# Issue #1018: a palette index the pane has no entry for is answered from the
# xterm 256 colour table.
#
# tmux's input_osc_4 (input.c:2945-2958) answers `OSC 4 ; N ; ?` from the
# pane's palette, and with no entry asks the outer terminal of a client
# (input_add_request, input.c:3583), holding the pane's later replies until it
# answers or 500 ms pass, and gives nothing when no client can be asked.
# psmux answered such an index with nothing at all. It now answers 16 to 255
# from the xterm table (the 6x6x6 cube, then 24 greys), the value nearly every
# terminal gives, in order with the other replies and never waiting. The
# sixteen ANSI colours keep their answer from the client's startup values.
#
# Cases, with no client and then with a client on the pipe route whose outer
# terminal (tests/fake_outer_terminal.cs, by tanaeakihiko, from PR #1019)
# logs every OSC 4 query it is asked and stays silent: the colour comes, the
# order with XTVERSION is kept, and the outer terminal is never asked.
# tests/reply_order_probe.cs runs in the pane with `raw`.
#   1. the inbox conhost
#   2. OpenConsole, with PSMUX_CONPTY_DIR_FOR_TEST (a directory with
#      conpty.dll and OpenConsole.exe; skipped without it)
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per route;
# cleanup is `-L <ns> kill-server` and the processes this suite started, by
# PID. PSMUX_TEST_BIN or PSMUX_EXE picks the binary (defaults to
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

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_ptab_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$probe = Join-Path $TempHome 'reply_order_probe.exe'
$TermExe = Join-Path $TempHome 'fake_outer_terminal.exe'
if ($csc) {
    & $csc.FullName /nologo /optimize /out:$probe (Join-Path $PSScriptRoot 'reply_order_probe.cs') 2>&1 | Out-Null
    & $csc.FullName /nologo /optimize /out:$TermExe (Join-Path $PSScriptRoot 'fake_outer_terminal.cs') 2>&1 | Out-Null
}
if (-not (Test-Path $probe) -or -not (Test-Path $TermExe)) {
    Write-Fail "build tests\reply_order_probe.cs and fake_outer_terminal.cs (csc.exe unavailable)"
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
    exit $script:TestsFailed
}

$E = '\e'; $A = '\a'; $ST = '\e\'
$script:NS = ""
$script:Term = $null
$script:Log = ""
$script:run = 0
function P { & $PSMUX -L $script:NS @args 2>&1 }

function Start-Term {
    $script:Log = Join-Path $TempHome "outer_$($script:NS).txt"
    $script:Term = Start-Process -FilePath $TermExe -PassThru -WindowStyle Hidden -ArgumentList @(
        $script:Log, '300', 'silent', $PSMUX, '-L', $script:NS, 'attach', '-t', 's')
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

# Run the probe with $queries in a new window; returns the replies as they came.
function Probe($queries) {
    $script:run++
    P new-window -d -t s -n "w$($script:run)" | Out-Null
    Start-Sleep -Milliseconds 2500
    $out = Join-Path $TempHome "r_$($script:NS)_$($script:run).txt"
    P send-keys -t "s:w$($script:run)" "& '$probe' '$out' '$queries' raw" Enter | Out-Null
    for ($k = 0; $k -lt 60 -and -not (Test-Path $out); $k++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 300
    if (Test-Path $out) { (Get-Content $out -Raw).Trim() } else { '(no reply file)' }
}

$C20 = 'rgb:0000/0000/d7d7'
$C21 = 'rgb:0000/0000/ffff'
$C196 = 'rgb:ffff/0000/0000'
$C232 = 'rgb:0808/0808/0808'
$C255 = 'rgb:eeee/eeee/eeee'

function Run-Table($label) {
    $got = Probe "$E]4;20;?$A"
    Check "${label}: index 20 with BEL is the xterm cube colour" $got "$E]4;20;$C20$A"
    $got = Probe "$E]4;196;?$ST"
    Check "${label}: index 196 with ST" $got "$E]4;196;$C196$ST"
    $got = Probe "$E]4;232;?;255;?$A"
    Check "${label}: the greys 232 and 255 in one OSC 4, in order" $got "$E]4;232;$C232$A$E]4;255;$C255$A"
    $got = Probe "$E]4;20;?$A$E[>q$E]4;21;?$ST"
    Check "${label}: in order with XTVERSION" ($got -match '^\\e\]4;20;rgb:0000/0000/d7d7\\a\\eP>\|[^\\]*\\e\\\\e\]4;21;rgb:0000/0000/ffff\\e\\$') $true
    $got = Probe "$E]4;20;#112233$A$E]4;20;?$A"
    Check "${label}: an index the pane set is answered from the pane" $got "$E]4;20;rgb:1111/2222/3333$A"
}

function Run-Cases($route) {
    $script:NS = "ptab_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500

    Write-Host "  -- no client" -ForegroundColor DarkCyan
    Run-Table "${route}, no client"

    Write-Host "  -- a client whose outer terminal stays silent" -ForegroundColor DarkCyan
    Start-Term
    Check "${route}: client attached" ("$(P list-clients)" -match 'attached') $true
    Run-Table "${route}, silent terminal"
    Check "${route}: the outer terminal was never asked" ((Outer) -join ' ') ''
    Stop-Term

    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== palette indexes 16 to 255 from the xterm table ===" -ForegroundColor Cyan
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
