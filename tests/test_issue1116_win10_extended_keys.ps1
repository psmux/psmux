# Issue #1116: on the Windows 10 inbox conhost, a program in an extended key mode gets no modified key from psmux.
#
# A program asks for an extended key mode with CSI > 4 ; 1 m or CSI > 4 ; 2 m
# (nvim does at startup), and with `extended-keys on` or `always` psmux then
# writes modified keys in the extended form, CSI 27;m;k~ (xterm format) or
# CSI k;m u (csi-u), as tmux does (#860).  The inbox conhost of Windows 10
# (10.0.19041.320 on 19045) drops both forms from its input: the program read
# nothing for C-a, M-a, C-1, S-Enter or C-BSpace, while CSI 1;5D and plain
# text arrived.  On an inbox host before build 22000 psmux now writes the
# legacy bytes, which that host hands on.  Windows 11 and a host supplied with
# PSMUX_CONPTY_DIR (OpenConsole) pass the extended forms, and keep them.
#
# The pane is a VT input reader (tests\rec860.cs) that writes the request
# followed by text: that host holds a request written on its own (as it does
# the queries of #1086), and text after it lets it through.
#
# Test 1: send-keys in mode 2, xterm format
# Test 2: send-keys in mode 2, csi-u format
# Test 3: send-keys under `extended-keys always`, mode 1, with no request
# Test 4: real key records through a real attached client, mode 2
#
# Isolation: own -L namespace, own PSMUX_DATA_DIR, cleanup with
# `-L <ns> kill-server` and by PID.  PSMUX_TEST_BIN picks the binary (default: psmux on PATH).
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue1116_win10_extended_keys.ps1
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } elseif ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$EXE_NAME = [IO.Path]::GetFileName($PSMUX)
$NS = "w10ek$PID"
$script:TestsPassed = 0; $script:TestsFailed = 0
$script:Started = @()
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }

foreach ($v in 'PSMUX_SESSION_NAME', 'PSMUX_SESSION', 'PSMUX_PANE', 'TMUX', 'TMUX_PANE') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm = $env:PSMUX_NO_WARM
$root = Join-Path $env:TEMP "psmux_w10ek_$PID"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $root "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_NO_WARM = "1"
$env:PSMUX_RUN_NOPAUSE = "1"

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$REC = Join-Path $root "rec860.exe"
$INJECT = Join-Path $root "recbatch860.exe"
& $csc /nologo /platform:x64 /out:$REC "$PSScriptRoot\rec860.cs" 2>&1 | Out-Null
& $csc /nologo /platform:x64 /out:$INJECT "$PSScriptRoot\recbatch860.cs" 2>&1 | Out-Null
if (-not (Test-Path $REC) -or -not (Test-Path $INJECT)) { Write-Host "could not build the C# helpers" -ForegroundColor Red; exit 1 }

# The Windows 10 inbox host, which drops the extended forms.
$build = [Environment]::OSVersion.Version.Build
$inboxWin10 = $build -lt 22000 -and -not $env:PSMUX_CONPTY_DIR
Write-Info ("psmux: {0}  ({1})  build={2}  inbox Windows 10 host: {3}" -f $PSMUX, ((& $PSMUX -V) -join ' '), $build, $inboxWin10)

$E = [char]27
function Hex([string]$s) { (([Text.Encoding]::ASCII.GetBytes($s) | ForEach-Object { '{0:X2}' -f $_ }) -join ' ') }
function Conf($name, $text) { $p = Join-Path $root "$name.conf"; $text | Set-Content $p -Encoding ASCII; $p }
function Stream($log) {
    $b = @()
    foreach ($l in (Get-Content $log -EA SilentlyContinue)) {
        if ($l -match '^BYTES .*hex=\[ ([0-9A-Fa-f ]+) \]') { $b += ($Matches[1].Trim() -split '\s+') }
    }
    ($b -join ' ').ToUpper()
}
function Key-Mode($sess) { (& $PSMUX -L $NS display-message -t $sess -p '#{pane_key_mode}' 2>&1 | Out-String).Trim() }

# The request, then text, so the inbox host lets the request through.
$REQUEST = '\e[>4;2mready'

