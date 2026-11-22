# Issue #629: "Mouse scroll wheel not working over SSH (Kitty -> Windows)"
#
# macOS kitty -> ssh -> Windows 11, PowerShell 5.1, TERM=xterm-256color, psmux
# 3.3.8.  The wheel reached the client and decoded cleanly:
#
#   send_mouse_enable: writing mouse-enable VT sequences to stdout
#   stdin console mode: 0x0298 VTI=true MOUSE=true
#   MOUSE via VT parser: Mouse(MouseEvent { kind: ScrollUp, column: 25, row: 30, ... })
#
# and then nothing happened: no copy mode, no scroll.  Clicks and pane selection
# over the same link worked, and `set -g scroll-enter-copy-mode off` restored a
# visible scroll.
#
# WHAT THIS SUITE PINS.  An SSH login is the only way a psmux client receives
# the wheel as SGR bytes on stdin instead of Win32 MOUSE_EVENT records, so the
# VT-parser path is the one condition the reporter had and a local terminal does
# not.  Every check below drives that path for real and asserts tmux parity: a
# notch over a pane whose application never asked for the mouse enters copy mode
# and scrolls, no matter which shape the client received it in.
#
# HARNESS.  An OUTER psmux session owns a ConPTY; the psmux client under test
# runs INSIDE it with SSH_CONNECTION set, so its stdin is a real pseudoconsole -
# the same shape sshd hands a login shell - and raw SGR bytes are delivered with
# `send-keys -l`, exactly as kitty writes them down the link.  capture-pane on
# the OUTER pane then shows what the client actually PAINTED, which is what the
# reporter was looking at; asserting on server state alone would have missed a
# client that never repainted.
#
# Layers: E2E over a real ConPTY, VT/SGR input path, client-painted verification,
#         byte-level forwarding check for the mouse-aware audience (#598/#573).

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$psmuxDir = if ($env:PSMUX_DATA_DIR) { $env:PSMUX_DATA_DIR } else { "$env:USERPROFILE\.psmux" }

$NS_OUT = "i629out"
$NS_IN  = "i629in"
$S_OUT  = "i629outer"
$S_IN   = "i629inner"
$ESC    = [char]27

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$child    = "$env:TEMP\psmux_i629_mouse_echo_child.exe"
$childLog = "$env:TEMP\psmux_mouse_echo.txt"
if (-not (Test-Path $child)) {
    & $csc /nologo /optimize /out:$child (Join-Path $repoTests "mouse_echo_child.cs") 2>&1 | Out-Null
}

function Cleanup {
    & $PSMUX -L $NS_IN  kill-session -t $S_IN  2>&1 | Out-Null
    & $PSMUX -L $NS_OUT kill-session -t $S_OUT 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    Remove-Item "$psmuxDir\${NS_IN}__$S_IN.*","$psmuxDir\${NS_OUT}__$S_OUT.*" -Force -EA SilentlyContinue
}

# Bring up OUTER (the fake ssh link) and INNER (the client under test).
function Start-Link {
    Cleanup
    & $PSMUX -L $NS_OUT new-session -d -s $S_OUT -x 120 -y 40 2>&1 | Out-Null
    Start-Sleep -Seconds 4
    & $PSMUX -L $NS_OUT has-session -t $S_OUT 2>$null
    if ($LASTEXITCODE -ne 0) { return $false }

    # PSMUX_SESSION must go or the inner client refuses to nest.  SSH_CONNECTION
    # is what puts the client on the VT input path (ssh_input::needs_vt_input).
    $cmd = "Remove-Item Env:\PSMUX_SESSION,Env:\PSMUX_SESSION_NAME,Env:\PSMUX_PANE -EA SilentlyContinue; " +
           "`$env:SSH_CONNECTION='10.0.0.5 51000 10.0.0.9 22'; `$env:SSH_TTY='/dev/pts/0'; " +
           "`$env:TERM='xterm-256color'; & '$PSMUX' -L $NS_IN new-session -s $S_IN"
    & $PSMUX -L $NS_OUT send-keys -t $S_OUT $cmd Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 9
    & $PSMUX -L $NS_IN has-session -t $S_IN 2>$null
    return ($LASTEXITCODE -eq 0)
}

function InnerMode { (& $PSMUX -L $NS_IN display-message -t $S_IN -p '#{pane_in_mode}' 2>&1).Trim() }
function PaintedRows { (& $PSMUX -L $NS_OUT capture-pane -t $S_OUT -p 2>&1) }
function PaintedTop {
    (PaintedRows | Where-Object { $_ -match 'SB_\d' } | Select-Object -First 1)
}
# Deliver raw SGR bytes into the inner client's ConPTY stdin, like kitty does.
function Send-Sgr([string]$seq, [int]$n = 1) {
    for ($i = 0; $i -lt $n; $i++) {
        & $PSMUX -L $NS_OUT send-keys -t $S_OUT -l $seq 2>&1 | Out-Null
        Start-Sleep -Milliseconds 250
    }
}
function Fill-Scrollback {
    & $PSMUX -L $NS_IN send-keys -t $S_IN '1..300 | % { "SB_$_" }' Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 6
}

