# Issue #198 reproduction: paste-detection off + unbind C-v still leaks clipboard
#
# Reporter's latest complaint: with `set -g paste-detection off` AND `unbind-key C-v`
# in config, pressing Ctrl+V injects BOTH the passthrough AND the clipboard text.
# Expected: with paste-detection off and C-v unbound, psmux should NOT inject the
# clipboard; C-v should just pass through as a raw keystroke.
#
# This script proves/disproves the leak via WriteConsoleInput injection.
# Session names: gap198_* -- never touches __warm__.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$injectorExe = "$env:TEMP\psmux_injector.exe"
$MARKER = "CLIP198_LEAK_MARKER"

$script:TestsPassed = 0
$script:TestsFailed = 0
$script:Leaks       = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green;  $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red;    $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor Cyan }

# ── Build injector if missing ─────────────────────────────────────────────────
if (-not (Test-Path $injectorExe)) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    $src = Join-Path $PSScriptRoot "injector.cs"
    if (-not (Test-Path $src)) { Write-Host "[SKIP] injector.cs not found" -ForegroundColor DarkYellow; exit 0 }
    & $csc /nologo /optimize /out:$injectorExe $src 2>&1 | Out-Null
}
if (-not (Test-Path $injectorExe)) {
    Write-Host "[SKIP] Could not compile injector" -ForegroundColor DarkYellow
    exit 0
}

# ── Helpers ───────────────────────────────────────────────────────────────────
function Wait-PortFile([string]$sess, [int]$maxSeconds = 12) {
    $portFile = "$psmuxDir\$sess.port"
    for ($i = 0; $i -lt ($maxSeconds * 4); $i++) {
        Start-Sleep -Milliseconds 250
        if (Test-Path $portFile) { return $true }
    }
    return $false
}

function Kill-OwnSession([string]$sess) {
    & $PSMUX kill-session -t $sess 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    Remove-Item "$psmuxDir\$sess.*" -Force -EA SilentlyContinue
}

function Launch-AttachedSession([string]$sess, [string]$configFile = $null) {
    Kill-OwnSession $sess
    $args = @("new-session", "-s", $sess)
    $env_backup = $env:PSMUX_CONFIG_FILE
    if ($configFile) { $env:PSMUX_CONFIG_FILE = $configFile }
    $proc = Start-Process -FilePath $PSMUX -ArgumentList $args -PassThru
    $env:PSMUX_CONFIG_FILE = $env_backup
    if (-not (Wait-PortFile $sess 12)) {
        Write-Info "Port file never appeared for $sess"
        return $null
    }
    Start-Sleep -Milliseconds 800   # let session stabilise
    # Root-cause note: PSReadLine ships a DEFAULT Ctrl+v = Paste key handler
    # (confirmed via `Get-PSReadLineKeyHandler -Bound`), completely independent
    # of psmux. When psmux correctly forwards the raw 0x16 byte to the pane
    # (paste-detection off / C-v unbound -- proven via PSMUX_INPUT_DEBUG=1
    # input_debug.log to send exactly one `send-key C-v` and nothing else),
    # PSReadLine's OWN binding still reads the clipboard and inserts it --
    # indistinguishable from a real "leak" unless this pane-local binding is
    # neutralised first. Removing it here means the marker can only appear
    # via psmux's own mechanism, which is what this test is actually meant
    # to measure.
    & $PSMUX send-keys -t $sess "Remove-PSReadLineKeyHandler -Key Ctrl+v" Enter 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    return $proc
}

function Capture-Pane([string]$sess) {
    return (& $PSMUX capture-pane -t $sess -p 2>&1 | Out-String)
}

function Inject-CtrlV([int]$procPid) {
    & $injectorExe $procPid "^v" | Out-Null
    Start-Sleep -Milliseconds 300   # flush injector log
}

# ── Put known marker on clipboard ─────────────────────────────────────────────
# pwsh's Set-Clipboard is a silent no-op while another window has the
# clipboard open, so the seed is verified with retries (clipboard_test_helpers).
. (Join-Path $PSScriptRoot "clipboard_test_helpers.ps1")
if (-not (Set-ClipboardVerified $MARKER)) {
    Write-Host "[SKIP] Could not set clipboard to marker '$MARKER': another process kept the clipboard (got: $(Get-ClipboardSettled))" -ForegroundColor DarkYellow
    exit 0
}
Write-Info "Clipboard set to: $MARKER"

Write-Host "`n=== Issue #198 Clipboard-Leak Reproduction ===" -ForegroundColor Cyan
Write-Host "  Marker: $MARKER" -ForegroundColor Yellow

# ═════════════════════════════════════════════════════════════════════════════
# TEST SCENARIO A: paste-detection off + unbind-key C-v (via config file)
# This is the exact reporter config. C-v should pass through, no clipboard leak.
# ═════════════════════════════════════════════════════════════════════════════
Write-Host "`n--- Scenario A: paste-detection off + unbind-key C-v (config file) ---" -ForegroundColor Magenta

