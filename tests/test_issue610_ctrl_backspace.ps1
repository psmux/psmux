# Issue #610: Ctrl+Backspace lost its modifier end to end, so no word delete.
#
# Two defects, both proven by measurement before anything was changed:
#   1. src/client.rs named EVERY Backspace "backspace" on the wire, so the
#      server never learned that Ctrl was held.
#   2. src/input.rs parse_modified_special_key had no Backspace arm, so
#      `send-keys C-BSpace` fell through to the generic "C-" handler, which
#      reads the character at index 2 of "C-BSPACE" and sent Ctrl+B (0x02).
#
# The correct bytes were measured against a real pseudoconsole rather than
# guessed.  ConPTY's input parser turns 0x7f into VK_BACK with no modifiers and
# 0x08 into VK_BACK with LEFT_CTRL_PRESSED, and conhost's own record to VT
# encoder produces exactly those bytes for Backspace and Ctrl+Backspace (and
# 1b 7f for Alt+Backspace, which is also what tmux 3.4 writes for M-BSpace).
# CSI forms (ESC [ 127;5 u, ESC [ 27;5;127 ~) are silently DISCARDED by the
# ConPTY input parser, so they would deliver nothing at all.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SOCK = "i610"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkGray }

$emptyConf = "$env:TEMP\psmux_610_empty.conf"
"" | Set-Content -Path $emptyConf -Encoding ASCII

# --- Build the measurement harnesses -----------------------------------------
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
$reclog = "$env:TEMP\psmux_reclog610.exe"
$inject = "$env:TEMP\psmux_bsinject610.exe"
foreach ($pair in @(@{s='reclog610.cs'; o=$reclog}, @{s='bsinject610.cs'; o=$inject})) {
    if (-not (Test-Path $pair.o)) {
        & $csc /nologo /platform:x64 /out:$($pair.o) "$PSScriptRoot\$($pair.s)" 2>&1 | Out-Null
    }
}
if (-not (Test-Path $reclog) -or -not (Test-Path $inject)) {
    Write-Host "  [FAIL] could not build the C# harnesses (csc at $csc)" -ForegroundColor Red
    exit 1
}

