# Issue #860: "Neovim not receiving C-[ or C-Space"
#
# Three defects, each reproduced with real keystrokes before anything changed:
#
#   1. Ctrl+[ (VK_OEM_4 + LEFT_CTRL, uChar 0x1b) is written to the pane as the
#      byte 0x1b by the C- arm of the named key writer, which used to go
#      straight to the pane writer and skip the #588 lone ESC repair.  After
#      any arrow or function key the pane's ConPTY is latched into win32 input
#      mode, conhost then holds a bare 0x1b and fuses it with the next key: the
#      C-[ vanished and the Backspace typed after it vanished with it, which is
#      the reporter's "backspace stops working until I type another letter".
#   2. A Windows Terminal sendInput action delivers each character as a vk = 0
#      record.  crossterm drops a vk = 0 ESC, so a Ctrl+[ remapped to
#      "\u001b" never arrived at all.
#   3. `send-keys C-Space` wrote 0x13 (Ctrl+S, XOFF) instead of NUL.
#
# What is NOT a psmux defect, measured with no psmux in the path: a real
# Ctrl+Space reaches a pane as NUL (tmux's byte too, input-keys.c standard_map),
# and nvim on Windows never reports NUL as <C-Space>, in conhost and in Windows
# Terminal alike.  That part is checked here only as the NUL byte.
#
# Every runtime check uses its own -L namespace and PSMUX_DATA_DIR, and every
# process this script starts is stopped by PID.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$EXE_NAME = [IO.Path]::GetFileName($PSMUX)
$NS = "t860_$PID"
$TMP = Join-Path $env:TEMP "psmux_t860_$PID"
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
$RECLOG = Join-Path $TMP "reclog610.exe"
$INJECT = Join-Path $TMP "recbatch860.exe"
& $csc /nologo /platform:x64 /out:$RECLOG "$PSScriptRoot\reclog610.cs" 2>&1 | Out-Null
& $csc /nologo /platform:x64 /out:$INJECT "$PSScriptRoot\recbatch860.cs" 2>&1 | Out-Null
if (-not (Test-Path $RECLOG) -or -not (Test-Path $INJECT)) { Write-Host "could not build the C# helpers" -ForegroundColor Red; exit 1 }

$emptyConf = Join-Path $TMP "empty.conf"
"" | Set-Content $emptyConf -Encoding ASCII
$KEYLOG_LUA = Join-Path $TMP "keylog.lua"
@'
local f = io.open(vim.env.NVLOG, 'a')
f:write('READY\n'); f:flush()
vim.defer_fn(function()
  while true do
    local c = vim.fn.getcharstr()
    local hex = {}
    for i = 1, #c do hex[#hex+1] = string.format('%02X', c:byte(i)) end
    f:write('KEY ' .. vim.fn.keytrans(c) .. ' hex=' .. table.concat(hex, ' ') .. '\n'); f:flush()
  end
end, 10)
'@ | Set-Content $KEYLOG_LUA -Encoding ASCII

function Kill-Ns { & $PSMUX -L $NS kill-server 2>&1 | Out-Null; Start-Sleep -Milliseconds 400 }

# The byte STREAM a VT reading pane logged, as upper case hex separated by spaces.
function Stream($log) {
    $b = @()
    foreach ($l in (Get-Content $log -EA SilentlyContinue)) {
        if ($l -match '^BYTES .*hex=\[ ([0-9A-Fa-f ]+) \]') { $b += ($Matches[1].Trim() -split '\s+') }
    }
    ($b -join ' ').ToUpper()
}

# Start an ATTACHED client in its own visible console window running $pane,
# return @{ host = <conhost pid>; client = <client pid> }.
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

# Wake the client's input pump with a throwaway '0' until the pane logs it.
function Wait-Pump($cpid, $log, $pattern) {
    for ($t = 0; $t -lt 10; $t++) {
        & $INJECT $cpid "g:30.30.0" | Out-Null
        Start-Sleep -Milliseconds 600
        if ((Get-Content $log -EA SilentlyContinue) -match $pattern) { return $true }
    }
    $false
}

Write-Host "`n=== Issue #860: C-[ and C-Space reach the pane ===" -ForegroundColor Cyan
$before = (& (Get-Command psmux -EA SilentlyContinue).Source ls 2>&1 | Out-String)

