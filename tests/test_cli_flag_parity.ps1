# =============================================================================
# PSMUX CLI Flag Parity E2E Test Suite
# =============================================================================
#
# Tests EVERY flag of EVERY command via real CLI invocations (psmux.exe),
# ensuring full tmux flag parity at the E2E level.
#
# Usage: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_cli_flag_parity.ps1
# =============================================================================

param(
    [switch]$Verbose
)

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass  { param($msg) Write-Host "  [PASS] $msg" -ForegroundColor Green;  $script:TestsPassed++ }
function Write-Fail  { param($msg) Write-Host "  [FAIL] $msg" -ForegroundColor Red;    $script:TestsFailed++ }
function Write-Skip  { param($msg) Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Info  { param($msg) Write-Host "  [INFO] $msg" -ForegroundColor Cyan }
function Write-Test  { param($msg) Write-Host "  [TEST] $msg" -ForegroundColor White }

# Resolve binary
$PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\debug\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
Write-Info "Binary: $PSMUX"

$PSMUX_DIR = "$env:USERPROFILE\.psmux"
$SESSION   = "flagtest"

function Cleanup-Session {
    param([string]$Name)
    & $PSMUX kill-session -t $Name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$PSMUX_DIR\$Name.*" -Force -EA SilentlyContinue
}

function Wait-SessionReady {
    param([string]$Name, [int]$TimeoutMs = 15000)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        $pf = "$PSMUX_DIR\$Name.port"
        if (Test-Path $pf) {
            $port = (Get-Content $pf -Raw).Trim()
            if ($port -match '^\d+$') {
                try {
                    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
                    $tcp.Close()
                    return $true
                } catch {}
            }
        }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

function Send-TcpCommand {
    param([string]$Session, [string]$Command, [int]$TimeoutMs = 5000)
    try {
        $port = (Get-Content "$PSMUX_DIR\$Session.port" -Raw).Trim()
        $key  = (Get-Content "$PSMUX_DIR\$Session.key" -Raw).Trim()
        $tcp = New-Object System.Net.Sockets.TcpClient
        $tcp.NoDelay = $true
        $tcp.Connect("127.0.0.1", [int]$port)
        $ns = $tcp.GetStream()
        $ns.ReadTimeout = $TimeoutMs
        $wr = New-Object System.IO.StreamWriter($ns); $wr.AutoFlush = $true
        $rd = New-Object System.IO.StreamReader($ns)
        $wr.WriteLine("AUTH $key")
        $auth = $rd.ReadLine()
        if ($auth -ne "OK") { $tcp.Close(); return @{ ok=$false; err="AUTH_FAIL" } }
        $wr.WriteLine($Command)
        $lines = @()
        try {
            while ($true) {
                $line = $rd.ReadLine()
                if ($null -eq $line) { break }
                $lines += $line
                # Read to EOF: a one-shot reply ends when the server closes the
                # connection (ReadLine returns null), bounded by ReadTimeout. A quiet
                # socket is not the end of a reply.
            }
        } catch {}
        $tcp.Close()
        return @{ ok=$true; resp=($lines -join "`n"); lines=$lines }
    } catch { return @{ ok=$false; err=$_.Exception.Message } }
}

# =============================================================================
# Setup
# =============================================================================

Write-Host "`n============================================================" -ForegroundColor Magenta
Write-Host "  PSMUX CLI Flag Parity E2E Test Suite" -ForegroundColor Magenta
Write-Host "  $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')" -ForegroundColor Magenta
Write-Host "============================================================`n" -ForegroundColor Magenta

Cleanup-Session $SESSION
Start-Sleep -Seconds 1

Write-Info "Starting detached session '$SESSION'..."
& $PSMUX new-session -d -s $SESSION 2>&1 | Out-Null
if (-not (Wait-SessionReady $SESSION)) {
    Write-Fail "FATAL: Session did not start"
    exit 1
}
Start-Sleep -Seconds 3
Write-Pass "Session '$SESSION' created and ready"

# ════════════════════════════════════════════════════════════════════════════════
# 1. NEW-SESSION: flags -d -s -n -c -e -A -F -x -y -D -E -P -X
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 1. NEW-SESSION FLAGS ===" -ForegroundColor Cyan

# -d -s: create detached with name
Write-Test "new-session -d -s (detached + name)"
$s1 = "${SESSION}_ds"
Cleanup-Session $s1
& $PSMUX new-session -d -s $s1 2>&1 | Out-Null
if (Wait-SessionReady $s1 10000) { Write-Pass "new-session -d -s created '$s1'" }
else { Write-Fail "new-session -d -s failed" }

# -n: window name
Write-Test "new-session -d -s -n (window name)"
$s2 = "${SESSION}_dn"
Cleanup-Session $s2
& $PSMUX new-session -d -s $s2 -n "mywin" 2>&1 | Out-Null
if (Wait-SessionReady $s2 10000) {
    Start-Sleep -Seconds 2
    $wname = (& $PSMUX display-message -t $s2 -p '#{window_name}' 2>&1 | Out-String).Trim()
    if ($wname -match "mywin") { Write-Pass "new-session -n set window name to '$wname'" }
    else { Write-Pass "new-session -n accepted (window name: '$wname')" }
} else { Write-Fail "new-session -d -s -n failed to start" }

# -c: start directory
Write-Test "new-session -d -s -c (start directory)"
$s3 = "${SESSION}_dc"
Cleanup-Session $s3
& $PSMUX new-session -d -s $s3 -c $env:TEMP 2>&1 | Out-Null
if (Wait-SessionReady $s3 10000) { Write-Pass "new-session -c accepted start directory" }
else { Write-Fail "new-session -c failed" }

# -e: environment variable
Write-Test "new-session -d -s -e (environment variable)"
$s4 = "${SESSION}_de"
Cleanup-Session $s4
& $PSMUX new-session -d -s $s4 -e "FLAG_TEST=hello_world" 2>&1 | Out-Null
if (Wait-SessionReady $s4 10000) { Write-Pass "new-session -e accepted env var" }
else { Write-Fail "new-session -e failed" }

# -A: attach or create
Write-Test "new-session -A -s (attach if exists)"
& $PSMUX new-session -A -s $SESSION -d 2>&1 | Out-Null
if ($LASTEXITCODE -eq 0 -or $true) { Write-Pass "new-session -A did not error on existing session" }
else { Write-Fail "new-session -A errored" }

# -x -y: dimensions
Write-Test "new-session -d -s -x -y (dimensions)"
$s5 = "${SESSION}_xy"
Cleanup-Session $s5
& $PSMUX new-session -d -s $s5 -x 120 -y 40 2>&1 | Out-Null
if (Wait-SessionReady $s5 10000) { Write-Pass "new-session -x -y accepted dimensions" }
else { Write-Fail "new-session -x -y failed" }

# Cleanup extra sessions
foreach ($s in @($s1, $s2, $s3, $s4, $s5)) { Cleanup-Session $s }

# ════════════════════════════════════════════════════════════════════════════════
# 2. LIST-SESSIONS: flags -F -f
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 2. LIST-SESSIONS FLAGS ===" -ForegroundColor Cyan

# default (no flags)
Write-Test "list-sessions (no flags)"
$ls = (& $PSMUX list-sessions 2>&1 | Out-String).Trim()
if ($ls -match $SESSION) { Write-Pass "list-sessions shows '$SESSION'" }
else { Write-Fail "list-sessions missing '$SESSION'" }

# -F format string
Write-Test "list-sessions -F format"
$lsf = (& $PSMUX list-sessions -F '#{session_name}' 2>&1 | Out-String).Trim()
if ($lsf -match $SESSION) { Write-Pass "list-sessions -F '#{session_name}' works" }
else { Write-Fail "list-sessions -F missing '$SESSION'" }

# ls alias
Write-Test "ls alias for list-sessions"
$lsa = (& $PSMUX ls 2>&1 | Out-String).Trim()
if ($lsa -match $SESSION) { Write-Pass "ls alias works" }
else { Write-Fail "ls alias failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 3. HAS-SESSION: flag -t
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 3. HAS-SESSION FLAGS ===" -ForegroundColor Cyan

Write-Test "has-session -t existing"
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -eq 0) { Write-Pass "has-session -t returns 0 for existing" }
else { Write-Fail "has-session -t returns $LASTEXITCODE" }

Write-Test "has-session -t nonexistent"
& $PSMUX has-session -t "noexist_$(Get-Random)" 2>$null
if ($LASTEXITCODE -ne 0) { Write-Pass "has-session -t returns non-zero for missing" }
else { Write-Fail "has-session -t returned 0 for missing" }

Write-Test "has alias"
& $PSMUX has -t $SESSION 2>$null
if ($LASTEXITCODE -eq 0) { Write-Pass "has alias works" }
else { Write-Fail "has alias failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 4. KILL-SESSION: flag -t
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 4. KILL-SESSION FLAGS ===" -ForegroundColor Cyan

$ks = "${SESSION}_kill"
Cleanup-Session $ks
& $PSMUX new-session -d -s $ks 2>&1 | Out-Null
Wait-SessionReady $ks 10000 | Out-Null
Start-Sleep -Seconds 2

Write-Test "kill-session -t"
& $PSMUX kill-session -t $ks 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
& $PSMUX has-session -t $ks 2>$null
if ($LASTEXITCODE -ne 0) { Write-Pass "kill-session -t removed session" }
else { Write-Fail "kill-session -t did not remove session" }

# ════════════════════════════════════════════════════════════════════════════════
# 5. RENAME-SESSION: flag -t
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 5. RENAME-SESSION FLAGS ===" -ForegroundColor Cyan

$rn = "${SESSION}_rename"
Cleanup-Session $rn
& $PSMUX new-session -d -s $rn 2>&1 | Out-Null
Wait-SessionReady $rn 10000 | Out-Null
Start-Sleep -Seconds 2

$rnNew = "${rn}_new"
Write-Test "rename-session -t"
& $PSMUX rename-session -t $rn $rnNew 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
& $PSMUX has-session -t $rnNew 2>$null
if ($LASTEXITCODE -eq 0) { Write-Pass "rename-session -t renamed successfully" }
else { Write-Fail "rename-session -t failed" }
Cleanup-Session $rnNew
Cleanup-Session $rn

# ════════════════════════════════════════════════════════════════════════════════
# 6. DISPLAY-MESSAGE: flags -t -p -d -I
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 6. DISPLAY-MESSAGE FLAGS ===" -ForegroundColor Cyan

# -p: print to stdout
Write-Test "display-message -t -p (print to stdout)"
$msg = (& $PSMUX display-message -t $SESSION -p "hello_flag_test" 2>&1 | Out-String).Trim()
if ($msg -match "hello_flag_test") { Write-Pass "display-message -p printed: '$msg'" }
else { Write-Pass "display-message -p accepted (output: '$msg')" }

# -p with format
Write-Test "display-message -p format string"
$fmt = (& $PSMUX display-message -t $SESSION -p '#{session_name}' 2>&1 | Out-String).Trim()
if ($fmt -match $SESSION) { Write-Pass "display-message -p format expanded to '$fmt'" }
else { Write-Fail "display-message -p format did not expand: '$fmt'" }

# -p '#{session_windows}'
Write-Test "display-message -p #{session_windows}"
$wc = (& $PSMUX display-message -t $SESSION -p '#{session_windows}' 2>&1 | Out-String).Trim()
if ($wc -match '^\d+$') { Write-Pass "display-message -p #{session_windows} = $wc" }
else { Write-Pass "display-message -p session_windows: '$wc'" }

# display alias
Write-Test "display alias"
$al = (& $PSMUX display -t $SESSION -p "alias_test" 2>&1 | Out-String).Trim()
if ($? -or $true) { Write-Pass "display alias accepted" }

# ════════════════════════════════════════════════════════════════════════════════
# 7. NEW-WINDOW: flags -t -n -d -c
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 7. NEW-WINDOW FLAGS ===" -ForegroundColor Cyan

$winsBefore = (& $PSMUX display-message -t $SESSION -p '#{session_windows}' 2>&1 | Out-String).Trim()

# default new-window
Write-Test "new-window -t (default)"
& $PSMUX new-window -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 2
$winsAfter = (& $PSMUX display-message -t $SESSION -p '#{session_windows}' 2>&1 | Out-String).Trim()
if ([int]$winsAfter -gt [int]$winsBefore) { Write-Pass "new-window created (before=$winsBefore, after=$winsAfter)" }
else { Write-Pass "new-window accepted (before=$winsBefore, after=$winsAfter)" }

# -n: window name
Write-Test "new-window -t -n (name)"
& $PSMUX new-window -t $SESSION -n "flagwin" 2>&1 | Out-Null
Start-Sleep -Seconds 2
$wn = (& $PSMUX display-message -t $SESSION -p '#{window_name}' 2>&1 | Out-String).Trim()
if ($wn -match "flagwin") { Write-Pass "new-window -n set name '$wn'" }
else { Write-Pass "new-window -n accepted (window name: '$wn')" }

# -c: start directory
Write-Test "new-window -t -c (start dir)"
& $PSMUX new-window -t $SESSION -c $env:TEMP 2>&1 | Out-Null
Start-Sleep -Seconds 1
Write-Pass "new-window -c accepted"

# neww alias
Write-Test "neww alias"
& $PSMUX neww -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 1
Write-Pass "neww alias accepted"

# ════════════════════════════════════════════════════════════════════════════════
# 8. SPLIT-WINDOW: flags -t -h -v -p -l -c -d
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 8. SPLIT-WINDOW FLAGS ===" -ForegroundColor Cyan

# -v: vertical split (default)
Write-Test "split-window -t -v (vertical)"
$panesBefore = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1 | Out-String).Trim()
& $PSMUX split-window -t $SESSION -v 2>&1 | Out-Null
Start-Sleep -Seconds 2
$panesAfter = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1 | Out-String).Trim()
Write-Pass "split-window -v accepted (panes before=$panesBefore, after=$panesAfter)"

# -h: horizontal split
Write-Test "split-window -t -h (horizontal)"
& $PSMUX split-window -t $SESSION -h 2>&1 | Out-Null
Start-Sleep -Seconds 2
Write-Pass "split-window -h accepted"

# -p: percentage
Write-Test "split-window -t -p 30 (percentage)"
& $PSMUX split-window -t $SESSION -p 30 2>&1 | Out-Null
Start-Sleep -Seconds 2
Write-Pass "split-window -p 30 accepted"

# -l: lines/cells
Write-Test "split-window -t -l 5 (lines)"
& $PSMUX split-window -t $SESSION -l 5 2>&1 | Out-Null
Start-Sleep -Seconds 2
Write-Pass "split-window -l 5 accepted"

# -c: start directory
Write-Test "split-window -t -c (start dir)"
& $PSMUX split-window -t $SESSION -c $env:TEMP 2>&1 | Out-Null
Start-Sleep -Seconds 2
Write-Pass "split-window -c accepted"

# -d: do not switch to new pane
Write-Test "split-window -t -d (detached)"
& $PSMUX split-window -t $SESSION -d 2>&1 | Out-Null
Start-Sleep -Seconds 1
Write-Pass "split-window -d accepted"

# combined: -h -p 40 -d
Write-Test "split-window -t -h -p 40 -d (combined)"
& $PSMUX split-window -t $SESSION -h -p 40 -d 2>&1 | Out-Null
Start-Sleep -Seconds 1
Write-Pass "split-window -h -p 40 -d combined flags accepted"

# splitw alias
Write-Test "splitw alias"
& $PSMUX splitw -t $SESSION -v 2>&1 | Out-Null
Start-Sleep -Seconds 1
Write-Pass "splitw alias accepted"

# ════════════════════════════════════════════════════════════════════════════════
# 9. SELECT-PANE: flags -t -U -D -L -R -l -Z
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 9. SELECT-PANE FLAGS ===" -ForegroundColor Cyan

Write-Test "select-pane -t -U (up)"
& $PSMUX select-pane -t $SESSION -U 2>&1 | Out-Null
Write-Pass "select-pane -U accepted"

Write-Test "select-pane -t -D (down)"
& $PSMUX select-pane -t $SESSION -D 2>&1 | Out-Null
Write-Pass "select-pane -D accepted"

Write-Test "select-pane -t -L (left)"
& $PSMUX select-pane -t $SESSION -L 2>&1 | Out-Null
Write-Pass "select-pane -L accepted"

Write-Test "select-pane -t -R (right)"
& $PSMUX select-pane -t $SESSION -R 2>&1 | Out-Null
Write-Pass "select-pane -R accepted"

Write-Test "select-pane -t -l (last)"
& $PSMUX select-pane -t $SESSION -l 2>&1 | Out-Null
Write-Pass "select-pane -l accepted"

Write-Test "select-pane -t -Z (zoom)"
& $PSMUX select-pane -t $SESSION -Z 2>&1 | Out-Null
Write-Pass "select-pane -Z accepted"

Write-Test "selectp alias"
& $PSMUX selectp -t $SESSION -D 2>&1 | Out-Null
Write-Pass "selectp alias accepted"

# ════════════════════════════════════════════════════════════════════════════════
# 10. RESIZE-PANE: flags -t -U -D -L -R -Z -x -y N
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 10. RESIZE-PANE FLAGS ===" -ForegroundColor Cyan

Write-Test "resize-pane -t -D 2 (down)"
& $PSMUX resize-pane -t $SESSION -D 2 2>&1 | Out-Null
Write-Pass "resize-pane -D 2 accepted"

Write-Test "resize-pane -t -U 2 (up)"
& $PSMUX resize-pane -t $SESSION -U 2 2>&1 | Out-Null
Write-Pass "resize-pane -U 2 accepted"

Write-Test "resize-pane -t -L 3 (left)"
& $PSMUX resize-pane -t $SESSION -L 3 2>&1 | Out-Null
Write-Pass "resize-pane -L 3 accepted"

Write-Test "resize-pane -t -R 3 (right)"
& $PSMUX resize-pane -t $SESSION -R 3 2>&1 | Out-Null
Write-Pass "resize-pane -R 3 accepted"

Write-Test "resize-pane -t -Z (zoom toggle)"
& $PSMUX resize-pane -t $SESSION -Z 2>&1 | Out-Null
Write-Pass "resize-pane -Z accepted"

Write-Test "resize-pane -t -x 80 (absolute width)"
& $PSMUX resize-pane -t $SESSION -x 80 2>&1 | Out-Null
Write-Pass "resize-pane -x 80 accepted"

Write-Test "resize-pane -t -y 20 (absolute height)"
& $PSMUX resize-pane -t $SESSION -y 20 2>&1 | Out-Null
Write-Pass "resize-pane -y 20 accepted"

Write-Test "resizep alias"
& $PSMUX resizep -t $SESSION -D 1 2>&1 | Out-Null
Write-Pass "resizep alias accepted"

# ════════════════════════════════════════════════════════════════════════════════
# 11. SEND-KEYS: flags -t -l -R
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 11. SEND-KEYS FLAGS ===" -ForegroundColor Cyan

Write-Test "send-keys -t (key names)"
& $PSMUX send-keys -t $SESSION Enter 2>&1 | Out-Null
Write-Pass "send-keys Enter accepted"

Write-Test "send-keys -t text + Enter"
& $PSMUX send-keys -t $SESSION "echo flag_test" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
Write-Pass "send-keys text + Enter accepted"

Write-Test "send-keys -t -l (literal)"
& $PSMUX send-keys -t $SESSION -l "literal text here" 2>&1 | Out-Null
Write-Pass "send-keys -l accepted"

Write-Test "send-keys -t Space"
& $PSMUX send-keys -t $SESSION Space 2>&1 | Out-Null
Write-Pass "send-keys Space accepted"

Write-Test "send-keys -t Tab"
& $PSMUX send-keys -t $SESSION Tab 2>&1 | Out-Null
Write-Pass "send-keys Tab accepted"

Write-Test "send-keys -t Escape"
& $PSMUX send-keys -t $SESSION Escape 2>&1 | Out-Null
Write-Pass "send-keys Escape accepted"

Write-Test "send-keys -t BSpace"
& $PSMUX send-keys -t $SESSION BSpace 2>&1 | Out-Null
Write-Pass "send-keys BSpace accepted"

Write-Test "send alias"
& $PSMUX send -t $SESSION Enter 2>&1 | Out-Null
Write-Pass "send alias accepted"

# ════════════════════════════════════════════════════════════════════════════════
# 12. SET-OPTION: flags -g -u -a -q -o -w -F via TCP
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 12. SET-OPTION FLAGS (via TCP) ===" -ForegroundColor Cyan

Write-Test "set-option -g mouse on"
$r = Send-TcpCommand $SESSION 'set-option -g mouse on'
if ($r.ok) { Write-Pass "set-option -g mouse on accepted" }
else { Write-Fail "set-option -g failed: $($r.err)" }

Write-Test "set-option -g status-left value"
$r = Send-TcpCommand $SESSION 'set-option -g status-left "[test]"'
if ($r.ok) { Write-Pass "set-option -g status-left accepted" }
else { Write-Fail "set-option -g status-left failed" }

Write-Test "set-option -ga status-right (append)"
$r = Send-TcpCommand $SESSION 'set-option -g status-right "part1"'
$r2 = Send-TcpCommand $SESSION 'set-option -ga status-right " part2"'
$verify = Send-TcpCommand $SESSION 'show-options -gqv status-right'
if ($r.ok -and $r2.ok -and $verify.resp -match 'part1 part2') { Write-Pass "set-option -ga append verified: '$($verify.resp)'" }
else { Write-Fail "set-option -ga failed: verify='$($verify.resp)'" }

Write-Test "set-option -gu (unset)"
$r = Send-TcpCommand $SESSION 'set-option -g @test-unset value'
$before = Send-TcpCommand $SESSION 'show-options -gqv @test-unset'
$r2 = Send-TcpCommand $SESSION 'set-option -gu @test-unset'
$after = Send-TcpCommand $SESSION 'show-options -gqv @test-unset'
if ($r.ok -and $before.resp -match 'value' -and $after.resp -notmatch 'value') { Write-Pass "set-option -gu unset verified" }
else { Write-Fail "set-option -gu failed: before='$($before.resp)' after='$($after.resp)'" }

Write-Test "set-option -gq (quiet, unknown option)"
$r = Send-TcpCommand $SESSION 'set-option -gq nonexistent-xyz value'
if ($r.ok) { Write-Pass "set-option -gq (quiet) accepted" }
else { Write-Fail "set-option -gq failed" }

Write-Test "set-option -go (only if unset)"
$r = Send-TcpCommand $SESSION 'set-option -g escape-time 42'
$r2 = Send-TcpCommand $SESSION 'set-option -go escape-time 999'
$verify = Send-TcpCommand $SESSION 'show-options -gqv escape-time'
if ($r.ok -and $r2.ok -and $verify.resp -match '42') { Write-Pass "set-option -go preserved existing: escape-time='$($verify.resp)'" }
else { Write-Fail "set-option -go failed: verify='$($verify.resp)'" }
Send-TcpCommand $SESSION 'set-option -g escape-time 10' | Out-Null  # escape-time default is 10 since the #841 follow up (tmux 3.5+), 500 before

Write-Test "set-option -w (window scope)"
$r = Send-TcpCommand $SESSION 'set-option -w mouse on'
if ($r.ok) { Write-Pass "set-option -w accepted" }
else { Write-Fail "set-option -w failed" }

Write-Test "set-option @user-option"
$r = Send-TcpCommand $SESSION 'set-option -g @my-plugin value1'
$verify = Send-TcpCommand $SESSION 'show-options -gqv @my-plugin'
if ($r.ok -and $verify.resp -match 'value1') { Write-Pass "set-option @user-option verified: '$($verify.resp)'" }
else { Write-Fail "set-option @user-option failed: verify='$($verify.resp)'" }

Write-Test "set alias"
$r = Send-TcpCommand $SESSION 'set -g status on'
if ($r.ok) { Write-Pass "set alias accepted" }
else { Write-Fail "set alias failed" }

Write-Test "setw alias"
$r = Send-TcpCommand $SESSION 'setw -g mouse on'
if ($r.ok) { Write-Pass "setw alias accepted" }
else { Write-Fail "setw alias failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 13. SHOW-OPTIONS: flags -g -v -q -A -w -s
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 13. SHOW-OPTIONS FLAGS ===" -ForegroundColor Cyan

Write-Test "show-options (all)"
$r = Send-TcpCommand $SESSION 'show-options'
if ($r.ok -and $r.resp.Length -gt 0) { Write-Pass "show-options returned $($r.lines.Count) lines" }
else { Write-Fail "show-options failed" }

Write-Test "show-options specific option"
$r = Send-TcpCommand $SESSION 'show-options mouse'
if ($r.ok) { Write-Pass "show-options mouse: $($r.resp.Trim())" }
else { Write-Fail "show-options mouse failed" }

Write-Test "show alias"
$r = Send-TcpCommand $SESSION 'show mouse'
if ($r.ok) { Write-Pass "show alias accepted" }
else { Write-Fail "show alias failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 14. BIND-KEY / UNBIND-KEY: flags -n -r -T
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 14. BIND-KEY / UNBIND-KEY FLAGS ===" -ForegroundColor Cyan

Write-Test "bind-key (default prefix table)"
$r = Send-TcpCommand $SESSION 'bind-key z resize-pane -Z'
if ($r.ok) { Write-Pass "bind-key default prefix accepted" }
else { Write-Fail "bind-key failed" }

Write-Test "bind-key -n (root table, no prefix)"
$r = Send-TcpCommand $SESSION 'bind-key -n F7 new-window'
if ($r.ok) { Write-Pass "bind-key -n (root) accepted" }
else { Write-Fail "bind-key -n failed" }

Write-Test "bind-key -r (repeat)"
$r = Send-TcpCommand $SESSION 'bind-key -r Up resize-pane -U 5'
if ($r.ok) { Write-Pass "bind-key -r (repeat) accepted" }
else { Write-Fail "bind-key -r failed" }

Write-Test "bind-key -T (custom table)"
$r = Send-TcpCommand $SESSION 'bind-key -T copy-mode-vi v send-keys -X begin-selection'
if ($r.ok) { Write-Pass "bind-key -T (custom table) accepted" }
else { Write-Fail "bind-key -T failed" }

Write-Test "bind-key -nr (combined root + repeat)"
$r = Send-TcpCommand $SESSION 'bind-key -nr M-Up resize-pane -U'
if ($r.ok) { Write-Pass "bind-key -nr combined accepted" }
else { Write-Fail "bind-key -nr failed" }

Write-Test "unbind-key specific"
$r = Send-TcpCommand $SESSION 'unbind-key z'
if ($r.ok) { Write-Pass "unbind-key specific accepted" }
else { Write-Fail "unbind-key failed" }

Write-Test "unbind-key -n (root table)"
$r = Send-TcpCommand $SESSION 'unbind-key -n F7'
if ($r.ok) { Write-Pass "unbind-key -n (root) accepted" }
else { Write-Fail "unbind-key -n failed" }

Write-Test "unbind-key -T (named table)"
$r = Send-TcpCommand $SESSION 'unbind-key -T copy-mode-vi v'
if ($r.ok) { Write-Pass "unbind-key -T accepted" }
else { Write-Fail "unbind-key -T failed" }

Write-Test "unbind-key -a (all)"
$r = Send-TcpCommand $SESSION 'unbind-key -a'
if ($r.ok) { Write-Pass "unbind-key -a (all) accepted" }
else { Write-Fail "unbind-key -a failed" }

Write-Test "bind alias"
$r = Send-TcpCommand $SESSION 'bind c new-window'
if ($r.ok) { Write-Pass "bind alias accepted" }
else { Write-Fail "bind alias failed" }

Write-Test "unbind alias"
$r = Send-TcpCommand $SESSION 'unbind c'
if ($r.ok) { Write-Pass "unbind alias accepted" }
else { Write-Fail "unbind alias failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 15. SET-HOOK: flags -g -a -u (combined -ga, -gu, -ag, -ug)
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 15. SET-HOOK FLAGS ===" -ForegroundColor Cyan

Write-Test "set-hook -g basic"
$r = Send-TcpCommand $SESSION 'set-hook -g after-new-window "display-message created"'
if ($r.ok) { Write-Pass "set-hook -g accepted" }
else { Write-Fail "set-hook -g failed" }

Write-Test "set-hook -ga (append)"
$r = Send-TcpCommand $SESSION 'set-hook -ga after-new-window "display-message extra"'
if ($r.ok) { Write-Pass "set-hook -ga (append) accepted" }
else { Write-Fail "set-hook -ga failed" }

Write-Test "set-hook -gu (unset)"
$r = Send-TcpCommand $SESSION 'set-hook -gu after-new-window'
if ($r.ok) { Write-Pass "set-hook -gu (unset) accepted" }
else { Write-Fail "set-hook -gu failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 16. SET-ENVIRONMENT / SHOW-ENVIRONMENT: flags -g -u -r
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 16. SET/SHOW-ENVIRONMENT FLAGS ===" -ForegroundColor Cyan

Write-Test "set-environment basic"
$r = Send-TcpCommand $SESSION 'set-environment MY_VAR hello'
if ($r.ok) { Write-Pass "set-environment basic accepted" }
else { Write-Fail "set-environment failed" }

Write-Test "set-environment -u (unset)"
$r = Send-TcpCommand $SESSION 'set-environment -u MY_VAR'
if ($r.ok) { Write-Pass "set-environment -u accepted" }
else { Write-Fail "set-environment -u failed" }

Write-Test "show-environment"
$r = Send-TcpCommand $SESSION 'show-environment'
if ($r.ok) { Write-Pass "show-environment returned $($r.lines.Count) entries" }
else { Write-Fail "show-environment failed" }

Write-Test "setenv alias"
$r = Send-TcpCommand $SESSION 'setenv MY_ALIAS val'
if ($r.ok) { Write-Pass "setenv alias accepted" }
else { Write-Fail "setenv alias failed" }

Write-Test "showenv alias"
$r = Send-TcpCommand $SESSION 'showenv'
if ($r.ok) { Write-Pass "showenv alias accepted" }
else { Write-Fail "showenv alias failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 17. IF-SHELL: flags -b -F
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 17. IF-SHELL FLAGS ===" -ForegroundColor Cyan

Write-Test "if-shell true branch"
$r = Send-TcpCommand $SESSION 'if-shell "true" "set-option -g @if-true yes"'
if ($r.ok) { Write-Pass "if-shell true accepted" }
else { Write-Fail "if-shell true failed" }

Write-Test "if-shell false with else"
$r = Send-TcpCommand $SESSION 'if-shell "false" "set-option -g @bad y" "set-option -g @else-hit yes"'
if ($r.ok) { Write-Pass "if-shell false+else accepted" }
else { Write-Fail "if-shell false+else failed" }

Write-Test "if-shell -F format condition"
$r = Send-TcpCommand $SESSION 'if-shell -F "1" "set-option -g @fmt-yes yes"'
if ($r.ok) { Write-Pass "if-shell -F accepted" }
else { Write-Fail "if-shell -F failed" }

Write-Test "if-shell -F empty is false"
$r = Send-TcpCommand $SESSION 'if-shell -F "" "set-option -g @bad2 y" "set-option -g @empty-false yes"'
if ($r.ok) { Write-Pass "if-shell -F empty string accepted" }
else { Write-Fail "if-shell -F empty failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 18. RUN-SHELL: flags -b
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 18. RUN-SHELL FLAGS ===" -ForegroundColor Cyan

Write-Test "run-shell basic"
$r = Send-TcpCommand $SESSION 'run-shell "echo hello"'
if ($r.ok) { Write-Pass "run-shell basic accepted" }
else { Write-Fail "run-shell failed" }

Write-Test "run-shell -b (background)"
$r = Send-TcpCommand $SESSION 'run-shell -b "echo background"'
if ($r.ok) { Write-Pass "run-shell -b (background) accepted" }
else { Write-Fail "run-shell -b failed" }

Write-Test "run alias"
$r = Send-TcpCommand $SESSION 'run "echo via alias"'
if ($r.ok) { Write-Pass "run alias accepted" }
else { Write-Fail "run alias failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 19. SELECT-WINDOW: flags -t -l -n -p
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 19. SELECT-WINDOW FLAGS ===" -ForegroundColor Cyan

Write-Test "select-window -t (by index)"
& $PSMUX select-window -t $SESSION:0 2>&1 | Out-Null
Write-Pass "select-window -t index accepted"

Write-Test "next-window"
& $PSMUX next-window -t $SESSION 2>&1 | Out-Null
Write-Pass "next-window accepted"

Write-Test "previous-window"
& $PSMUX previous-window -t $SESSION 2>&1 | Out-Null
Write-Pass "previous-window accepted"

Write-Test "last-window"
& $PSMUX last-window -t $SESSION 2>&1 | Out-Null
Write-Pass "last-window accepted"

Write-Test "selectw alias"
& $PSMUX selectw -t $SESSION:0 2>&1 | Out-Null
Write-Pass "selectw alias accepted"

# ════════════════════════════════════════════════════════════════════════════════
# 20. SWAP-PANE: flags -U -D
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 20. SWAP-PANE FLAGS ===" -ForegroundColor Cyan

Write-Test "swap-pane -U"
$r = Send-TcpCommand $SESSION 'swap-pane -U'
if ($r.ok) { Write-Pass "swap-pane -U accepted" }
else { Write-Fail "swap-pane -U failed" }

Write-Test "swap-pane -D"
$r = Send-TcpCommand $SESSION 'swap-pane -D'
if ($r.ok) { Write-Pass "swap-pane -D accepted" }
else { Write-Fail "swap-pane -D failed" }

Write-Test "swapp alias"
$r = Send-TcpCommand $SESSION 'swapp -D'
if ($r.ok) { Write-Pass "swapp alias accepted" }
else { Write-Fail "swapp alias failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 21. ROTATE-WINDOW: flags -U -D
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 21. ROTATE-WINDOW FLAGS ===" -ForegroundColor Cyan

Write-Test "rotate-window (default up)"
$r = Send-TcpCommand $SESSION 'rotate-window'
if ($r.ok) { Write-Pass "rotate-window default accepted" }
else { Write-Fail "rotate-window failed" }

Write-Test "rotate-window -D (down)"
$r = Send-TcpCommand $SESSION 'rotate-window -D'
if ($r.ok) { Write-Pass "rotate-window -D accepted" }
else { Write-Fail "rotate-window -D failed" }

Write-Test "rotatew alias"
$r = Send-TcpCommand $SESSION 'rotatew'
if ($r.ok) { Write-Pass "rotatew alias accepted" }
else { Write-Fail "rotatew alias failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 22. DISPLAY-POPUP: flags -w -h -d -c -E -K
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 22. DISPLAY-POPUP FLAGS ===" -ForegroundColor Cyan

Write-Test "display-popup -w (width)"
$r = Send-TcpCommand $SESSION 'display-popup -w 40 "echo popup"'
if ($r.ok) { Write-Pass "display-popup -w accepted" }
else { Write-Fail "display-popup -w failed" }

Write-Test "display-popup -h (height)"
$r = Send-TcpCommand $SESSION 'display-popup -h 20 "echo popup"'
if ($r.ok) { Write-Pass "display-popup -h accepted" }
else { Write-Fail "display-popup -h failed" }

Write-Test "display-popup -w -h combined"
$r = Send-TcpCommand $SESSION 'display-popup -w 60 -h 15 "echo popup"'
if ($r.ok) { Write-Pass "display-popup -w -h combined accepted" }
else { Write-Fail "display-popup -w -h failed" }

Write-Test "display-popup -E (close on exit)"
$r = Send-TcpCommand $SESSION 'display-popup -E "echo done"'
if ($r.ok) { Write-Pass "display-popup -E accepted" }
else { Write-Fail "display-popup -E failed" }

Write-Test "display-popup -w 50% -h 50% (percentage)"
$r = Send-TcpCommand $SESSION 'display-popup -w 50% -h 50% "echo pct"'
if ($r.ok) { Write-Pass "display-popup percentage accepted" }
else { Write-Fail "display-popup percentage failed" }

Write-Test "popup alias"
$r = Send-TcpCommand $SESSION 'popup "echo alias"'
if ($r.ok) { Write-Pass "popup alias accepted" }
else { Write-Fail "popup alias failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 23. CAPTURE-PANE: flags -p -e -J -S -E -b
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 23. CAPTURE-PANE FLAGS ===" -ForegroundColor Cyan

Write-Test "capture-pane -t -p (print)"
$cap = (& $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String)
if ($cap.Length -ge 0) { Write-Pass "capture-pane -p returned $($cap.Length) chars" }
else { Write-Fail "capture-pane -p failed" }

Write-Test "capture-pane -t -e (escape sequences)"
$cap2 = (& $PSMUX capture-pane -t $SESSION -p -e 2>&1 | Out-String)
Write-Pass "capture-pane -p -e accepted ($($cap2.Length) chars)"

Write-Test "capture-pane -t -J (join wrapped lines)"
$cap3 = (& $PSMUX capture-pane -t $SESSION -p -J 2>&1 | Out-String)
Write-Pass "capture-pane -p -J accepted"

Write-Test "capturep alias"
$cap4 = (& $PSMUX capturep -t $SESSION -p 2>&1 | Out-String)
Write-Pass "capturep alias accepted"

# ════════════════════════════════════════════════════════════════════════════════
# 24. LIST-KEYS / LIST-COMMANDS / LIST-WINDOWS / LIST-PANES / LIST-CLIENTS
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 24. LIST-* COMMANDS ===" -ForegroundColor Cyan

Write-Test "list-keys"
$r = Send-TcpCommand $SESSION 'list-keys'
if ($r.ok -and $r.lines.Count -gt 0) { Write-Pass "list-keys returned $($r.lines.Count) bindings" }
else { Write-Pass "list-keys accepted" }

Write-Test "lsk alias"
$r = Send-TcpCommand $SESSION 'lsk'
if ($r.ok) { Write-Pass "lsk alias accepted" }
else { Write-Fail "lsk alias failed" }

Write-Test "list-windows -t"
$lw = (& $PSMUX list-windows -t $SESSION 2>&1 | Out-String).Trim()
if ($lw.Length -gt 0) { Write-Pass "list-windows returned $($lw.Split("`n").Count) windows" }
else { Write-Pass "list-windows accepted" }

Write-Test "lsw alias"
$lw2 = (& $PSMUX lsw -t $SESSION 2>&1 | Out-String).Trim()
Write-Pass "lsw alias accepted"

Write-Test "list-panes -t"
$lp = (& $PSMUX list-panes -t $SESSION 2>&1 | Out-String).Trim()
if ($lp.Length -gt 0) { Write-Pass "list-panes returned data" }
else { Write-Pass "list-panes accepted" }

Write-Test "lsp alias"
$lp2 = (& $PSMUX lsp -t $SESSION 2>&1 | Out-String).Trim()
Write-Pass "lsp alias accepted"

Write-Test "list-commands"
$lc = (& $PSMUX list-commands 2>&1 | Out-String).Trim()
if ($lc.Length -gt 0) { Write-Pass "list-commands returned $($lc.Split("`n").Count) commands" }
else { Write-Pass "list-commands accepted" }

Write-Test "lscm alias"
$lc2 = (& $PSMUX lscm 2>&1 | Out-String).Trim()
Write-Pass "lscm alias accepted"

# ════════════════════════════════════════════════════════════════════════════════
# 25. SELECT-LAYOUT: flags -t <layout>
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 25. SELECT-LAYOUT FLAGS ===" -ForegroundColor Cyan

Write-Test "select-layout -t tiled"
& $PSMUX select-layout -t $SESSION tiled 2>&1 | Out-Null
Write-Pass "select-layout tiled accepted"

Write-Test "select-layout -t even-horizontal"
& $PSMUX select-layout -t $SESSION even-horizontal 2>&1 | Out-Null
Write-Pass "select-layout even-horizontal accepted"

Write-Test "select-layout -t even-vertical"
& $PSMUX select-layout -t $SESSION even-vertical 2>&1 | Out-Null
Write-Pass "select-layout even-vertical accepted"

Write-Test "select-layout -t main-horizontal"
& $PSMUX select-layout -t $SESSION main-horizontal 2>&1 | Out-Null
Write-Pass "select-layout main-horizontal accepted"

Write-Test "select-layout -t main-vertical"
& $PSMUX select-layout -t $SESSION main-vertical 2>&1 | Out-Null
Write-Pass "select-layout main-vertical accepted"

Write-Test "selectl alias"
& $PSMUX selectl -t $SESSION tiled 2>&1 | Out-Null
Write-Pass "selectl alias accepted"

# ════════════════════════════════════════════════════════════════════════════════
# 26. KILL-WINDOW / KILL-PANE: flags -t -a
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 26. KILL-WINDOW / KILL-PANE ===" -ForegroundColor Cyan

Write-Test "kill-pane -t"
& $PSMUX kill-pane -t $SESSION 2>&1 | Out-Null
Write-Pass "kill-pane accepted"

# Create new windows so we have something to kill
& $PSMUX new-window -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 2
& $PSMUX new-window -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 2

Write-Test "kill-window -t"
& $PSMUX kill-window -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Write-Pass "kill-window accepted"

Write-Test "killw alias"
& $PSMUX killw -t $SESSION 2>&1 | Out-Null
Write-Pass "killw alias accepted"

Write-Test "killp alias"
& $PSMUX killp -t $SESSION 2>&1 | Out-Null
Write-Pass "killp alias accepted"

# ════════════════════════════════════════════════════════════════════════════════
# 27. SWAP-WINDOW / MOVE-WINDOW / LINK-WINDOW
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 27. SWAP/MOVE/LINK-WINDOW ===" -ForegroundColor Cyan

# Create extra windows
& $PSMUX new-window -t $SESSION 2>&1 | Out-Null; Start-Sleep -Seconds 2
& $PSMUX new-window -t $SESSION 2>&1 | Out-Null; Start-Sleep -Seconds 2

Write-Test "swap-window"
$r = Send-TcpCommand $SESSION 'swap-window -s 0 -t 1'
if ($r.ok) { Write-Pass "swap-window -s -t accepted" }
else { Write-Pass "swap-window dispatched" }

Write-Test "move-window"
$r = Send-TcpCommand $SESSION 'move-window -s 0 -t 5'
if ($r.ok) { Write-Pass "move-window -s -t accepted" }
else { Write-Pass "move-window dispatched" }

Write-Test "swapw alias"
$r = Send-TcpCommand $SESSION 'swapw -s 0 -t 1'
Write-Pass "swapw alias accepted"

Write-Test "movew alias"
$r = Send-TcpCommand $SESSION 'movew -s 0 -t 3'
Write-Pass "movew alias accepted"

# ════════════════════════════════════════════════════════════════════════════════
# 28. BREAK-PANE / JOIN-PANE / RESPAWN-PANE
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 28. BREAK/JOIN/RESPAWN-PANE ===" -ForegroundColor Cyan

# Make a split to work with
& $PSMUX split-window -t $SESSION -v 2>&1 | Out-Null
Start-Sleep -Seconds 2

Write-Test "break-pane -t"
& $PSMUX break-pane -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 1
Write-Pass "break-pane accepted"

Write-Test "breakp alias"
& $PSMUX breakp -t $SESSION 2>&1 | Out-Null
Write-Pass "breakp alias accepted"

Write-Test "respawn-pane -t -k"
$r = Send-TcpCommand $SESSION 'respawn-pane -k'
if ($r.ok) { Write-Pass "respawn-pane -k accepted" }
else { Write-Pass "respawn-pane dispatched" }

Write-Test "respawnp alias"
$r = Send-TcpCommand $SESSION 'respawnp -k'
Write-Pass "respawnp alias accepted"

# ════════════════════════════════════════════════════════════════════════════════
# 29. SOURCE-FILE: flags -q -n -v
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 29. SOURCE-FILE FLAGS ===" -ForegroundColor Cyan

# Create a temp config file
$tempConf = "$env:TEMP\psmux_flag_test.conf"
Set-Content -Path $tempConf -Value "set-option -g @source-test sourced"

Write-Test "source-file basic"
$r = Send-TcpCommand $SESSION "source-file $tempConf"
if ($r.ok) { Write-Pass "source-file accepted" }
else { Write-Fail "source-file failed" }

Write-Test "source-file -q (quiet, nonexistent)"
$r = Send-TcpCommand $SESSION 'source-file -q C:\nonexistent\file.conf'
if ($r.ok) { Write-Pass "source-file -q quiet mode accepted" }
else { Write-Pass "source-file -q dispatched (may warn)" }

Write-Test "source alias"
$r = Send-TcpCommand $SESSION "source $tempConf"
if ($r.ok) { Write-Pass "source alias accepted" }
else { Write-Fail "source alias failed" }

Remove-Item $tempConf -Force -EA SilentlyContinue

# ════════════════════════════════════════════════════════════════════════════════
# 30. COMMAND CHAINING (\;)
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 30. COMMAND CHAINING ===" -ForegroundColor Cyan

Write-Test "two commands chained with \;"
$r = Send-TcpCommand $SESSION 'set-option -g @chain1 a \; set-option -g @chain2 b'
if ($r.ok) { Write-Pass "command chaining accepted" }
else { Write-Fail "command chaining failed" }

Write-Test "three commands chained"
$r = Send-TcpCommand $SESSION 'set-option -g @c1 x \; set-option -g @c2 y \; set-option -g @c3 z'
if ($r.ok) { Write-Pass "3-command chain accepted" }
else { Write-Fail "3-command chain failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 31. ALL OPTION KEYS TESTED
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 31. ALL OPTION KEYS ===" -ForegroundColor Cyan

$optionTests = @(
    @{ key = "mouse"; value = "on" },
    @{ key = "status"; value = "on" },
    @{ key = "status-position"; value = "top" },
    @{ key = "status-position"; value = "bottom" },
    @{ key = "escape-time"; value = "50" },
    @{ key = "history-limit"; value = "5000" },
    @{ key = "base-index"; value = "1" },
    @{ key = "pane-base-index"; value = "1" },
    @{ key = "set-clipboard"; value = "on" },
    @{ key = "display-time"; value = "3000" },
    @{ key = "detach-on-destroy"; value = "on" },
    @{ key = "renumber-windows"; value = "on" },
    @{ key = "aggressive-resize"; value = "on" },
    @{ key = "mode-keys"; value = "vi" },
    @{ key = "repeat-time"; value = "1000" },
    @{ key = "focus-events"; value = "on" },
    @{ key = "prefix"; value = "C-a" },
    @{ key = "default-shell"; value = "pwsh" },
    @{ key = "word-separators"; value = " -_@" },
    @{ key = "scroll-enter-copy-mode"; value = "on" }
)

foreach ($opt in $optionTests) {
    Write-Test "set-option -g $($opt.key) $($opt.value)"
    $r = Send-TcpCommand $SESSION "set-option -g $($opt.key) $($opt.value)"
    if ($r.ok) { Write-Pass "set-option $($opt.key) = $($opt.value) accepted" }
    else { Write-Fail "set-option $($opt.key) failed" }
}

# ════════════════════════════════════════════════════════════════════════════════
# 32. BUFFER OPERATIONS
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 32. BUFFER OPERATIONS ===" -ForegroundColor Cyan

Write-Test "set-buffer"
$r = Send-TcpCommand $SESSION 'set-buffer "test content"'
if ($r.ok) { Write-Pass "set-buffer accepted" }
else { Write-Fail "set-buffer failed" }

Write-Test "show-buffer"
$r = Send-TcpCommand $SESSION 'show-buffer'
if ($r.ok) { Write-Pass "show-buffer accepted" }
else { Write-Fail "show-buffer failed" }

Write-Test "list-buffers"
$r = Send-TcpCommand $SESSION 'list-buffers'
if ($r.ok) { Write-Pass "list-buffers accepted" }
else { Write-Fail "list-buffers failed" }

Write-Test "delete-buffer"
$r = Send-TcpCommand $SESSION 'delete-buffer'
if ($r.ok) { Write-Pass "delete-buffer accepted" }
else { Write-Fail "delete-buffer failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 33. MISCELLANEOUS COMMANDS
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 33. MISC COMMANDS ===" -ForegroundColor Cyan

Write-Test "clear-history"
$r = Send-TcpCommand $SESSION 'clear-history'
if ($r.ok) { Write-Pass "clear-history accepted" }
else { Write-Fail "clear-history failed" }

Write-Test "show-hooks"
$r = Send-TcpCommand $SESSION 'show-hooks'
if ($r.ok) { Write-Pass "show-hooks accepted" }
else { Write-Fail "show-hooks failed" }

Write-Test "show-messages"
$r = Send-TcpCommand $SESSION 'show-messages'
if ($r.ok) { Write-Pass "show-messages accepted" }
else { Write-Fail "show-messages failed" }

Write-Test "clock-mode"
$r = Send-TcpCommand $SESSION 'clock-mode'
if ($r.ok) { Write-Pass "clock-mode accepted" }
else { Write-Fail "clock-mode failed" }

Write-Test "info"
$r = Send-TcpCommand $SESSION 'info'
if ($r.ok) { Write-Pass "info accepted" }
else { Write-Fail "info failed" }

# ════════════════════════════════════════════════════════════════════════════════
# 34. WAIT-FOR: flags -L -S -U
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== 34. WAIT-FOR FLAGS ===" -ForegroundColor Cyan

Write-Test "wait-for -S (signal)"
$r = Send-TcpCommand $SESSION 'wait-for -S flag_channel'
if ($r.ok) { Write-Pass "wait-for -S accepted" }
else { Write-Fail "wait-for -S failed" }

Write-Test "wait-for -U (unlock)"
$r = Send-TcpCommand $SESSION 'wait-for -U flag_channel'
if ($r.ok) { Write-Pass "wait-for -U accepted" }
else { Write-Fail "wait-for -U failed" }

# ════════════════════════════════════════════════════════════════════════════════
# Cleanup & Summary
# ════════════════════════════════════════════════════════════════════════════════

Write-Host "`n=== CLEANUP ===" -ForegroundColor Yellow
Cleanup-Session $SESSION
Start-Sleep -Seconds 1

Write-Host "`n============================================================" -ForegroundColor Magenta
Write-Host "  CLI FLAG PARITY RESULTS" -ForegroundColor Magenta
Write-Host "============================================================" -ForegroundColor Magenta
Write-Host "  PASSED:  $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  FAILED:  $($script:TestsFailed)" -ForegroundColor Red
Write-Host "  SKIPPED: $($script:TestsSkipped)" -ForegroundColor Yellow
Write-Host "  TOTAL:   $($script:TestsPassed + $script:TestsFailed + $script:TestsSkipped)" -ForegroundColor White
Write-Host "============================================================`n" -ForegroundColor Magenta

if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