function Start-Rec($sess, $conf, $emit, $tag = '') {
    $log = Join-Path $root "$sess$tag.txt"
    if ($emit) { & $PSMUX -L $NS -f $conf new-session -d -s $sess -x 100 -y 30 -- $REC vt $log $emit 2>&1 | Out-Null }
    else { & $PSMUX -L $NS -f $conf new-session -d -s $sess -x 100 -y 30 -- $REC vt $log 2>&1 | Out-Null }
    for ($i = 0; $i -lt 40 -and -not ((Get-Content $log -EA SilentlyContinue) -match 'READY'); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 1200
    $log
}

function Wait-KeyMode($sess, $want) {
    for ($i = 0; $i -lt 30; $i++) {
        $km = Key-Mode $sess
        if ($km -eq $want) { break }
        Start-Sleep -Milliseconds 100
    }
    $km
}

# Now and then a pane loses the request its program writes at once on a fresh
# server: the text after it is on the screen, but #{pane_key_mode} stays VT10x
# (master d633a2b1 too, about 1 in 30 starts on 26300).  This suite is about
# the keys, so it starts such a pane again, up to three times.
function Start-ModeRec($sess, $conf, $emit, $want) {
    for ($try = 1; $try -le 3; $try++) {
        $log = Start-Rec $sess $conf $emit "_$try"
        $km = Wait-KeyMode $sess $want
        if ($km -eq $want -or $try -eq 3) { break }
        Write-Info "start $($try): #{pane_key_mode} is [$km], the request was lost; starting the pane again"
        & $PSMUX -L $NS kill-session -t $sess 2>&1 | Out-Null
        Start-Sleep -Milliseconds 500
    }
    @{ log = $log; mode = $km }
}

# send-keys each key between '|' (7C) markers; returns the bytes per key.
function Send-Each($sess, $log, $names) {
    & $PSMUX -L $NS send-keys -t $sess '|' 2>&1 | Out-Null
    foreach ($k in $names) {
        & $PSMUX -L $NS send-keys -t $sess $k 2>&1 | Out-Null
        Start-Sleep -Milliseconds 120
        & $PSMUX -L $NS send-keys -t $sess '|' 2>&1 | Out-Null
        Start-Sleep -Milliseconds 120
    }
    Start-Sleep -Milliseconds 900
    $parts = (Stream $log) -split ' ?7C ?'
    $got = @{}
    for ($i = 0; $i -lt $names.Count; $i++) { $got[$names[$i]] = $parts[$i + 1] }
    $got
}

# key -> legacy bytes, mode 2 xterm, mode 2 csi-u, mode 1 xterm.
$table = [ordered]@{
    'C-a'      = @{ L = '01';    X2 = (Hex "$E[27;5;97~");  U2 = (Hex "$E[97;5u");  X1 = '01' }
    'M-a'      = @{ L = '1B 61'; X2 = (Hex "$E[27;3;97~");  U2 = (Hex "$E[97;3u");  X1 = '1B 61' }
    'C-1'      = @{ L = '31';    X2 = (Hex "$E[27;5;49~");  U2 = (Hex "$E[49;5u");  X1 = (Hex "$E[27;5;49~") }
    'C-i'      = @{ L = '09';    X2 = (Hex "$E[27;5;105~"); U2 = (Hex "$E[105;5u"); X1 = '09' }
    "M-'"      = @{ L = '1B 27'; X2 = (Hex "$E[27;3;39~");  U2 = (Hex "$E[39;3u");  X1 = '1B 27' }
    'S-Enter'  = @{ L = '1B 0D'; X2 = (Hex "$E[27;2;13~");  U2 = (Hex "$E[13;2u");  X1 = (Hex "$E[27;2;13~") }
    'C-BSpace' = @{ L = '08';    X2 = (Hex "$E[27;5;127~"); U2 = (Hex "$E[127;5u"); X1 = (Hex "$E[27;5;127~") }
}
$names = @($table.Keys)

function Check-Keys($label, $got, $col) {
    foreach ($k in $names) {
        $want = if ($inboxWin10) { $table[$k]['L'] } else { $table[$k][$col] }
        $how = if ($inboxWin10) { 'legacy' } else { 'extended' }
        if ($got[$k] -eq $want) { Write-Pass "$label $k -> $how [$want]" }
        else { Write-Fail "$label $k got [$($got[$k])] want $how [$want]" }
    }
}

function Start-Attached($sess, $conf, $pane) {
    $cmd = Join-Path $root "launch_$sess.cmd"
    @"
@echo off
set PSMUX_SESSION_NAME=
set PSMUX_SESSION=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set PSMUX_DATA_DIR=$($env:PSMUX_DATA_DIR)
set PSMUX_NO_WARM=1
"$PSMUX" -L $NS -f "$conf" new-session -s $sess -x 100 -y 30 -- $pane
"@ | Set-Content $cmd -Encoding ASCII
    $h = Start-Process conhost.exe -ArgumentList "cmd /c `"$cmd`"" -PassThru
    $script:Started += $h.Id
    $c = $null
    for ($i = 0; $i -lt 60 -and -not $c; $i++) {
        Start-Sleep -Milliseconds 250
        $c = Get-CimInstance Win32_Process -Filter "Name='$EXE_NAME'" |
            Where-Object { $_.CommandLine -match "-L $NS " -and $_.CommandLine -match "new-session -s $sess " } |
            Select-Object -ExpandProperty ProcessId -First 1
    }
    if ($c) { $script:Started += $c }
    @{ host = $h.Id; client = $c }
}
function Wait-Pump($cpid, $log, $pattern) {
    for ($t = 0; $t -lt 10; $t++) {
        & $INJECT $cpid "g:30.30.0" | Out-Null
        Start-Sleep -Milliseconds 600
        if ((Get-Content $log -EA SilentlyContinue) -match $pattern) { return $true }
    }
    $false
}

try {
    Write-Host "`n=== Issue #1116: extended keys on the Windows 10 inbox host ===" -ForegroundColor Cyan

    Write-Host "`n[Test 1] send-keys, mode 2, xterm format" -ForegroundColor Yellow
    $recPane = Start-ModeRec 'x2' (Conf 'x2' 'set -g extended-keys on') $REQUEST 'Ext 2'
    if ($recPane.mode -eq 'Ext 2') { Write-Pass "the pane asked for mode 2: #{pane_key_mode} is Ext 2" } else { Write-Fail "#{pane_key_mode} is [$($recPane.mode)], want [Ext 2]" }
    Check-Keys 'xterm' (Send-Each 'x2' $recPane.log $names) 'X2'
    & $PSMUX -L $NS kill-session -t x2 2>&1 | Out-Null

    Write-Host "`n[Test 2] send-keys, mode 2, csi-u format" -ForegroundColor Yellow
    $recPane = Start-ModeRec 'u2' (Conf 'u2' "set -g extended-keys on`nset -g extended-keys-format csi-u") $REQUEST 'Ext 2'
    Check-Keys 'csi-u' (Send-Each 'u2' $recPane.log $names) 'U2'
    & $PSMUX -L $NS kill-session -t u2 2>&1 | Out-Null

    Write-Host "`n[Test 3] send-keys under extended-keys always, no request" -ForegroundColor Yellow
    $log = Start-Rec 'a1' (Conf 'a1' 'set -g extended-keys always') ''
    $km = Key-Mode 'a1'
    if ($km -eq 'Ext 1') { Write-Pass "always puts the pane in mode 1: #{pane_key_mode} is Ext 1" } else { Write-Fail "#{pane_key_mode} is [$km], want [Ext 1]" }
    Check-Keys 'always' (Send-Each 'a1' $log $names) 'X1'
    & $PSMUX -L $NS kill-session -t a1 2>&1 | Out-Null
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500

    Write-Host "`n[Test 4] real key records through an attached client, mode 2" -ForegroundColor Yellow
    $attConf = Conf 'att' 'set -g extended-keys on'
    for ($try = 1; $try -le 3; $try++) {
        $log = Join-Path $root "att_$try.txt"
        $a = Start-Attached 'att' $attConf "`"$REC`" vt `"$log`" `"$REQUEST`""
        $ok = $a.client -and (Wait-Pump $a.client $log 'hex=\[ 30 \]')
        if (-not $ok) { break }
        # The lost request of Start-ModeRec, with an attached client.
        $km = Wait-KeyMode 'att' 'Ext 2'
        if ($km -eq 'Ext 2' -or $try -eq 3) { break }
        Write-Info "start $($try): #{pane_key_mode} is [$km], the request was lost; starting the client again"
        & $PSMUX -L $NS kill-server 2>&1 | Out-Null
        foreach ($p in $a.host, $a.client) { if ($p) { Stop-Process -Id $p -Force -EA SilentlyContinue } }
        Start-Sleep -Milliseconds 500
    }
    if (-not $ok) {
        Write-Fail "the attached client did not start or the pane never read input"
    } else {
        # Records (vk.char.ctrl): 1 Ctrl+A 2 Alt+A 3 Shift+Enter 4 Ctrl+Backspace 5, digits as markers.
        & $INJECT $a.client "g:31.31.0" "sleep:250" "g:41.1.8" "sleep:250" "g:32.32.0" "sleep:250" "g:41.61.2" "sleep:250" `
            "g:33.33.0" "sleep:250" "g:0D.0D.10" "sleep:250" "g:34.34.0" "sleep:250" "g:08.7F.8" "sleep:250" "g:35.35.0" "sleep:500" | Out-Null
        Start-Sleep -Milliseconds 1200
        $s = Stream $log
        foreach ($r in @(@('Ctrl+A', '31', '32', 'C-a'), @('Alt+A', '32', '33', 'M-a'), @('Shift+Enter', '33', '34', 'S-Enter'), @('Ctrl+Backspace', '34', '35', 'C-BSpace'))) {
            $bytes = if ($inboxWin10) { $table[$r[3]]['L'] } else { $table[$r[3]]['X2'] }
            $want = "$($r[1]) $bytes $($r[2])"
            $how = if ($inboxWin10) { 'legacy' } else { 'extended' }
            if ($s.Contains($want)) { Write-Pass "attached: $($r[0]) -> $how [$bytes]" } else { Write-Fail "attached: $($r[0]) want [$want] in [$s]" }
        }
    }
}
finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    foreach ($p in $script:Started) { Stop-Process -Id $p -Force -EA SilentlyContinue }
    $env:PSMUX_DATA_DIR = $savedDataDir
    $env:PSMUX_NO_WARM = $savedNoWarm
    for ($i = 0; $i -lt 10 -and (Test-Path $root); $i++) {
        Start-Sleep -Milliseconds 300
        Remove-Item -Recurse -Force $root -EA SilentlyContinue
    }
}

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
exit $script:TestsFailed
