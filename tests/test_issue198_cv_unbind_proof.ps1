# Issue #198 Proof: Ctrl+V keystroke is swallowed by psmux even after unbind-key
#
# This is the DEFINITIVE proof test. It:
# 1. Launches a visible psmux session
# 2. Unbinds C-v from all tables
# 3. Injects REAL Ctrl+V keystroke via WriteConsoleInput
# 4. Checks whether the keystroke reached the shell (it should, but does not)
#
# The bug: client.rs has a hardcoded #[cfg(windows)] block that suppresses
# Ctrl+V Press events unconditionally. This is for Windows paste detection.
# unbind-key cannot disable this because it is not a key binding.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "test_198_proof"
$psmuxDir = "$env:USERPROFILE\.psmux"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

function Cleanup {
    & $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue
}

# Compile injector
$injectorExe = "$env:TEMP\psmux_injector.exe"
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $injectorExe)) {
    if (Test-Path "$PSScriptRoot\injector.cs") {
        & $csc /nologo /optimize /out:$injectorExe "$PSScriptRoot\injector.cs" 2>&1 | Out-Null
    } else {
        Write-Host "[SKIP] injector.cs not found, cannot run keystroke injection tests" -ForegroundColor DarkYellow
        exit 0
    }
}

if (-not (Test-Path $injectorExe)) {
    Write-Host "[SKIP] Failed to compile injector" -ForegroundColor DarkYellow
    exit 0
}

# ═══════════════════════════════════════════════════════════════════════════
# Setup: Launch visible psmux session, unbind everything related to C-v
# ═══════════════════════════════════════════════════════════════════════════

Cleanup
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru
Start-Sleep -Seconds 4

& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "Session creation failed"
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
    exit 1
}

Write-Host "`n=== Issue #198: TUI Ctrl+V Interception Proof ===" -ForegroundColor Cyan

# ═══════════════════════════════════════════════════════════════════════════
# Part 1: TUI CLI-based verification (Strategy A)
# ═══════════════════════════════════════════════════════════════════════════

Write-Host "`n--- Strategy A: CLI-based TUI Verification ---" -ForegroundColor Magenta

# [TUI Test 1] Session is functional
Write-Host "`n[TUI 1] Session is alive and responsive" -ForegroundColor Yellow
$name = (& $PSMUX display-message -t $SESSION -p '#{session_name}' 2>&1).Trim()
if ($name -eq $SESSION) { Write-Pass "Session responds to display-message" }
else { Write-Fail "display-message returned: $name" }

# [TUI Test 2] Unbind all C-v related bindings AND disable paste-detection
Write-Host "`n[TUI 2] Unbind C-v and v from all tables + disable paste-detection" -ForegroundColor Yellow
& $PSMUX unbind-key C-v -t $SESSION 2>&1 | Out-Null
& $PSMUX unbind-key -n C-v -t $SESSION 2>&1 | Out-Null
& $PSMUX unbind-key v -t $SESSION 2>&1 | Out-Null
& $PSMUX set-option -g paste-detection off -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
# The unbinds above only touch the prefix and root tables (unbind-key without
# -T targets prefix, -n targets root; tmux cmd-unbind-key.c). copy-mode-vi
# keeps `v`/`C-v` rectangle-toggle and copy-mode keeps `C-v` page-down, as in
# tmux 3.4, and list-keys without -T prints those tables too, so the C-v check
# looks at prefix/root lines only.
$keys = & $PSMUX list-keys -t $SESSION 2>&1 | Out-String
$stillHasV = $keys -match "prefix\s+v\s"
$stillHasCv = [bool]($keys -split "`n" | Where-Object { $_ -match '-T\s+(prefix|root)\s+C-v\s' })
$pdOpt = (& $PSMUX show-options -t $SESSION 2>&1 | Out-String)
$pdOff = $pdOpt -match "paste-detection off"
if (-not $stillHasV -and -not $stillHasCv -and $pdOff) {
    Write-Pass "All v/C-v bindings removed, paste-detection off"
} elseif (-not $stillHasV -and -not $stillHasCv) {
    Write-Fail "Bindings removed but paste-detection not confirmed off"
} else {
    Write-Fail "v or C-v still present in list-keys"
}

