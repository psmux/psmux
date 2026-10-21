# Issue #860 (second half): "Neovim not receiving C-Space"
#
# tmux keeps an extended key mode per pane.  An application asks for it with
# CSI > 4 ; 1 m or CSI > 4 ; 2 m (nvim 0.12 sends ; 2 m at startup), the
# request counts only while `extended-keys` is on or always, and `always`
# puts every pane in mode 1.  In mode 2 every modified key with no builtin
# sequence is written in the extended form, CSI 27;m;k~ (extended-keys-format
# xterm, the default) or CSI k;mu (csi-u); in mode 1 the classic control keys
# keep their byte (input-keys.c input_key, input_key_mode1).
#
# Before the fix psmux wrote the legacy byte whatever the pane asked for, so
# Ctrl+Space was always NUL, which nvim on Windows never reads as <C-Space>.
#
# Checked here, each with the bytes a VT input reader in the pane receives:
#   1. send-keys in every mode (off, app 4;1 and 4;2 with the option off,
#      on + 4;1, on + 4;2, always, csi-u, reset by CSI > 4 n and by RIS)
#   2. the legacy table with the mode off, byte for byte
#   3. #{pane_key_mode}
#   4. real console key records through a REAL attached client, in the
#      standard mode, mode 2 and mode 1, after an arrow key latched the pane
#      ConPTY into win32 input mode
#   5. nvim itself: <C-Space> through send-keys and through the client
#
# Every runtime check uses its own -L namespace and PSMUX_DATA_DIR, and every
# process this script starts is stopped by PID.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$EXE_NAME = [IO.Path]::GetFileName($PSMUX)
$NS = "t860x_$PID"
$TMP = Join-Path $env:TEMP "psmux_t860x_$PID"
New-Item -ItemType Directory -Force $TMP | Out-Null
if (-not $env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR = Join-Path $TMP "data" }
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_RUN_NOPAUSE = "1"

$script:Pass = 0
$script:Fail = 0
$script:Skip = 0
$script:Started = @()
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor DarkYellow; $script:Skip++ }

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$REC = Join-Path $TMP "rec860.exe"
$INJECT = Join-Path $TMP "recbatch860.exe"
& $csc /nologo /platform:x64 /out:$REC "$PSScriptRoot\rec860.cs" 2>&1 | Out-Null
& $csc /nologo /platform:x64 /out:$INJECT "$PSScriptRoot\recbatch860.cs" 2>&1 | Out-Null
if (-not (Test-Path $REC) -or -not (Test-Path $INJECT)) { Write-Host "could not build the C# helpers" -ForegroundColor Red; exit 1 }

function Kill-Ns { & $PSMUX -L $NS kill-server 2>&1 | Out-Null; Start-Sleep -Milliseconds 400 }

# The byte STREAM a VT reading pane logged, upper case hex separated by spaces.
function Stream($log) {
    $b = @()
    foreach ($l in (Get-Content $log -EA SilentlyContinue)) {
        if ($l -match '^BYTES .*hex=\[ ([0-9A-Fa-f ]+) \]') { $b += ($Matches[1].Trim() -split '\s+') }
    }
    ($b -join ' ').ToUpper()
}
function Hex([string]$s) { (([Text.Encoding]::ASCII.GetBytes($s) | ForEach-Object { '{0:X2}' -f $_ }) -join ' ') }
function Conf($name, $text) { $p = Join-Path $TMP "$name.conf"; $text | Set-Content $p -Encoding ASCII; $p }

