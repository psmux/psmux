# Issue #941: a pane's query replies come back in the order asked, whichever
# path each takes.
#
# psmux answered a pane's queries on two paths: XTVERSION and the colour
# queries were injected into the console input straight off the read, while
# DA1, DECRQM and the window reports went down the PTY from the server loop.
# The injected ones always arrived first, so DA1 asked before XTVERSION came
# back after it, and the colour replies came in a fixed order (the scheme
# reply, OSC 10, OSC 11, OSC 4 by index) whatever the order asked.  tmux
# answers each query as its parser meets it (input_reply, input.c:1176 in tmux
# 3.8).  Now the pane's parser finds every query and its thread answers them
# all in one injection, in the order asked.
#
# tests/reply_order_probe.cs runs in the pane, writes the queries of a case in
# one write and records the order of the replies.
#   1. inbox console host: a window report and XTVERSION, and the colour
#      queries with them.  The inbox host passes on only XTVERSION, OSC 4,
#      ?996n and the window reports 14 and 16 (it answers DA1 itself and
#      drops OSC 10 and 11).
#   2. OpenConsole, with PSMUX_CONPTY_DIR_FOR_TEST (a directory with
#      conpty.dll and OpenConsole.exe; skipped without it): DA1 before
#      XTVERSION and OSC 11, with a client attached and without, the colour
#      queries in an order that is not the old fixed one, two palette
#      queries in one OSC 4 (master answered the first only), and yazi's probe,
#      which was already in order (#930).
#
# Isolation: private PSMUX_DATA_DIR and empty config, unique -L namespace on
# every command, cleanup only with `-L <ns> kill-server` and the client this
# suite started.

$ErrorActionPreference = 'Continue'

$exe = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$conptyDir = $env:PSMUX_CONPTY_DIR_FOR_TEST
$ns = "trq_$([guid]::NewGuid().ToString('N').Substring(0, 8))"
$data = Join-Path ([IO.Path]::GetTempPath()) "psmux_reply_queue_$ns"
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

# Run the probe in session $s with $queries (`\e` for ESC) and return the
# reply names in arrival order, space separated.
$script:run = 0
function Probe($s, $queries) {
    $script:run++
    $out = Join-Path $data "order_$($script:run).txt"
    P send-keys -t $s "& '$probe' '$out' '$queries'" Enter | Out-Null
    for ($i = 0; $i -lt 40 -and -not (Test-Path $out); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 300
    if (Test-Path $out) { return (@(Get-Content $out | Where-Object { $_ }) -join ' ') }
    return ''
}

function Case($s, $label, $queries, $want, $times) {
    foreach ($i in 1..$times) {
        $got = Probe $s $queries
        Check "$label, probe ${i}: $want" ($got -eq $want) "(got: $got)"
    }
}

Write-Host "`n=== pane query replies arrive in the order asked ===" -ForegroundColor Cyan
Write-Host "  exe: $exe ($(& $exe -V 2>$null | Select-Object -Last 1))"

Write-Host "`n--- 1. inbox console host ---" -ForegroundColor Cyan
P new-session -d -s i -x 120 -y 30 | Out-Null
if (-not (WaitSession 'i')) { Check 'session starts' $false ''; Cleanup; exit $script:TestsFailed }
Start-Sleep -Seconds 2
Case 'i' 'window report, then XTVERSION' '\e[16t\e[>q' 'CELL XTVERSION' 3
Case 'i' 'scheme, palette, window report, XTVERSION' '\e[?996n\e]4;1;?\e\\e[14t\e[>q' 'SCHEME OSC4:1 AREA XTVERSION' 2

if (-not $conptyDir -or -not (Test-Path (Join-Path $conptyDir 'conpty.dll')) -or -not (Test-Path (Join-Path $conptyDir 'OpenConsole.exe'))) {
    Write-Host "`n--- 2. OpenConsole: SKIP, set PSMUX_CONPTY_DIR_FOR_TEST to a directory with conpty.dll and OpenConsole.exe ---" -ForegroundColor Yellow
} else {
    $env:PSMUX_CONPTY_DIR = $conptyDir
    P new-session -d -s a -x 120 -y 30 | Out-Null
    P new-session -d -s b -x 120 -y 30 | Out-Null
    Remove-Item Env:\PSMUX_CONPTY_DIR
    if (-not ((WaitSession 'a') -and (WaitSession 'b'))) { Check 'sessions start on OpenConsole' $false ''; Cleanup; exit $script:TestsFailed }
    Start-Sleep -Seconds 2

    Write-Host "`n--- 2a. OpenConsole, with a client attached ---" -ForegroundColor Cyan
    $client = Start-Process -FilePath $exe -ArgumentList @('-L', $ns, 'attach', '-t', 'a') -PassThru -WindowStyle Minimized
    Start-Sleep -Seconds 3
    Case 'a' 'DA1 first, with a client' '\e[c\e[>q\e]11;?\e\' 'DA1 XTVERSION OSC11' 3
    Case 'a' "yazi's probe, with a client" '\e[>q\e]11;?\e\\e[16t\e[c' 'XTVERSION OSC11 CELL DA1' 2
    if ($client -and -not $client.HasExited) { try { Stop-Process -Id $client.Id -Force -EA SilentlyContinue } catch {} }

    Write-Host "`n--- 2b. OpenConsole, with no client ---" -ForegroundColor Cyan
    Case 'b' 'DA1 first' '\e[c\e[>q\e]11;?\e\' 'DA1 XTVERSION OSC11' 2
    Case 'b' 'background before scheme' '\e]11;?\e\\e[?996n' 'OSC11 SCHEME' 2
    Case 'b' 'palette 2 before palette 1' '\e]4;2;?\e\\e]4;1;?\e\' 'OSC4:2 OSC4:1' 2
    Case 'b' 'two palette queries in one OSC 4' '\e]4;2;?;1;?\e\' 'OSC4:2 OSC4:1' 2
    Case 'b' 'foreground after DA1 and a mode query' '\e[c\e[?1049$p\e]10;?\e\' 'DA1 \e[?1049;2$y OSC10' 2
}

Cleanup
Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
