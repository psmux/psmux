# Issue #1036: "SSH input reads horizontal wheel (buttons 66/67) as
# ScrollDown, so trackpad scrolling jitters"
#
# macOS Ghostty -> ssh -> Windows 11, mouse on. A vertical trackpad swipe
# drifts sideways a little, and the terminal reports that drift as SGR wheel
# left/right (`ESC[<66;x;yM`, `ESC[<67;x;yM`) between the wheel up reports.
# The SSH input decoder read every wheel button other than 64 as wheel down,
# so each sideways report scrolled copy mode back down: up, up, down, up, ...
#
# tmux never scrolls on 66/67 either: they are buttons 6 and 7
# (MouseDown6Pane / MouseDown7Pane), not WheelUp/WheelDown.
#
# HARNESS (same shape as test_issue629_ssh_vt_wheel.ps1). An OUTER psmux
# session owns a ConPTY; the psmux client under test runs INSIDE it with
# SSH_CONNECTION set, so its stdin is a real pseudoconsole and it takes the
# SSH VT input path. Raw SGR bytes are delivered with `send-keys -l`, exactly
# as the terminal writes them down the link.
#
# Every psmux call uses its own -L namespaces and PSMUX_DATA_DIR.

$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }

$rand   = Get-Random -Maximum 999999
$NS_OUT = "i1036out$rand"
$NS_IN  = "i1036in$rand"
$S_OUT  = "outer"
$S_IN   = "inner"
$ESC    = [char]27

$ownDataDir = $false
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $env:TEMP "psmux_i1036_$rand"
    New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
    $ownDataDir = $true
}
$psmuxDir = $env:PSMUX_DATA_DIR

$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

function Cleanup {
    & $PSMUX -L $NS_IN  kill-server 2>&1 | Out-Null
    & $PSMUX -L $NS_OUT kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
}

function Finish {
    Cleanup
    if ($ownDataDir) { Remove-Item $psmuxDir -Recurse -Force -EA SilentlyContinue }
    Write-Host "`n=== Results ===" -ForegroundColor Cyan
    Write-Host "  Passed: $($script:TestsPassed)"
    Write-Host "  Failed: $($script:TestsFailed)"
    exit $(if ($script:TestsFailed -gt 0) { 1 } else { 0 })
}

function Start-Link {
    & $PSMUX -L $NS_OUT new-session -d -s $S_OUT -x 120 -y 40 2>&1 | Out-Null
    Start-Sleep -Seconds 4
    & $PSMUX -L $NS_OUT has-session -t $S_OUT 2>$null
    if ($LASTEXITCODE -ne 0) { return $false }
    # PSMUX_SESSION must go or the inner client refuses to nest. SSH_CONNECTION
    # puts the client on the VT input path; PSMUX_SSH_DEBUG logs every decoded
    # event to ssh_input.log in the data dir.
    $cmd = "Remove-Item Env:\PSMUX_SESSION,Env:\PSMUX_SESSION_NAME,Env:\PSMUX_PANE -EA SilentlyContinue; " +
           "`$env:SSH_CONNECTION='10.0.0.5 51000 10.0.0.9 22'; `$env:SSH_TTY='/dev/pts/0'; " +
           "`$env:TERM='xterm-256color'; `$env:PSMUX_SSH_DEBUG='1'; " +
           "`$env:PSMUX_DATA_DIR='$psmuxDir'; & '$PSMUX' -L $NS_IN new-session -s $S_IN"
    & $PSMUX -L $NS_OUT send-keys -t $S_OUT $cmd Enter 2>&1 | Out-Null
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep -Milliseconds 500
        & $PSMUX -L $NS_IN has-session -t $S_IN 2>$null
        if ($LASTEXITCODE -eq 0) { Start-Sleep -Seconds 3; return $true }
    }
    return $false
}

function InnerFmt([string]$f) { (& $PSMUX -L $NS_IN display-message -t $S_IN -p $f 2>&1 | Out-String).Trim() }
function InMode { InnerFmt '#{pane_in_mode}' }
function ScrollPos { $v = InnerFmt '#{scroll_position}'; if ($v -match '^\d+$') { [int]$v } else { -1 } }
function Send-Sgr([string]$seq, [int]$n = 1) {
    for ($i = 0; $i -lt $n; $i++) {
        & $PSMUX -L $NS_OUT send-keys -t $S_OUT -l $seq 2>&1 | Out-Null
        Start-Sleep -Milliseconds 250
    }
}
function SshLog {
    $log = Join-Path $psmuxDir "ssh_input.log"
    if (-not (Test-Path $log)) { return "" }
    $tmp = Join-Path $psmuxDir "ssh_input.snapshot"
    Copy-Item $log $tmp -Force -EA SilentlyContinue
    if (Test-Path $tmp) { Get-Content $tmp -Raw -EA SilentlyContinue } else { "" }
}

