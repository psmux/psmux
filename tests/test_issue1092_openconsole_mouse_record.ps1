# Issue #1092: each mouse event reaches a pane app once when the panes run on
# the console host of PSMUX_CONPTY_DIR (OpenConsole), on any build.
#
# Below CONPTY_MOUSE_MIN_BUILD (22523) psmux injects a MOUSE_EVENT record for
# every mouse event (#597), as the inbox conhost of those builds drops the SGR
# report psmux writes into the pane's input pipe. psmux took that from the
# build number, but OpenConsole does not drop the report: with the app's VT
# input off it makes a record of it itself, so the app got two. Measured on
# 19045 with OpenConsole 1.24 at master 5d9f695f, for a record reader:
# 3 notches -> 6 wheel records (delta 128 from the host, 120 from psmux),
# 1 click -> 2 presses and 2 releases, 1 drag of 8 steps -> 2 presses and
# 16 moves. With VT input on (Bubble Tea) the host passes the report on as
# text and psmux's record is the only copy, so that shape must keep it.
#
# Each case attaches a real client in a conhost window and writes real mouse
# records into the client's console (what a physical mouse produces):
#   record reader            native_mouse_child (mouse input on, VT input off)
#   record reader, VT input  native_mouse_child vti
#   VT app                   mouse_echo_child (DECSET 1000/1006, reads SGR)
# Needs the OpenConsole fixture (PSMUX_TEST_CONPTY_DIR, or
# %TEMP%\occ-conpty\nugetstable); skips without it.
#
# Every psmux call uses its own random -L namespace and PSMUX_DATA_DIR.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$Tests = Split-Path -Parent $MyInvocation.MyCommand.Path
$rand  = Get-Random -Maximum 999999
$N = 3

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow }

$occDir = if ($env:PSMUX_TEST_CONPTY_DIR) { $env:PSMUX_TEST_CONPTY_DIR } else { Join-Path $env:TEMP "occ-conpty\nugetstable" }
if (-not (Test-Path (Join-Path $occDir "OpenConsole.exe"))) {
    Write-Skip "OpenConsole fixture not found at $occDir"
    Write-Host "`n=== Results: 0 passed, 0 failed ==="
    exit 0
}

$ownDataDir = $false
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP "psmux_occ_mouse_$rand"
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
    $ownDataDir = $true
}
$dd = $env:PSMUX_DATA_DIR
$savedConpty = $env:PSMUX_CONPTY_DIR
$env:PSMUX_CONPTY_DIR = $occDir

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$nat   = Join-Path $dd "native_mouse_child.exe"
$echo  = Join-Path $dd "mouse_echo_child.exe"
$wheel = Join-Path $dd "mouse_injector.exe"
$click = Join-Path $dd "click_injector.exe"
$drag  = Join-Path $dd "mouse_drag_injector.exe"
& $csc /nologo /out:$nat   (Join-Path $Tests "native_mouse_child.cs") | Out-Null
& $csc /nologo /out:$echo  (Join-Path $Tests "mouse_echo_child.cs") | Out-Null
& $csc /nologo /out:$wheel (Join-Path $Tests "mouse_injector.cs") | Out-Null
& $csc /nologo /out:$click (Join-Path $Tests "click_injector.cs") | Out-Null
& $csc /nologo /out:$drag  (Join-Path $Tests "mouse_drag_injector.cs") | Out-Null
$natLog = Join-Path $env:TEMP "psmux_native_mouse.txt"
$build = [Environment]::OSVersion.Version.Build
Write-Host "build $build, $((& $PSMUX -V) -join ' '), panes on $occDir"

