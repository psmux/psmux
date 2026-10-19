#!/usr/bin/env pwsh
# test_control_mode.ps1
# Integration tests for tmux-compatible control mode (-C / -CC)
# Tests: basic connection, command dispatch, %begin/%end framing,
#        list-windows, list-panes, new-window, send-keys, capture-pane,
#        notification emission, echo mode vs no-echo mode

$ErrorActionPreference = "Continue"
$exe = "$PSScriptRoot\..\target\release\psmux.exe"
if (-not (Test-Path $exe)) { $exe = "$PSScriptRoot\..\target\debug\psmux.exe" }
if (-not (Test-Path $exe)) { $exe = (Get-Command psmux -ErrorAction SilentlyContinue).Source }
if (-not $exe -or -not (Test-Path $exe)) { Write-Error "psmux binary not found"; exit 1 }

$SESSION = "test-ctrl-mode"
$results = @()

function Add-Result($name, $pass, $detail="") {
    $script:results += [PSCustomObject]@{
        Test=$name
        Result=if($pass){"PASS"}else{"FAIL"}
        Detail=$detail
    }
    $mark = if($pass) { "[PASS]" } else { "[FAIL]" }
    $color = if($pass) { "Green" } else { "Red" }
    Write-Host "  $mark $name$(if($detail){' '+$detail}else{''})" -ForegroundColor $color
}