$UP    = "$ESC[<64;25;10M"
$DOWN  = "$ESC[<65;25;10M"
$LEFT  = "$ESC[<66;25;10M"
$RIGHT = "$ESC[<67;25;10M"

Write-Host "`n=== Issue #1036: horizontal wheel over SSH must never scroll ===" -ForegroundColor Cyan

if (-not (Start-Link)) {
    Write-Fail "could not bring up the nested SSH shaped link"
    Finish
}
& $PSMUX -L $NS_IN set-option -t $S_IN -g mouse on 2>&1 | Out-Null
& $PSMUX -L $NS_IN send-keys -t $S_IN '1..400 | % { "SB_$_" }' Enter 2>&1 | Out-Null
Start-Sleep -Seconds 5

# ---------------------------------------------------------------------
Write-Host "`n[Test 1] wheel left/right over a plain pane does nothing" -ForegroundColor Yellow
Send-Sgr $LEFT 3
Send-Sgr $RIGHT 3
Start-Sleep -Milliseconds 800
$m = InMode
if ($m -eq "0") { Write-Pass "pane_in_mode=0 after 6 sideways notches" }
else { Write-Fail "pane_in_mode=$m after sideways notches" }

# ---------------------------------------------------------------------
Write-Host "`n[Test 2] sideways notches inside copy mode keep the scroll position" -ForegroundColor Yellow
Send-Sgr $UP 6
Start-Sleep -Milliseconds 1200
$p0 = ScrollPos
if ((InMode) -eq "1" -and $p0 -gt 0) { Write-Pass "wheel up entered copy mode (scroll_position=$p0)" }
else { Write-Fail "wheel up did not enter copy mode (mode=$(InMode) pos=$p0); the rest is not meaningful" }
Send-Sgr $LEFT 2
Send-Sgr $RIGHT 2
Start-Sleep -Milliseconds 1000
$p1 = ScrollPos
if ($p1 -eq $p0 -and (InMode) -eq "1") { Write-Pass "scroll_position stayed $p0 after 4 sideways notches" }
else { Write-Fail "sideways notches moved the view: scroll_position $p0 -> $p1 (mode=$(InMode)); #1036" }

# ---------------------------------------------------------------------
Write-Host "`n[Test 3] the reporter's swipe (up, left, up, right, up) only goes up" -ForegroundColor Yellow
$q0 = ScrollPos
foreach ($s in @($UP, $LEFT, $UP, $RIGHT, $UP)) { Send-Sgr $s 1 }
Start-Sleep -Milliseconds 1000
$q1 = ScrollPos
$step = $q1 - $q0
# Three up notches must move as far as three up notches alone. Measure that
# reference directly instead of assuming the lines per notch.
Send-Sgr $UP 3
Start-Sleep -Milliseconds 1000
$q2 = ScrollPos
$ref = $q2 - $q1
if ($step -gt 0 -and $step -eq $ref) { Write-Pass "the swipe moved $step lines, same as 3 plain up notches ($ref)" }
else { Write-Fail "the swipe moved $step lines but 3 up notches move $ref ($q0 -> $q1 -> $q2); jitter of #1036" }

# ---------------------------------------------------------------------
Write-Host "`n[Test 4] vertical wheel down still scrolls down" -ForegroundColor Yellow
$d0 = ScrollPos
Send-Sgr $DOWN 2
Start-Sleep -Milliseconds 1000
$d1 = ScrollPos
if ($d1 -lt $d0) { Write-Pass "wheel down moved $d0 -> $d1" }
else { Write-Fail "wheel down did not scroll ($d0 -> $d1)" }

# ---------------------------------------------------------------------
Write-Host "`n[Test 5] the SSH decoder log names the sideways reports" -ForegroundColor Yellow
$log = SshLog
# PSMUX_SSH_DEBUG=1 logs each decoded event as `emit(char): Mouse(MouseEvent
# { kind: ScrollLeft, ... })`; without it the line reads `MOUSE via VT parser:`.
$left  = ([regex]::Matches($log, 'kind: ScrollLeft\b')).Count
$right = ([regex]::Matches($log, 'kind: ScrollRight\b')).Count
$down  = ([regex]::Matches($log, 'kind: ScrollDown\b')).Count
if ($left -ge 6 -and $right -ge 6) { Write-Pass "ssh_input.log: $left ScrollLeft, $right ScrollRight, $down ScrollDown (2 real down notches sent)" }
else { Write-Fail "ssh_input.log: $left ScrollLeft, $right ScrollRight, $down ScrollDown (expected 6+ of each sideways kind)" }
if ($down -eq 2) { Write-Pass "only the 2 real wheel down notches decoded as ScrollDown" }
else { Write-Fail "$down ScrollDown decoded for 2 real down notches" }

Finish
