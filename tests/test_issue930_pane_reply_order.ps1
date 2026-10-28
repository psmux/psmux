# Issue #930: a pane program's terminal queries are answered in the order it
# asked them, with a client attached too.
#
# psmux answers a pane's queries on two paths: XTVERSION and the colour
# queries are injected into the console input straight off the read, while
# DA1, DSR, DECRQM and the window reports are composed by the parser thread
# and written down the PTY by the server loop.  The read handed its bytes to
# the parser thread before injecting, so the PTY path could win, and with a
# client attached it did nearly every time: the DA1 answer arrived before the
# XTVERSION and OSC 11 answers to queries sent ahead of it, and the XTVERSION
# answer was sometimes lost.  A program that takes the DA1 answer as the end
# of its replies (yazi) then gives up on the rest of its probe.  A terminal,
# and tmux, answer in the order asked.
#
# Needs PSMUX_CONPTY_DIR_FOR_TEST (a directory with conpty.dll and
# OpenConsole.exe): the inbox console host answers DA1 itself, so only an
# OpenConsole pane gets psmux's DA1 answer.  Without it the suite reports SKIP
# and exits 0.  tests/reply_order_probe.cs runs in the pane, sends XTVERSION,
# OSC 11, CSI 16 t and DA1 in one write and records the order of the replies.
#   1. with a client attached, every one of several probes gets all four, in
#      the order asked,
#   2. the same with no client attached.
#
# Isolation: private PSMUX_DATA_DIR and empty config, unique -L namespace on
# every command, cleanup only with `-L <ns> kill-server` and the client this
# suite started.

$ErrorActionPreference = 'Continue'

$conptyDir = $env:PSMUX_CONPTY_DIR_FOR_TEST
if (-not $conptyDir -or -not (Test-Path (Join-Path $conptyDir 'conpty.dll')) -or -not (Test-Path (Join-Path $conptyDir 'OpenConsole.exe'))) {
    Write-Host "SKIP: set PSMUX_CONPTY_DIR_FOR_TEST to a directory with conpty.dll and OpenConsole.exe" -ForegroundColor Yellow
    exit 0
}

$exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$ns = "tro_$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$data = Join-Path ([IO.Path]::GetTempPath()) "psmux_reply_order_$ns"
New-Item -ItemType Directory -Force $data | Out-Null
$saved = @{}
foreach ($k in 'PSMUX_DATA_DIR', 'PSMUX_CONFIG_FILE', 'PSMUX_CONPTY_DIR', 'PSMUX_SESSION', 'TMUX', 'TMUX_PANE') {
    $saved[$k] = [Environment]::GetEnvironmentVariable($k)
    Remove-Item "Env:\$k" -ErrorAction SilentlyContinue
}
$env:PSMUX_DATA_DIR = $data
$env:PSMUX_CONFIG_FILE = Join-Path $data 'empty.conf'
Set-Content -Path $env:PSMUX_CONFIG_FILE -Value '' -Encoding UTF8

$script:TestsPassed = 0
$script:TestsFailed = 0
function Check($name, $ok, $detail) {
    if ($ok) { $script:TestsPassed++; Write-Host "  [PASS] $name  $detail" -ForegroundColor Green }
    else { $script:TestsFailed++; Write-Host "  [FAIL] $name  $detail" -ForegroundColor Red }
}
function P { & $exe -L $ns @args 2>&1 }

$client = $null
function Cleanup {
    & $exe -L $ns kill-server 2>$null | Out-Null
    if ($client -and -not $client.HasExited) { try { Stop-Process -Id $client.Id -Force -EA SilentlyContinue } catch {} }
    Start-Sleep -Milliseconds 500
    foreach ($k in $saved.Keys) {
        if ($null -ne $saved[$k]) { Set-Item "Env:\$k" $saved[$k] } else { Remove-Item "Env:\$k" -ErrorAction SilentlyContinue }
    }
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
}

$csc = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue | Select-Object -Last 1
$probe = Join-Path $data 'reply_order_probe.exe'
if ($csc) { & $csc.FullName /nologo /optimize /out:$probe (Join-Path $PSScriptRoot 'reply_order_probe.cs') 2>&1 | Out-Null }
if (-not (Test-Path $probe)) {
    Check 'build tests\reply_order_probe.cs' $false '(csc.exe unavailable)'
    Cleanup
    exit $script:TestsFailed
}

function WaitSession($s) {
    for ($i = 0; $i -lt 60; $i++) {
        & $exe -L $ns has-session -t $s 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

# Run the probe in session $s and return the reply names in arrival order.
$script:run = 0
function Probe($s) {
    $script:run++
    $out = Join-Path $data "order_$($script:run).txt"
    P send-keys -t $s "& '$probe' '$out'" Enter | Out-Null
    for ($i = 0; $i -lt 40 -and -not (Test-Path $out); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 300
    if (Test-Path $out) { return @(Get-Content $out | Where-Object { $_ }) }
    return @()
}

$want = 'XTVERSION OSC11 CELL DA1'
Write-Host "`n=== pane query replies arrive in the order asked ===" -ForegroundColor Cyan
Write-Host "  exe: $exe ($(& $exe -V 2>$null | Select-Object -Last 1))"

$env:PSMUX_CONPTY_DIR = $conptyDir
P new-session -d -s a -x 120 -y 30 | Out-Null
P new-session -d -s b -x 120 -y 30 | Out-Null
Remove-Item Env:\PSMUX_CONPTY_DIR
if (-not ((WaitSession 'a') -and (WaitSession 'b'))) { Check 'sessions start on OpenConsole' $false ''; Cleanup; exit $script:TestsFailed }
Start-Sleep -Seconds 2

Write-Host "`n--- 1. with a client attached ---" -ForegroundColor Cyan
$client = Start-Process -FilePath $exe -ArgumentList @('-L', $ns, 'attach', '-t', 'a') -PassThru -WindowStyle Minimized
Start-Sleep -Seconds 3
foreach ($i in 1..5) {
    $got = (Probe 'a') -join ' '
    Check "probe $i with a client: $want" ($got -eq $want) "(got: $got)"
}
if ($client -and -not $client.HasExited) { try { Stop-Process -Id $client.Id -Force -EA SilentlyContinue } catch {} }

Write-Host "`n--- 2. with no client ---" -ForegroundColor Cyan
foreach ($i in 1..3) {
    $got = (Probe 'b') -join ' '
    Check "probe $i without a client: $want" ($got -eq $want) "(got: $got)"
}

Cleanup
Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
