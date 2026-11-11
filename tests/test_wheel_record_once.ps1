# One wheel notch, one wheel event, for every kind of pane app.
#
# A pane app that reads its mouse as console records got each VERTICAL notch
# twice: psmux writes the notch to the pane's ConPTY pipe as SGR and also
# injects a MOUSE_WHEELED record (#277), and with the app's VT input off (a
# deliberate record reader, which #623 leaves alone) the console host turns
# that SGR into a wheel record of its own. Measured before the fix on build
# 26300, inbox conhost and OpenConsole alike: 5 notches -> 10 records, 5 with
# delta 128 (the host's) and 5 with delta 120 (psmux's).
#
# Each shape gets N real notches: MOUSE_WHEELED records written by PID into
# the console of a conhost window hosting the psmux client, the same thing a
# physical wheel produces. Then exactly N must arrive:
#   reader      native_mouse_child (mouse input on, cooked off, VTI off)
#   reader+vti  native_mouse_child vti (Bubble Tea shape, #277: the injected
#               record is its only mouse event)
#   vt app      mouse_echo_child (DECSET 1000/1006, reads SGR bytes)
# on the inbox conhost and, when the fixture exists, on OpenConsole
# (PSMUX_CONPTY_DIR). With mouse off the reader gets nothing at all.
#
# Every psmux call uses its own random -L namespace and PSMUX_DATA_DIR.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$Tests = Split-Path -Parent $MyInvocation.MyCommand.Path
$rand  = Get-Random -Maximum 999999
$N = 5

$ownDataDir = $false
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP "psmux_wheel_once_$rand"
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
    $ownDataDir = $true
}
$dd = $env:PSMUX_DATA_DIR
$occDir = if ($env:PSMUX_TEST_CONPTY_DIR) { $env:PSMUX_TEST_CONPTY_DIR } else { Join-Path $env:TEMP "occ-conpty\nugetstable" }
$savedConpty = $env:PSMUX_CONPTY_DIR

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow }

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$nat  = Join-Path $dd "native_mouse_child.exe"
$echo = Join-Path $dd "mouse_echo_child.exe"
$inj  = Join-Path $dd "mouse_injector.exe"
& $csc /nologo /out:$nat  (Join-Path $Tests "native_mouse_child.cs") | Out-Null
& $csc /nologo /out:$echo (Join-Path $Tests "mouse_echo_child.cs") | Out-Null
& $csc /nologo /out:$inj  (Join-Path $Tests "mouse_injector.cs") | Out-Null
$natLog = Join-Path $env:TEMP "psmux_native_mouse.txt"

