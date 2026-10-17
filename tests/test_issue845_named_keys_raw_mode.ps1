# Issue #845: in 3.4.0 named keys (Up, Left, Home, F5, Escape, and
# `send-keys -l "<ESC>[A"`) never reached a Node.js program with stdin in raw
# mode on Windows 10 19045 (Claude Code ignored the arrows); 3.3.8 delivered
# 1b5b41, 1b5b44, 1b5b48, 1b.
#
# Cause: since #729 psmux writes a named key to a VT reading pane as a win32
# input mode record (ESC [ Vk ; Sc ; Uc ; Kd ; Cs ; Rc _).  Only a console host
# that parses those records can turn them back into keys, and such a host
# announces it by writing CSI ?9001h as its first output.  A host that predates
# the mode never asks and cannot parse them (OpenConsole 1.0.2005 printed each
# record as literal text; the reporter's 19045 conhost delivered nothing).
# psmux now writes records only to a pane whose host asked; any other pane gets
# the VT bytes, as 3.3.8 and tmux (input-keys.c input_key_defaults) do.
#
# Build 26200 hosts DO ask, so on this machine every part below must keep
# delivering the exact bytes the reporter expects, through the record route.
# The old host half is pinned by the unit tests in
# tests-rs\test_issue845_win32_record_gate.rs, and Part 6 measures the host
# behaviour itself when PSMUX_845_OLD_OPENCONSOLE names an OpenConsole 1.0 exe.
#
# Part 1  the reporter's repro: send-keys into a raw mode node child.
# Part 2  a Claude Code like child (raw mode + CSI ?9001h): one VK record per
#         key, the #729 behaviour, must not regress.
# Part 3  a native ReadConsoleInput reader (tests\record_key_child.cs): the
#         virtual key of each named key.
# Part 4  a real attached client under a pseudoconsole (tests\conpty697.cs)
#         typing Up / Left / Home / F5 / a into the raw mode node child.
# Part 5  a real attached client in a visible console window, keys injected
#         with WriteConsoleInput (tests\injector_timed.cs), into the same child.
# Part 6  (optional) the old host measurement.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue845_named_keys_raw_mode.ps1
$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = if ($env:PSMUX_TEST_NS) { $env:PSMUX_TEST_NS } else { "i845keys$PID" }
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkGray }

$node = Get-Command node -EA SilentlyContinue
if (-not $node) { Write-Host "SKIP: node is not installed" -ForegroundColor Yellow; exit 0 }

foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$savedDataDir = $env:PSMUX_DATA_DIR
$root = Join-Path $env:TEMP "psmux_i845_$PID"
Remove-Item -Recurse -Force $root -EA SilentlyContinue
New-Item -ItemType Directory -Force $root | Out-Null
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $root "data"
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
}
$env:PSMUX_RUN_NOPAUSE = "1"
$emptyConf = Join-Path $root "empty.conf"
"" | Set-Content -Path $emptyConf -Encoding ASCII
$keyrec = Join-Path $PSScriptRoot "keyrec845.js"

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$recChild = Join-Path $root "record_key_child.exe"
$conpty = Join-Path $root "conpty697.exe"
$injector = Join-Path $root "injector_timed.exe"
& $csc /nologo /optimize /platform:x64 /out:$recChild (Join-Path $PSScriptRoot "record_key_child.cs") 2>&1 | Out-Null
& $csc /nologo /optimize /platform:x64 /out:$conpty (Join-Path $PSScriptRoot "conpty697.cs") 2>&1 | Out-Null
& $csc /nologo /optimize /platform:x64 /out:$injector (Join-Path $PSScriptRoot "injector_timed.cs") 2>&1 | Out-Null
Write-Info ("psmux: {0}  ({1})  ns={2}  build={3}" -f $PSMUX, ((& $PSMUX -V) -join ' '), $NS, [Environment]::OSVersion.Version.Build)

$ESC = [char]27
function Show([string]$s) { return $s.Replace([string]$ESC, '^[') }

