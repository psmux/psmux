# Issue #888: "M-' doesn't get passed through"
#
# An attached client forwards a key as one wire line, `send-key <name>`, and
# the server tokenizes that line with the quote aware command parser.  Raw,
# `send-key M-'` opened a single quote at the `'`: the server read the key as
# `M-` and wrote nothing to the pane.  Alt+" (and Ctrl+', Ctrl+Alt+') went
# the same way.  The CLI route (psmux send-keys "M-'") quoted its argv and
# always worked, which is why only the real keyboard lost the key.
#
# tmux writes M-' as ESC ' (input-keys.c input_key_vt10x: the meta prefix,
# then the key), and in extended mode 2 as CSI 27;3;39~ (input_key_extended).
#
# Checked here, with the bytes a VT input reader in the pane receives:
#   A. send-keys with every neighbour of ' (" \ ; # { } `), standard mode,
#      extended mode 2 (xterm and csi-u)
#   B. REAL Alt+' and friends as console key records through a REAL attached
#      client: standard mode, mode 2, and the shared server
#   C. a root binding on M-' fires from the real key
#   D. nvim reads <M-'> from the real key
#
# Every runtime check uses its own -L namespace and PSMUX_DATA_DIR, and every
# process this script starts is stopped by PID.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_EXE) { $env:PSMUX_TEST_EXE } else { (Get-Command psmux -EA Stop).Source }
$EXE_NAME = [IO.Path]::GetFileName($PSMUX)
$NS = "t888_$PID"
$TMP = Join-Path $env:TEMP "psmux_t888_$PID"
New-Item -ItemType Directory -Force $TMP | Out-Null
if (-not $env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR = Join-Path $TMP "data" }
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$env:PSMUX_RUN_NOPAUSE = "1"

$script:TestsPassed = 0
$script:TestsFailed = 0
$script:Skip = 0
$script:Started = @()
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor DarkYellow; $script:Skip++ }

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$REC = Join-Path $TMP "rec888.exe"
$INJECT = Join-Path $TMP "recbatch888.exe"
& $csc /nologo /platform:x64 /out:$REC "$PSScriptRoot\rec860.cs" 2>&1 | Out-Null
& $csc /nologo /platform:x64 /out:$INJECT "$PSScriptRoot\recbatch860.cs" 2>&1 | Out-Null
if (-not (Test-Path $REC) -or -not (Test-Path $INJECT)) { Write-Host "could not build the C# helpers" -ForegroundColor Red; exit 1 }

function Kill-Ns($ns = $NS) { & $PSMUX -L $ns kill-server 2>&1 | Out-Null; Start-Sleep -Milliseconds 400 }
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
# key name -> (code point, legacy bytes)
$keyTable = [ordered]@{
    "M-'" = @(39, '1B 27')
    'M-"' = @(34, '1B 22')
    'M-\' = @(92, '1B 5C')
    'M-;' = @(59, '1B 3B')
    'M-#' = @(35, '1B 23')
    'M-{' = @(123, '1B 7B')
    'M-}' = @(125, '1B 7D')
    'M-`' = @(96, '1B 60')
    'M-a' = @(97, '1B 61')
}

Write-Host "`n=== Issue #888: M-' and its neighbours reach the pane ===" -ForegroundColor Cyan
$defaultPsmux = Get-Command psmux -EA SilentlyContinue
$before = if ($defaultPsmux) { (& $defaultPsmux.Source ls 2>&1 | Out-String) } else { "" }

