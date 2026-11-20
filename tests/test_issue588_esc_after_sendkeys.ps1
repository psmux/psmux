# Issue #588: after one `psmux send-keys ... C-m` into a pane, no Escape key
# ever reached that pane's application again.  Reported against cygwin bash,
# reproduced identically in a WSL pane and in nvim's insert mode, which is what
# told the reporter it was not a shell problem but a psmux one.
#
# ROOT CAUSE, measured before anything was changed:
#
#   `send-keys C-<letter>` writes a WIN32 INPUT MODE key sequence
#   `ESC [ Vk ; Sc ; Uc ; Kd ; Cs ; Rc _` into the pane's ConPTY input pipe,
#   because the plain control byte does not give the child a real
#   VK + LEFT_CTRL_PRESSED record (issue #305).
#
#   Writing one is a ONE-WAY LATCH on conhost's side.  Its input state machine
#   concludes the terminal speaks win32 input mode and, from then on, stops
#   dispatching a DANGLING `ESC` at the end of a write as the Escape key: it
#   holds it as the possible start of a longer sequence.  psmux delivers the
#   Escape key as a bare 0x1b, so after a single `send-keys ... C-m` the key was
#   gone for the life of that pane.
#
#   Test 1 is the oracle for that sentence and needs no psmux at all: it feeds a
#   BARE pseudoconsole the exact bytes and reads what the child received.
#
# The fix remembers the latch on the pane and, once set, writes a lone Escape in
# win32 form too -- the only form conhost still accepts.  The C- sequence itself
# is unchanged, so #305 keeps working.
#
# Run: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue588_esc_after_sendkeys.ps1

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$SOCK  = "i588"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green;    $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red;      $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor DarkGray }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor Cyan }

# --- helper binaries ---------------------------------------------------------
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
$injector = "$env:TEMP\psmux_588_injector.exe"
$feeder   = "$env:TEMP\psmux_588_conptyfeed.exe"
$keylog   = "$env:TEMP\psmux_588_keylog.exe"
& $csc /nologo /platform:x64 /out:$injector "$PSScriptRoot\injector.cs"       2>&1 | Out-Null
& $csc /nologo /platform:x64 /out:$feeder   "$PSScriptRoot\conptyfeed610.cs"  2>&1 | Out-Null
& $csc /nologo /platform:x64 /out:$keylog   "$PSScriptRoot\keylog_child.cs"   2>&1 | Out-Null
foreach ($exe in @($injector, $feeder, $keylog)) {
    if (-not (Test-Path $exe)) {
        Write-Host "  [FAIL] could not build the C# harnesses (csc at $csc): missing $exe" -ForegroundColor Red
        exit 1
    }
}

# Real keystrokes go through Invoke-GuardedInjector. In sweep
# 2026-10-01_01-53-14 Defender quarantined psmux_588_injector.exe in the middle
# of Test 2; every later raw `& $injector` call failed silently, and Test 3
# reported "#588 REGRESSION: no ESC reached the pane app" because its delivery
# oracle read a STALE psmux_inject.log full of `vk=` lines from an earlier,
# successful injection. The guard deletes that log before every run, so the
# oracle only ever describes the injection it follows, and it records why the
# injector could not run (with any Defender detection) in $script:InjectorBlocked.
. "$PSScriptRoot\injector_guard.ps1"
function Send-Keys588($procId, [string]$Keys) { $null = Invoke-GuardedInjector $injector $procId $Keys }
function No-Delivery-Note { if ($script:InjectorBlocked) { " ($($script:InjectorBlocked))" } else { "" } }

$emptyConf = "$env:TEMP\psmux_588_empty.conf"
"" | Set-Content -Path $emptyConf -Encoding ASCII

# keylog_child.cs always writes to this path.
$KEYLOG_OUT = "$env:TEMP\psmux_keylog.txt"

function Cleanup-Session($name) {
    & $PSMUX -L $SOCK kill-session -t $name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
}
function Cap($target) { (& $PSMUX -L $SOCK capture-pane -p -S -3000 -t $target 2>&1 | Out-String) }

Write-Host "`n=== Issue #588: send-keys C-<letter> must not kill the Escape key ===" -ForegroundColor Cyan

