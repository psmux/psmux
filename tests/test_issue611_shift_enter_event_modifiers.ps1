# Issue #611: Shift+Enter degrades to plain Enter under host load.
#
# Two defects, both proven by measurement against real consoles BEFORE anything
# was changed.
#
#   1. src/platform.rs augment_enter_shift decided Enter's modifiers by polling
#      GetAsyncKeyState(VK_SHIFT) at PROCESSING time rather than reading them
#      from the KEY_EVENT_RECORD that carried them at EVENT time.  A poll can
#      never answer a question about the past, and it also overwrote modifiers
#      the event DID carry: an Alt+Enter (what ConPTY makes of the bytes
#      1b 0d) had its ALT stripped whenever the stale poll said Shift was down.
#
#   2. src/ssh_input.rs InputSource::Crossterm forwarded an ESC that was
#      immediately followed by a CR as two independent keys, so the pane child
#      received 1b and 0d in two separate reads and a readline style app saw a
#      lone ESC (cancel) then CR (submit) instead of a newline.  #397 fixed the
#      same thing on the SSH input path; the local path had no equivalent.
#
# The platform facts these tests rely on were measured, not assumed:
#
#   a real Shift+Enter typed into Windows Terminal, at the ConPTY child:
#     REC DOWN vk=0x0D scan=0x1C uChar=0x000D ctrl=0x0010 [SHIFT]
#   the bytes 1b 0d written into a pseudoconsole in ONE write:
#     REC DOWN vk=0x0D scan=0x1C uChar=0x000D ctrl=0x0002 [LALT]
#   the same bytes in TWO writes:
#     REC DOWN vk=0x1B ... then REC DOWN vk=0x0D ctrl=0x0000
#   CSI 27;2;13~ and CSI 13;2u are DISCARDED by the ConPTY input parser
#   (zero records), so neither can carry a modified Enter on Windows.
#
# The keys are injected with WriteConsoleInput into a REAL attached client's
# console, because this is a client input path feature: send-keys would enter
# through the server and never touch the code under test.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$DATA  = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }
$SOCK  = "i611"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkGray }

$emptyConf = "$env:TEMP\psmux_611_empty.conf"
"" | Set-Content -Path $emptyConf -Encoding ASCII

# --- Build the measurement harnesses -----------------------------------------
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
$reclog  = "$env:TEMP\psmux_reclog611.exe"
$inject  = "$env:TEMP\psmux_entinject611.exe"
$feeder  = "$env:TEMP\psmux_conptyfeed611.exe"
foreach ($pair in @(
    @{ s='reclog610.cs';      o=$reclog },
    @{ s='entinject611.cs';   o=$inject },
    @{ s='conptyfeed610.cs';  o=$feeder })) {
    if (-not (Test-Path $pair.o)) {
        & $csc /nologo /platform:x64 /out:$($pair.o) "$PSScriptRoot\$($pair.s)" 2>&1 | Out-Null
    }
}
if (-not (Test-Path $reclog) -or -not (Test-Path $inject) -or -not (Test-Path $feeder)) {
    Write-Host "  [FAIL] could not build the C# harnesses (csc at $csc)" -ForegroundColor Red
    exit 1
}

function Cleanup-Session($name) {
    & $PSMUX -L $SOCK kill-session -t $name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
}
function Get-ClientPid($name) {
    Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -match "new-session -s\s+$name\b" } |
        Sort-Object CreationDate -Descending |
        Select-Object -ExpandProperty ProcessId -First 1
}
function Drop-StaleClients($name) {
    Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
        Where-Object { $_.CommandLine -match "new-session -s\s+$name\b" } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
    Start-Sleep -Milliseconds 500
}

Write-Host "`n=== Issue #611 Tests: Shift+Enter keeps its modifier, ESC+CR stays atomic ===" -ForegroundColor Cyan