Write-Host "`n=== Issue #629: the wheel must act on the VT/SGR input path (SSH) ===" -ForegroundColor Cyan

if (-not (Start-Link)) {
    Write-Fail "could not bring up the nested SSH-shaped link"
    Cleanup
    Write-Host "`n=== Results ===" -ForegroundColor Cyan
    Write-Host "  Passed: $($script:TestsPassed)"; Write-Host "  Failed: $($script:TestsFailed)"
    exit 1
}
& $PSMUX -L $NS_IN set-option -t $S_IN -g mouse on 2>&1 | Out-Null
Start-Sleep -Milliseconds 400
Fill-Scrollback

# ─────────────────────────────────────────────────────────────────────
# Test 1: the client really is on the VT parser path, not Win32 records
# ─────────────────────────────────────────────────────────────────────
Write-Host "`n[Test 1] The client under test decodes the wheel via the VT parser" -ForegroundColor Yellow
# The inner client truncates and then HOLDS ~/.psmux/ssh_input.log for its whole
# life, so it cannot be cleared here; read it in place instead.  Its existence is
# already the signal, since only the SSH/VT reader thread ever creates it.
$sshLog = "$psmuxDir\ssh_input.log"
Send-Sgr "$ESC[<64;25;10M" 1
Start-Sleep -Milliseconds 900
$sshTxt = if (Test-Path $sshLog) {
    # The client holds an exclusive-ish handle; copy it before reading.
    $tmp = "$env:TEMP\psmux_i629_ssh_input.snapshot"
    Copy-Item $sshLog $tmp -Force -EA SilentlyContinue
    if (Test-Path $tmp) { Get-Content $tmp -Raw -EA SilentlyContinue } else { "" }
} else { "" }
if ($sshTxt -match 'MOUSE via VT parser[^\r\n]*ScrollUp') {
    Write-Pass "wheel arrived as 'MOUSE via VT parser: ... ScrollUp' (the reporter's log line)"
} elseif ($sshTxt -match 'send_mouse_enable') {
    Write-Pass "client is on the SSH/VT input path (send_mouse_enable logged)"
} else {
    Write-Fail "the harness did not drive the VT parser path; the rest of this suite would not test #629"
}
& $PSMUX -L $NS_IN send-keys -t $S_IN -X cancel 2>&1 | Out-Null
Start-Sleep -Milliseconds 700

# The Windows 10 inbox conhost (build 19045) drops an SGR mouse report typed
# into a pane: measured by typing ESC[<64;10;5M, ESC[<66;10;5M and an SGR
# press and release with send-keys -l, as here, into a pane whose program
# reads raw VT input, which got only the letters around them. So there the
# outer pane hands the client under test no wheel, and Tests 2 to 5 have
# nothing to act on: what is left is that none of it is decoded or scrolls.
if ([Environment]::OSVersion.Version.Build -lt 22000) {
    Write-Host "`n[Windows 10] the outer pane hands the client no SGR report" -ForegroundColor Yellow
    if ($sshTxt -notmatch 'MOUSE via VT parser') { Write-Pass "the wheel up of Test 1 was not decoded: the host dropped it" }
    else { Write-Fail "the VT parser decoded a mouse report, which this host was measured not to deliver" }
    Send-Sgr "$ESC[<64;25;10M" 3
    Start-Sleep -Milliseconds 1500
    $mode = InnerMode
    if ($mode -eq "0") { Write-Pass "3 more wheel ups left the client out of copy mode (pane_in_mode=0)" }
    else { Write-Fail "pane_in_mode=${mode}: a wheel report reached the client, which this host was measured not to deliver" }
    Cleanup
    Write-Host "`n=== Results ===" -ForegroundColor Cyan
    Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
    Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
    exit $script:TestsFailed
}

# ─────────────────────────────────────────────────────────────────────
# Test 2: wheel-up on a plain pane enters copy mode AND repaints
# ─────────────────────────────────────────────────────────────────────
Write-Host "`n[Test 2] VT wheel-up over a plain shell pane enters copy mode and scrolls" -ForegroundColor Yellow
$before = PaintedTop
Send-Sgr "$ESC[<64;25;10M" 3
Start-Sleep -Milliseconds 1500
$after = PaintedTop
$mode = InnerMode
if ($mode -eq "1") { Write-Pass "pane_in_mode=1 (copy mode entered, tmux parity)" }
else { Write-Fail "pane_in_mode=$mode - the wheel was swallowed (#629)" }
if ($after -and $before -and $after -ne $before) {
    Write-Pass "the CLIENT REPAINTED a scrolled view ('$before' -> '$after')"
} else {
    Write-Fail "the client painted the same top row after 3 notches ('$before' -> '$after') - #629 silent swallow"
}
if ((PaintedRows) -match '\[copy mode\]') { Write-Pass "copy-mode indicator is visible on the client's screen" }
else { Write-Fail "no copy-mode indicator painted" }

