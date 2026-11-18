# test_issue298_set_buffer_clipboard.ps1
# E2E tests for issue #298: set-buffer -w should propagate to Windows clipboard
# https://github.com/psmux/psmux/issues/298
#
# Usage: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue298_set_buffer_clipboard.ps1
#
# Safety: its own -L namespace and PSMUX_DATA_DIR, honours PSMUX_TEST_BIN,
# cleans up with -L kill-server only and never touches ~/.psmux; the user's
# clipboard is saved and restored.

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot "clipboard_test_helpers.ps1")
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$NS = "test298clip_$PID"
$savedDataDir = $env:PSMUX_DATA_DIR
$root = Join-Path $env:TEMP "test298clip_$PID"
$env:PSMUX_DATA_DIR = Join-Path $root "d"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
function P { & $PSMUX -L $NS @args }

$session = "test298clip"
$passed = 0
$failed = 0

function Setup {
    P new-session -d -s $session
    Start-Sleep -Seconds 4
    $rc = $LASTEXITCODE
    if ($rc -ne 0) { throw "Failed to create session (exit=$rc)" }
}

$skipped = 0
function Test($name, $script) {
    Write-Host ""
    Write-Host "[Test] $name"
    try {
        & $script
        $script:passed++
        Write-Host "  [PASS] $name" -ForegroundColor Green
    } catch {
        if ("$_" -like 'SKIP:*') {
            $script:skipped++
            Write-Host "  [SKIP] $name : $_" -ForegroundColor Yellow
            return
        }
        $script:failed++
        Write-Host "  [FAIL] $name : $_" -ForegroundColor Red
    }
}

# Seeds the clipboard and proves it took: pwsh's Set-Clipboard is a silent
# no-op while another window has the clipboard open (clipboard_test_helpers.ps1).
function Seed([string]$value) {
    if (-not (Set-ClipboardVerified $value)) { throw "SKIP: could not seed the clipboard with '$value' (held by another process)" }
}

$savedClip = $null; $clipSaved = $false
try { $savedClip = Get-Clipboard -Raw -EA Stop; $clipSaved = $true } catch {}

Write-Host "`n=== Issue #298 Clipboard Tests ===`n"

# The finally below cleans up even if Setup throws.
try {
Setup

# ── Test 1: set-buffer -w propagates to Windows clipboard ──
Test "set-buffer -w propagates to clipboard" {
    Seed "SENTINEL_BEFORE"
    P set-buffer -w -t $session "ISSUE298_SETBUF" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $clip = Get-ClipboardSettled
    if ($clip -ne "ISSUE298_SETBUF") {
        throw "Expected 'ISSUE298_SETBUF', got '$clip'"
    }
}

# ── Test 2: set-buffer -w -b (named buffer) also propagates ──
Test "set-buffer -w -b named propagates to clipboard" {
    Seed "SENTINEL_BEFORE"
    P set-buffer -w -b mybuf -t $session "NAMED298" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $clip = Get-ClipboardSettled
    if ($clip -ne "NAMED298") {
        throw "Expected 'NAMED298', got '$clip'"
    }
}

# ── Test 3: set-buffer WITHOUT -w does NOT touch clipboard ──
Test "set-buffer without -w leaves clipboard untouched" {
    Seed "SENTINEL_UNTOUCHED"
    P set-buffer -t $session "SHOULD_NOT_CLIP" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $clip = Get-ClipboardSettled
    if ($clip -ne "SENTINEL_UNTOUCHED") {
        throw "Expected 'SENTINEL_UNTOUCHED', got '$clip'"
    }
}

# ── Test 4: load-buffer -w still works (PR #293 regression guard) ──
Test "load-buffer -w still propagates (PR #293)" {
    Seed "SENTINEL_BEFORE"
    $tmpFile = Join-Path $root "psmux_test298.txt"
    "LOADBUF298" | Set-Content $tmpFile -NoNewline
    P load-buffer -w -t $session $tmpFile 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $clip = Get-ClipboardSettled
    Remove-Item $tmpFile -Force -EA SilentlyContinue
    if ($clip -ne "LOADBUF298") {
        throw "Expected 'LOADBUF298', got '$clip'"
    }
}

# ── Test 5: set-buffer -w content also lands in paste buffer ──
Test "set-buffer -w also stores in paste buffer" {
    P set-buffer -w -t $session "DUAL_CHECK" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $buf = (P show-buffer -t $session 2>&1 | Out-String).Trim()
    if ($buf -ne "DUAL_CHECK") {
        throw "Expected paste buffer 'DUAL_CHECK', got '$buf'"
    }
}

# ── Test 6: set-buffer -w with spaces in content ──
Test "set-buffer -w with spaces in content" {
    Seed "SENTINEL"
    P set-buffer -w -t $session "hello world 298" 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $clip = Get-ClipboardSettled
    if ($clip -ne "hello world 298") {
        throw "Expected 'hello world 298', got '$clip'"
    }
}

}
catch {
    $failed++
    Write-Host "  [FAIL] unexpected: $_" -ForegroundColor Red
}
finally {
    try { P kill-server 2>&1 | Out-Null } catch {}
    if ($clipSaved -and $null -ne $savedClip) { [void](Set-ClipboardVerified $savedClip) }
    $env:PSMUX_DATA_DIR = $savedDataDir
    Start-Sleep -Milliseconds 300
    Remove-Item -Recurse -Force $root -EA SilentlyContinue
}

Write-Host "`n=== Results ==="
Write-Host "  Passed: $passed"
Write-Host "  Failed: $failed"
Write-Host "  Skipped: $skipped"
if ($failed -gt 0) { exit 1 }