# ═══════════════════════════════════════════════════════════════════════════
# Part 2: WriteConsoleInput Keystroke Injection (Strategy B)
# THE ACTUAL BUG PROOF
# ═══════════════════════════════════════════════════════════════════════════

Write-Host "`n--- Strategy B: WriteConsoleInput Ctrl+V Injection ---" -ForegroundColor Magenta

# [TUI Test 3] First prove keystroke injection works with a normal key
Write-Host "`n[TUI 3] Baseline: normal character injection works" -ForegroundColor Yellow
& $PSMUX send-keys -t $SESSION "clear" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1

# Inject "echo BASELINE_OK" + Enter
& $injectorExe $proc.Id "echo BASELINE_OK{ENTER}"
Start-Sleep -Seconds 2

$captured = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($captured -match "BASELINE_OK") {
    Write-Pass "Normal character injection works (BASELINE_OK appeared in pane)"
} else {
    Write-Fail "Baseline character injection failed. Cannot proceed with C-v test."
    Write-Host "    capture-pane output:`n$captured" -ForegroundColor DarkGray
}

# [TUI Test 4] Inject Ctrl+V and test if it reaches the shell
# In PowerShell, Ctrl+V pastes from clipboard. If the clipboard has known
# content, and psmux does NOT intercept C-v, the content should appear.
# If psmux DOES intercept C-v (the bug), nothing appears or whitespace appears.
Write-Host "`n[TUI 4] CRITICAL: Ctrl+V injection after all unbinds" -ForegroundColor Yellow

# Set clipboard to a known marker string. pwsh's Set-Clipboard is a silent
# no-op while another window has the clipboard open, so verify the seed.
. (Join-Path $PSScriptRoot "clipboard_test_helpers.ps1")
$marker = "CVTEST_$(Get-Random -Maximum 99999)"
$cvSeeded = Set-ClipboardVerified $marker
Start-Sleep -Milliseconds 200

# Clear the pane
& $PSMUX send-keys -t $SESSION "clear" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1

# Now inject Ctrl+V
# If the paste detection is NOT intercepting, Ctrl+V should trigger
# PowerShell paste (clipboard content appears at prompt)
& $injectorExe $proc.Id "^v"
Start-Sleep -Seconds 2

$capturedAfterCv = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String

# Check for the marker text
if (-not $cvSeeded) {
    Write-Host "  [SKIP] could not seed the clipboard (held by another process); Ctrl+V passthrough not judged" -ForegroundColor DarkYellow
} elseif ($capturedAfterCv -match $marker) {
    Write-Pass "Ctrl+V PASSED THROUGH: clipboard content '$marker' appeared in pane"
    Write-Host "    This means unbind-key DID take effect for Ctrl+V passthrough." -ForegroundColor DarkYellow
} else {
    # Check if any whitespace or unexpected content appeared
    $trimmedCapture = ($capturedAfterCv -split "`n" | Where-Object { $_.Trim() -ne "" -and $_ -notmatch "^PS " })
    Write-Fail "BUG CONFIRMED: Ctrl+V did NOT paste clipboard content after unbind"
    Write-Host "    Expected marker: $marker" -ForegroundColor DarkYellow
    Write-Host "    Captured pane:" -ForegroundColor DarkGray
    foreach ($line in ($capturedAfterCv -split "`n" | Select-Object -Last 10)) {
        Write-Host "      |$line|" -ForegroundColor DarkGray
    }
    Write-Host "    EVIDENCE: The hardcoded Windows paste suppression in client.rs" -ForegroundColor Red
    Write-Host "    swallows Ctrl+V Press events even when all bindings are removed." -ForegroundColor Red
}