# ---------------------------------------------------------------------------
# TEST 1: send-keys writes tmux's bytes (CLI / TCP path)
# ---------------------------------------------------------------------------
Write-Host "`n[Test 1] send-keys C-[ and C-Space bytes" -ForegroundColor Yellow
$log = Join-Path $TMP "t1.txt"
& $PSMUX -L $NS -f $emptyConf new-session -d -s t1 -x 100 -y 30 -- $RECLOG vt $log 2>&1 | Out-Null
for ($i = 0; $i -lt 40 -and -not (Test-Path $log); $i++) { Start-Sleep -Milliseconds 250 }
Start-Sleep -Seconds 1
foreach ($k in @('1', 'C-[', '2', 'C-Space', '3', 'Up', '4', 'C-[', 'BSpace', '5')) {
    & $PSMUX -L $NS send-keys -t t1 $k 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
}
Start-Sleep -Milliseconds 700
$s = Stream $log
if ($s -match '31 1B 32') { Write-Pass "send-keys C-[ -> 1B" } else { Write-Fail "send-keys C-[ wrong: [$s]" }
if ($s -match '32 00 33') { Write-Pass "send-keys C-Space -> 00 (tmux standard_map NUL)" } else { Write-Fail "send-keys C-Space wrong, want 00 between 32 and 33: [$s]" }
if ($s -match '34 1B 7F 35') { Write-Pass "send-keys C-[ BSpace after Up -> 1B 7F" } else { Write-Fail "send-keys C-[ BSpace after Up wrong: [$s]" }
Kill-Ns

# ---------------------------------------------------------------------------
# TEST 2: a REAL attached client, real Ctrl+[ records, after an arrow key
# ---------------------------------------------------------------------------
# Records: Up (latches the pane ConPTY), Ctrl+[, Ctrl+[ then Backspace, Escape.
$latchKeys = @("g:31.31.0", "sleep:300", "g:26.0.100", "sleep:300", "g:32.32.0", "sleep:300",
    "g:DB.1B.8", "sleep:300", "g:33.33.0", "sleep:300", "g:DB.1B.8", "sleep:300", "g:08.08.0", "sleep:300",
    "g:34.34.0", "sleep:300", "g:1B.1B.0", "sleep:300", "g:35.35.0", "sleep:300", "g:DB.1B.8", "sleep:300", "g:36.36.0", "sleep:500")
foreach ($mode in @('off', 'on', 'always')) {
    Write-Host "`n[Test 2/$mode] real Ctrl+[ after Up, extended-keys $mode" -ForegroundColor Yellow
    $conf = Join-Path $TMP "ek_$mode.conf"
    "set -g extended-keys $mode" | Set-Content $conf -Encoding ASCII
    $log = Join-Path $TMP "t2_$mode.txt"
    $a = Start-Attached "t2$mode" $conf "`"$RECLOG`" vt `"$log`""
    if (-not $a.client -or -not (Wait-Pump $a.client $log 'hex=\[ 30 \]')) {
        Write-Skip "attached client did not start or never read input"
        Stop-Attached $a
        continue
    }
    & $INJECT $a.client @latchKeys | Out-Null
    Start-Sleep -Milliseconds 1200
    $s = Stream $log
    if ($s -match '31 1B 5B 41 32') { Write-Pass "Up arrived (the latch is armed)" } else { Write-Fail "Up did not arrive: [$s]" }
    if ($s -match '32 1B 33') { Write-Pass "Ctrl+[ after Up -> 1B" } else { Write-Fail "Ctrl+[ after Up lost, want 32 1B 33: [$s]" }
    if ($s -match '33 1B 7F 34') { Write-Pass "Ctrl+[ then Backspace -> 1B 7F, the Backspace survives" } else { Write-Fail "Ctrl+[ then Backspace wrong, want 33 1B 7F 34: [$s]" }
    if ($s -match '34 1B 35') { Write-Pass "plain Escape still 1B (#588 guard)" } else { Write-Fail "plain Escape wrong: [$s]" }
    if ($s -match '35 1B 36') { Write-Pass "Ctrl+[ then a letter: no Alt fusion" } else { Write-Fail "Ctrl+[ then a letter wrong: [$s]" }
    Stop-Attached $a
}

# ---------------------------------------------------------------------------
# TEST 3: the Windows Terminal sendInput shape (every char a vk = 0 record)
# ---------------------------------------------------------------------------
Write-Host "`n[Test 3] sendInput records: \u001b alone and \u001b[32;5u" -ForegroundColor Yellow
$log = Join-Path $TMP "t3.txt"
$a = Start-Attached "t3" $emptyConf "`"$RECLOG`" vt `"$log`""
if (-not $a.client -or -not (Wait-Pump $a.client $log 'hex=\[ 30 \]')) {
    Write-Skip "attached client did not start or never read input"
} else {
    & $INJECT $a.client "g:31.31.0" "sleep:300" 'wt:\e' "sleep:300" "g:32.32.0" "sleep:300" 'wt:\e[32;5u' "sleep:300" `
        "g:33.33.0" "sleep:300" 'wt:\e' "sleep:300" "g:08.08.0" "sleep:300" "g:34.34.0" "sleep:300" `
        'wt:\e[13;2u' "sleep:300" "g:35.35.0" "sleep:500" | Out-Null
    Start-Sleep -Milliseconds 1200
    $s = Stream $log
    if ($s -match '31 1B 32') { Write-Pass "sendInput \u001b -> 1B" } else { Write-Fail "sendInput \u001b lost, want 31 1B 32: [$s]" }
    if ($s -match '32 00 33') { Write-Pass "sendInput CSI 32;5u decodes to C-Space -> 00, no stray ESC or text" } else { Write-Fail "sendInput CSI 32;5u wrong, want 32 00 33: [$s]" }
    if ($s -match '33 1B 7F 34') { Write-Pass "sendInput \u001b then Backspace -> 1B 7F" } else { Write-Fail "sendInput \u001b then Backspace wrong: [$s]" }
    if ($s -match '34 1B 0D 35') { Write-Pass "sendInput CSI 13;2u is still Shift+Enter -> 1B 0D (#611 guard)" } else { Write-Fail "sendInput CSI 13;2u wrong, want 34 1B 0D 35: [$s]" }
}
Stop-Attached $a