# =============================================================================
# TEST 1: the platform contract.  ConPTY's input parser is the thing that
# decides whether the modifier reaches psmux at all, so pin down what it does
# with each candidate encoding before asserting anything about psmux.
# =============================================================================
Write-Host "`n[Test 1] what ConPTY's input parser makes of each Shift+Enter encoding" -ForegroundColor Yellow
$ctrl = "$env:TEMP\psmux_611_feed_ctrl.txt"
$fout = "$env:TEMP\psmux_611_feed_out.txt"
$frec = "$env:TEMP\psmux_611_feed_rec.txt"
Remove-Item -Force -EA SilentlyContinue $ctrl, $fout, $frec
"" | Set-Content $ctrl -Encoding ASCII
$fp = Start-Process -FilePath $feeder -ArgumentList @($ctrl, $fout, $reclog, 'rec', $frec) -PassThru -WindowStyle Hidden
Start-Sleep -Seconds 3
function Feed($line) { Add-Content -Path $ctrl -Value $line -Encoding ASCII; Start-Sleep -Milliseconds 600 }
if (-not (Test-Path $frec)) {
    Write-Skip "the pseudoconsole child never started"
} else {
    Feed 'HEX 31'                                   # marker 1
    Feed 'HEX 1b 0d'                                # ESC+CR in ONE write
    Feed 'HEX 32'                                   # marker 2
    Feed 'HEX 1b'                                   # ESC alone
    Feed 'HEX 0d'                                   # CR later
    Feed 'HEX 33'                                   # marker 3
    Feed 'HEX 1b 5b 32 37 3b 32 3b 31 33 7e'        # CSI 27;2;13~
    Feed 'HEX 1b 5b 31 33 3b 32 75'                 # CSI 13;2u
    Feed 'HEX 34'                                   # marker 4
    Feed 'HEX 1a'
    Start-Sleep -Seconds 1
    Feed 'QUIT'
    Start-Sleep -Seconds 1
    if (-not $fp.HasExited) { Stop-Process -Id $fp.Id -Force -EA SilentlyContinue }

    $recs = @(Get-Content $frec -EA SilentlyContinue | Where-Object { $_ -match '^REC DOWN' })
    $joined = ($recs -join ' | ')

    # 1b 0d in one write must become ONE Enter record carrying LALT.
    $altEnter = @($recs | Where-Object { $_ -match 'vk=0x0D' -and $_ -match 'ctrl=0x0002' })
    if ($altEnter.Count -ge 1) {
        Write-Pass "ESC+CR in one write -> $($altEnter[0].Trim())"
    } else {
        Write-Fail "ESC+CR in one write produced no Alt+Enter record; saw: $joined"
    }

    # The split form must produce a separate Escape record: this is the shape a
    # loaded host delivers and the one the coalescer has to put back together.
    $escRec = @($recs | Where-Object { $_ -match 'vk=0x1B' })
    if ($escRec.Count -ge 1) {
        Write-Pass "ESC and CR in two writes -> a separate $($escRec[0].Trim())"
    } else {
        Write-Fail "ESC in its own write produced no Escape record; saw: $joined"
    }

    # Neither CSI form survives, so they can never be the answer on Windows.
    $between = $false
    $csiRecs = @()
    foreach ($r in $recs) {
        if ($r -match 'uChar=0x0033') { $between = $true; continue }
        if ($r -match 'uChar=0x0034') { $between = $false; continue }
        if ($between) { $csiRecs += $r }
    }
    if ($csiRecs.Count -eq 0) {
        Write-Pass "CSI 27;2;13~ and CSI 13;2u are discarded by ConPTY (zero records)"
    } else {
        Write-Fail "a CSI modified-Enter form unexpectedly produced records: $($csiRecs -join ' | ')"
    }
}

# =============================================================================
# TEST 2 (claim A): a Shift+Enter KEY_EVENT_RECORD reaches the pane as ESC+CR
# even though the PHYSICAL Shift key is not held.
#
# WriteConsoleInput does not touch the hardware key state, so during this test
# GetAsyncKeyState(VK_SHIFT) says "up" while the record says SHIFT: exactly the
# divergence the reporter hits when the host is too busy to process the event
# before the key is released.  Plain 0d here is the bug (Claude Code submits).
# =============================================================================
Write-Host "`n[Test 2] Shift+Enter taken from the event, not from a hardware poll" -ForegroundColor Yellow

