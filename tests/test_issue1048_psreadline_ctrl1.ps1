# PR #1048: Ctrl+1 reached PSReadLine as a bare '1'.
#
# PSReadLine reads keys with Console.ReadKey in console mode 0x01E4: LINE_INPUT
# and VIRTUAL_TERMINAL_INPUT clear, ECHO_INPUT still set.  ECHO has no effect on
# ReadConsoleInput (and none at all without LINE_INPUT), yet psmux required it
# clear before it delivered Ctrl+digit as a key record, so a Ctrl+1 binding never
# ran and '1' was inserted instead.
#
# Measured before the fix, child in mode 0x01E4:
#   send-keys C-1    -> KEY down vk=0x31 ch=0x0031 ctrl=0x0000   (a plain '1')
#   injected Ctrl+1  -> KEY down vk=0x31 ch=0x0031 ctrl=0x0000
# After the fix both give vk=0x31 ch=0x0000 ctrl=0x0008, as a native console does.
#
# Test 1: tests/record_key_child.cs in PSReadLine's mode (0x01E4) and with ECHO
#         off (0x01E0) gets Ctrl+1 with LEFT_CTRL_PRESSED; plain 1 stays a '1'.
# Test 2: a VT reader (0x03E4) keeps tmux's standard byte '1' for C-1.
# Test 3: real pwsh + PSReadLine with a Ctrl+1 key handler: the handler runs and
#         no '1' is inserted; plain 1 still types.
#
# Isolation: own -L namespace per run, own PSMUX_DATA_DIR, cleanup only with
# `-L <ns> kill-server` and by the PID of the client this script started.
# PSMUX_TEST_BIN picks the binary (default: psmux on PATH).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } elseif ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:Passed = 0
$script:Failed = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Passed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Failed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor DarkGray }

$root = Join-Path $env:TEMP "psmux_1048_$(Get-Random)"
New-Item -ItemType Directory -Force $root | Out-Null
$saved = @{}
foreach ($v in 'PSMUX_DATA_DIR', 'PSMUX_NO_WARM', 'PSMUX_RECORD_LOG', 'PSMUX_RECORD_MODE', 'PSMUX_RUN_NOPAUSE') { $saved[$v] = [Environment]::GetEnvironmentVariable($v, 'Process') }
$env:PSMUX_DATA_DIR = $root
$env:PSMUX_NO_WARM = '1'
$env:PSMUX_RUN_NOPAUSE = '1'
$conf = Join-Path $root 'empty.conf'
'' | Set-Content -Path $conf -Encoding ASCII

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$injector = Join-Path $root 'injector.exe'
$recChild = Join-Path $root 'reckey.exe'
& $csc /nologo /platform:x64 /out:$injector "$PSScriptRoot\injector.cs" 2>&1 | Out-Null
& $csc /nologo /platform:x64 /out:$recChild "$PSScriptRoot\record_key_child.cs" 2>&1 | Out-Null
if (-not (Test-Path $injector) -or -not (Test-Path $recChild)) {
    Write-Host "  [FAIL] could not build the C# harnesses with $csc" -ForegroundColor Red
    exit 1
}

$script:Opened = @()
$script:Namespaces = @()

function Start-Client($ns, [string[]]$cmd) {
    $argv = @('-L', $ns, '-f', $conf, 'new-session', '-s', 'p', '-x', '100', '-y', '30', '--') + $cmd
    $p = Start-Process -FilePath $PSMUX -PassThru -ArgumentList $argv
    $script:Opened += $p
    $script:Namespaces += $ns
    $p
}
function Stop-Client($ns, $p) {
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -EA SilentlyContinue }
}

# Starts the record logger in $mode, then returns the first KEY down record
# (modifier key records skipped) that each action produced.
function Probe-Records($mode, $actions) {
    $ns = "t1048r$(Get-Random)"
    $log = Join-Path $root "rec_$mode.txt"
    $env:PSMUX_RECORD_LOG = $log
    $env:PSMUX_RECORD_MODE = $mode
    $client = Start-Client $ns @($recChild)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 10000 -and -not (Test-Path $log)) { Start-Sleep -Milliseconds 100 }
    Start-Sleep -Milliseconds 1500
    $out = [ordered]@{ mode = ((Get-Content $log -EA SilentlyContinue | Where-Object { $_ -match '^mode ' }) -join '') }
    $seen = @(Get-Content $log -EA SilentlyContinue).Count
    foreach ($a in $actions) {
        if ($a -like 'INJ:*') { & $injector $client.Id $a.Substring(4) 2>&1 | Out-Null }
        else { & $PSMUX -L $ns send-keys -t p $a 2>&1 | Out-Null }
        Start-Sleep -Milliseconds 700
        $all = @(Get-Content $log -EA SilentlyContinue)
        $new = @($all | Select-Object -Skip $seen | Where-Object { $_ -match '^KEY down' -and $_ -notmatch 'vk=0x1[012] ' })
        $seen = $all.Count
        $out[$a] = if ($new.Count) { $new[0] } else { '<none>' }
    }
    Stop-Client $ns $client
    $out
}