$E = [char]27
# key -> expected bytes per mode.  'L' is the legacy (mode off) byte string,
# '1' mode 1 and '2' mode 2, both in the xterm format.
$table = [ordered]@{
    'C-Space'  = @{ L = '00';       '1' = '00';                        '2' = (Hex "$E[27;5;32~") }
    'C-['      = @{ L = '1B';       '1' = '1B';                        '2' = (Hex "$E[27;5;91~") }
    'C-i'      = @{ L = '09';       '1' = '09';                        '2' = (Hex "$E[27;5;105~") }
    'Tab'      = @{ L = '09';       '1' = '09';                        '2' = '09' }
    'C-m'      = @{ L = '0D';       '1' = '0D';                        '2' = (Hex "$E[27;5;109~") }
    'Enter'    = @{ L = '0D';       '1' = '0D';                        '2' = '0D' }
    'C-a'      = @{ L = '01';       '1' = '01';                        '2' = (Hex "$E[27;5;97~") }
    'M-a'      = @{ L = '1B 61';    '1' = '1B 61';                     '2' = (Hex "$E[27;3;97~") }
    'C-1'      = @{ L = '31';       '1' = (Hex "$E[27;5;49~");         '2' = (Hex "$E[27;5;49~") }
    'C-/'      = @{ L = '1F';       '1' = '1F';                        '2' = (Hex "$E[27;5;47~") }
    'S-Enter'  = @{ L = '1B 0D';    '1' = (Hex "$E[27;2;13~");         '2' = (Hex "$E[27;2;13~") }
    'C-BSpace' = @{ L = '08';       '1' = (Hex "$E[27;5;127~");        '2' = (Hex "$E[27;5;127~") }
    'BTab'     = @{ L = '1B 5B 5A'; '1' = '1B 5B 5A';                  '2' = (Hex "$E[27;2;9~") }
    'C-F5'     = @{ L = (Hex "$E[15;5~"); '1' = (Hex "$E[15;5~");      '2' = (Hex "$E[15;5~") }
    'C-Up'     = @{ L = (Hex "$E[1;5A");  '1' = (Hex "$E[1;5A");       '2' = (Hex "$E[1;5A") }
    'F5'       = @{ L = (Hex "$E[15~");   '1' = (Hex "$E[15~");        '2' = (Hex "$E[15~") }
    'Escape'   = @{ L = '1B';       '1' = '1B';                        '2' = '1B' }
    'BSpace'   = @{ L = '7F';       '1' = '7F';                        '2' = '7F' }
}
$keys = @($table.Keys)