$node = (Get-Command node -EA SilentlyContinue)
$launchLog = "$env:TEMP\psmux_611_launch_log.cmd"
@"
@echo off
set PSMUX_SESSION_NAME=
set PSMUX_SESSION=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set NO_COLOR=
set PSMUX_DATA_DIR=$DATA
"$PSMUX" -L $SOCK -f "$emptyConf" new-session -s %1 -x 120 -y 30 -- %2 %3 %4
"@ | Set-Content -Path $launchLog -Encoding ASCII

if (-not $node) {
    Write-Skip "node is not on PATH; the byte level assertions need a raw mode child"
} else {
    $sess = "i611_keys"
    $nlog = "$env:TEMP\psmux_611_node.txt"
    Remove-Item -Force -EA SilentlyContinue $nlog
    Cleanup-Session $sess
    Drop-StaleClients $sess
    Start-Process -FilePath $launchLog -ArgumentList @($sess, "`"$($node.Source)`"", "`"$PSScriptRoot\nodekeylog611.js`"", "`"$nlog`"") | Out-Null
    Start-Sleep -Seconds 10
    $cpid = Get-ClientPid $sess
    if (-not $cpid -or -not (Test-Path $nlog)) {
        Write-Fail "attached client for the node pane did not start (pid=$cpid, log=$(Test-Path $nlog))"
    } else {
        # The client's console input pump can be live a moment after the pane
        # appears.  Probe until a byte lands, otherwise the measured keys go
        # into a console nobody reads.
        $ready = $false
        for ($try = 0; $try -lt 12; $try++) {
            & $inject $cpid 'text:0' | Out-Null
            Start-Sleep -Milliseconds 700
            if ((Get-Content $nlog -EA SilentlyContinue) -match 'hex=') { $ready = $true; break }
        }
        if (-not $ready) {
            Write-Skip "node pane never acknowledged a probe keystroke; cannot measure"
        } else {
            # marker, injector specs, expected byte run after the marker, why
            $cases = @(
                @{ m='1'; spec=@('sent');      want='1b 0d'; n='Shift+Enter record with the physical Shift NOT held' },
                @{ m='2'; spec=@('ent');       want='0d';    n='plain Enter still submits (regression guard)' },
                @{ m='3'; spec=@('ment');      want='1b 0d'; n='Alt+Enter record keeps its ALT, no poll rewrite' },
                @{ m='4'; spec=@('escent');    want='1b 0d'; n='ESC+CR in one console write stays atomic' },
                @{ m='5'; spec=@('escent:5');  want='1b 0d'; n='ESC+CR split across two console reads is rejoined' }
            )
            foreach ($c in $cases) {
                & $inject $cpid "text:$($c.m)" 'sleep:400' | Out-Null
                Start-Sleep -Milliseconds 600
                & $inject $cpid @($c.spec) | Out-Null
                Start-Sleep -Milliseconds 1600
            }
            Start-Sleep -Seconds 2

            # node coalesces reads, so assert on the whole byte STREAM: the
            # marker digit followed by the bytes the key produced.
            $bytes = @()
            foreach ($l in (Get-Content $nlog -EA SilentlyContinue)) {
                if ($l -match 'hex=\[ ([0-9a-f ]+) \]') { $bytes += ($Matches[1].Trim() -split '\s+') }
            }
            $stream = ($bytes -join ' ')
            foreach ($c in $cases) {
                $mk = "{0:x2}" -f ([int][char]$c.m)
                if ($stream -match ($mk + ' ' + $c.want + '(\s|$)')) {
                    Write-Pass "$($c.n) -> [$($c.want)]"
                } else {
                    Write-Fail "$($c.n): expected [$mk $($c.want)] in the stream, got [$stream]"
                }
            }

            # The ESC+CR pair must also arrive in ONE read, not two.  A split
            # is what makes a readline app cancel and then submit, and it is
            # invisible to a byte-stream-only assertion.
            $reads = @()
            foreach ($l in (Get-Content $nlog -EA SilentlyContinue)) {
                if ($l -match '^(\d+) BYTES hex=\[ ([0-9a-f ]+) \]') {
                    $reads += @{ t = [int]$Matches[1]; hex = $Matches[2].Trim() }
                }
            }
            $atomic = @($reads | Where-Object { $_.hex -eq '1b 0d' })
            $loneEsc = @($reads | Where-Object { $_.hex -eq '1b' })
            if ($atomic.Count -ge 4) {
                Write-Pass "every modified Enter arrived as a single [1b 0d] read ($($atomic.Count) of them)"
            } else {
                Write-Fail "only $($atomic.Count) atomic [1b 0d] reads; reads were: $(($reads | ForEach-Object { $_.hex }) -join ' / ')"
            }
            # The only lone ESC allowed is none: every ESC in this run belonged
            # to a pair.
            if ($loneEsc.Count -eq 0) {
                Write-Pass "no lone ESC leaked ahead of a CR"
            } else {
                Write-Fail "$($loneEsc.Count) lone ESC read(s) leaked; a readline child reads that as cancel"
            }

            # A real Escape must still get through, and exactly once.
            & $inject $cpid 'text:9' 'sleep:400' | Out-Null
            Start-Sleep -Milliseconds 600
            & $inject $cpid 'esc' | Out-Null
            Start-Sleep -Seconds 2
            $bytes = @()
            foreach ($l in (Get-Content $nlog -EA SilentlyContinue)) {
                if ($l -match 'hex=\[ ([0-9a-f ]+) \]') { $bytes += ($Matches[1].Trim() -split '\s+') }
            }
            $stream = ($bytes -join ' ')
            if ($stream -match '39 1b(\s|$)') {
                Write-Pass "a lone Escape still reaches the pane exactly once after its window"
            } else {
                Write-Fail "lone Escape did not arrive as a single 1b; stream was [$stream]"
            }
        }
    }
    Cleanup-Session $sess
    Drop-StaleClients $sess
}