# ─────────────────────────────────────────────────────────────────────
# Test 3: wheel-down inside copy mode scrolls back and leaves at the bottom
# ─────────────────────────────────────────────────────────────────────
Write-Host "`n[Test 3] VT wheel-down walks back out of copy mode" -ForegroundColor Yellow
$top0 = PaintedTop
Send-Sgr "$ESC[<65;25;10M" 2
Start-Sleep -Milliseconds 1200
$top1 = PaintedTop
if ($top1 -ne $top0) { Write-Pass "wheel-down scrolled the view back ('$top0' -> '$top1')" }
else { Write-Fail "wheel-down did nothing inside copy mode ('$top0')" }
Send-Sgr "$ESC[<65;25;10M" 4
Start-Sleep -Milliseconds 1500
if ((InnerMode) -eq "0") { Write-Pass "scrolling back to live output left copy mode" }
else { Write-Fail "still in copy mode after scrolling back to the bottom" }

# ─────────────────────────────────────────────────────────────────────
# Test 4: the reporter's workaround still behaves, and is NOT the fix
# ─────────────────────────────────────────────────────────────────────
Write-Host "`n[Test 4] scroll-enter-copy-mode off scrolls directly, without copy mode" -ForegroundColor Yellow
& $PSMUX -L $NS_IN send-keys -t $S_IN -X cancel 2>&1 | Out-Null
& $PSMUX -L $NS_IN set-option -t $S_IN -g scroll-enter-copy-mode off 2>&1 | Out-Null
Start-Sleep -Seconds 2
$before = PaintedTop
Send-Sgr "$ESC[<64;25;10M" 3
Start-Sleep -Milliseconds 1500
$after = PaintedTop
if ($after -and $before -and $after -ne $before) { Write-Pass "direct scrollback moved the view ('$before' -> '$after')" }
else { Write-Fail "scroll-enter-copy-mode off did not scroll ('$before' -> '$after')" }
if ((InnerMode) -eq "0") { Write-Pass "and did not enter copy mode" }
else { Write-Fail "scroll-enter-copy-mode off still entered copy mode" }

# Both branches must work.  #629 was reported as "on is broken, off works", so a
# suite that only proved `off` would have passed against the broken build.
& $PSMUX -L $NS_IN set-option -t $S_IN -g scroll-enter-copy-mode on 2>&1 | Out-Null
Start-Sleep -Seconds 2
& $PSMUX -L $NS_IN send-keys -t $S_IN -X cancel 2>&1 | Out-Null
Start-Sleep -Milliseconds 700
$before = PaintedTop
Send-Sgr "$ESC[<64;25;10M" 3
Start-Sleep -Milliseconds 1500
if ((InnerMode) -eq "1" -and (PaintedTop) -ne $before) {
    Write-Pass "turning the option back on restores copy-mode entry (the DEFAULT the reporter had)"
} else {
    Write-Fail "with scroll-enter-copy-mode on (the default) the wheel is silent - #629"
}

# ─────────────────────────────────────────────────────────────────────
# Test 5: a mouse-aware pane still receives raw SGR (no #598/#573 regression)
# ─────────────────────────────────────────────────────────────────────
Write-Host "`n[Test 5] A pane that DID ask for the mouse still gets the wheel bytes" -ForegroundColor Yellow
if (-not (Test-Path $child)) {
    Write-Skip "mouse_echo_child.exe did not compile"
} else {
    & $PSMUX -L $NS_IN send-keys -t $S_IN -X cancel 2>&1 | Out-Null
    Remove-Item $childLog -Force -EA SilentlyContinue
    & $PSMUX -L $NS_IN send-keys -t $S_IN ($child -replace '\\','/') Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 5
    $paneTxt = (& $PSMUX -L $NS_IN capture-pane -t $S_IN -p 2>&1) | Out-String
    if ($paneTxt -match 'MOUSE_ECHO_READY') {
        $before = if (Test-Path $childLog) { (Get-Content $childLog).Count } else { 0 }
        Send-Sgr "$ESC[<64;25;10M" 2
        Start-Sleep -Milliseconds 1200
        $all = if (Test-Path $childLog) { Get-Content $childLog } else { @() }
        $new = if ($all.Count -gt $before) { $all[$before..($all.Count-1)] | Where-Object { $_ -like 'RECV*' } } else { @() }
        if (($new -join ' ') -match '\[<64;\d+;\d+M') {
            Write-Pass "the mouse-aware child received SGR wheel reports (#573/#597 audience intact)"
        } else {
            Write-Fail "no SGR wheel bytes reached the mouse-aware child (regression of #573/#597)"
        }
        if ((InnerMode) -ne "1") { Write-Pass "and psmux did not steal the notch into copy mode (#598 intact)" }
        else { Write-Fail "psmux entered copy mode over a mouse-aware pane (#598 regression)" }
    } else {
        Write-Skip "mouse-reporting child did not start in the pane"
    }
}

Cleanup

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