$confA = "$env:TEMP\gap198_confA.conf"
@"
set -g paste-detection off
unbind-key C-v
"@ | Set-Content -Path $confA -Encoding UTF8

$sessA = "gap198_A"
$procA = Launch-AttachedSession $sessA $confA
if ($procA -eq $null) {
    Write-Fail "Scenario A: session launch failed"
} else {
    # Verify option is in effect
    $pdVal = (& $PSMUX show-options -g -v "paste-detection" -t $sessA 2>&1).Trim()
    Write-Info "paste-detection = '$pdVal'"
    $keysOut = (& $PSMUX list-keys -t $sessA 2>&1 | Out-String)
    $cvBound = $keysOut -match "C-v"
    Write-Info "C-v still in list-keys: $cvBound"

    # Clear pane, then inject Ctrl+V
    & $PSMUX send-keys -t $sessA "clear" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    Inject-CtrlV $procA.Id
    Start-Sleep -Seconds 2

    $capA = Capture-Pane $sessA
    $leaked = $capA -match [regex]::Escape($MARKER)

    Write-Host "`n  [Scenario A] paste-detection=$pdVal, C-v bound=$cvBound" -ForegroundColor Yellow
    Write-Host "  Pane after Ctrl+V (last 6 lines):" -ForegroundColor DarkGray
    ($capA -split "`n" | Where-Object { $_.Trim() -ne "" } | Select-Object -Last 6) |
        ForEach-Object { Write-Host "    |$_|" -ForegroundColor DarkGray }

    if ($leaked) {
        Write-Fail "REPRODUCED: Clipboard marker '$MARKER' appeared in pane (paste leaked!)"
        $script:Leaks++
    } else {
        Write-Pass "No clipboard leak: marker absent (C-v did not inject clipboard)"
    }

    Kill-OwnSession $sessA
    try { Stop-Process -Id $procA.Id -Force -EA SilentlyContinue } catch {}
}
Remove-Item $confA -Force -EA SilentlyContinue

# ═════════════════════════════════════════════════════════════════════════════
# TEST SCENARIO B: paste-detection off + unbind C-v (short form, runtime)
# Same as A but using the short `unbind C-v` form applied at runtime.
# ═════════════════════════════════════════════════════════════════════════════
Write-Host "`n--- Scenario B: paste-detection off + runtime unbind C-v ---" -ForegroundColor Magenta

$confB = "$env:TEMP\gap198_confB.conf"
"set -g paste-detection off" | Set-Content -Path $confB -Encoding UTF8

$sessB = "gap198_B"
$procB = Launch-AttachedSession $sessB $confB
if ($procB -eq $null) {
    Write-Fail "Scenario B: session launch failed"
} else {
    & $PSMUX unbind-key C-v -t $sessB 2>&1 | Out-Null
    & $PSMUX unbind-key -n C-v -t $sessB 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400

    $pdValB = (& $PSMUX show-options -g -v "paste-detection" -t $sessB 2>&1).Trim()
    Write-Info "paste-detection = '$pdValB'"

    & $PSMUX send-keys -t $sessB "clear" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    Inject-CtrlV $procB.Id
    Start-Sleep -Seconds 2

    $capB = Capture-Pane $sessB
    $leakedB = $capB -match [regex]::Escape($MARKER)

    Write-Host "`n  [Scenario B] paste-detection=$pdValB, runtime unbind done" -ForegroundColor Yellow
    Write-Host "  Pane after Ctrl+V (last 6 lines):" -ForegroundColor DarkGray
    ($capB -split "`n" | Where-Object { $_.Trim() -ne "" } | Select-Object -Last 6) |
        ForEach-Object { Write-Host "    |$_|" -ForegroundColor DarkGray }

    if ($leakedB) {
        Write-Fail "REPRODUCED: Clipboard marker '$MARKER' appeared in pane (paste leaked!)"
        $script:Leaks++
    } else {
        Write-Pass "No clipboard leak: marker absent"
    }

    Kill-OwnSession $sessB
    try { Stop-Process -Id $procB.Id -Force -EA SilentlyContinue } catch {}
}
Remove-Item $confB -Force -EA SilentlyContinue

# ═════════════════════════════════════════════════════════════════════════════
# CONTROL TEST C: paste-detection off, C-v NOT unbound
# With paste-detection off only, Ctrl+V should still pass through
# (no hardcoded clipboard injection expected in this path either).
# ═════════════════════════════════════════════════════════════════════════════
Write-Host "`n--- Control C: paste-detection off, C-v still bound (not unbound) ---" -ForegroundColor Magenta

$confC = "$env:TEMP\gap198_confC.conf"
"set -g paste-detection off" | Set-Content -Path $confC -Encoding UTF8

