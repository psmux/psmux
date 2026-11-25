# Issue #1089: on Windows 10 a program that reads its mouse as console
# records gets the wheel, clicks and drags.
#
# On a build below CONPTY_MOUSE_MIN_BUILD (22523) the inbox console host never
# tells psmux that a pane program wants the mouse: it passes on no DECSET, no
# ?1049h, and does not publish ENABLE_MOUSE_INPUT as ?1003;1006h the way the
# newer hosts do. psmux's wheel and click gates waited for one of those, so a
# record reader (crossterm, ratatui, Bubble Tea, Far Manager, vim) had the
# wheel taken for copy mode and its clicks dropped, although the record
# channel #597 widened for these builds delivers them. Measured on 19045 at
# master 2e0c2e1b: 3 notches, 0 records and copy mode; with @mouse-force on,
# 3 notches, 3 records.
#
# Each case attaches a real client in a conhost window and writes real mouse
# records into the client's console (what a physical mouse produces):
#   record reader            native_mouse_child, wheel up 3 notches
#   record reader, VT input  native_mouse_child vti (Bubble Tea), wheel up
#   record reader, click     a left press and release
#   record reader, drag      a left drag: the client must hand it over too,
#                            which it does only for a leaf marked wants_mouse
#   shell prompt             pwsh -NoProfile: the wheel still enters copy mode
#   mouse off                the reader gets nothing
#   reader exits             run from a pwsh prompt, then quit: the wheel at
#                            the prompt enters copy mode again, although the
#                            reader may leave its console mode behind
# It expects the same on every build: on 22523 and above the host publishes the
# reader's mode, so these passed there already.
#
# Every psmux call uses its own random -L namespace and PSMUX_DATA_DIR.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } elseif ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$Tests = Split-Path -Parent $MyInvocation.MyCommand.Path
$rand  = Get-Random -Maximum 999999
$N = 3

$ownDataDir = $false
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP "psmux_rec_mouse_$rand"
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
    $ownDataDir = $true
}
$dd = $env:PSMUX_DATA_DIR
$savedConpty = $env:PSMUX_CONPTY_DIR
Remove-Item Env:PSMUX_CONPTY_DIR -EA SilentlyContinue

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$nat   = Join-Path $dd "native_mouse_child.exe"
$wheel = Join-Path $dd "mouse_injector.exe"
$click = Join-Path $dd "click_injector.exe"
$drag  = Join-Path $dd "mouse_drag_injector.exe"
& $csc /nologo /out:$nat   (Join-Path $Tests "native_mouse_child.cs") | Out-Null
& $csc /nologo /out:$wheel (Join-Path $Tests "mouse_injector.cs") | Out-Null
& $csc /nologo /out:$click (Join-Path $Tests "click_injector.cs") | Out-Null
& $csc /nologo /out:$drag  (Join-Path $Tests "mouse_drag_injector.cs") | Out-Null
$natLog = Join-Path $env:TEMP "psmux_native_mouse.txt"
Write-Host "build $([Environment]::OSVersion.Version.Build), $((& $PSMUX -V) -join ' ')"

