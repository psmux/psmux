# switch-client and the cursor shape and blink: what an attached client tells
# its terminal when it moves to another session.
#
# psmux runs one server per session, so the client attaches again for the
# session it switches to. It started that attach from "nothing asserted", so a
# session with no cursor-style left the terminal drawing the shape (or the
# blink) of the session it left until detach. tmux keeps `tty->cstyle` and the
# cursor mode on the tty across the switch and sends `Se` for a pane with the
# default style (tty_update_cursor, tty.c at tag 3.8).
#
# tests/pipe_client_cursor_colour_log.cs plays the client's terminal on the
# pipe route and, with PSMUX_LOG_CURSOR_STYLE set, logs every DECSCUSR and
# ESC [ ? 12 h or l the client writes.
#
# Private USERPROFILE, PSMUX_DATA_DIR and config, a -L namespace per case;
# cleanup is `-L <ns> kill-server` and the logger this suite started.
# PSMUX_TEST_BIN or PSMUX_EXE picks the binary (defaults to
# target\release\psmux.exe).

. "$PSScriptRoot\helpers\stop_own_tree.ps1"
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { $env:PSMUX_EXE }
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
Remove-Item Env:SHELL -EA SilentlyContinue
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($name, $got, $want) {
    if ("$got" -ceq "$want") { Write-Pass "$name = '$got'" } else { Write-Fail "$name expected '$want', got '$got'" }
}

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_swcs_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "psmux.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""
$env:PSMUX_LOG_CURSOR_STYLE = "1"

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$TermExe = Join-Path $TempHome 'pipe_client_cursor_style_log.exe'
if ($csc) { & $csc.FullName /nologo /optimize /out:$TermExe (Join-Path $PSScriptRoot 'pipe_client_cursor_colour_log.cs') 2>&1 | Out-Null }
if (-not (Test-Path $TermExe)) {
    Write-Fail "build tests\pipe_client_cursor_colour_log.cs (csc.exe unavailable)"
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
    exit $script:TestsFailed
}

$script:NS = ""
$script:Term = $null
function P { & $PSMUX -L $script:NS @args 2>&1 }
function Lines($log) { if (Test-Path $log) { @(Get-Content $log | Where-Object { $_ }) } else { @() } }

# Attach to s, run $c.s there (and $c.t in t beforehand), switch to t, and
# check what the terminal was told after the switch, then switch back.
function Case($c) {
    $script:NS = "swcs_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s s -x 120 -y 30 | Out-Null
    P new-session -d -s t -x 120 -y 30 | Out-Null
    Start-Sleep -Milliseconds 2500
    if ($c.t) { & $c.t | Out-Null }
    $log = Join-Path $TempHome "log_$($script:NS).txt"
    $script:Term = Start-Process -FilePath $TermExe -PassThru -WindowStyle Hidden -ArgumentList @($log, $PSMUX, '-L', $script:NS, 'attach', '-t', 's')
    for ($i = 0; $i -lt 50 -and -not ("$(P list-clients)" -match 'attached'); $i++) { Start-Sleep -Milliseconds 200 }
    Start-Sleep -Milliseconds 800
    & $c.s | Out-Null
    Start-Sleep -Milliseconds 2000
    $before = Lines $log
    Check "$($c.n): told in s" ($before -join ' ') $c.before
    P switch-client -t t | Out-Null
    Start-Sleep -Milliseconds 3000
    $mid = Lines $log
    Check "$($c.n): told after switch-client to t" (($mid | Select-Object -Skip $before.Count) -join ' ') $c.after
    P switch-client -t s | Out-Null
    Start-Sleep -Milliseconds 3000
    Check "$($c.n): told after switching back to s" ((Lines $log | Select-Object -Skip $mid.Count) -join ' ') $c.back
    if ($script:Term -and -not $script:Term.HasExited) { Stop-OwnTree -RootPid $script:Term.Id }
    $script:Term = $null
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

$E = '\e'
$cases = @(
    @{ n = 'cursor-style block in s, none in t'; s = { P set -g -t s cursor-style block }; before = "$E[2 q"; after = "$E[0 q"; back = "$E[2 q" }
    @{ n = 'a program bar in s, none in t'; s = { P send-keys -t s '[Console]::Write("$([char]27)[6 q")' Enter }; before = "$E[6 q"; after = "$E[0 q"; back = "$E[6 q" }
    @{ n = 'cursor-blink on in s, none in t'; s = { P set -g -t s cursor-blink on }; before = "$E[?12h"; after = "$E[?12l"; back = "$E[?12h" }
    # The same shape on both sides is not written again.
    @{ n = 'cursor-style bar in s and in t'; t = { P set -g -t t cursor-style bar }; s = { P set -g -t s cursor-style bar }; before = "$E[6 q"; after = ''; back = '' }
    # Nothing asked for anywhere: nothing written at all.
    @{ n = 'nothing in either'; s = {}; before = ''; after = ''; back = '' }
)

Write-Host "`n=== switch-client: cursor shape and blink ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
try {
    foreach ($c in $cases) { Case $c }
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