# ---------------------------------------------------------------------------
# PART A: send-keys
# ---------------------------------------------------------------------------
$modeCases = @(
    @{ n = 'off'; conf = ''; emit = ''; fmt = 'L' },
    @{ n = 'on42'; conf = 'set -g extended-keys on'; emit = '\e[>4;2m'; fmt = 'X' },
    @{ n = 'csiu42'; conf = "set -g extended-keys on`nset -g extended-keys-format csi-u"; emit = '\e[>4;2m'; fmt = 'U' }
)
foreach ($mc in $modeCases) {
    Write-Host "`n[A/$($mc.n)] send-keys" -ForegroundColor Yellow
    $conf = Conf "a$($mc.n)" $mc.conf
    $log = Join-Path $TMP "a$($mc.n).txt"
    if ($mc.emit) { & $PSMUX -L $NS -f $conf new-session -d -s "a$($mc.n)" -x 100 -y 30 -- $REC vt $log $mc.emit 2>&1 | Out-Null }
    else { & $PSMUX -L $NS -f $conf new-session -d -s "a$($mc.n)" -x 100 -y 30 -- $REC vt $log 2>&1 | Out-Null }
    for ($i = 0; $i -lt 40 -and -not ((Get-Content $log -EA SilentlyContinue) -match 'READY'); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 700
    & $PSMUX -L $NS send-keys -t "a$($mc.n)" '|' 2>&1 | Out-Null
    $names = @($keyTable.Keys)
    foreach ($k in $names) {
        & $PSMUX -L $NS send-keys -t "a$($mc.n)" $k 2>&1 | Out-Null
        Start-Sleep -Milliseconds 100
        & $PSMUX -L $NS send-keys -t "a$($mc.n)" '|' 2>&1 | Out-Null
        Start-Sleep -Milliseconds 100
    }
    Start-Sleep -Milliseconds 800
    $parts = (Stream $log) -split ' ?7C ?'
    for ($i = 0; $i -lt $names.Count; $i++) {
        $k = $names[$i]; $cp = $keyTable[$k][0]
        $want = switch ($mc.fmt) { 'L' { $keyTable[$k][1] } 'X' { Hex "$E[27;3;$cp~" } 'U' { Hex "$E[$cp;3u" } }
        if ($parts[$i + 1] -eq $want) { Write-Pass "send-keys $($mc.n) $k -> [$want]" } else { Write-Fail "send-keys $($mc.n) $k got [$($parts[$i + 1])] want [$want]" }
    }
}
Kill-Ns

# ---------------------------------------------------------------------------
# PART B: real key records through a REAL attached client
# ---------------------------------------------------------------------------
function Start-Attached($ns, $sess, $conf, $pane, $extra = "") {
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
"$PSMUX" -L $ns -f "$conf" new-session -s $sess -x 100 -y 30 -- $pane
"@ | Set-Content $cmd -Encoding ASCII
    $h = Start-Process conhost.exe -ArgumentList "cmd /c `"$cmd`"" -PassThru
    $script:Started += $h.Id
    $c = $null
    for ($i = 0; $i -lt 60 -and -not $c; $i++) {
        Start-Sleep -Milliseconds 250
        $c = Get-CimInstance Win32_Process -Filter "Name='$EXE_NAME'" |
            Where-Object { $_.CommandLine -match "-L $ns " -and $_.CommandLine -match "new-session -s $sess " } |
            Select-Object -ExpandProperty ProcessId -First 1
    }
    if ($c) { $script:Started += $c }
    @{ host = $h.Id; client = $c; ns = $ns }
}
function Stop-Attached($a) {
    Kill-Ns $a.ns
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

# Records (vk.char.ctrl, hex): OEM_7 DE is ' and ", OEM_5 DC \ and |,
# OEM_1 BA ;, OEM_4 DB {, OEM_6 DD }, OEM_3 C0 `.  ctrl 2 = LEFT_ALT,
# 10 = SHIFT, 12 = SHIFT+LEFT_ALT, 8 = LEFT_CTRL.  | (Shift+\) separates.
$recCases = @(
    @("Alt+'", "DE.27.2", 39, '1B 27'), @('Alt+"', "DE.22.12", 34, '1B 22'), @('Alt+\', "DC.5C.2", 92, '1B 5C'),
    @('Alt+;', "BA.3B.2", 59, '1B 3B'), @('Alt+#', "33.23.12", 35, '1B 23'), @('Alt+{', "DB.7B.12", 123, '1B 7B'),
    @('Alt+}', "DD.7D.12", 125, '1B 7D'), @('Alt+`', "C0.60.2", 96, '1B 60'), @("Ctrl+'", "DE.0.8", 39, '27'),
    @("plain '", "DE.27.0", 39, '27'), @('Alt+a', "41.61.2", 97, '1B 61')
)
$steps = @("g:DC.7C.10", "sleep:250")
foreach ($rc in $recCases) { $steps += "g:$($rc[1])"; $steps += "sleep:250"; $steps += "g:DC.7C.10"; $steps += "sleep:250" }
$attCases = @(
    @{ n = 'off'; conf = ''; emit = ''; extra = ''; fmt = 'L' },
    @{ n = 'on42'; conf = 'set -g extended-keys on'; emit = '\e[>4;2m'; extra = ''; fmt = 'X' },
    @{ n = 'shared'; conf = ''; emit = ''; extra = 'set PSMUX_SERVER_MODE=shared'; fmt = 'L' }
)
foreach ($c in $attCases) {
    Write-Host "`n[B/$($c.n)] real Alt+' and friends through an attached client" -ForegroundColor Yellow
    $ns = "$NS$($c.n)"
    $conf = Conf "b$($c.n)" $c.conf
    $log = Join-Path $TMP "b$($c.n).txt"
    $pane = "`"$REC`" vt `"$log`""
    if ($c.emit) { $pane += " `"$($c.emit)`"" }
    $a = Start-Attached $ns "b$($c.n)" $conf $pane $c.extra
    if (-not $a.client -or -not (Wait-Pump $a.client $log 'hex=\[ 30 \]')) {
        Write-Skip "attached client did not start or never read input"
        Stop-Attached $a
        continue
    }
    if ($c.n -eq 'shared') {
        $srv = Get-CimInstance Win32_Process -Filter "Name='$EXE_NAME'" | Where-Object { $_.CommandLine -match "--shared-server" -and $_.CommandLine -match "-L $ns(\s|$)" }
        if ($srv) { Write-Pass "shared: the session runs in the shared server" } else { Write-Fail "shared: no --shared-server process for $ns" }
    }
    & $INJECT $a.client @steps | Out-Null
    Start-Sleep -Milliseconds 1200
    $parts = (Stream $log) -split ' ?7C ?'
    for ($i = 0; $i -lt $recCases.Count; $i++) {
        $rc = $recCases[$i]
        $want = $rc[3]
        if ($c.fmt -eq 'X' -and $rc[0] -ne "plain '") {
            $mod = if ($rc[0] -like 'Ctrl*') { 5 } else { 3 }
            $want = Hex "$E[27;$mod;$($rc[2])~"
        }
        if ($parts[$i + 1] -eq $want) { Write-Pass "attached $($c.n): $($rc[0]) -> [$want]" }
        else { Write-Fail "attached $($c.n): $($rc[0]) got [$($parts[$i + 1])] want [$want]" }
    }
    Stop-Attached $a
}

# ---------------------------------------------------------------------------
# PART C: a root binding on M-' fires from the real key
# ---------------------------------------------------------------------------
Write-Host "`n[C] bind -n M-' runs from a real Alt+'" -ForegroundColor Yellow
$ns = "${NS}bind"
$conf = Conf "cbind" "bind -n `"M-'`" set -g @hit888 yes`nbind -n 'M-`"' set -g @hit888dq yes"
$log = Join-Path $TMP "cbind.txt"
$a = Start-Attached $ns "cbind" $conf "`"$REC`" vt `"$log`""
if (-not $a.client -or -not (Wait-Pump $a.client $log 'hex=\[ 30 \]')) {
    Write-Skip "attached client did not start or never read input"
} else {
    $lk = (& $PSMUX -L $ns list-keys -T root 2>&1 | Out-String)
    if ($lk -match "@hit888 yes") { Write-Pass "list-keys shows the M-' binding" } else { Write-Fail "list-keys: [$lk]" }
    & $INJECT $a.client "g:DE.27.2" "sleep:300" "g:DE.22.12" "sleep:600" | Out-Null
    Start-Sleep -Milliseconds 800
    $hit = (& $PSMUX -L $ns show -gv '@hit888' 2>&1 | Out-String).Trim()
    $hitdq = (& $PSMUX -L $ns show -gv '@hit888dq' 2>&1 | Out-String).Trim()
    if ($hit -eq 'yes') { Write-Pass "real Alt+' ran its root binding" } else { Write-Fail "Alt+' binding: @hit888=[$hit]" }
    if ($hitdq -eq 'yes') { Write-Pass "real Alt+`" ran its root binding" } else { Write-Fail "Alt+`" binding: @hit888dq=[$hitdq]" }
    $s = Stream $log
    if ($s -notmatch '1B 27|1B 22') { Write-Pass "bound keys did not also reach the pane" } else { Write-Fail "bound key leaked into the pane: [$s]" }
}
Stop-Attached $a

# ---------------------------------------------------------------------------
# PART D: nvim reads <M-'>
# ---------------------------------------------------------------------------
Write-Host "`n[D] nvim receives <M-'> from a real Alt+'" -ForegroundColor Yellow
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
    foreach ($nc in @(@{ n = 'off'; conf = '' }, @{ n = 'on'; conf = 'set -g extended-keys on' })) {
        $ns = "${NS}nv$($nc.n)"
        $conf = Conf "nv$($nc.n)" $nc.conf
        $log = Join-Path $TMP "nv_$($nc.n).txt"
        $a = Start-Attached $ns "nv$($nc.n)" $conf "`"$($nvim.Source)`" --clean -u NONE -c `"luafile $KEYLOG_LUA`"" "set NVLOG=$log"
        if (-not $a.client -or -not (Wait-Pump $a.client $log '^KEY 0')) {
            Write-Skip "nvim ($($nc.n)) did not start or never read input"
        } else {
            & $INJECT $a.client "g:31.31.0" "sleep:300" "g:DE.27.2" "sleep:300" "g:32.32.0" "sleep:300" "g:DE.22.12" "sleep:300" "g:33.33.0" "sleep:800" | Out-Null
            Start-Sleep -Milliseconds 1000
            $seen = (Get-Content $log | Where-Object { $_ -match '^KEY ' } | ForEach-Object { $_.Substring(4) }) -join ' '
            if ($seen -match "1 <M-'> 2") { Write-Pass "nvim ($($nc.n)): Alt+' is <M-'>" } else { Write-Fail "nvim ($($nc.n)) Alt+': [$seen]" }
            if ($seen -match '2 <M-"> 3') { Write-Pass "nvim ($($nc.n)): Alt+`" is <M-`">" } else { Write-Fail "nvim ($($nc.n)) Alt+`": [$seen]" }
        }
        Stop-Attached $a
    }
}

Kill-Ns
foreach ($p in $script:Started) { Stop-Process -Id $p -Force -EA SilentlyContinue }
$after = if ($defaultPsmux) { (& $defaultPsmux.Source ls 2>&1 | Out-String) } else { "" }
if ($before -ne $after) { Write-Host "  [WARN] default namespace session list changed during the run" -ForegroundColor Red }

Write-Host "`n=== Results: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:Skip) skipped ===" -ForegroundColor Cyan
exit $script:TestsFailed