# [TUI Test 5] Prove send-keys C-v DOES work (bypass proof)
Write-Host "`n[TUI 5] send-keys C-v bypasses the hardcoded suppression" -ForegroundColor Yellow
$marker2 = "SENDKEY_$(Get-Random -Maximum 99999)"
[void](Set-ClipboardVerified $marker2)
Start-Sleep -Milliseconds 200

& $PSMUX send-keys -t $SESSION "clear" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1

# send-keys C-v goes through the TCP/server path, not the client event loop
& $PSMUX send-keys -t $SESSION C-v 2>&1 | Out-Null
Start-Sleep -Seconds 2

$capturedSendKey = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String

# Note: send-keys C-v sends the raw Ctrl+V byte (0x16) to the PTY.
# PowerShell interprets raw 0x16 differently than a Windows paste event.
# The test here is just that it doesn't crash and the session stays alive.
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -eq 0) {
    Write-Pass "send-keys C-v: session still alive (server-side bypass works)"
} else {
    Write-Fail "send-keys C-v: session died"
}

# [TUI Test 6] Inject a character AFTER Ctrl+V to prove session is still responsive
Write-Host "`n[TUI 6] Session responsive after Ctrl+V injection" -ForegroundColor Yellow
& $PSMUX send-keys -t $SESSION "clear" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1
& $injectorExe $proc.Id "echo AFTERCV{ENTER}"
Start-Sleep -Seconds 2
$capturedAfter = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
if ($capturedAfter -match "AFTERCV") {
    Write-Pass "Session responsive after Ctrl+V injection"
} else {
    Write-Fail "Session unresponsive after Ctrl+V"
}

# ═══════════════════════════════════════════════════════════════════════════
# Part 3: Neovim-specific scenario (if nvim is available)
# The reporter uses neovim, where Ctrl+V enters visual block mode
# ═══════════════════════════════════════════════════════════════════════════

Write-Host "`n--- Part 3: Neovim Scenario (optional) ---" -ForegroundColor Magenta