# =============================================================================
# TEST 3: send-keys parity.  The server side names must still produce the same
# bytes as the client side event, so a binding and a keystroke agree.
# =============================================================================
Write-Host "`n[Test 3] send-keys agrees with the client path on modified Enter" -ForegroundColor Yellow
$sess = "i611_sk"
$log  = "$env:TEMP\psmux_611_vt.txt"
Remove-Item -Force -EA SilentlyContinue $log
Cleanup-Session $sess
& $PSMUX -L $SOCK -f $emptyConf new-session -d -s $sess -x 100 -y 30 -- $reclog vt $log 2>&1 | Out-Null
Start-Sleep -Seconds 3
if (-not (Test-Path $log)) {
    Write-Fail "byte logging pane never started"
} else {
    $cases = @(
        @{ m='1'; k='Enter';   want='0D';    why='plain Enter submits' },
        @{ m='2'; k='S-Enter'; want='1B 0D'; why='Shift+Enter is ESC CR, what libuv turns into meta+return' },
        @{ m='3'; k='M-Enter'; want='1B 0D'; why='Alt+Enter is the same bytes, matching tmux M-Enter' }
    )
    foreach ($c in $cases) {
        & $PSMUX -L $SOCK send-keys -t $sess $c.m 2>&1 | Out-Null
        Start-Sleep -Milliseconds 250
        & $PSMUX -L $SOCK send-keys -t $sess $c.k 2>&1 | Out-Null
        Start-Sleep -Milliseconds 350
    }
    Start-Sleep -Milliseconds 700
    $lines = @(Get-Content $log -EA SilentlyContinue | Where-Object { $_ -match '^BYTES' })
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

& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null

Write-Host "`n=== Issue #611 Results ===" -ForegroundColor Cyan
Write-Host "Passed: $script:TestsPassed" -ForegroundColor Green
Write-Host "Failed: $script:TestsFailed" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