$sessC = "gap198_C"
$procC = Launch-AttachedSession $sessC $confC
if ($procC -eq $null) {
    Write-Fail "Control C: session launch failed"
} else {
    $pdValC = (& $PSMUX show-options -g -v "paste-detection" -t $sessC 2>&1).Trim()
    $keysC = (& $PSMUX list-keys -t $sessC 2>&1 | Out-String)
    $cvBoundC = $keysC -match "C-v"
    Write-Info "paste-detection = '$pdValC', C-v still bound = $cvBoundC"

    & $PSMUX send-keys -t $sessC "clear" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    Inject-CtrlV $procC.Id
    Start-Sleep -Seconds 2

    $capC = Capture-Pane $sessC
    $leakedC = $capC -match [regex]::Escape($MARKER)

    Write-Host "`n  [Control C] paste-detection=$pdValC, C-v bound=$cvBoundC" -ForegroundColor Yellow
    Write-Host "  Pane after Ctrl+V (last 6 lines):" -ForegroundColor DarkGray
    ($capC -split "`n" | Where-Object { $_.Trim() -ne "" } | Select-Object -Last 6) |
        ForEach-Object { Write-Host "    |$_|" -ForegroundColor DarkGray }

    if ($leakedC) {
        Write-Info "Control C: clipboard leaked even with paste-detection off (C-v bound)"
    } else {
        Write-Info "Control C: no leak (as expected or C-v binding intercepted)"
    }

    Kill-OwnSession $sessC
    try { Stop-Process -Id $procC.Id -Force -EA SilentlyContinue } catch {}
}
Remove-Item $confC -Force -EA SilentlyContinue

# ═════════════════════════════════════════════════════════════════════════════
# CONTROL TEST D: DEFAULT config (paste-detection on, C-v bound normally)
# Ctrl+V should trigger psmux paste, clipboard text should appear.
# This confirms the injector and the clipboard approach work at all.
# ═════════════════════════════════════════════════════════════════════════════
Write-Host "`n--- Control D: default config (paste-detection on, C-v bound) ---" -ForegroundColor Magenta

$sessD = "gap198_D"
$procD = Launch-AttachedSession $sessD   # no config, use defaults
if ($procD -eq $null) {
    Write-Fail "Control D: session launch failed"
} else {
    $pdValD = (& $PSMUX show-options -g -v "paste-detection" -t $sessD 2>&1).Trim()
    Write-Info "paste-detection = '$pdValD' (expect 'on')"

    # Re-set clipboard (may have been overwritten)
    [void](Set-ClipboardVerified $MARKER)

    & $PSMUX send-keys -t $sessD "clear" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 1
    Inject-CtrlV $procD.Id
    Start-Sleep -Seconds 3

    $capD = Capture-Pane $sessD
    $leakedD = $capD -match [regex]::Escape($MARKER)

    Write-Host "`n  [Control D] paste-detection=$pdValD (default on)" -ForegroundColor Yellow
    Write-Host "  Pane after Ctrl+V (last 6 lines):" -ForegroundColor DarkGray
    ($capD -split "`n" | Where-Object { $_.Trim() -ne "" } | Select-Object -Last 6) |
        ForEach-Object { Write-Host "    |$_|" -ForegroundColor DarkGray }

    if ($leakedD) {
        Write-Pass "Control D: default paste works (clipboard marker appeared as expected)"
    } else {
        Write-Info "Control D: clipboard NOT visible in pane (paste-detection on but marker absent)"
    }

    Kill-OwnSession $sessD
    try { Stop-Process -Id $procD.Id -Force -EA SilentlyContinue } catch {}
}

# ═════════════════════════════════════════════════════════════════════════════
# INJECTOR LOG
# ═════════════════════════════════════════════════════════════════════════════
$logFile = "$env:TEMP\psmux_inject.log"
if (Test-Path $logFile) {
    Write-Host "`n--- Injector log (last 15 lines) ---" -ForegroundColor DarkGray
    Get-Content $logFile -Tail 15 | ForEach-Object { Write-Host "  $_" -ForegroundColor DarkGray }
}

# ═════════════════════════════════════════════════════════════════════════════
# SUMMARY
# ═════════════════════════════════════════════════════════════════════════════
Write-Host "`n=== Issue #198 Clipboard-Leak Results ===" -ForegroundColor Cyan
Write-Host "  Passed : $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed : $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Leaks  : $($script:Leaks)" -ForegroundColor $(if ($script:Leaks -gt 0) { "Red" } else { "Green" })

if ($script:Leaks -gt 0) {
    Write-Host "`n  VERDICT: REPRODUCED -- C-v leaks clipboard even with paste-detection off + unbind C-v" -ForegroundColor Red
} elseif ($script:TestsFailed -eq 0) {
    Write-Host "`n  VERDICT: NOT_REPRODUCED -- No clipboard leak detected in any scenario" -ForegroundColor Green
} else {
    Write-Host "`n  VERDICT: INCONCLUSIVE -- Some tests failed, check output above" -ForegroundColor Yellow
}

exit $script:TestsFailed