# Run one shape: start a session whose pane runs $cmd, attach a client in a
# conhost window, inject $N notches in $dir, return the child's log lines.
function Run-Shape([string]$host_, [string[]]$cmd, [string]$dir, [bool]$mouse, [string]$echoLog) {
    $ns = "wonce$rand$(Get-Random -Maximum 9999)"
    if ($host_ -eq 'OpenConsole') { $env:PSMUX_CONPTY_DIR = $occDir }
    elseif ($savedConpty) { $env:PSMUX_CONPTY_DIR = $savedConpty }
    else { Remove-Item Env:PSMUX_CONPTY_DIR -EA SilentlyContinue }
    if ($echoLog) { $env:PSMUX_MOUSE_ECHO_LOG = $echoLog }
    Remove-Item $natLog -Force -EA SilentlyContinue
    & $PSMUX -L $ns new-session -d -s w -x 100 -y 30 @cmd | Out-Null
    Start-Sleep -Seconds 3
    & $PSMUX -L $ns set -g mouse $(if ($mouse) { 'on' } else { 'off' }) | Out-Null
    $proc = Start-Process -FilePath "$env:WINDIR\System32\conhost.exe" -ArgumentList "`"$PSMUX`"","-L",$ns,"attach","-t","w" -PassThru
    try {
        Start-Sleep -Seconds 4
        $c = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($proc.Id)" | Where-Object { $_.Name -eq 'psmux.exe' } | Select-Object -First 1
        if (-not $c) { return $null }
        for ($i = 0; $i -lt $N; $i++) {
            & $inj ([int]$c.ProcessId) $dir 1 20 8 | Out-Null
            Start-Sleep -Milliseconds 500
        }
        Start-Sleep -Seconds 1
        if ($echoLog) { if (Test-Path $echoLog) { Get-Content $echoLog } else { @() } }
        elseif (Test-Path $natLog) { Get-Content $natLog } else { @() }
    } finally {
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
        Start-Sleep -Milliseconds 800
        if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }
    }
}

$hosts = @('inbox conhost')
if (Test-Path (Join-Path $occDir "OpenConsole.exe")) { $hosts += 'OpenConsole' }
else { Write-Skip "OpenConsole fixture not found at $occDir; inbox conhost only" }

foreach ($h in $hosts) {
    Write-Host "`n=== $h ===" -ForegroundColor Cyan
    foreach ($dir in 'up', 'down') {
        $sign = if ($dir -eq 'up') { '' } else { '-' }

        Write-Host "`n[$h] record reader, VT input off, wheel $dir" -ForegroundColor Yellow
        $lines = Run-Shape $h @($nat) $dir $true $null
        if ($null -eq $lines) { Write-Fail "client did not start"; continue }
        $wheel = @($lines | Where-Object { $_ -match '^MOUSE .*flags=0x4 ' })
        $right = @($wheel | Where-Object { $_ -match "wheel=$sign\d" -and $_ -notmatch "wheel=$(if ($sign) {''} else {'-'})\d" })
        if ($wheel.Count -eq $N -and $right.Count -eq $N) { Write-Pass "$N notches -> $($wheel.Count) wheel records" }
        else { Write-Fail "$N notches -> $($wheel.Count) wheel records: $(($wheel | ForEach-Object { ($_ -split 'buttons=')[1] }) -join ' / ')" }

        Write-Host "`n[$h] record reader, VT input on (Bubble Tea, #277), wheel $dir" -ForegroundColor Yellow
        $lines = Run-Shape $h @($nat, 'vti') $dir $true $null
        if ($null -eq $lines) { Write-Fail "client did not start"; continue }
        $wheel = @($lines | Where-Object { $_ -match '^MOUSE .*flags=0x4 ' })
        if ($wheel.Count -eq $N) { Write-Pass "$N notches -> $($wheel.Count) wheel records (the injected record still arrives)" }
        else { Write-Fail "$N notches -> $($wheel.Count) wheel records with VTI on: $(($wheel | ForEach-Object { ($_ -split 'buttons=')[1] }) -join ' / ')" }
    }

    Write-Host "`n[$h] VT mouse app" -ForegroundColor Yellow
    $elog = Join-Path $dd "echo_$($h -replace ' ','')_$rand.txt"
    $lines = Run-Shape $h @($echo) 'up' $true $elog
    $reports = @($lines | Where-Object { $_ -like 'RECV*' } | ForEach-Object { [regex]::Matches($_, '\[<64;\d+;\d+M').Count } | Measure-Object -Sum).Sum
    if ($reports -eq $N) { Write-Pass "$N notches -> $reports SGR wheel reports" }
    else { Write-Fail "$N notches -> $reports SGR wheel reports" }

    Write-Host "`n[$h] mouse off: the reader gets nothing" -ForegroundColor Yellow
    $lines = Run-Shape $h @($nat) 'up' $false $null
    $wheel = @($lines | Where-Object { $_ -match '^MOUSE' })
    if ($wheel.Count -eq 0) { Write-Pass "no records with mouse off" }
    else { Write-Fail "$($wheel.Count) records with mouse off" }
}

if ($savedConpty) { $env:PSMUX_CONPTY_DIR = $savedConpty } else { Remove-Item Env:PSMUX_CONPTY_DIR -EA SilentlyContinue }
if ($ownDataDir) { Remove-Item $dd -Recurse -Force -EA SilentlyContinue }
Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)"
Write-Host "  Failed: $($script:TestsFailed)"
exit $(if ($script:TestsFailed -gt 0) { 1 } else { 0 })