function Cleanup-Session($name) {
    & $PSMUX -L $SOCK kill-session -t $name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== Issue #610 Tests: Ctrl+Backspace keeps its modifier ===" -ForegroundColor Cyan

# =============================================================================
# TEST 1: send-keys writes the right BYTES into the pane (VT reading child)
# =============================================================================
Write-Host "`n[Test 1] send-keys writes the native Windows bytes for modified Backspace" -ForegroundColor Yellow
$sess = "i610_bytes"
$log  = "$env:TEMP\psmux_610_vt.txt"
Remove-Item -Force -EA SilentlyContinue $log
Cleanup-Session $sess
& $PSMUX -L $SOCK -f $emptyConf new-session -d -s $sess -x 100 -y 30 -- $reclog vt $log 2>&1 | Out-Null
Start-Sleep -Seconds 3

if (-not (Test-Path $log)) {
    Write-Fail "byte logging pane never started"
} else {
    # marker digit, key name, expected hex the pane must receive
    $cases = @(
        @{ m='1'; k='BSpace';     want='7F';    why='plain Backspace stays DEL (regression guard)' },
        @{ m='2'; k='C-BSpace';   want='08';    why='Ctrl+Backspace is 0x08, what ConPTY turns into VK_BACK+CTRL' },
        @{ m='3'; k='S-BSpace';   want='7F';    why='Shift+Backspace has no distinct encoding' },
        @{ m='4'; k='M-BSpace';   want='1B 7F'; why='Alt+Backspace is ESC DEL, byte for byte what tmux 3.4 sends' },
        @{ m='5'; k='C-M-BSpace'; want='1B 08'; why='Ctrl+Alt+Backspace is ESC 0x08' },
        @{ m='6'; k='C-w';        want='17';    why='Ctrl+W untouched (regression guard)' },
        @{ m='7'; k='C-h';        want='08';    why='Ctrl+H untouched (regression guard)' },
        @{ m='8'; k='C-?';        want='7F';    why='C-? untouched (regression guard)' }
    )
    foreach ($c in $cases) {
        & $PSMUX -L $SOCK send-keys -t $sess $c.m 2>&1 | Out-Null
        Start-Sleep -Milliseconds 250
        & $PSMUX -L $SOCK send-keys -t $sess $c.k 2>&1 | Out-Null
        Start-Sleep -Milliseconds 350
    }
    Start-Sleep -Milliseconds 700
    $lines = @(Get-Content $log -EA SilentlyContinue | Where-Object { $_ -match '^BYTES' })
    # Pair up: each marker line is followed by the line for the key under test.
    foreach ($c in $cases) {
        $markerHex = "{0:X2}" -f ([int][char]$c.m)
        $idx = -1
        for ($i = 0; $i -lt $lines.Count; $i++) {
            if ($lines[$i] -match "hex=\[ $markerHex \]") { $idx = $i; break }
        }
        if ($idx -lt 0 -or $idx + 1 -ge $lines.Count) {
            Write-Fail "send-keys $($c.k): no logged bytes (marker $($c.m) missing)"
            continue
        }
        $got = ''
        if ($lines[$idx + 1] -match 'hex=\[ ([0-9A-F ]+) \]') { $got = $Matches[1].Trim() }
        if ($got -eq $c.want) { Write-Pass "send-keys $($c.k) -> [$got] : $($c.why)" }
        else { Write-Fail "send-keys $($c.k) -> [$got], want [$($c.want)] : $($c.why)" }
    }
}
Cleanup-Session $sess

# =============================================================================
# TEST 2: the pane CONSOLE sees VK_BACK with the Ctrl flag (PSReadLine's channel)
# =============================================================================
Write-Host "`n[Test 2] send-keys C-BSpace reaches the pane console as VK_BACK + Ctrl" -ForegroundColor Yellow
$sess = "i610_rec"
$log  = "$env:TEMP\psmux_610_rec.txt"
Remove-Item -Force -EA SilentlyContinue $log
Cleanup-Session $sess
& $PSMUX -L $SOCK -f $emptyConf new-session -d -s $sess -x 100 -y 30 -- $reclog rec $log 2>&1 | Out-Null
Start-Sleep -Seconds 3
if (-not (Test-Path $log)) {
    Write-Fail "record logging pane never started"
} else {
    & $PSMUX -L $SOCK send-keys -t $sess 'C-BSpace' 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900
    $recs = @(Get-Content $log -EA SilentlyContinue | Where-Object { $_ -match '^REC DOWN vk=0x08' })
    $ctrlBs = @($recs | Where-Object { $_ -match 'ctrl=0x0008' -or $_ -match 'ctrl=0x0004' })
    if ($ctrlBs.Count -gt 0) { Write-Pass "pane console got $($ctrlBs[0].Trim())" }
    else { Write-Fail "no VK_BACK record with a Ctrl flag; saw: $($recs -join ' | ')" }

    # And plain Backspace must still arrive WITHOUT the Ctrl flag.
    Remove-Item -Force -EA SilentlyContinue $log
    Cleanup-Session $sess
    & $PSMUX -L $SOCK -f $emptyConf new-session -d -s $sess -x 100 -y 30 -- $reclog rec $log 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    & $PSMUX -L $SOCK send-keys -t $sess 'BSpace' 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900
    $recs = @(Get-Content $log -EA SilentlyContinue | Where-Object { $_ -match '^REC DOWN vk=0x08' })
    if ($recs -and $recs[0] -match 'ctrl=0x0000') { Write-Pass "plain BSpace still has no modifier: $($recs[0].Trim())" }
    else { Write-Fail "plain BSpace record wrong: $($recs -join ' | ')" }
}
Cleanup-Session $sess

# =============================================================================
# TEST 3: bind-key C-BSpace parses and round trips
# =============================================================================
Write-Host "`n[Test 3] bind-key accepts the modified Backspace names" -ForegroundColor Yellow
$sess = "i610_bind"
Cleanup-Session $sess
& $PSMUX -L $SOCK -f $emptyConf new-session -d -s $sess -x 80 -y 24 2>&1 | Out-Null
Start-Sleep -Seconds 2
foreach ($k in @('C-BSpace','S-BSpace','M-BSpace','C-M-BSpace')) {
    & $PSMUX -L $SOCK bind-key -T t610 $k send-keys x 2>&1 | Out-Null
    $lk = (& $PSMUX -L $SOCK list-keys -T t610 2>&1 | Out-String)
    if ($lk -match [regex]::Escape("-T t610 $k ")) { Write-Pass "bind-key $k round trips in list-keys" }
    else { Write-Fail "bind-key $k did not round trip: $($lk.Trim())" }
    & $PSMUX -L $SOCK unbind-key -T t610 $k 2>&1 | Out-Null
}
Cleanup-Session $sess

# =============================================================================
# TEST 4 + 5: Win32 TUI. A REAL attached client, a REAL Ctrl+Backspace keystroke.
# =============================================================================
Write-Host "`n[Test 4] real Ctrl+Backspace in an attached client: PSReadLine kills a word" -ForegroundColor Yellow

$launchShell = "$env:TEMP\psmux_610_launch_sh.cmd"
@"
@echo off
set PSMUX_SESSION_NAME=
set PSMUX_SESSION=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set NO_COLOR=
"$PSMUX" -L $SOCK -f "$emptyConf" new-session -s %1 -x 120 -y 30 "pwsh -NoLogo -NoProfile -NoExit"
"@ | Set-Content -Path $launchShell -Encoding ASCII

$launchLog = "$env:TEMP\psmux_610_launch_log.cmd"
@"
@echo off
set PSMUX_SESSION_NAME=
set PSMUX_SESSION=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set NO_COLOR=
"$PSMUX" -L $SOCK -f "$emptyConf" new-session -s %1 -x 120 -y 30 -- %2 %3 %4
"@ | Set-Content -Path $launchLog -Encoding ASCII

# Pick the NEWEST matching client.  A client whose session was killed can
# outlive the session for a moment, and grabbing that corpse means injecting
# keystrokes into a console nobody reads.
function Get-ClientPid($name) {
    Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -match "new-session -s\s+$name\b" } |
        Sort-Object CreationDate -Descending |
        Select-Object -ExpandProperty ProcessId -First 1
}
# Kill any client left over from an earlier run of this same test.
function Drop-StaleClients($name) {
    Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -match "new-session -s\s+$name\b" } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
    Start-Sleep -Milliseconds 500
}
function Last-PaneLine($name) {
    ((& $PSMUX -L $SOCK capture-pane -p -t $name 2>&1) |
        Where-Object { $_ -match '\S' } | Select-Object -Last 1)
}