# Start a session whose pane runs $cmd, attach a client in a conhost window,
# run $act with the client's PID, and return the reader's log lines and
# #{pane_in_mode} afterwards.
function Run-Case([string[]]$cmd, [bool]$mouse, [scriptblock]$act) {
    $ns = "recm$rand$(Get-Random -Maximum 9999)"
    Remove-Item $natLog -Force -EA SilentlyContinue
    & $PSMUX -L $ns new-session -d -s w -x 100 -y 30 @cmd | Out-Null
    Start-Sleep -Seconds 3
    & $PSMUX -L $ns set -g mouse $(if ($mouse) { 'on' } else { 'off' }) | Out-Null
    $proc = Start-Process -FilePath "$env:WINDIR\System32\conhost.exe" -ArgumentList "`"$PSMUX`"","-L",$ns,"attach","-t","w" -PassThru
    try {
        Start-Sleep -Seconds 4
        $c = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($proc.Id)" | Where-Object { $_.Name -eq 'psmux.exe' } | Select-Object -First 1
        if (-not $c) { return $null }
        & $act ([int]$c.ProcessId) $ns
        Start-Sleep -Seconds 1
        $mode = ((& $PSMUX -L $ns display-message -p -t w '#{pane_in_mode}') -join '').Trim()
        $log = if (Test-Path $natLog) { @(Get-Content $natLog) } else { @() }
        return [pscustomobject]@{ Log = $log; InMode = $mode }
    } finally {
        & $PSMUX -L $ns kill-server 2>&1 | Out-Null
        Start-Sleep -Milliseconds 800
        if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue }
    }
}
$notches = { param($id) for ($i = 0; $i -lt $N; $i++) { & $wheel $id up 1 20 8 | Out-Null; Start-Sleep -Milliseconds 500 } }

Write-Host "`n[1] record reader, VT input off: the wheel" -ForegroundColor Yellow
$r = Run-Case @($nat) $true $notches
if ($null -eq $r) { Write-Fail "client did not start" }
else {
    $w = @($r.Log | Where-Object { $_ -match '^MOUSE .*flags=0x4 wheel=\d' })
    if ($w.Count -eq $N) { Write-Pass "$N notches -> $($w.Count) wheel records" }
    else { Write-Fail "$N notches -> $($w.Count) wheel records" }
    if ($r.InMode -eq '0') { Write-Pass "the pane did not enter copy mode" }
    else { Write-Fail "the wheel was taken for copy mode (pane_in_mode=$($r.InMode))" }
}

Write-Host "`n[2] record reader, VT input on (Bubble Tea): the wheel" -ForegroundColor Yellow
$r = Run-Case @($nat, 'vti') $true $notches
if ($null -eq $r) { Write-Fail "client did not start" }
else {
    $w = @($r.Log | Where-Object { $_ -match '^MOUSE .*flags=0x4 wheel=\d' })
    if ($w.Count -eq $N) { Write-Pass "$N notches -> $($w.Count) wheel records" }
    else { Write-Fail "$N notches -> $($w.Count) wheel records" }
    if ($r.InMode -eq '0') { Write-Pass "the pane did not enter copy mode" }
    else { Write-Fail "the wheel was taken for copy mode (pane_in_mode=$($r.InMode))" }
}

Write-Host "`n[3] record reader: a left click" -ForegroundColor Yellow
$r = Run-Case @($nat) $true { param($id) & $click $id 20 8 | Out-Null; Start-Sleep -Milliseconds 500 }
if ($null -eq $r) { Write-Fail "client did not start" }
else {
    $press = @($r.Log | Where-Object { $_ -match '^MOUSE .*buttons=0x1 flags=0x0 ' })
    $release = @($r.Log | Where-Object { $_ -match '^MOUSE .*buttons=0x0 flags=0x0 ' })
    if ($press.Count -ge 1) { Write-Pass "the press arrived: $($press[0])" }
    else { Write-Fail "no press record: $(($r.Log | Where-Object { $_ -match '^MOUSE' }) -join ' / ')" }
    if ($release.Count -ge 1) { Write-Pass "the release arrived: $($release[0])" }
    else { Write-Fail "no release record: $(($r.Log | Where-Object { $_ -match '^MOUSE' }) -join ' / ')" }
}

Write-Host "`n[4] record reader: a left drag" -ForegroundColor Yellow
$r = Run-Case @($nat) $true { param($id) & $drag $id "drag" 10 5 30 12 8 60 | Out-Null; Start-Sleep -Milliseconds 800 }
if ($null -eq $r) { Write-Fail "client did not start" }
else {
    $moves = @($r.Log | Where-Object { $_ -match '^MOUSE .*buttons=0x1 flags=0x1 ' })
    if ($moves.Count -ge 1) { Write-Pass "$($moves.Count) drag records arrived, the first: $($moves[0])" }
    else { Write-Fail "no drag record: $(($r.Log | Where-Object { $_ -match '^MOUSE' }) -join ' / ')" }
}

Write-Host "`n[5] shell prompt: the wheel still enters copy mode" -ForegroundColor Yellow
$r = Run-Case @('pwsh', '-NoProfile') $true { param($id) & $wheel $id up 1 20 8 | Out-Null; Start-Sleep -Milliseconds 500 }
if ($null -eq $r) { Write-Fail "client did not start" }
elseif ($r.InMode -eq '1') { Write-Pass "the wheel over a pwsh prompt entered copy mode" }
else { Write-Fail "the wheel over a pwsh prompt did not enter copy mode (pane_in_mode=$($r.InMode))" }

Write-Host "`n[6] mouse off: the reader gets nothing" -ForegroundColor Yellow
$r = Run-Case @($nat) $false $notches
if ($null -eq $r) { Write-Fail "client did not start" }
else {
    $m = @($r.Log | Where-Object { $_ -match '^MOUSE' })
    if ($m.Count -eq 0) { Write-Pass "no records with mouse off" }
    else { Write-Fail "$($m.Count) records with mouse off" }
}

Write-Host "`n[7] a record reader exits: the wheel at the prompt enters copy mode again" -ForegroundColor Yellow
$r = Run-Case @('pwsh', '-NoProfile') $true {
    param($id, $ns)
    & $PSMUX -L $ns send-keys -t w "& '$nat'" Enter | Out-Null
    Start-Sleep -Seconds 3
    & $wheel $id up 1 20 8 | Out-Null; Start-Sleep -Milliseconds 600
    & $PSMUX -L $ns send-keys -t w C-z | Out-Null    # native_mouse_child quits on Ctrl+Z
    Start-Sleep -Seconds 4
    & $wheel $id up 1 20 8 | Out-Null; Start-Sleep -Milliseconds 600
}
if ($null -eq $r) { Write-Fail "client did not start" }
else {
    $w = @($r.Log | Where-Object { $_ -match '^MOUSE .*flags=0x4 wheel=\d' })
    if ($w.Count -eq 1 -and ($r.Log -contains 'NATIVE_MOUSE END')) { Write-Pass "the reader got the notch while it ran, then quit" }
    else { Write-Fail "the reader got $($w.Count) wheel records and $(if ($r.Log -contains 'NATIVE_MOUSE END') { 'quit' } else { 'did not quit' })" }
    if ($r.InMode -eq '1') { Write-Pass "the wheel at the prompt after it entered copy mode" }
    else { Write-Fail "the wheel at the prompt after the reader quit did not enter copy mode (pane_in_mode=$($r.InMode))" }
}

if ($savedConpty) { $env:PSMUX_CONPTY_DIR = $savedConpty }
if ($ownDataDir) { Remove-Item $dd -Recurse -Force -EA SilentlyContinue }
Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)"
Write-Host "  Failed: $($script:TestsFailed)"
exit $script:TestsFailed