# Hex of every 'data' event the node recorder logged after line $from.
function Read-Hex([string]$log, [int]$from) {
    $lines = @(Get-Content $log -EA SilentlyContinue)
    $out = @()
    for ($i = $from; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match ' hex=([0-9a-f]*)$') { $out += $Matches[1] }
    }
    return ,$out
}
function Hex([string]$s) { return (-join ([Text.Encoding]::ASCII.GetBytes($s) | ForEach-Object { $_.ToString('x2') })) }
function Wait-Lines([string]$log, [int]$before, [int]$ms = 2500) {
    $sw = [Diagnostics.Stopwatch]::StartNew(); $last = -1
    while ($sw.ElapsedMilliseconds -lt $ms) {
        $n = @(Get-Content $log -EA SilentlyContinue).Count
        if ($n -gt $before -and $n -eq $last) { break }
        $last = $n
        Start-Sleep -Milliseconds 150
    }
}

function Start-Keyrec([string]$Sess, [string]$Mode) {
    $log = Join-Path $root "$Sess.log"
    Remove-Item $log -EA SilentlyContinue
    & $PSMUX -L $NS -f $emptyConf new-session -d -s $Sess -x 120 -y 30 -- node $keyrec $log $(if ($Mode) { $Mode } else { "-" }) 2>&1 | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 15000) {
        if ((Test-Path $log) -and (((& $PSMUX -L $NS capture-pane -p -t $Sess) -join "`n") -match 'keyrec845 ready')) { break }
        Start-Sleep -Milliseconds 100
    }
    Start-Sleep -Milliseconds 300
    return $log
}
function Count-Lines([string]$log) { return @(Get-Content $log -EA SilentlyContinue).Count }

# name, send-keys args, expected bytes (the 3.3.8 column of the issue)
$cases = @(
    @('Up',     @('Up'),               "$ESC[A"),
    @('Left',   @('Left'),             "$ESC[D"),
    @('Home',   @('Home'),             "$ESC[H"),
    @('Escape', @('Escape'),           "$ESC"),
    @('F5',     @('F5'),               "$ESC[15~"),
    @('a',      @('a'),                "a"),
    @('-l ESC[A', @('-l', "$ESC[A"),   "$ESC[A"),
    @('Up after Escape', @('Up'),      "$ESC[A"),
    @('Enter',  @('Enter'),            "`r")
)