# =============================================================================
# TEST 1: the conhost contract, under a bare pseudoconsole, no psmux involved
# =============================================================================
Write-Host "`n[Test 1] a lone ESC byte dies after a win32-input-mode sequence, the win32 form survives" -ForegroundColor Yellow

$ctrl = "$env:TEMP\psmux_588_ctrl.txt"
$out  = "$env:TEMP\psmux_588_ptyout.bin"
foreach ($f in @($ctrl, $out, $KEYLOG_OUT)) { if (Test-Path $f) { Remove-Item -LiteralPath $f -Force } }
"" | Set-Content $ctrl -Encoding ASCII -NoNewline

$feed = Start-Process -FilePath $feeder -PassThru -WindowStyle Hidden -ArgumentList $ctrl,$out,$keylog
Start-Sleep -Seconds 2

function Feed($line) { Add-Content -Path $ctrl -Value $line -Encoding ASCII; Start-Sleep -Milliseconds 900 }
function KeyLines() { if (Test-Path $KEYLOG_OUT) { @(Get-Content $KEYLOG_OUT) } else { @() } }

# 1a. a bare ESC before any win32 sequence: the baseline that used to hold.
Feed "HEX 1b"
$afterFirstEsc = KeyLines
if (@($afterFirstEsc | Where-Object { $_ -match 'key=Escape' }).Count -ge 1) {
    Write-Pass "a bare 0x1b arrives as key=Escape on a fresh ConPTY"
} else {
    Write-Skip "the harness child logged no Escape at all (log: $($afterFirstEsc -join ' | ')) - conhost contract untested"
    $script:Test1Skipped = $true
}

if (-not $script:Test1Skipped) {
    # 1b. the sequence psmux emits for `send-keys C-m`:
    #     ESC[77;50;13;1;8;1_ ESC[77;50;13;0;8;1_   (VK_M=77 scan=50 uchar=13 ctrl=8)
    Feed "HEX 1b 5b 37 37 3b 35 30 3b 31 33 3b 31 3b 38 3b 31 5f 1b 5b 37 37 3b 35 30 3b 31 33 3b 30 3b 38 3b 31 5f"
    $afterCtrlM = KeyLines
    $newCtrlM = @($afterCtrlM | Select-Object -Skip $afterFirstEsc.Count)
    # The inbox pseudo console of Windows 10 (19045) has no win32-input-mode,
    # so neither win32 sequence can be read there; the #588 latch it repairs
    # belongs to a host that has the mode.
    $NoW32 = [Environment]::OSVersion.Version.Build -lt 22000
    if (@($newCtrlM | Where-Object { $_ -match 'key=M' -and $_ -match 'Control' }).Count -ge 1) {
        Write-Pass "the win32-input-mode sequence delivers a real Ctrl+M record (this is what #305 needs)"
    } elseif ($NoW32) {
        Write-Skip "Windows 10 inbox pseudo console has no win32-input-mode (got: $($newCtrlM -join ' | '))"
    } else {
        Write-Fail "the win32-input-mode sequence did not deliver Ctrl+M (got: $($newCtrlM -join ' | '))"
    }

    # 1c. THE BUG: bare ESC bytes after the latch are swallowed by conhost.
    Feed "HEX 1b"
    Feed "HEX 1b"
    Feed "HEX 1b"
    $afterDeadEsc = KeyLines
    $newDead = @($afterDeadEsc | Select-Object -Skip $afterCtrlM.Count)
    if ($newDead.Count -eq 0) {
        Write-Pass "three bare 0x1b bytes after the latch reach the child as NOTHING (the #588 mechanism)"
    } else {
        Write-Info "this Windows build still dispatches a bare ESC after a win32 sequence: $($newDead -join ' | ')"
        Write-Info "the #588 repair is then merely redundant, never wrong - Test 3 is the behavioural gate"
    }

    # 1d. the repair: the SAME key written as a win32 sequence still arrives.
    #     ESC[27;1;27;1;0;1_ ESC[27;1;27;0;0;1_   (VK_ESCAPE=27 scan=1 uchar=27)
    Feed "HEX 1b 5b 32 37 3b 31 3b 32 37 3b 31 3b 30 3b 31 5f 1b 5b 32 37 3b 31 3b 32 37 3b 30 3b 30 3b 31 5f"
    $afterFix = KeyLines
    $newFix = @($afterFix | Select-Object -Skip $afterDeadEsc.Count)
    if (@($newFix | Where-Object { $_ -match 'key=Escape' }).Count -ge 1) {
        Write-Pass "Escape written in win32 form arrives as key=Escape even on a latched ConPTY (the fix)"
    } elseif ($NoW32) {
        Write-Skip "Windows 10 inbox pseudo console has no win32-input-mode, Escape in win32 form is not read there"
    } else {
        Write-Fail "Escape in win32 form did not arrive (got: $($newFix -join ' | '))"
    }
}