$sess = "i610_psrl"
Cleanup-Session $sess
Drop-StaleClients $sess
Start-Process -FilePath $launchShell -ArgumentList @($sess) | Out-Null
Start-Sleep -Seconds 10
$cpid = Get-ClientPid $sess
if (-not $cpid) {
    Write-Fail "attached client for the PSReadLine pane did not start"
} else {
    foreach ($case in @(
        @{ n='Ctrl+Backspace'; k='cbs';   want='foo';    why='BackwardKillWord' },
        @{ n='Backspace';      k='bs';    want='foo ba'; why='BackwardDeleteChar (regression guard)' },
        @{ n='Ctrl+W';         k='ctrlw'; want='foo';    why='BackwardKillWord (regression guard)' })) {
        # Escape reverts whatever is on the edit line, then a real `clear` gives
        # a fresh screen so the last pane line is unambiguously the prompt we
        # are about to type into.  Without that the leftovers of earlier cases
        # scroll around and the assertion reads the wrong row.
        & $inject $cpid 'raw:1B,1B,00' 'sleep:250' | Out-Null
        & $PSMUX -L $SOCK send-keys -t $sess 'clear' Enter 2>&1 | Out-Null
        Start-Sleep -Milliseconds 1400
        & $inject $cpid 'text:foo bar' 'sleep:700' | Out-Null
        # PSReadLine only redraws once the client has flushed the typed text
        # (it may arrive as a bracketed paste), so settle before measuring.
        Start-Sleep -Milliseconds 1600
        $before = Last-PaneLine $sess
        if ($before -notmatch 'foo bar$') {
            Write-Skip "$($case.n): could not type 'foo bar' into the pane (line was [$before])"
            continue
        }
        & $inject $cpid $case.k 'sleep:700' | Out-Null
        Start-Sleep -Milliseconds 1400
        $after = Last-PaneLine $sess
        if ($after -match ([regex]::Escape($case.want) + '$')) {
            Write-Pass "$($case.n) turned 'foo bar' into '$($case.want)' : $($case.why)"
        } else {
            Write-Fail "$($case.n): 'foo bar' -> [$after], expected it to end with '$($case.want)' : $($case.why)"
        }
    }
}
Cleanup-Session $sess
Drop-StaleClients $sess

