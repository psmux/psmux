# Issue #1072: a wheel notch is lost for a pane app that reads its mouse as
# records with VT input on (the Bubble Tea shape, #277).
#
# psmux writes each notch to the pane's ConPTY pipe as SGR, which such an app
# gets as KEY_EVENT text, and injects a MOUSE_EVENT record, its only wheel
# event. The injection attaches psmux to the pane's console first. An attach
# right after psmux had attached to and left the same console (the console
# mode checks that run before the first notch and again 2 s later) failed
# now and then with ERROR_INVALID_FUNCTION (1), and free_and_attach retried
# only error 5, so the record was dropped: the notch's SGR text arrived, its
# record did not. Measured on build 26300 before the fix: the 1st or the 5th
# of 5 notches lost in 9 of 12 rounds (twice), on the inbox conhost and on
# OpenConsole alike; after it 12 of 12 got 5 (twice).
#
# Each round starts a session whose pane runs native_mouse_child vti, attaches
# a client in a conhost window, sends N real notches 500 ms apart with
# WriteConsoleInput into that window (mouse_injector), and then expects N
# wheel records of psmux's own delta (120) in the right direction. Every psmux
# call uses its own random -L namespace and PSMUX_DATA_DIR.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$Tests = Split-Path -Parent $MyInvocation.MyCommand.Path
$rand = Get-Random -Maximum 999999
$N = 5
$Rounds = 6

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow }

$ownDataDir = $false
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP "psmux_attach_wheel_$rand"
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
    $ownDataDir = $true
}
$dd = $env:PSMUX_DATA_DIR
$occDir = if ($env:PSMUX_TEST_CONPTY_DIR) { $env:PSMUX_TEST_CONPTY_DIR } else { Join-Path $env:TEMP "occ-conpty\nugetstable" }
$savedConpty = $env:PSMUX_CONPTY_DIR

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$nat = Join-Path $dd "native_mouse_child.exe"
$inj = Join-Path $dd "mouse_injector.exe"
& $csc /nologo /out:$nat (Join-Path $Tests "native_mouse_child.cs") | Out-Null
& $csc /nologo /out:$inj (Join-Path $Tests "mouse_injector.cs") | Out-Null
if (-not (Test-Path $nat) -or -not (Test-Path $inj)) { Write-Fail "could not compile the helpers"; exit 1 }
$natLog = Join-Path $env:TEMP "psmux_native_mouse.txt"

function Count-Records([string]$sign) {
    # psmux's record carries delta 120; a host that also turns the SGR into
    # a record (OpenConsole on Windows 10) uses 128, which is not counted.
    if (-not (Test-Path $natLog)) { return 0 }
    $pat = "^MOUSE .*flags=0x4 wheel=${sign}120`$"
    @(Get-Content $natLog | Where-Object { $_ -match $pat }).Count
}

# One round: returns the record count after each notch, or $null when the
# client did not start.
function Run-Round([string]$host_, [string]$dir) {
    $ns = "iatw$rand$(Get-Random -Maximum 9999)"
    if ($host_ -eq 'OpenConsole') { $env:PSMUX_CONPTY_DIR = $occDir }
    elseif ($savedConpty) { $env:PSMUX_CONPTY_DIR = $savedConpty }
    else { Remove-Item Env:PSMUX_CONPTY_DIR -EA SilentlyContinue }
    Remove-Item $natLog -Force -EA SilentlyContinue
    $sign = if ($dir -eq 'up') { '' } else { '-' }
    & $PSMUX -L $ns new-session -d -s w -x 100 -y 30 $nat vti | Out-Null
    Start-Sleep -Seconds 3
    & $PSMUX -L $ns set -g mouse on | Out-Null
    $proc = Start-Process -FilePath "$env:WINDIR\System32\conhost.exe" -ArgumentList "`"$PSMUX`"","-L",$ns,"attach","-t","w" -PassThru
    try {
        Start-Sleep -Seconds 4
        $c = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($proc.Id)" | Where-Object { $_.Name -eq 'psmux.exe' } | Select-Object -First 1
        if (-not $c) { return $null }
        $trail = @()
        for ($i = 0; $i -lt $N; $i++) {
            & $inj ([int]$c.ProcessId) $dir 1 20 8 | Out-Null
            Start-Sleep -Milliseconds 500
            $trail += Count-Records $sign
        }
        Start-Sleep -Seconds 1
        $trail += Count-Records $sign
        , $trail
    } finally {
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
        Start-Sleep -Milliseconds 800
        if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }
    }
}

Write-Host "`n=== Issue #1072: every wheel notch reaches a VT input on record reader ===" -ForegroundColor Cyan
$hosts = @()
if ([Environment]::OSVersion.Version.Build -lt 22000) {
    # The inbox conhost of Windows 10 (19045) hands this app no injected
    # wheel record at all, every notch alike; that is not this race.
    Write-Skip "inbox conhost on Windows 10: no injected wheel record reaches the app there at all"
} else { $hosts += 'inbox conhost' }
if (Test-Path (Join-Path $occDir "OpenConsole.exe")) { $hosts += 'OpenConsole' }
else { Write-Skip "OpenConsole fixture not found at $occDir" }

foreach ($h in $hosts) {
    for ($r = 1; $r -le $Rounds; $r++) {
        $dir = if ($r % 2) { 'up' } else { 'down' }
        Write-Host "`n[$h] round $r, wheel $dir" -ForegroundColor Yellow
        $trail = Run-Round $h $dir
        if ($null -eq $trail) { Write-Fail "[$h] round ${r}: client did not start"; continue }
        $final = $trail[-1]
        $shown = ($trail[0..($N - 1)] -join ' ')
        if ($final -eq $N) { Write-Pass "[$h] round ${r}: $N notches -> $final records (after each notch: $shown)" }
        else { Write-Fail "[$h] round ${r}: $N notches -> $final records (after each notch: $shown)" }
    }
}

if ($savedConpty) { $env:PSMUX_CONPTY_DIR = $savedConpty } else { Remove-Item Env:PSMUX_CONPTY_DIR -EA SilentlyContinue }
if ($ownDataDir) { Remove-Item $dd -Recurse -Force -EA SilentlyContinue }
Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