# Start a detached session whose pane is the recorder; $emit is written by
# the recorder before it reads (\e = ESC).  Returns the log path.
function Start-Rec($sess, $conf, $emit) {
    $log = Join-Path $TMP "$sess.txt"
    if ($emit) { & $PSMUX -L $NS -f $conf new-session -d -s $sess -x 100 -y 30 -- $REC vt $log $emit 2>&1 | Out-Null }
    else { & $PSMUX -L $NS -f $conf new-session -d -s $sess -x 100 -y 30 -- $REC vt $log 2>&1 | Out-Null }
    for ($i = 0; $i -lt 40 -and -not ((Get-Content $log -EA SilentlyContinue) -match 'READY'); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 700
    $log
}

# send-keys every key with a '|' (7C) marker around it; returns key -> bytes.
function SendAll($sess, $log) {
    & $PSMUX -L $NS send-keys -t $sess '|' 2>&1 | Out-Null
    foreach ($k in $keys) {
        & $PSMUX -L $NS send-keys -t $sess $k 2>&1 | Out-Null
        Start-Sleep -Milliseconds 100
        & $PSMUX -L $NS send-keys -t $sess '|' 2>&1 | Out-Null
        Start-Sleep -Milliseconds 100
    }
    Start-Sleep -Milliseconds 800
    $parts = (Stream $log) -split ' ?7C ?'
    $got = @{}
    for ($i = 0; $i -lt $keys.Count; $i++) { $got[$keys[$i]] = $parts[$i + 1] }
    $got
}

function Check-Mode($label, $got, $col) {
    $bad = @()
    foreach ($k in $keys) {
        $want = $table[$k][$col]
        if ($got[$k] -ne $want) { $bad += "$k got [$($got[$k])] want [$want]" }
    }
    if ($bad.Count -eq 0) { Write-Pass "$label : all $($keys.Count) keys as tmux mode $col" }
    else { Write-Fail "$label : $($bad -join '; ')" }
}

function Key-Mode($sess) { (& $PSMUX -L $NS display-message -t $sess -p '#{pane_key_mode}' 2>&1 | Out-String).Trim() }

Write-Host "`n=== Issue #860: extended key mode per pane ===" -ForegroundColor Cyan
$defaultPsmux = Get-Command psmux -EA SilentlyContinue
$before = if ($defaultPsmux) { (& $defaultPsmux.Source ls 2>&1 | Out-String) } else { "" }

# ---------------------------------------------------------------------------
# PART A: send-keys bytes in every mode
# ---------------------------------------------------------------------------
$cases = @(
    @{ n = 'off';        conf = '';                                                         emit = '';                col = 'L'; km = 'VT10x' },
    @{ n = 'off41';      conf = '';                                                         emit = '\e[>4;1m';        col = 'L'; km = 'VT10x' },
    @{ n = 'off42';      conf = '';                                                         emit = '\e[>4;2m';        col = 'L'; km = 'VT10x' },
    @{ n = 'on';         conf = 'set -g extended-keys on';                                  emit = '';                col = 'L'; km = 'VT10x' },
    @{ n = 'on41';       conf = 'set -g extended-keys on';                                  emit = '\e[>4;1m';        col = '1'; km = 'Ext 1' },
    @{ n = 'on42';       conf = 'set -g extended-keys on';                                  emit = '\e[>4;2m';        col = '2'; km = 'Ext 2' },
    @{ n = 'always';     conf = 'set -g extended-keys always';                              emit = '';                col = '1'; km = 'Ext 1' },
    @{ n = 'always42';   conf = 'set -g extended-keys always';                              emit = '\e[>4;2m';        col = '2'; km = 'Ext 2' },
    @{ n = 'on42reset';  conf = 'set -g extended-keys on';                                  emit = '\e[>4;2m\e[>4n';  col = 'L'; km = 'VT10x' },
    @{ n = 'on42ris';    conf = 'set -g extended-keys on';                                  emit = '\e[>4;2m\ec';     col = 'L'; km = 'VT10x' },
    @{ n = 'always42ris';conf = 'set -g extended-keys always';                              emit = '\e[>4;2m\ec';     col = '1'; km = 'Ext 1' }
)
foreach ($c in $cases) {
    Write-Host "`n[A/$($c.n)] send-keys, conf=[$($c.conf)] app wrote [$($c.emit)]" -ForegroundColor Yellow
    $conf = Conf $c.n $c.conf
    $log = Start-Rec $c.n $conf $c.emit
    $km = Key-Mode $c.n
    if ($km -eq $c.km) { Write-Pass "#{pane_key_mode} is $km" } else { Write-Fail "#{pane_key_mode} is [$km], want [$($c.km)]" }
    Check-Mode "send-keys $($c.n)" (SendAll $c.n $log) $c.col
    & $PSMUX -L $NS kill-session -t $c.n 2>&1 | Out-Null
}

# csi-u format: the same mode 2 keys as CSI k;mu.
Write-Host "`n[A/csiu] extended-keys-format csi-u in mode 2" -ForegroundColor Yellow
$conf = Conf 'csiu' "set -g extended-keys on`nset -g extended-keys-format csi-u"
$log = Start-Rec 'csiu' $conf '\e[>4;2m'
$got = SendAll 'csiu' $log
$want = @{ 'C-Space' = (Hex "$E[32;5u"); 'C-[' = (Hex "$E[91;5u"); 'M-a' = (Hex "$E[97;3u"); 'S-Enter' = (Hex "$E[13;2u"); 'Tab' = '09'; 'C-F5' = (Hex "$E[15;5~") }
foreach ($k in $want.Keys) {
    if ($got[$k] -eq $want[$k]) { Write-Pass "csi-u $k -> $($want[$k])" } else { Write-Fail "csi-u $k got [$($got[$k])] want [$($want[$k])]" }
}
& $PSMUX -L $NS kill-session -t csiu 2>&1 | Out-Null

# The option is live: turning extended-keys off does not drop a mode the pane
# already has (tmux reads the option only when the request arrives), and a
# NEW request made while it is off is ignored.
Write-Host "`n[A/live] set -g extended-keys at run time" -ForegroundColor Yellow
$conf = Conf 'live' ''
$log = Start-Rec 'live' $conf ''
& $PSMUX -L $NS set -g extended-keys on 2>&1 | Out-Null
$v = (& $PSMUX -L $NS show -gv extended-keys 2>&1 | Out-String).Trim()
if ($v -eq 'on') { Write-Pass "show -gv extended-keys reads on" } else { Write-Fail "show -gv extended-keys read [$v]" }
$log2 = Join-Path $TMP "live2.txt"
& $PSMUX -L $NS new-window -t live -- $REC vt $log2 '\e[>4;2m' 2>&1 | Out-Null
for ($i = 0; $i -lt 40 -and -not ((Get-Content $log2 -EA SilentlyContinue) -match 'READY'); $i++) { Start-Sleep -Milliseconds 250 }
Start-Sleep -Milliseconds 700
$km = Key-Mode 'live'
if ($km -eq 'Ext 2') { Write-Pass "a pane started after set -g extended-keys on takes mode 2" } else { Write-Fail "pane_key_mode [$km] after set at run time" }
& $PSMUX -L $NS set -g extended-keys off 2>&1 | Out-Null
$got = SendAll 'live' $log2
if ($got['C-Space'] -eq $table['C-Space']['2']) { Write-Pass "extended-keys off later keeps the mode the pane already has (tmux)" } else { Write-Fail "C-Space after option off: [$($got['C-Space'])]" }
& $PSMUX -L $NS kill-session -t live 2>&1 | Out-Null
Kill-Ns

# ---------------------------------------------------------------------------
# PART A2: a pane that reads console RECORDS keeps its record path
# ---------------------------------------------------------------------------
# `extended-keys always` puts every pane in mode 1, PSReadLine and Far
# included.  Those read INPUT_RECORDs, which cannot carry CSI 27;5;49~, so
# they keep the #623 win32 record: Ctrl+1 must still arrive as vk 0x31 with
# LEFT_CTRL_PRESSED, not as the characters of an escape sequence.
Write-Host "`n[A2] extended-keys always, record reading pane" -ForegroundColor Yellow
$conf = Conf 'recrd' 'set -g extended-keys always'
$logR = Join-Path $TMP "recrd.txt"
& $PSMUX -L $NS -f $conf new-session -d -s recrd -x 100 -y 30 -- $REC rec $logR 2>&1 | Out-Null
for ($i = 0; $i -lt 40 -and -not ((Get-Content $logR -EA SilentlyContinue) -match 'READY'); $i++) { Start-Sleep -Milliseconds 250 }
Start-Sleep -Milliseconds 800
$km = Key-Mode 'recrd'
if ($km -eq 'Ext 1') { Write-Pass "record reader pane is in Ext 1 under always" } else { Write-Fail "record reader pane_key_mode [$km]" }
foreach ($k in @('C-1', 'C-Space', 'S-Enter')) { & $PSMUX -L $NS send-keys -t recrd $k 2>&1 | Out-Null; Start-Sleep -Milliseconds 300 }
Start-Sleep -Milliseconds 600
$recs = (Get-Content $logR | Where-Object { $_ -match '^REC DOWN' }) -join "`n"
if ($recs -match 'vk=0x31 \S+ uChar=0x0000 ctrl=0x0008') { Write-Pass "Ctrl+1 reaches the record reader as vk=0x31 LEFT_CTRL (#623 path kept)" } else { Write-Fail "Ctrl+1 record wrong: [$recs]" }
if ($recs -notmatch 'uChar=0x005B') { Write-Pass "no '[' character record: no escape sequence was typed into the record reader" } else { Write-Fail "an escape sequence reached the record reader as text: [$recs]" }
& $PSMUX -L $NS kill-session -t recrd 2>&1 | Out-Null
Kill-Ns

# ---------------------------------------------------------------------------
# PART A3: modifiers in any order (send-keys M-C-a wrote ESC C)
# ---------------------------------------------------------------------------
# tmux reads C-, M- and S- in any order and case (key-string.c:213) and, in
# the standard mode, writes the meta prefix plus the standard_map byte with
# Shift ignored (input-keys.c input_key_vt10x).  C-a goes first so every
# later key also crosses a ConPTY that a win32 record has latched (#588),
# where ESC [ and ESC ESC used to vanish; ESC NUL lost its NUL on any ConPTY.
# Case matters (M-A is not M-a), so a list, not a hash table: name, legacy, mode 2.
$orderKeys = @(
    ,@('C-a', '01', (Hex "$E[27;5;97~"))
    ,@('C-M-a', '1B 01', (Hex "$E[27;7;97~"))
    ,@('M-C-a', '1B 01', (Hex "$E[27;7;97~"))
    ,@('m-c-a', '1B 01', (Hex "$E[27;7;97~"))
    ,@('M-C-A', '1B 01', (Hex "$E[27;7;65~"))
    ,@('M-S-a', '1B 61', (Hex "$E[27;4;97~"))
    ,@('S-M-a', '1B 61', (Hex "$E[27;4;97~"))
    ,@('C-S-a', '01', (Hex "$E[27;6;97~"))
    ,@('S-C-a', '01', (Hex "$E[27;6;97~"))
    ,@('C-M-S-a', '1B 01', (Hex "$E[27;8;97~"))
    ,@('S-M-C-a', '1B 01', (Hex "$E[27;8;97~"))
    ,@('M-C-Space', '1B 00', (Hex "$E[27;7;32~"))
    ,@('C-S-M-Space', '1B 00', (Hex "$E[27;8;32~"))
    ,@('M-C-[', '1B 1B', (Hex "$E[27;7;91~"))
    ,@('C-M-[', '1B 1B', (Hex "$E[27;7;91~"))
    ,@('M-C-1', '1B 31', (Hex "$E[27;7;49~"))
    ,@('M-Space', '1B 20', (Hex "$E[27;3;32~"))
    ,@('M-[', '1B 5B', (Hex "$E[27;3;91~"))
    ,@('M-Escape', '1B 1B', (Hex "$E[27;3;27~"))
    ,@('M-a', '1B 61', (Hex "$E[27;3;97~"))
    ,@('M-A', '1B 41', (Hex "$E[27;3;65~"))
    ,@('S-a', '61', (Hex "$E[27;2;97~"))
)
foreach ($oc in @(@{ n = 'ordL'; conf = ''; emit = ''; col = 'L' }, @{ n = 'ordX'; conf = 'set -g extended-keys on'; emit = '\e[>4;2m'; col = 'X' })) {
    Write-Host "`n[A3/$($oc.n)] modifier order, conf=[$($oc.conf)]" -ForegroundColor Yellow
    $conf = Conf $oc.n $oc.conf
    $log = Start-Rec $oc.n $conf $oc.emit
    & $PSMUX -L $NS send-keys -t $oc.n '|' 2>&1 | Out-Null
    foreach ($ok in $orderKeys) {
        & $PSMUX -L $NS send-keys -t $oc.n $ok[0] 2>&1 | Out-Null
        Start-Sleep -Milliseconds 100
        & $PSMUX -L $NS send-keys -t $oc.n '|' 2>&1 | Out-Null
        Start-Sleep -Milliseconds 100
    }
    Start-Sleep -Milliseconds 800
    $parts = (Stream $log) -split ' ?7C ?'
    $i = 1
    foreach ($ok in $orderKeys) {
        $k = $ok[0]
        $want = if ($oc.col -eq 'L') { $ok[1] } else { $ok[2] }
        if ($parts[$i] -eq $want) { Write-Pass "$($oc.n) $k -> [$want]" } else { Write-Fail "$($oc.n) $k got [$($parts[$i])] want [$want]" }
        $i++
    }
    & $PSMUX -L $NS kill-session -t $oc.n 2>&1 | Out-Null
}
Kill-Ns

# ---------------------------------------------------------------------------
# PART B: real key records through a REAL attached client
# ---------------------------------------------------------------------------
function Start-Attached($sess, $conf, $pane, $extra = "") {
    $cmd = Join-Path $TMP "launch_$sess.cmd"
    @"
@echo off
set PSMUX_SESSION_NAME=
set PSMUX_SESSION=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set PSMUX_DATA_DIR=$($env:PSMUX_DATA_DIR)
$extra
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
function Stop-Attached($a) {
    Kill-Ns
    foreach ($p in @($a.client, $a.host)) { if ($p) { Stop-Process -Id $p -Force -EA SilentlyContinue } }
}
function Wait-Pump($cpid, $log, $pattern) {
    for ($t = 0; $t -lt 10; $t++) {
        & $INJECT $cpid "g:30.30.0" | Out-Null
        Start-Sleep -Milliseconds 600
        if ((Get-Content $log -EA SilentlyContinue) -match $pattern) { return $true }
    }
    $false
}

# Records: 1 Up (latches the pane ConPTY) 2 Ctrl+Space 3 Ctrl+A 4 Ctrl+[ 5
# Alt+A 6 Enter 7 Tab 8 Shift+Enter 9 Ctrl+Space again, digits as markers.
$recKeys = @("g:31.31.0", "sleep:250", "g:26.0.100", "sleep:250", "g:32.32.0", "sleep:250",
    "g:20.0.8", "sleep:250", "g:33.33.0", "sleep:250", "g:41.1.8", "sleep:250", "g:34.34.0", "sleep:250",
    "g:DB.1B.8", "sleep:250", "g:35.35.0", "sleep:250", "g:41.61.2", "sleep:250", "g:36.36.0", "sleep:250",
    "g:0D.0D.0", "sleep:250", "g:37.37.0", "sleep:250", "g:09.09.0", "sleep:250", "g:38.38.0", "sleep:250",
    "g:0D.0D.10", "sleep:250", "g:39.39.0", "sleep:250", "g:20.0.8", "sleep:250", "g:31.31.0", "sleep:500")
$attCases = @(
    @{ n = 'off'; conf = ''; emit = ''; col = 'L' },
    @{ n = 'on42'; conf = 'set -g extended-keys on'; emit = '\e[>4;2m'; col = '2' },
    @{ n = 'always'; conf = 'set -g extended-keys always'; emit = ''; col = '1' }
)
foreach ($c in $attCases) {
    Write-Host "`n[B/$($c.n)] real Ctrl+Space and friends through an attached client" -ForegroundColor Yellow
    $conf = Conf "att$($c.n)" $c.conf
    $log = Join-Path $TMP "att$($c.n).txt"
    $pane = "`"$REC`" vt `"$log`""
    if ($c.emit) { $pane += " `"$($c.emit)`"" }
    $a = Start-Attached "att$($c.n)" $conf $pane
    if (-not $a.client -or -not (Wait-Pump $a.client $log 'hex=\[ 30 \]')) {
        Write-Skip "attached client did not start or never read input"
        Stop-Attached $a
        continue
    }
    & $INJECT $a.client @recKeys | Out-Null
    Start-Sleep -Milliseconds 1200
    $s = Stream $log
    $col = $c.col
    $exp = [ordered]@{
        'Up latches'        = "31 1B 5B 41 32"
        'Ctrl+Space'        = "32 $($table['C-Space'][$col]) 33"
        'Ctrl+A'            = "33 $($table['C-a'][$col]) 34"
        'Ctrl+['            = "34 $($table['C-['][$col]) 35"
        'Alt+A'             = "35 $($table['M-a'][$col]) 36"
        'Enter'             = "36 0D 37"
        'Tab'               = "37 09 38"
        'Shift+Enter'       = "38 $($table['S-Enter'][$col]) 39"
        'Ctrl+Space again'  = "39 $($table['C-Space'][$col]) 31"
    }
    foreach ($k in $exp.Keys) {
        if ($s.Contains($exp[$k])) { Write-Pass "attached $($c.n): $k -> [$($exp[$k])]" }
        else { Write-Fail "attached $($c.n): $k want [$($exp[$k])] in [$s]" }
    }
    Stop-Attached $a
}

# Real Ctrl+Alt chords through the attached client, standard mode, after Up
# latched the ConPTY: Ctrl+Alt+A is ESC ^A (with the record's character 0x01
# and with none), Alt+[ is ESC [ (tmux input_key_vt10x).  The Ctrl+Alt+A
# record used to reach the pane as 01 alone: the client's native Ctrl+Alt
# injection lost the Alt for a VT reader.
Write-Host "`n[B2] real Ctrl+Alt chords through an attached client" -ForegroundColor Yellow
$conf = Conf "attord" ''
$log = Join-Path $TMP "attord.txt"
$a = Start-Attached "attord" $conf "`"$REC`" vt `"$log`""
if (-not $a.client -or -not (Wait-Pump $a.client $log 'hex=\[ 30 \]')) {
    Write-Skip "attached client did not start or never read input"
} else {
    & $INJECT $a.client "g:31.31.0" "sleep:250" "g:26.0.100" "sleep:250" "g:32.32.0" "sleep:250" "g:41.1.A" "sleep:250" "g:36.36.0" "sleep:250" "g:41.0.A" "sleep:250" `
        "g:33.33.0" "sleep:250" "g:DB.5B.2" "sleep:250" "g:34.34.0" "sleep:500" | Out-Null
    Start-Sleep -Milliseconds 1200
    $s = Stream $log
    foreach ($e in @(@('Ctrl+Alt+A (uChar 01)', '32 1B 01 36'), @('Ctrl+Alt+A (uChar 00)', '36 1B 01 33'), @('Alt+[', '33 1B 5B 34'))) {
        if ($s.Contains($e[1])) { Write-Pass "attached: $($e[0]) -> [$($e[1])]" } else { Write-Fail "attached: $($e[0]) want [$($e[1])] in [$s]" }
    }
}
Stop-Attached $a

# ---------------------------------------------------------------------------
# PART C: nvim reads <C-Space>
# ---------------------------------------------------------------------------
Write-Host "`n[C] nvim receives <C-Space> with extended-keys on" -ForegroundColor Yellow
$nvim = Get-Command nvim -EA SilentlyContinue
if (-not $nvim) {
    Write-Skip "nvim is not installed"
} else {
    $KEYLOG_LUA = Join-Path $TMP "keylog.lua"
    @'
local f = io.open(vim.env.NVLOG, 'a')
f:write('READY\n'); f:flush()
vim.defer_fn(function()
  while true do
    local c = vim.fn.getcharstr()
    f:write('KEY ' .. vim.fn.keytrans(c) .. '\n'); f:flush()
  end
end, 10)
'@ | Set-Content $KEYLOG_LUA -Encoding ASCII
    $conf = Conf 'nv' 'set -g extended-keys on'
    # send-keys route
    $log = Join-Path $TMP "nv_sk.txt"
    $env:NVLOG = $log
    & $PSMUX -L $NS -f $conf new-session -d -s nvsk -x 100 -y 30 -- $nvim.Source --clean -u NONE -c "luafile $KEYLOG_LUA" 2>&1 | Out-Null
    Remove-Item Env:NVLOG
    for ($i = 0; $i -lt 40 -and -not ((Get-Content $log -EA SilentlyContinue) -match 'READY'); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Seconds 2
    $km = Key-Mode 'nvsk'
    if ($km -eq 'Ext 2') { Write-Pass "nvim asked for mode 2: #{pane_key_mode} = Ext 2" } else { Write-Fail "nvim pane_key_mode [$km]" }
    foreach ($k in @('1', 'C-Space', '2', 'C-[', '3', 'C-i', '4', 'Tab', '5', 'M-a', '6')) {
        & $PSMUX -L $NS send-keys -t nvsk $k 2>&1 | Out-Null
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Milliseconds 800
    $keysSeen = (Get-Content $log | Where-Object { $_ -match '^KEY ' } | ForEach-Object { $_.Substring(4) }) -join ' '
    if ($keysSeen -match '1 <C-Space> 2') { Write-Pass "nvim (send-keys): C-Space is <C-Space>" } else { Write-Fail "nvim (send-keys) C-Space: [$keysSeen]" }
    if ($keysSeen -match '2 <(Esc|C-\[)> 3') { Write-Pass "nvim (send-keys): C-[ still reads as Escape" } else { Write-Fail "nvim (send-keys) C-[: [$keysSeen]" }
    if ($keysSeen -match '3 <(C-I|Tab)> 4 <Tab> 5') { Write-Pass "nvim (send-keys): C-i and Tab arrive" } else { Write-Fail "nvim (send-keys) C-i/Tab: [$keysSeen]" }
    if ($keysSeen -match '5 <M-a> 6') { Write-Pass "nvim (send-keys): M-a is <M-a>" } else { Write-Fail "nvim (send-keys) M-a: [$keysSeen]" }
    & $PSMUX -L $NS kill-session -t nvsk 2>&1 | Out-Null

    # attached client route, real Ctrl+Space record after an arrow
    $log = Join-Path $TMP "nv_att.txt"
    $a = Start-Attached "nvatt" $conf "`"$($nvim.Source)`" --clean -u NONE -c `"luafile $KEYLOG_LUA`"" "set NVLOG=$log"
    if (-not $a.client -or -not (Wait-Pump $a.client $log '^KEY 0')) {
        Write-Skip "nvim pane did not start or never read input"
    } else {
        & $INJECT $a.client "g:31.31.0" "sleep:300" "g:20.0.8" "sleep:300" "g:32.32.0" "sleep:300" "g:26.0.100" "sleep:300" `
            "g:33.33.0" "sleep:300" "g:20.0.8" "sleep:300" "g:34.34.0" "sleep:300" "g:DB.1B.8" "sleep:300" "g:35.35.0" "sleep:600" | Out-Null
        Start-Sleep -Milliseconds 1200
        $keysSeen = (Get-Content $log | Where-Object { $_ -match '^KEY ' } | ForEach-Object { $_.Substring(4) }) -join ' '
        if ($keysSeen -match '1 <C-Space> 2') { Write-Pass "nvim (attached): real Ctrl+Space is <C-Space>" } else { Write-Fail "nvim (attached) Ctrl+Space: [$keysSeen]" }
        if ($keysSeen -match '3 <C-Space> 4') { Write-Pass "nvim (attached): Ctrl+Space after <Up> (latched ConPTY) is <C-Space>" } else { Write-Fail "nvim (attached) Ctrl+Space after Up: [$keysSeen]" }
        if ($keysSeen -match '4 <(Esc|C-\[)> 5') { Write-Pass "nvim (attached): Ctrl+[ still Escape in mode 2" } else { Write-Fail "nvim (attached) Ctrl+[: [$keysSeen]" }
        $km = Key-Mode 'nvatt'
        if ($km -eq 'Ext 2') { Write-Pass "TUI: attached nvim pane shows #{pane_key_mode} Ext 2" } else { Write-Fail "TUI: pane_key_mode [$km]" }
    }
    Stop-Attached $a
}

Kill-Ns
foreach ($p in $script:Started) { Stop-Process -Id $p -Force -EA SilentlyContinue }
$after = if ($defaultPsmux) { (& $defaultPsmux.Source ls 2>&1 | Out-String) } else { "" }
if ($before -ne $after) { Write-Host "  [WARN] default namespace session list changed during the run" -ForegroundColor Red }

Write-Host "`n=== Issue #860 extended keys Results ===" -ForegroundColor Cyan
Write-Host "Passed:  $script:Pass" -ForegroundColor Green
Write-Host "Failed:  $script:Fail" -ForegroundColor $(if ($script:Fail) { "Red" } else { "Green" })
Write-Host "Skipped: $script:Skip" -ForegroundColor DarkYellow
exit $script:Fail