Write-Host "`n[Test 5] real Ctrl+Backspace in an attached client: a node raw mode pane sees 0x08" -ForegroundColor Yellow
$node = (Get-Command node -EA SilentlyContinue)
if (-not $node) {
    Write-Skip "node is not on PATH"
} else {
    $sess = "i610_node"
    $nlog = "$env:TEMP\psmux_610_node.txt"
    Remove-Item -Force -EA SilentlyContinue $nlog
    Cleanup-Session $sess
    Drop-StaleClients $sess
    Start-Process -FilePath $launchLog -ArgumentList @($sess, "`"$($node.Source)`"", "`"$PSScriptRoot\nodekeylog610.js`"", "`"$nlog`"") | Out-Null
    Start-Sleep -Seconds 10
    $cpid = Get-ClientPid $sess
    if (-not $cpid -or -not (Test-Path $nlog)) {
        Write-Fail "attached client for the node pane did not start (pid=$cpid, log=$(Test-Path $nlog))"
    } else {
        # The client can take a moment after the pane appears before its console
        # input pump is live.  Probe with a throwaway char until a byte lands,
        # otherwise the real keys are injected into a console nobody is reading.
        $ready = $false
        for ($try = 0; $try -lt 10; $try++) {
            & $inject $cpid 'text:0' 'sleep:200' | Out-Null
            Start-Sleep -Milliseconds 700
            if ((Get-Content $nlog -EA SilentlyContinue) -match 'hex=') { $ready = $true; break }
        }
        if (-not $ready) {
            Write-Skip "node pane never acknowledged a probe keystroke; cannot measure"
        } else {
        & $inject $cpid 'text:1' 'sleep:300' 'bs' 'sleep:400' 'text:2' 'sleep:300' 'cbs' 'sleep:600' | Out-Null
        Start-Sleep -Seconds 2
        # node may coalesce several keys into one stdin read, so assert on the
        # whole byte STREAM rather than per read: marker '1' (0x31), the plain
        # Backspace byte, marker '2' (0x32), then the Ctrl+Backspace byte.
        $bytes = @()
        foreach ($l in (Get-Content $nlog -EA SilentlyContinue)) {
            if ($l -match 'hex=\[ ([0-9a-f ]+) \]') {
                $bytes += ($Matches[1].Trim() -split '\s+')
            }
        }
        $stream = ($bytes -join ' ')
        if ($stream -match '31 7f') {
            Write-Pass "node pane got 7f for plain Backspace (regression guard) [$stream]"
        } else {
            Write-Fail "node pane plain Backspace wrong: [$stream]"
        }
        if ($stream -match '32 08') {
            Write-Pass "node pane got 08 for Ctrl+Backspace, identical to what it gets outside psmux [$stream]"
        } else {
            Write-Fail "node pane Ctrl+Backspace wrong: [$stream]"
        }
        }
    }
    Cleanup-Session $sess
    Drop-StaleClients $sess
}