Feed "QUIT"
if ($feed -and -not $feed.HasExited) { Stop-Process -Id $feed.Id -Force -EA SilentlyContinue }

# =============================================================================
# TEST 2: the reporter's own repro, cygwin bash + stty raw + dd
# =============================================================================
Write-Host "`n[Test 2] the reporter's repro: ESC reaches a raw-mode cygwin pane after send-keys C-m" -ForegroundColor Yellow

$BASH = "C:/cygwin64/bin/bash.exe"
$LINE = "  /usr/bin/stty raw -echo; /usr/bin/dd bs=1 count=3 | /usr/bin/od -An -tx1; /usr/bin/stty sane"

if (-not (Test-Path $BASH)) {
    Write-Skip "cygwin is not installed at C:\cygwin64 - install it to run the reporter's own repro"
} else {
    # $poison: run `send-keys " cd ." C-m` against the pane before probing.
    function Run-CygwinArm($sess, $poison) {
        Cleanup-Session $sess
        $client = Start-Process -FilePath $PSMUX -PassThru -ArgumentList `
            "-L",$SOCK,"-f",$emptyConf,"new-session","-s",$sess,"-x","100","-y","30","--",$BASH
        Start-Sleep -Seconds 5

        & $PSMUX -L $SOCK has-session -t $sess 2>$null
        if ($LASTEXITCODE -ne 0) { return @{ up = $false; client = $client } }
        & $PSMUX -L $SOCK set-option -t $sess remain-on-exit on 2>&1 | Out-Null

        if ($poison) {
            & $PSMUX -L $SOCK send-keys -t $sess " cd ." C-m 2>&1 | Out-Null
            Start-Sleep -Seconds 2
        }

        # The stty/dd line is typed with REAL keystrokes, never send-keys:
        # send-keys is the variable under test and must not leak into the arm.
        Send-Keys588 $client.Id $LINE
        Start-Sleep -Milliseconds 700
        if ((Cap $sess) -notmatch '/usr/bin/stty raw') {
            Send-Keys588 $client.Id $LINE
            Start-Sleep -Milliseconds 700
        }
        Send-Keys588 $client.Id "{ENTER}"
        Start-Sleep -Seconds 2

        Send-Keys588 $client.Id "{ESC}{SLEEP:400}{ESC}{SLEEP:400}{ESC}"
        # The injector log is the delivery oracle: no `vk=` line means the keys
        # never reached the console input buffer, which is a SKIP, not a FAIL.
        $ilog = Get-InjectorLog
        Start-Sleep -Seconds 3

        $cap = Cap $sess
        # Unblock the dd so the pane is not left holding a raw-mode read.
        if ($cap -notmatch '1b\s+1b\s+1b') { Send-Keys588 $client.Id "zzz"; Start-Sleep -Seconds 2 }
        return @{ up = $true; client = $client; cap = $cap; ilog = $ilog }
    }

    # 2a. baseline: no send-keys anywhere near the pane.
    $base = Run-CygwinArm "i588_base" $false
    if (-not $base.up) {
        Write-Fail "the attached cygwin session did not come up"
    } elseif ($base.ilog -notmatch 'vk=') {
        Write-Skip "the injector delivered no key records (focus/console refused) - not a psmux result$(No-Delivery-Note)"
    } elseif ($base.cap -match '1b\s+1b\s+1b') {
        Write-Pass "baseline: three real ESC presses arrive as 1b 1b 1b"

        # 2b. the bug: the SAME thing after one send-keys with C-m.
        $bad = Run-CygwinArm "i588_bad" $true
        if (-not $bad.up) {
            Write-Fail "the attached cygwin session did not come up for the send-keys arm"
        } elseif ($bad.ilog -notmatch 'vk=') {
            Write-Skip "the injector delivered no key records for the send-keys arm$(No-Delivery-Note)"
        } elseif ($bad.cap -match '1b\s+1b\s+1b') {
            Write-Pass "after `send-keys ' cd .' C-m` the ESC presses STILL arrive as 1b 1b 1b (#588 fixed)"
        } else {
            $tail = (($bad.cap -split "`r?`n" | Where-Object { $_.Trim() -ne "" }) | Select-Object -Last 4) -join " / "
            Write-Fail "#588 REGRESSION: after send-keys C-m the ESC presses were swallowed (screen: $tail)"
        }
        if ($bad.client -and -not $bad.client.HasExited) { Stop-Process -Id $bad.client.Id -Force -EA SilentlyContinue }
        Cleanup-Session "i588_bad"
    } else {
        $tail = (($base.cap -split "`r?`n" | Where-Object { $_.Trim() -ne "" }) | Select-Object -Last 4) -join " / "
        Write-Skip "the baseline itself produced no 1b 1b 1b (screen: $tail) - the arm proves nothing"
    }
    if ($base.client -and -not $base.client.HasExited) { Stop-Process -Id $base.client.Id -Force -EA SilentlyContinue }
    Cleanup-Session "i588_base"
}

# =============================================================================
# TEST 3: shell-agnostic gate -- a record-reading pane app, no cygwin needed
# =============================================================================
Write-Host "`n[Test 3] a real ESC keystroke reaches a record-reading pane app after send-keys C-m" -ForegroundColor Yellow

$sess3 = "i588_rec"
Cleanup-Session $sess3
if (Test-Path $KEYLOG_OUT) { Remove-Item -LiteralPath $KEYLOG_OUT -Force }

$client3 = Start-Process -FilePath $PSMUX -PassThru -ArgumentList `
    "-L",$SOCK,"-f",$emptyConf,"new-session","-s",$sess3,"-x","100","-y","30","--",$keylog
Start-Sleep -Seconds 5

& $PSMUX -L $SOCK has-session -t $sess3 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "the attached session for the record-reading pane did not come up"
} elseif (-not (Test-Path $KEYLOG_OUT)) {
    Write-Skip "the key-logging pane app never started (no $KEYLOG_OUT)"
} else {
    # Latch the pane's ConPTY into win32 input mode, exactly as the reporter did.
    & $PSMUX -L $SOCK send-keys -t $sess3 " cd ." C-m 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    $before = @(Get-Content $KEYLOG_OUT)

    Send-Keys588 $client3.Id "{ESC}{SLEEP:400}{ESC}{SLEEP:400}{ESC}"
    $ilog3 = Get-InjectorLog
    Start-Sleep -Seconds 3

    $after = @(Get-Content $KEYLOG_OUT)
    $new = @($after | Select-Object -Skip $before.Count)
    $escapes = @($new | Where-Object { $_ -match 'key=Escape' })

    if ($ilog3 -notmatch 'vk=') {
        Write-Skip "the injector delivered no key records (focus/console refused) - not a psmux result$(No-Delivery-Note)"
    } elseif ($escapes.Count -ge 3) {
        Write-Pass "all three ESC presses reached the pane app after send-keys C-m ($($escapes.Count) Escape records)"
    } elseif ($escapes.Count -ge 1) {
        Write-Fail "#588: only $($escapes.Count) of 3 ESC presses reached the pane app (new records: $($new -join ' | '))"
    } else {
        Write-Fail "#588 REGRESSION: no ESC reached the pane app after send-keys C-m (new records: $($new -join ' | '))"
    }
}
if ($client3 -and -not $client3.HasExited) { Stop-Process -Id $client3.Id -Force -EA SilentlyContinue }
Cleanup-Session $sess3

# =============================================================================
Write-Host "`n=== Results: $script:TestsPassed passed, $script:TestsFailed failed ===" -ForegroundColor Cyan
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