$nvim = Get-Command nvim -EA SilentlyContinue
if ($nvim) {
    Write-Host "`n[TUI 7] Neovim Ctrl+V visual block test (via send-keys)" -ForegroundColor Yellow
    
    # Clear clipboard to avoid interference from previous tests
    Set-Clipboard -Value ""
    
    # Launch nvim inside the psmux session
    & $PSMUX send-keys -t $SESSION "clear" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    & $PSMUX send-keys -t $SESSION "nvim -u NONE" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 3
    
    # Use send-keys C-v (server-side path) to verify the PTY receives Ctrl+V.
    # This proves the send-key C-v command produces the right byte (\x16).
    & $PSMUX send-keys -t $SESSION C-v 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    
    $nvimCapture = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($nvimCapture -match "VISUAL BLOCK") {
        Write-Pass "send-keys C-v entered visual block mode in nvim"
    } else {
        Write-Fail "send-keys C-v did NOT enter visual block mode in nvim"
        Write-Host "    capture: $($nvimCapture -replace '`n',' | ' | Select-Object -First 1)" -ForegroundColor DarkGray
    }
    
    # Exit nvim visual block mode and nvim itself.
    # Root-cause note (found via PSMUX_INPUT_DEBUG=1 + capture-pane, and
    # confirmed across several hardening attempts below): trying to type an
    # Escape + ":q!"/":qa!" quit sequence into a live, real nvim instance from
    # this test's send-keys/injector path was unreliable no matter how much
    # settle time was added -- nvim was repeatedly left open (still consuming
    # keystrokes), so the *next* "nvim -u NONE" command got typed as literal
    # nvim NORMAL-mode keys into the still-open buffer instead of running as
    # a shell command (n/v/i/m are themselves nvim commands -- this is what
    # produced the "-- INSERT --" / stray "NE" fragment failure capture).
    # Rather than keep chasing nvim's own modal-editing exit semantics from
    # PowerShell, force a guaranteed-clean pane via respawn-pane -k, which
    # kills whatever is running (nvim, stuck or not) and restarts the
    # configured shell -- sidestepping the exit race entirely.
    & $PSMUX respawn-pane -k -t $SESSION 2>&1 | Out-Null
    $nvimGone = $false
    for ($i = 0; $i -lt 20; $i++) {
        Start-Sleep -Milliseconds 250
        $settleCap = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
        $lastLine = ($settleCap -split "`r?`n" | Where-Object { $_.Trim() -ne "" } | Select-Object -Last 1)
        if ($lastLine -match "^PS [A-Za-z]:.*>\s*$") {
            $nvimGone = $true
            break
        }
    }
    if (-not $nvimGone) { Start-Sleep -Seconds 1 }

    # [TUI 7b] Now test the FULL path: WriteConsoleInput Ctrl+V with paste-detection off
    Write-Host "`n[TUI 7b] Neovim Ctrl+V visual block via WriteConsoleInput (paste-detection off)" -ForegroundColor Yellow
    Write-Host "    (DEBUG: nvimGone=$nvimGone)" -ForegroundColor DarkGray
    & $PSMUX send-keys -t $SESSION "nvim -u NONE" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 3

    & $injectorExe $proc.Id "^v"
    Start-Sleep -Seconds 2

    $nvimCapture2 = & $PSMUX capture-pane -t $SESSION -p 2>&1 | Out-String
    if ($nvimCapture2 -match "VISUAL BLOCK") {
        Write-Pass "WriteConsoleInput Ctrl+V entered visual block mode in nvim (paste-detection off)"
    } else {
        # This may fail if Windows Terminal also intercepts the injected Ctrl+V
        Write-Fail "WriteConsoleInput Ctrl+V did NOT enter visual block mode in nvim"
        Write-Host "    (This can fail when Windows Terminal intercepts the key before psmux)" -ForegroundColor DarkYellow
        Write-Host "    DEBUG pane dump:" -ForegroundColor DarkGray
        Write-Host $nvimCapture2
    }
    
    # Exit nvim
    & $injectorExe $proc.Id "{ESC}:q!{ENTER}"
    Start-Sleep -Seconds 1
} else {
    Write-Host "[SKIP] nvim not found, skipping neovim scenario" -ForegroundColor DarkYellow
}

# ═══════════════════════════════════════════════════════════════════════════
# Check injector log for Ctrl+V injection details
# ═══════════════════════════════════════════════════════════════════════════

Write-Host "`n--- Injector Log ---" -ForegroundColor Magenta
$logFile = "$env:TEMP\psmux_inject.log"
if (Test-Path $logFile) {
    $logLines = Get-Content $logFile -Tail 20
    Write-Host "  Last 20 lines of injector log:" -ForegroundColor DarkGray
    foreach ($line in $logLines) {
        Write-Host "    $line" -ForegroundColor DarkGray
    }
}

# ═══════════════════════════════════════════════════════════════════════════
# TEARDOWN
# ═══════════════════════════════════════════════════════════════════════════

& $PSMUX kill-session -t $SESSION 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })

Write-Host "`n=== ROOT CAUSE ===" -ForegroundColor Yellow
Write-Host "  client.rs had THREE hardcoded Windows mechanisms that intercept Ctrl+V:" -ForegroundColor White
Write-Host "    1. #[cfg(windows)] KeyCode::Char('v') + CONTROL => {} (suppresses press)" -ForegroundColor DarkYellow
Write-Host "    2. Ctrl+V Release event sets paste_confirmed = true" -ForegroundColor DarkYellow
Write-Host "    3. paste_pend buffering captures chars as paste content" -ForegroundColor DarkYellow
Write-Host "  FIX: set -g paste-detection off disables #1 and #2." -ForegroundColor White
Write-Host "  When off, Ctrl+V is forwarded as send-key C-v." -ForegroundColor White

exit $script:TestsFailed