# =============================================================================
# TEST 6: the VT input path (SSH / WezTerm / JetBrains) must agree with the
# console path. needs_vt_input() switches the client to the VT parser when
# SSH_CONNECTION/SSH_CLIENT/SSH_TTY is set, TERM_PROGRAM=WezTerm, or
# TERMINAL_EMULATOR contains JetBrains. That parser used to map the incoming
# byte 0x08 to an unmodified Backspace, dropping the modifier exactly like the
# console path did, and additionally corrupting Ctrl+H from 0x08 into 0x7f.
# =============================================================================
Write-Host "`n[Test 6] the VT input path decodes Ctrl+Backspace and Ctrl+H like the console path" -ForegroundColor Yellow

function Probe-VtPath {
    param([string]$Label, [string]$ExtraEnv)
    $sess = "i610_vt"
    $log  = "$env:TEMP\psmux_610_vt_$Label.txt"
    $lc   = "$env:TEMP\psmux_610_vt_$Label.cmd"
    Remove-Item -Force -EA SilentlyContinue $log
    @"
@echo off
set PSMUX_SESSION_NAME=
set PSMUX_SESSION=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set NO_COLOR=
$ExtraEnv
"$PSMUX" -L $SOCK -f "$emptyConf" new-session -s %1 -x 120 -y 30 -- %2 %3 %4
"@ | Set-Content -Path $lc -Encoding ASCII
    Cleanup-Session $sess
    Drop-StaleClients $sess
    Start-Process -FilePath $lc -ArgumentList @($sess, "`"$reclog`"", 'vt', "`"$log`"") | Out-Null
    Start-Sleep -Seconds 9
    $cp = Get-ClientPid $sess
    if (-not $cp -or -not (Test-Path $log)) {
        Write-Skip "$Label : client or logging pane did not start"
        Cleanup-Session $sess; Drop-StaleClients $sess
        return
    }
    # Wake the client's input pump before the measured keys.
    $ready = $false
    for ($t = 0; $t -lt 10; $t++) {
        & $inject $cp 'text:0' 'sleep:200' | Out-Null
        Start-Sleep -Milliseconds 700
        if ((Get-Content $log -EA SilentlyContinue) -match 'hex=') { $ready = $true; break }
    }
    if (-not $ready) {
        Write-Skip "$Label : client never acknowledged a probe keystroke"
        Cleanup-Session $sess; Drop-StaleClients $sess
        return
    }
    & $inject $cp 'text:1' 'sleep:300' 'bs'    'sleep:400' `
                  'text:2' 'sleep:300' 'cbs'   'sleep:400' `
                  'text:3' 'sleep:300' 'ctrlh' 'sleep:400' `
                  'text:4' 'sleep:300' 'ctrlw' 'sleep:600' | Out-Null
    Start-Sleep -Seconds 2
    $b = @()
    foreach ($l in (Get-Content $log -EA SilentlyContinue)) {
        if ($l -match 'hex=\[ ([0-9A-Fa-f ]+) \]') { $b += ($Matches[1].Trim() -split '\s+') }
    }
    $stream = ($b -join ' ').ToUpper()
    foreach ($c in @(
        @{ mk='31'; want='7F'; n='plain Backspace' },
        @{ mk='32'; want='08'; n='Ctrl+Backspace' },
        @{ mk='33'; want='08'; n='Ctrl+H' },
        @{ mk='34'; want='17'; n='Ctrl+W' })) {
        if ($stream -match ($c.mk + ' ' + $c.want)) {
            Write-Pass "$Label : $($c.n) -> $($c.want)"
        } else {
            Write-Fail "$Label : $($c.n) did not yield $($c.want); stream was [$stream]"
        }
    }
    Cleanup-Session $sess
    Drop-StaleClients $sess
}

Probe-VtPath 'ssh'     'set SSH_CONNECTION=1.2.3.4 5 6.7.8.9 22'
Probe-VtPath 'wezterm' 'set TERM_PROGRAM=WezTerm'

& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null

Write-Host "`n=== Issue #610 Results ===" -ForegroundColor Cyan
Write-Host "Passed: $script:TestsPassed" -ForegroundColor Green
Write-Host "Failed: $script:TestsFailed" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