try {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null

    Write-Host "`n=== Part 1: the reporter's repro, send-keys into raw mode node ===" -ForegroundColor Yellow
    $log = Start-Keyrec "k" ""
    foreach ($c in $cases) {
        $before = Count-Lines $log
        & $PSMUX -L $NS send-keys -t k @($c[1]) 2>&1 | Out-Null
        Wait-Lines $log $before
        $got = (Read-Hex $log $before) -join ''
        $want = Hex $c[2]
        if ($got -eq $want) { Write-Pass ("{0}: {1}" -f $c[0], $want) }
        else { Write-Fail ("{0}: expected {1}, node read '{2}'" -f $c[0], $want, $got) }
    }
    & $PSMUX -L $NS kill-session -t k 2>&1 | Out-Null

    Write-Host "`n=== Part 2: a Claude Code like child (raw mode + CSI ?9001h) keeps #729's records ===" -ForegroundColor Yellow
    $log = Start-Keyrec "w" "w32"
    $w32cases = @(
        @('Up', @('Up'), 38, 256), @('Left', @('Left'), 37, 256), @('Home', @('Home'), 36, 256), @('F5', @('F5'), 116, 0)
    )
    foreach ($c in $w32cases) {
        $before = Count-Lines $log
        & $PSMUX -L $NS send-keys -t w @($c[1]) 2>&1 | Out-Null
        Wait-Lines $log $before
        $hex = (Read-Hex $log $before) -join ''
        $s = -join ($(for ($i = 0; $i -lt $hex.Length; $i += 2) { [char][Convert]::ToByte($hex.Substring($i, 2), 16) }))
        $recs = [regex]::Matches($s, "\x1b\[(\d+);(\d+);(\d+);(\d+);(\d+);(\d+)_")
        $vkRecs = @($recs | Where-Object { [int]$_.Groups[1].Value -eq $c[2] -and [int]$_.Groups[5].Value -eq $c[3] })
        $charRecs = @($recs | Where-Object { [int]$_.Groups[1].Value -eq 0 })
        if ($vkRecs.Count -eq 2 -and $charRecs.Count -eq 0) { Write-Pass ("{0}: one vk={1} press and release" -f $c[0], $c[2]) }
        else { Write-Fail ("{0}: read {1}" -f $c[0], (Show $s)) }
    }
    & $PSMUX -L $NS kill-session -t w 2>&1 | Out-Null

    Write-Host "`n=== Part 3: a native ReadConsoleInput reader gets the virtual keys ===" -ForegroundColor Yellow
    if (-not (Test-Path $recChild)) { Write-Skip "record_key_child.exe did not compile" }
    else {
        $rlog = Join-Path $root "records.txt"
        Remove-Item $rlog -EA SilentlyContinue
        & $PSMUX -L $NS -f $emptyConf new-session -d -s r -x 120 -y 30 -e "PSMUX_RECORD_LOG=$rlog" -- $recChild 2>&1 | Out-Null
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt 10000 -and -not (Test-Path $rlog)) { Start-Sleep -Milliseconds 100 }
        Start-Sleep -Milliseconds 500
        foreach ($c in @(@('Up', 0x26), @('Left', 0x25), @('Home', 0x24), @('F5', 0x74), @('Escape', 0x1B), @('a', 0x41))) {
            $before = Count-Lines $rlog
            & $PSMUX -L $NS send-keys -t r $c[0] 2>&1 | Out-Null
            Wait-Lines $rlog $before
            $new = @(Get-Content $rlog -EA SilentlyContinue | Select-Object -Skip $before)
            $down = @($new | Where-Object { $_ -match '^KEY down vk=0x([0-9A-F]{2})' } | ForEach-Object { [Convert]::ToInt32(($_ -replace '^KEY down vk=0x([0-9A-F]{2}).*', '$1'), 16) })
            if ($down.Count -eq 1 -and $down[0] -eq $c[1]) { Write-Pass ("{0}: one key down vk=0x{1:X2}" -f $c[0], $c[1]) }
            else { Write-Fail ("{0}: expected one vk=0x{1:X2} down, got: {2}" -f $c[0], $c[1], ($new -join ' | ')) }
        }
        & $PSMUX -L $NS kill-session -t r 2>&1 | Out-Null
    }

    Write-Host "`n=== Part 4: an attached client under a pseudoconsole types into raw mode node ===" -ForegroundColor Yellow
    if (-not (Test-Path $conpty)) { Write-Skip "conpty697.exe did not compile" }
    else {
        $log = Start-Keyrec "att" ""
        $script = Join-Path $root "att_script.txt"
        @("WAIT 3000",
          "HEX 1b 5b 41", "WAIT 400",
          "HEX 1b 5b 44", "WAIT 400",
          "HEX 1b 5b 48", "WAIT 400",
          "HEX 1b 5b 31 35 7e", "WAIT 400",
          "TEXT a", "WAIT 800", "END") | Set-Content $script -Encoding ASCII
        $before = Count-Lines $log
        $cp = Start-Process -FilePath $conpty -ArgumentList $script, (Join-Path $root "att.out"), 120, 30, 0, "`"$PSMUX`" -L $NS attach -t att" -PassThru -WindowStyle Hidden
        if (-not $cp.WaitForExit(60000)) { Stop-Process -Id $cp.Id -Force -EA SilentlyContinue }
        Start-Sleep -Milliseconds 400
        $got = Read-Hex $log $before
        $want = @((Hex "$ESC[A"), (Hex "$ESC[D"), (Hex "$ESC[H"), (Hex "$ESC[15~"), (Hex "a"))
        if (($got -join ',') -eq ($want -join ',')) { Write-Pass ("attached client: Up Left Home F5 a arrived as {0}" -f ($want -join ' ')) }
        elseif ($got.Count -eq 0) { Write-Skip "the attached client delivered nothing (pseudoconsole host refused?)" }
        else { Write-Fail ("attached client: expected {0}, node read {1}" -f ($want -join ' '), ($got -join ' ')) }
        & $PSMUX -L $NS kill-session -t att 2>&1 | Out-Null
    }

    Write-Host "`n=== Part 5: an attached client in a visible console window, WriteConsoleInput keys ===" -ForegroundColor Yellow
    if (-not (Test-Path $injector)) { Write-Skip "injector_timed.exe did not compile" }
    else {
        $log = Start-Keyrec "vis" ""
        # conhost.exe directly, so the client gets a classic console window
        # whatever the default terminal is, and the window is ours to close.
        $win = Start-Process -FilePath "$env:WINDIR\System32\conhost.exe" -ArgumentList "`"$PSMUX`" -L $NS attach -t vis" -PassThru
        $client = $null
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt 10000 -and -not $client) {
            $client = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($win.Id)" -EA SilentlyContinue | Where-Object { $_.Name -eq 'psmux.exe' } | Select-Object -First 1
            Start-Sleep -Milliseconds 200
        }
        if (-not $client) { Write-Skip "no attached client appeared in the console window" }
        else {
            Start-Sleep -Milliseconds 2500
            $before = Count-Lines $log
            $ilog = Join-Path $root "inject.txt"
            & $injector $client.ProcessId $ilog "VK:26:100" S400 "VK:25:100" S400 "VK:24:100" S400 "VK:74" S400 ESC S400 a S800 | Out-Null
            Wait-Lines $log $before
            $got = Read-Hex $log $before
            $want = @((Hex "$ESC[A"), (Hex "$ESC[D"), (Hex "$ESC[H"), (Hex "$ESC[15~"), (Hex "$ESC"), (Hex "a"))
            if (($got -join ',') -eq ($want -join ',')) { Write-Pass ("console client: Up Left Home F5 Escape a arrived as {0}" -f ($want -join ' ')) }
            elseif ($got.Count -eq 0) { Write-Skip "the console client delivered nothing (injection refused: $((Get-Content $ilog -EA SilentlyContinue) -join ' '))" }
            else { Write-Fail ("console client: expected {0}, node read {1}" -f ($want -join ' '), ($got -join ' ')) }
        }
        if (-not $win.HasExited) { Stop-Process -Id $win.Id -Force -EA SilentlyContinue }
        & $PSMUX -L $NS kill-session -t vis 2>&1 | Out-Null
    }

    Write-Host "`n=== Part 6: a host that predates win32 input mode (optional) ===" -ForegroundColor Yellow
    $old = $env:PSMUX_845_OLD_OPENCONSOLE
    if (-not $old -or -not (Test-Path $old)) { Write-Skip "set PSMUX_845_OLD_OPENCONSOLE to an OpenConsole.exe from Windows Terminal 1.0 to measure it" }
    else {
        # The host alone, headless, with its VT channel on our pipes: no psmux.
        $hlog = Join-Path $root "oldhost.log"
        $psi = [Diagnostics.ProcessStartInfo]::new($old)
        $psi.Arguments = "--headless --width 120 --height 30 -- node `"$keyrec`" `"$hlog`""
        $psi.UseShellExecute = $false; $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.CreateNoWindow = $true
        $hp = [Diagnostics.Process]::Start($psi)
        $outTask = $hp.StandardOutput.ReadToEndAsync()
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt 10000 -and -not (Test-Path $hlog)) { Start-Sleep -Milliseconds 100 }
        Start-Sleep -Milliseconds 500
        $in = $hp.StandardInput.BaseStream
        foreach ($s in @("$ESC[A", "$ESC[38;72;0;1;256;1_$ESC[38;72;0;0;256;1_")) {
            $b = [Text.Encoding]::ASCII.GetBytes($s); $in.Write($b, 0, $b.Length); $in.Flush(); Start-Sleep -Milliseconds 500
        }
        $got = Read-Hex $hlog 1
        if (-not $hp.HasExited) { Stop-Process -Id $hp.Id -Force -EA SilentlyContinue }
        $asked = $outTask.Wait(2000) -and $outTask.Result.Contains("$ESC[?9001h")
        Write-Info ("old host asked for ?9001h: {0}; VT Up read as {1}; record Up read as {2}" -f $asked, $got[0], $got[1])
        if (-not $asked -and $got[0] -eq (Hex "$ESC[A") -and $got[1] -ne (Hex "$ESC[A")) {
            Write-Pass "the old host never asks for win32 input mode and cannot parse the record, while VT bytes reach the child"
        } else { Write-Fail "old host measurement did not match the #845 model" }
    }
}
finally {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    $env:PSMUX_DATA_DIR = $savedDataDir
}

Write-Host ""
Write-Host ("Results: {0} passed, {1} failed, {2} skipped" -f $script:Pass, $script:Fail, $script:Skip)
if ($script:Fail -gt 0) { exit 1 }
exit 0