# Start a session whose pane runs $cmd, attach a client in a conhost window,
# run $act with the client's PID, and return the app's log lines.
function Run-Case([string[]]$cmd, [scriptblock]$act, [string]$echoLog) {
    $ns = "occm$rand$(Get-Random -Maximum 9999)"
    Remove-Item $natLog -Force -EA SilentlyContinue
    if ($echoLog) { $env:PSMUX_MOUSE_ECHO_LOG = $echoLog }
    & $PSMUX -L $ns new-session -d -s w -x 100 -y 30 @cmd | Out-Null
    Start-Sleep -Seconds 3
    & $PSMUX -L $ns set -g mouse on | Out-Null
    $proc = Start-Process -FilePath "$env:WINDIR\System32\conhost.exe" -ArgumentList "`"$PSMUX`"","-L",$ns,"attach","-t","w" -PassThru
    try {
        Start-Sleep -Seconds 4
        $c = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($proc.Id)" | Where-Object { $_.Name -eq 'psmux.exe' } | Select-Object -First 1
        if (-not $c) { return $null }
        & $act ([int]$c.ProcessId)
        Start-Sleep -Seconds 1
        $file = if ($echoLog) { $echoLog } else { $natLog }
        return ,@(if (Test-Path $file) { Get-Content $file } else { @() })
    } finally {
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
        Start-Sleep -Milliseconds 800
        if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }
        Remove-Item Env:PSMUX_MOUSE_ECHO_LOG -EA SilentlyContinue
    }
}
$notches = { param($id) for ($i = 0; $i -lt $N; $i++) { & $wheel $id up 1 20 8 | Out-Null; Start-Sleep -Milliseconds 500 } }
$click1  = { param($id) & $click $id 20 8 | Out-Null; Start-Sleep -Milliseconds 500 }
$drag1   = { param($id) & $drag $id "drag" 10 5 30 12 8 60 | Out-Null; Start-Sleep -Milliseconds 800 }
function Records($log) { ($log | Where-Object { $_ -match '^MOUSE' } | ForEach-Object { ($_ -split 'MOUSE ')[1] }) -join ' / ' }

foreach ($vti in @($false, $true)) {
    $cmd = if ($vti) { @($nat, 'vti') } else { @($nat) }
    $tag = if ($vti) { 'record reader, VT input on' } else { 'record reader, VT input off' }

    Write-Host "`n[$tag] $N wheel notches" -ForegroundColor Yellow
    $log = Run-Case $cmd $notches $null
    if ($null -eq $log) { Write-Fail "client did not start" }
    else {
        $w = @($log | Where-Object { $_ -match '^MOUSE .*flags=0x4 ' })
        if ($w.Count -eq $N) { Write-Pass "$N notches -> $($w.Count) wheel records" }
        else { Write-Fail "$N notches -> $($w.Count) wheel records: $(Records $w)" }
    }

    # On 22523 and above psmux gives a VT input reader the wheel record only
    # (#277); its clicks are the host's text there, so there is no record to
    # count. Below the build the #597 record is its only copy.
    if ($vti -and $build -ge 22523) {
        Write-Skip "$tag, click and drag: psmux injects no click record on build $build (#277 is wheel only)"
        continue
    }

    Write-Host "`n[$tag] a left click" -ForegroundColor Yellow
    $log = Run-Case $cmd $click1 $null
    if ($null -eq $log) { Write-Fail "client did not start" }
    else {
        $p = @($log | Where-Object { $_ -match '^MOUSE .*buttons=0x1 flags=0x0 ' })
        $r = @($log | Where-Object { $_ -match '^MOUSE .*buttons=0x0 flags=0x0 ' })
        if ($p.Count -eq 1 -and $r.Count -eq 1) { Write-Pass "1 click -> 1 press, 1 release" }
        else { Write-Fail "1 click -> $($p.Count) press, $($r.Count) release: $(Records $log)" }
    }

    Write-Host "`n[$tag] a left drag of 8 steps" -ForegroundColor Yellow
    $log = Run-Case $cmd $drag1 $null
    if ($null -eq $log) { Write-Fail "client did not start" }
    else {
        $p = @($log | Where-Object { $_ -match '^MOUSE .*buttons=0x1 flags=0x0 ' })
        $m = @($log | Where-Object { $_ -match '^MOUSE .*buttons=0x1 flags=0x1 ' })
        if ($p.Count -eq 1 -and $m.Count -eq 8) { Write-Pass "1 drag -> 1 press, 8 moves" }
        else { Write-Fail "1 drag -> $($p.Count) press, $($m.Count) moves" }
    }
}

Write-Host "`n[VT app] $N wheel notches and a click" -ForegroundColor Yellow
$el = Join-Path $dd "echo_wheel.txt"
$log = Run-Case @($echo) $notches $el
if ($null -eq $log) { Write-Fail "client did not start" }
else {
    $n = @($log | Where-Object { $_ -match '^RECV .*\[<64;' }).Count
    if ($n -eq $N) { Write-Pass "$N notches -> $n SGR wheel reports" }
    else { Write-Fail "$N notches -> $n SGR wheel reports" }
}
$el = Join-Path $dd "echo_click.txt"
$log = Run-Case @($echo) $click1 $el
if ($null -eq $log) { Write-Fail "client did not start" }
else {
    $p = @($log | Where-Object { $_ -cmatch '^RECV .*\[<0;\d+;\d+M' }).Count
    $r = @($log | Where-Object { $_ -cmatch '^RECV .*\[<0;\d+;\d+m' }).Count
    if ($p -eq 1 -and $r -eq 1) { Write-Pass "1 click -> 1 SGR press, 1 SGR release" }
    else { Write-Fail "1 click -> $p SGR press, $r SGR release" }
}

if ($savedConpty) { $env:PSMUX_CONPTY_DIR = $savedConpty } else { Remove-Item Env:PSMUX_CONPTY_DIR -EA SilentlyContinue }
if ($ownDataDir) { Remove-Item -Recurse -Force $dd -EA SilentlyContinue }
Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ==="
exit $script:TestsFailed
