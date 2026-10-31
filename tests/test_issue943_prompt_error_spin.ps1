# Issue #943: a command that fails at the command prompt froze the client.
#
# The attached client sends what is typed at prefix + : over its persistent
# connection, and the server's connection thread runs it. That thread reads
# the client's next line only once `line` is empty, so every arm that ends
# with `continue` clears it first. Four did not, so their command ran again
# on every turn: the connection thread spun, the client's keys were never
# read again (Escape, the prefix, typing into the pane), and the error popup
# came back as soon as it was closed. The spin went on after the client
# window was closed, about two cores of CPU, until kill-server.
#
#   show-options codepoint-width   a name no option table knows   (#846)
#   show-options @nope             a user option nothing holds    (#795)
#   display-message -x hi          a flag display-message lacks   (#795)
#   show-options -p                the pane options               (#580)
#
# tmux reports each failure (cmd-show-options.c:132, cmdq_error) and the
# client carries on. Here each case types the command at prefix + :, then
# checks that the error is shown where there is one, that Escape closes it,
# that the next command typed at the prompt still runs, that typing reaches
# the pane, and that the server is not spinning.
#
# GROUND TRUTH: keys are sent with tests\injector.cs (WriteConsoleInput) into
# a client this suite starts and tracks by PID, the client's screen is read
# with tests\conread.cs, and the rest is read back over the CLI. Private
# USERPROFILE, PSMUX_DATA_DIR and empty config, a -L namespace per case;
# cleanup is `-L <ns> kill-server` plus the client PID. PSMUX_EXE picks the
# binary (defaults to target\release\psmux.exe).

$ErrorActionPreference = "Continue"
foreach ($name in @(Get-ChildItem Env: | Where-Object { $_.Name -like 'PSMUX_*' -and $_.Name -ne 'PSMUX_EXE' -or $_.Name -like 'TMUX*' } | ForEach-Object Name)) {
    Remove-Item "Env:$name"
}
$PSMUX = $env:PSMUX_EXE
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Check($name, $ok, $detail) { if ($ok) { Write-Pass $name } else { Write-Fail "$name $detail" } }

$TempHome = Join-Path ([System.IO.Path]::GetTempPath()) ("psmux_pes_" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TempHome | Out-Null
$env:USERPROFILE = $TempHome
$env:PSMUX_DATA_DIR = $TempHome
$env:PSMUX_CONFIG_FILE = Join-Path $TempHome "empty.conf"
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value ""
# Each case runs on a server of its own, so a spin left by one case cannot
# be counted against the next.
$script:NS = "pes_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
$S = "pes"
function P { & $PSMUX -L $script:NS @args 2>&1 }

Write-Host "`n=== a failing command at the prompt leaves the client working ===" -ForegroundColor Cyan
Write-Host "  Binary: $PSMUX"
Write-Host "  $((P -V) -join ' ')"

$toolDir = Join-Path $TempHome "tools"
New-Item -ItemType Directory -Force $toolDir | Out-Null
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$INJ = Join-Path $toolDir "injector.exe"
$READ = Join-Path $toolDir "conread.exe"
& $csc /nologo /optimize /out:$INJ  (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
& $csc /nologo /optimize /out:$READ (Join-Path $PSScriptRoot "conread.cs") 2>&1 | Out-Null
if (-not ((Test-Path $INJ) -and (Test-Path $READ))) { Write-Fail "harness tools did not compile"; exit 1 }

$script:clients = @()
function Start-Client {
    $p = Start-Process -FilePath $PSMUX -ArgumentList "-L", $script:NS, "attach", "-t", $S -PassThru
    $script:clients += $p
    Start-Sleep -Seconds 4
    return $p
}
function Stop-Client($p) {
    try { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } catch {}
    Start-Sleep -Milliseconds 700
}
function Send-Keys($p, $keys) {
    & $INJ $p.Id $keys | Out-Null
    Start-Sleep -Milliseconds 900
}
function Read-Screen($p) { return ([string[]](& $READ $p.Id)) -join "`n" }
# CPU seconds the server used over two seconds of wall time.
function Server-Cpu {
    $serverPid = [int]((P display -p -t $S '#{pid}') | Select-Object -Last 1)
    $sp = Get-Process -Id $serverPid -EA SilentlyContinue
    if (-not $sp) { return -1 }
    $t0 = $sp.TotalProcessorTime
    Start-Sleep -Seconds 2
    $sp.Refresh()
    return [math]::Round(($sp.TotalProcessorTime - $t0).TotalSeconds, 2)
}

$script:case = 0
# $shown: text the client shows for the failure, or $null for none.
function Case($command, $shown) {
    $script:case++
    Write-Host "`n[$($script:case)] prefix + : $command" -ForegroundColor Yellow
    $script:NS = "pes_" + [guid]::NewGuid().ToString('N').Substring(0, 6)
    P new-session -d -s $S -x 120 -y 30 | Out-Null
    Start-Sleep -Seconds 2
    P has-session -t $S | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Fail "session did not start"; return }
    $c = Start-Client
    Send-Keys $c "^b{SLEEP:500}:{SLEEP:600}$command{ENTER}{SLEEP:1500}"
    if ($shown) {
        Check "the failure is shown: $shown" ((Read-Screen $c).Contains($shown)) ""
    }
    Send-Keys $c "{ESC}{SLEEP:800}"
    if ($shown) {
        Check "Escape closes it" (-not (Read-Screen $c).Contains($shown)) ""
    }
    Send-Keys $c "^b{SLEEP:500}:{SLEEP:600}set -g @alive $($script:case){ENTER}{SLEEP:1200}"
    $alive = (P show -gv '@alive') -join ''
    Check "the next command typed at the prompt runs" ($alive -eq "$($script:case)") "(@alive = '$alive')"
    $mark = "PES_TYPED_$($script:case)"
    Send-Keys $c "echo $mark{ENTER}{SLEEP:1500}"
    Check "typing reaches the pane" ([bool]((P capture-pane -p -t $S) -match $mark)) ""
    $cpu = Server-Cpu
    Check "the server is not spinning" ($cpu -ge 0 -and $cpu -lt 0.5) "($cpu s of CPU in 2 s)"
    Stop-Client $c
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
}

try {
    Case 'show-options codepoint-width' 'invalid option: codepoint-width'
    Case 'show-options @nope' 'invalid option: @nope'
    Case 'display-message -x hi' $null
    Case 'show-options -p' $null
    # The same prompt with a command that works, for comparison.
    Case 'show-options -s codepoint-widths' 'codepoint-widths'
}
finally {
    foreach ($p in $script:clients) { if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } }
    P kill-server | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item -Recurse -Force $TempHome -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