# Helper: run a control mode session, feed commands, return output lines
function Invoke-ControlMode {
    param(
        [string]$Mode = "-CC",  # -C or -CC
        [string[]]$Commands,
        [int]$TimeoutMs = 5000
    )
    # Build a script that pipes commands into control mode
    $cmdInput = ($Commands -join "`n") + "`n"
    $tempIn = [System.IO.Path]::GetTempFileName()
    [System.IO.File]::WriteAllText($tempIn, $cmdInput)

    $env:PSMUX_SESSION_NAME = $SESSION
    try {
        $proc = Start-Process -FilePath $exe -ArgumentList $Mode -RedirectStandardInput $tempIn `
            -RedirectStandardOutput "$env:TEMP\psmux_ctrl_out.txt" `
            -RedirectStandardError "$env:TEMP\psmux_ctrl_err.txt" `
            -PassThru -NoNewWindow

        # Wait for commands to be processed
        $finished = $proc.WaitForExit($TimeoutMs)
        if (-not $finished) {
            $proc.Kill()
            Start-Sleep -Milliseconds 500
        }

        $output = Get-Content "$env:TEMP\psmux_ctrl_out.txt" -ErrorAction SilentlyContinue
        return $output
    } finally {
        Remove-Item $tempIn -Force -ErrorAction SilentlyContinue
        Remove-Item "$env:TEMP\psmux_ctrl_out.txt" -Force -ErrorAction SilentlyContinue
        Remove-Item "$env:TEMP\psmux_ctrl_err.txt" -Force -ErrorAction SilentlyContinue
        Remove-Item env:\PSMUX_SESSION_NAME -ErrorAction SilentlyContinue
    }
}

Write-Host "`n================================================" -ForegroundColor Cyan
Write-Host "Control Mode (-C / -CC) Integration Test Suite" -ForegroundColor Cyan
Write-Host "================================================`n" -ForegroundColor Cyan

# ---- Cleanup ----
& $exe kill-session -t $SESSION 2>$null
& $exe kill-server 2>$null
Start-Sleep -Seconds 1

# ---- Setup: create a detached session ----
Write-Host "Setting up test session..." -ForegroundColor Yellow
& $exe new-session -d -s $SESSION -x 120 -y 30 2>$null
Start-Sleep -Seconds 3

# ============================================================
# TEST 1: Basic -CC connection and list-windows
# ============================================================
Write-Host "`n--- Test 1: Basic -CC connection and list-windows ---" -ForegroundColor Cyan

$output = Invoke-ControlMode -Mode "-CC" -Commands @("list-windows", "exit")
$joined = ($output | Out-String)

# Should see %begin and %end framing
$hasBegin = $joined -match "%begin"
$hasEnd = $joined -match "%end"
Add-Result "list-windows has %begin framing" $hasBegin "output: $($joined.Substring(0, [Math]::Min(200, $joined.Length)))"
Add-Result "list-windows has %end framing" $hasEnd

# Should contain window information (at least one window exists)
$hasWindowInfo = $joined -match "0:" -or $joined -match "active"
Add-Result "list-windows returns window data" $hasWindowInfo

# ============================================================
# TEST 2: list-panes output
# ============================================================
Write-Host "`n--- Test 2: list-panes ---" -ForegroundColor Cyan

$output = Invoke-ControlMode -Mode "-CC" -Commands @("list-panes")
$joined = ($output | Out-String)
$hasBegin = $joined -match "%begin"
$hasPaneInfo = $joined -match "%\d+" -or $joined -match "active"
Add-Result "list-panes has %begin framing" $hasBegin
Add-Result "list-panes returns pane data" $hasPaneInfo

# ============================================================
# TEST 3: display-message with format string
# ============================================================
Write-Host "`n--- Test 3: display-message ---" -ForegroundColor Cyan

$output = Invoke-ControlMode -Mode "-CC" -Commands @("display-message -p '#{session_name}'")
$joined = ($output | Out-String)
$hasSessionName = $joined -match $SESSION
Add-Result "display-message shows session name" $hasSessionName "output: $($joined.Substring(0, [Math]::Min(200, $joined.Length)))"

# ============================================================
# TEST 4: new-window and notification
# ============================================================
Write-Host "`n--- Test 4: new-window ---" -ForegroundColor Cyan

$output = Invoke-ControlMode -Mode "-CC" -Commands @("new-window", "list-windows") -TimeoutMs 5000
$joined = ($output | Out-String)

# Should have commands framed
$hasBegin = $joined -match "%begin"
Add-Result "new-window commands are framed" $hasBegin

# list-windows should show at least 2 windows now
$windowLines = $output | Where-Object { $_ -match "^\d+:" }
$multiWindows = $windowLines.Count -ge 2 -or ($joined -match "1:")
Add-Result "new-window creates second window" $multiWindows "windows seen: $($windowLines.Count)"

# ============================================================
# TEST 5: send-keys and capture-pane
# ============================================================
Write-Host "`n--- Test 5: send-keys and capture-pane ---" -ForegroundColor Cyan

$marker = "CTRL_TEST_MARKER_$(Get-Random)"
# First: send keys
$null = Invoke-ControlMode -Mode "-CC" -Commands @(
    "send-keys -t $SESSION 'echo $marker' Enter"
)
# Wait for shell to execute the echo command
Start-Sleep -Seconds 2
# Second: capture pane
$output = Invoke-ControlMode -Mode "-CC" -Commands @(
    "capture-pane -t $SESSION -p"
)
$joined = ($output | Out-String)
$hasMarker = $joined -match $marker
Add-Result "send-keys + capture-pane shows marker" $hasMarker

# ============================================================
# TEST 5b: repeated -CC connect/disconnect against an ALREADY-LIVE
# session must never re-provision (warm-claim/cold-spawn) a replacement
# server. Regression test for the "second no-op -CC connection collapses
# scrollback" bug: main.rs's bare-invocation session bootstrap used to
# unconditionally warm-claim/cold-spawn for PSMUX_SESSION_NAME, clobbering
# the live session's port/key/sid files with a brand-new, empty-scrollback
# server on every single -CC connect (not just the second). That silently
# orphaned the running server (and its scrollback) while a fresh one took
# over transparently, ~40% of the time depending on timing. Proven via
# server_pid/pane_pid tracking across a 10x connect/disconnect loop:
# before the fix (main.rs missing the `server_already_alive` guard on the
# bare-invocation session bootstrap), this failed on roughly 4/10 iterations;
# after the fix it is 10/10 clean.
# ============================================================
Write-Host "`n--- Test 5b: repeated no-op -CC connects preserve scrollback (loop) ---" -ForegroundColor Cyan

$loopMarker = "CTRL_LOOP_MARKER_$(Get-Random)"
$null = Invoke-ControlMode -Mode "-CC" -Commands @(
    "send-keys -t $SESSION 'echo $loopMarker' Enter"
)
Start-Sleep -Milliseconds 800

$initialPanePid = & $exe display-message -t $SESSION -p '#{pane_pid}' 2>$null
$loopFails = 0
$loopIterations = 10
for ($li = 1; $li -le $loopIterations; $li++) {
    # Completely no-op -CC connect/disconnect cycle to the SAME session.
    $null = Invoke-ControlMode -Mode "-CC" -Commands @(
        "display-message -p '#{session_name}'"
    )
    Start-Sleep -Milliseconds 250

    $curPanePid = & $exe display-message -t $SESSION -p '#{pane_pid}' 2>$null
    $capture = & $exe capture-pane -t $SESSION -p 2>$null
    $joined = ($capture | Out-String)

    $ok = ($joined -match [regex]::Escape($loopMarker)) -and ($curPanePid -eq $initialPanePid)
    if (-not $ok) {
        $loopFails++
        Write-Host "  [loop $li] FAIL marker_found=$($joined -match [regex]::Escape($loopMarker)) pane_pid_before=$initialPanePid pane_pid_after=$curPanePid" -ForegroundColor Red
    }
}
Add-Result "no-op -CC connects never re-provision the live session ($loopIterations iterations)" ($loopFails -eq 0) "fails=$loopFails/$loopIterations"

# ============================================================
# TEST 6: -C mode (echo enabled)
# ============================================================
Write-Host "`n--- Test 6: -C echo mode ---" -ForegroundColor Cyan

$output = Invoke-ControlMode -Mode "-C" -Commands @("list-sessions")
$joined = ($output | Out-String)

# stdin here is a file, not a terminal: tmux never echoes a command line, so
# nothing but protocol lines comes back (issue #857). This used to assert the
# echo, which was psmux writing the line back where tmux does not; a console
# stdin still gets it, standing in for the console echo the client turns off.
$hasEcho = @($output | Where-Object { $_ -eq "list-sessions" }).Count -gt 0
$hasBegin = $joined -match "%begin"
Add-Result "-C mode with stdin a file does not echo commands" (-not $hasEcho)
Add-Result "-C mode has %begin framing" $hasBegin
Add-Result "-C mode opens with the flag 0 reply to its command line" ([string]$output[0] -match '^%begin \d+ \d+ 0$') "first='$($output[0])'"

# ============================================================
# TEST 7: -CC mode (no echo)
# ============================================================
Write-Host "`n--- Test 7: -CC no-echo mode ---" -ForegroundColor Cyan

$output = Invoke-ControlMode -Mode "-CC" -Commands @("list-sessions")
$joined = ($output | Out-String)

$hasBegin = $joined -match "%begin"
# In -CC mode, the actual command "list-sessions" should NOT be echoed as a raw line
# But it might appear in the output data. Check that %begin appears before any data.
$lines = $output | Where-Object { $_ -ne "" }
$firstNonEmpty = $lines | Select-Object -First 2
$startsWithProtocol = ($firstNonEmpty | Out-String) -match "^(%|$)" -or ($firstNonEmpty.Count -eq 0)
Add-Result "-CC mode has protocol output" $hasBegin

# ============================================================
# TEST 8: kill-window cleanup
# ============================================================
Write-Host "`n--- Test 8: kill-window ---" -ForegroundColor Cyan

# First count current windows
$beforeOutput = Invoke-ControlMode -Mode "-CC" -Commands @("list-windows")
$beforeLines = $beforeOutput | Where-Object { $_ -match "^\d+:" }
$beforeCount = $beforeLines.Count

# Kill the extra window we created in test 4
$output = Invoke-ControlMode -Mode "-CC" -Commands @("kill-window -t $SESSION`:1", "list-windows")
$joined = ($output | Out-String)
$afterLines = $output | Where-Object { $_ -match "^\d+:" }
$afterCount = $afterLines.Count

$windowKilled = $afterCount -lt $beforeCount -or $afterCount -le 1
Add-Result "kill-window reduces window count" $windowKilled "before=$beforeCount after=$afterCount"

# ============================================================
# TEST 9: Error handling (bad command)
# ============================================================
Write-Host "`n--- Test 9: Error handling ---" -ForegroundColor Cyan

$output = Invoke-ControlMode -Mode "-CC" -Commands @("nonexistent-command-12345")
$joined = ($output | Out-String)
$hasError = $joined -match "%error" -or $joined -match "%end"
Add-Result "Bad command returns %error or %end" $hasError

# ============================================================
# TEST 10: Multiple commands in sequence
# ============================================================
Write-Host "`n--- Test 10: Multiple commands ---" -ForegroundColor Cyan

$output = Invoke-ControlMode -Mode "-CC" -Commands @(
    "list-windows",
    "list-panes",
    "display-message -p '#{window_index}'"
)
$joined = ($output | Out-String)

# Should have multiple %begin/%end pairs
$beginCount = ([regex]::Matches($joined, "%begin")).Count
$endCount = ([regex]::Matches($joined, "%end")).Count + ([regex]::Matches($joined, "%error")).Count

Add-Result "Multiple commands have multiple %begin" ($beginCount -ge 3) "beginCount=$beginCount"
Add-Result "Multiple commands have matching %end" ($endCount -ge 3) "endCount=$endCount"

# ============================================================
# TEST 11: has-session responds correctly
# ============================================================
Write-Host "`n--- Test 11: has-session ---" -ForegroundColor Cyan

$output = Invoke-ControlMode -Mode "-CC" -Commands @("has-session -t $SESSION")
$joined = ($output | Out-String)
$hasEnd = $joined -match "%end"
$noError = -not ($joined -match "%error")
Add-Result "has-session succeeds for existing session" ($hasEnd -and $noError)

# ============================================================
# TEST 12: rename-session
# ============================================================
Write-Host "`n--- Test 12: rename-session ---" -ForegroundColor Cyan

$newName = "ctrl-renamed"
$output = Invoke-ControlMode -Mode "-CC" -Commands @(
    "rename-session $newName",
    "display-message -p '#{session_name}'"
)
$joined = ($output | Out-String)
$hasNewName = $joined -match $newName
Add-Result "rename-session changes name" $hasNewName

# Rename back for cleanup
$env:PSMUX_SESSION_NAME = $newName
$null = Invoke-ControlMode -Mode "-CC" -Commands @("rename-session $SESSION")
Remove-Item env:\PSMUX_SESSION_NAME -ErrorAction SilentlyContinue
$env:PSMUX_SESSION_NAME = $SESSION

# ---- Cleanup ----
Write-Host "`n--- Cleanup ---" -ForegroundColor Yellow
& $exe kill-session -t $SESSION 2>$null
& $exe kill-session -t "ctrl-renamed" 2>$null
Start-Sleep -Milliseconds 500
& $exe kill-server 2>$null
Start-Sleep -Milliseconds 500

# ---- Report ----
Write-Host "`n================================================" -ForegroundColor Cyan
$pass = ($results | Where-Object { $_.Result -eq "PASS" }).Count
$fail = ($results | Where-Object { $_.Result -eq "FAIL" }).Count
$total = $results.Count
Write-Host "Control Mode Tests: Total=$total  Pass=$pass  Fail=$fail" -ForegroundColor $(if($fail -gt 0){"Red"}else{"Green"})
Write-Host "================================================`n" -ForegroundColor Cyan

if ($fail -gt 0) {
    Write-Host "Failed tests:" -ForegroundColor Red
    $results | Where-Object { $_.Result -eq "FAIL" } | ForEach-Object {
        Write-Host "  - $($_.Test) $($_.Detail)" -ForegroundColor Red
    }
    exit 1
} else {
    exit 0
}