# ---------------------------------------------------------------------------
# TEST 4: nvim itself, through a real attached client
# ---------------------------------------------------------------------------
Write-Host "`n[Test 4] nvim receives <Esc> for Ctrl+[ after <Up>" -ForegroundColor Yellow
$nvim = Get-Command nvim -EA SilentlyContinue
if (-not $nvim) {
    Write-Skip "nvim is not installed"
} else {
    $log = Join-Path $TMP "t4.txt"
    $a = Start-Attached "t4" $emptyConf "`"$($nvim.Source)`" --clean -u NONE -c `"luafile $KEYLOG_LUA`"" "set NVLOG=$log"
    if (-not $a.client -or -not (Wait-Pump $a.client $log '^KEY 0')) {
        Write-Skip "nvim pane did not start or never read input"
    } else {
        & $INJECT $a.client @latchKeys | Out-Null
        Start-Sleep -Milliseconds 1500
        $keys = (Get-Content $log | Where-Object { $_ -match '^KEY ' } | ForEach-Object { ($_ -split ' ')[1] }) -join ' '
        if ($keys -match '1 <Up> 2 <Esc> 3') { Write-Pass "nvim: <Up> then Ctrl+[ is <Esc>" } else { Write-Fail "nvim: Ctrl+[ after <Up> lost: [$keys]" }
        if ($keys -match '3 <Esc> <BS> 4') { Write-Pass "nvim: Ctrl+[ then Backspace is <Esc> <BS>" } else { Write-Fail "nvim: Backspace after Ctrl+[ wrong: [$keys]" }
        if ($keys -match '5 <Esc> 6') { Write-Pass "nvim: Ctrl+[ then a letter is <Esc> 6, not <M-6>" } else { Write-Fail "nvim: Ctrl+[ then letter wrong: [$keys]" }
    }
    Stop-Attached $a
}

# ---------------------------------------------------------------------------
# TEST 5: TUI layer, the attached client is a live rendered session
# ---------------------------------------------------------------------------
Write-Host "`n[Test 5] TUI: a visible attached session renders and takes keys" -ForegroundColor Yellow
$log = Join-Path $TMP "t5.txt"
$a = Start-Attached "t5" $emptyConf "`"$RECLOG`" vt `"$log`""
if (-not $a.client -or -not (Wait-Pump $a.client $log 'hex=\[ 30 \]')) {
    Write-Skip "attached client did not start"
} else {
    $clients = (& $PSMUX -L $NS list-clients -t t5 2>&1 | Out-String)
    if ($clients -match 't5') { Write-Pass "list-clients shows the attached client" } else { Write-Fail "no attached client listed: $clients" }
    $att = (& $PSMUX -L $NS display-message -t t5 -p '#{session_attached}' 2>&1 | Out-String).Trim()
    if ($att -eq '1') { Write-Pass "session_attached is 1" } else { Write-Fail "session_attached is [$att]" }
    & $INJECT $a.client "g:26.0.100" "sleep:300" "g:DB.1B.8" "sleep:300" "g:37.37.0" "sleep:500" | Out-Null
    Start-Sleep -Milliseconds 800
    $s = Stream $log
    if ($s -match '1B 5B 41 1B 37') { Write-Pass "the rendered session still takes Up, Ctrl+[, 7" } else { Write-Fail "keys into the rendered session wrong: [$s]" }
}
Stop-Attached $a

Kill-Ns
foreach ($p in $script:Started) { Stop-Process -Id $p -Force -EA SilentlyContinue }
$after = (& (Get-Command psmux -EA SilentlyContinue).Source ls 2>&1 | Out-String)
if ($before -ne $after) { Write-Host "  [WARN] default namespace session list changed during the run" -ForegroundColor Red }

Write-Host "`n=== Issue #860 Results ===" -ForegroundColor Cyan
Write-Host "Passed:  $script:Pass" -ForegroundColor Green
Write-Host "Failed:  $script:Fail" -ForegroundColor $(if ($script:Fail) { "Red" } else { "Green" })
Write-Host "Skipped: $script:Skip" -ForegroundColor DarkYellow
if ($script:Fail -gt 0) { exit 1 } else { exit 0 }