try {
    Write-Host "`n=== PR #1048: Ctrl+1 reaches a key record reader with ECHO set ===" -ForegroundColor Cyan

    Write-Host "`n[Test 1] record reader in PSReadLine's mode and with ECHO off" -ForegroundColor Yellow
    foreach ($mode in '01E4', '01E0') {
        $r = Probe-Records $mode @('C-1', 'INJ:{RAW:31:0000:0008}', '1')
        if ($r.mode -notmatch "-> 0x$mode") { Write-Skip "the child could not take mode 0x$mode ($($r.mode))"; continue }
        foreach ($k in 'C-1', 'INJ:{RAW:31:0000:0008}') {
            if ($r[$k] -match 'vk=0x31' -and $r[$k] -match 'ch=0x0000' -and $r[$k] -match 'ctrl=0x0008') { Write-Pass "0x$mode ${k}: $($r[$k])" }
            else { Write-Fail "0x$mode ${k}: modifier lost -> $($r[$k])" }
        }
        if ($r['1'] -match 'vk=0x31' -and $r['1'] -match 'ch=0x0031' -and $r['1'] -match 'ctrl=0x0000') { Write-Pass "0x$mode plain 1 stays '1'" }
        else { Write-Fail "0x$mode plain 1 -> $($r['1'])" }
    }

    Write-Host "`n[Test 2] a VT reader keeps tmux's standard byte for C-1" -ForegroundColor Yellow
    $r = Probe-Records '03E4' @('C-1')
    if ($r.mode -notmatch '-> 0x03E4') { Write-Skip "the child could not take mode 0x03E4 ($($r.mode))" }
    elseif ($r['C-1'] -match 'ch=0x0031' -and $r['C-1'] -match 'ctrl=0x0000') { Write-Pass "VT reader C-1 -> byte '1' ($($r['C-1']))" }
    else { Write-Fail "VT reader C-1 changed -> $($r['C-1'])" }

    Write-Host "`n[Test 3] PSReadLine runs a Ctrl+1 key handler" -ForegroundColor Yellow
    $pwsh = (Get-Command pwsh -EA SilentlyContinue).Source
    if (-not $pwsh) { Write-Skip "pwsh is not installed" }
    else {
        $ns = "t1048p$(Get-Random)"
        $hit = Join-Path $root 'hit.txt'
        $init = Join-Path $root 'init.ps1'
        "Set-PSReadLineOption -PredictionSource None; Set-PSReadLineKeyHandler -Chord Ctrl+1 -ScriptBlock { Add-Content -LiteralPath '$hit' 'HIT' }; 'PSRL_READY'" | Set-Content $init
        $client = Start-Client $ns @($pwsh, '-NoProfile', '-NoExit', '-File', $init)
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $ready = $false
        while ($sw.ElapsedMilliseconds -lt 20000) {
            if (((& $PSMUX -L $ns capture-pane -p -t p 2>$null) -join "`n") -match 'PSRL_READY') { $ready = $true; break }
            Start-Sleep -Milliseconds 200
        }
        Start-Sleep -Milliseconds 1000
        function Hits { if (Test-Path $hit) { @(Get-Content $hit).Count } else { 0 } }
        function PromptLine { ((& $PSMUX -L $ns capture-pane -p -t p 2>$null) | Where-Object { $_ -match '^PS .*>' } | Select-Object -Last 1) }
        if (-not $ready) { Write-Fail "pwsh never printed PSRL_READY" }
        else {
            & $PSMUX -L $ns send-keys -t p C-1; Start-Sleep -Milliseconds 800
            $h1 = Hits; $l1 = PromptLine
            if ($h1 -eq 1 -and $l1 -notmatch '>\s*1') { Write-Pass "send-keys C-1 ran the binding (hits=$h1)" }
            else { Write-Fail "send-keys C-1: hits=$h1 prompt='$l1' (a '1' means the modifier was lost)" }
            & $injector $client.Id '{RAW:31:0000:0008}' 2>&1 | Out-Null; Start-Sleep -Milliseconds 800
            $h2 = Hits; $l2 = PromptLine
            if ($h2 -eq 2 -and $l2 -notmatch '>\s*1') { Write-Pass "a real Ctrl+1 keystroke ran the binding (hits=$h2)" }
            else { Write-Fail "injected Ctrl+1: hits=$h2 prompt='$l2'" }
            & $PSMUX -L $ns send-keys -t p 1; Start-Sleep -Milliseconds 800
            $h3 = Hits; $l3 = PromptLine
            if ($h3 -eq 2 -and $l3 -match '>\s*1\s*$') { Write-Pass "plain 1 still types '1'" }
            else { Write-Fail "plain 1: hits=$h3 prompt='$l3'" }
        }
        Stop-Client $ns $client
    }
}
finally {
    foreach ($ns in $script:Namespaces) { & $PSMUX -L $ns kill-server 2>&1 | Out-Null }
    foreach ($p in $script:Opened) { if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -EA SilentlyContinue } }
    foreach ($v in $saved.Keys) { if ($null -eq $saved[$v]) { Remove-Item "Env:\$v" -EA SilentlyContinue } else { Set-Item "Env:\$v" $saved[$v] } }
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

Write-Host "`n=== Results: $($script:Passed) passed, $($script:Failed) failed ===" -ForegroundColor Cyan
if ($script:Failed -gt 0) { exit 1 }
exit 0
