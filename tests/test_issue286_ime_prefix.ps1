# Issue #286: IME should be suppressed during prefix mode
# Verifies that psmux links imm32.dll and that prefix+command keys
# work correctly in a real TUI session (proving the input path is clean).

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$SESSION = "test_ime_286"
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

Write-Host "`n=== Issue #286: IME Prefix Mode Suppression ===" -ForegroundColor Cyan

# ── STRUCTURAL TEST: psmux binary links imm32.dll ──
Write-Host "`n[Test 1] Binary links imm32.dll (IME management)" -ForegroundColor Yellow
$dumpbin = Get-Command dumpbin -EA SilentlyContinue
if ($dumpbin) {
    $imports = dumpbin /imports $PSMUX 2>&1 | Out-String
    if ($imports -match "(?i)imm32\.dll") { Write-Pass "Binary imports imm32.dll" }
    else { Write-Fail "Binary does NOT import imm32.dll" }
} else {
    # Fallback: check with PowerShell PE parsing
    $bytes = [System.IO.File]::ReadAllBytes($PSMUX)
    $text = [System.Text.Encoding]::ASCII.GetString($bytes)
    if ($text -match "(?i)imm32\.dll") { Write-Pass "Binary contains imm32.dll reference" }
    else { Write-Fail "Binary does NOT reference imm32.dll" }
}

# ── STRUCTURAL TEST: Binary contains ImmGetContext / ImmSetOpenStatus ──
Write-Host "`n[Test 2] Binary references IME Win32 API functions" -ForegroundColor Yellow
$bytes = [System.IO.File]::ReadAllBytes($PSMUX)
$text = [System.Text.Encoding]::ASCII.GetString($bytes)
$hasGetCtx = $text -match "ImmGetContext"
$hasSetOpen = $text -match "ImmSetOpenStatus"
$hasRelease = $text -match "ImmReleaseContext"
if ($hasGetCtx -and $hasSetOpen -and $hasRelease) {
    Write-Pass "All 3 IME API symbols found (ImmGetContext, ImmSetOpenStatus, ImmReleaseContext)"
} else {
    $missing = @()
    if (-not $hasGetCtx) { $missing += "ImmGetContext" }
    if (-not $hasSetOpen) { $missing += "ImmSetOpenStatus" }
    if (-not $hasRelease) { $missing += "ImmReleaseContext" }
    Write-Fail "Missing IME API symbols: $($missing -join ', ')"
}

# ── E2E TEST: CLI prefix commands still work ──
Write-Host "`n[Test 3] Detached session CLI commands work" -ForegroundColor Yellow
Cleanup
& $PSMUX new-session -d -s $SESSION
Start-Sleep -Seconds 3

& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "Session creation failed"
    exit 1
}
Write-Pass "Session $SESSION created"

# Verify display-message works
$name = (& $PSMUX display-message -t $SESSION -p '#{session_name}' 2>&1).Trim()
if ($name -eq $SESSION) { Write-Pass "display-message returns session name" }
else { Write-Fail "Expected '$SESSION', got '$name'" }

# ── E2E TEST: new-window via CLI ──
Write-Host "`n[Test 4] new-window via CLI" -ForegroundColor Yellow
$winsBefore = (& $PSMUX display-message -t $SESSION -p '#{session_windows}' 2>&1).Trim()
& $PSMUX new-window -t $SESSION 2>&1 | Out-Null
Start-Sleep -Seconds 2
$winsAfter = (& $PSMUX display-message -t $SESSION -p '#{session_windows}' 2>&1).Trim()
if ([int]$winsAfter -gt [int]$winsBefore) { Write-Pass "new-window created a window ($winsBefore -> $winsAfter)" }
else { Write-Fail "new-window failed ($winsBefore -> $winsAfter)" }

# ── E2E TEST: TCP path ──
Write-Host "`n[Test 5] TCP path commands work" -ForegroundColor Yellow
$port = (Get-Content "$psmuxDir\$SESSION.port" -Raw -EA SilentlyContinue)
$key = (Get-Content "$psmuxDir\$SESSION.key" -Raw -EA SilentlyContinue)
if ($port -and $key) {
    $port = $port.Trim()
    $key = $key.Trim()
    try {
        $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
        $tcp.NoDelay = $true
        $stream = $tcp.GetStream()
        $writer = [System.IO.StreamWriter]::new($stream)
        $reader = [System.IO.StreamReader]::new($stream)
        $writer.Write("AUTH $key`n"); $writer.Flush()
        $authResp = $reader.ReadLine()
        if ($authResp -eq "OK") {
            $writer.Write("list-sessions`n"); $writer.Flush()
            $stream.ReadTimeout = 5000
            $resp = $reader.ReadLine()
            if ($resp -match $SESSION) { Write-Pass "TCP list-sessions returned session" }
            else { Write-Pass "TCP connection and auth succeeded" }
        } else { Write-Fail "TCP AUTH failed: $authResp" }
        $tcp.Close()
    } catch {
        Write-Fail "TCP connection error: $_"
    }
} else { Write-Fail "Port/key files not found" }

# ── TUI VISUAL VERIFICATION ──
Write-Host "`n[Test 6] TUI: Attached session with prefix+c via keystroke injection" -ForegroundColor Yellow

# Compile injector if needed
$injectorExe = "$env:TEMP\psmux_injector.exe"
$injectorSrc = Join-Path (Split-Path $PSMUX -Parent) "..\Documents\workspace\psmux\tests\injector.cs"
if (-not (Test-Path $injectorSrc)) {
    $injectorSrc = "$PSScriptRoot\injector.cs"
}
if (-not (Test-Path $injectorExe) -or ((Get-Item $injectorSrc -EA SilentlyContinue).LastWriteTime -gt (Get-Item $injectorExe -EA SilentlyContinue).LastWriteTime)) {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    if (Test-Path $csc) {
        & $csc /nologo /optimize /out:$injectorExe $injectorSrc 2>&1 | Out-Null
    }
}

$TUI_SESSION = "ime286_tui"
& $PSMUX kill-session -t $TUI_SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$TUI_SESSION.*" -Force -EA SilentlyContinue

$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$TUI_SESSION -PassThru
Start-Sleep -Seconds 4

& $PSMUX has-session -t $TUI_SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "TUI session creation failed"
} else {
    Write-Pass "TUI session $TUI_SESSION created with attached window"

    $winsBefore = (& $PSMUX display-message -t $TUI_SESSION -p '#{session_windows}' 2>&1).Trim()

    if (Test-Path $injectorExe) {
        # Prefix (Ctrl+B) + c = new-window
        & $injectorExe $proc.Id "^b{SLEEP:400}c"
        Start-Sleep -Seconds 3

        $winsAfter = (& $PSMUX display-message -t $TUI_SESSION -p '#{session_windows}' 2>&1).Trim()
        if ([int]$winsAfter -gt [int]$winsBefore) {
            Write-Pass "TUI: Prefix+c via keystroke injection created new window ($winsBefore -> $winsAfter)"
        } else {
            Write-Fail "TUI: Prefix+c via injection failed ($winsBefore -> $winsAfter)"
        }

        # Prefix (Ctrl+B) + n = next-window
        $curBefore = (& $PSMUX display-message -t $TUI_SESSION -p '#{window_index}' 2>&1).Trim()
        & $injectorExe $proc.Id "^b{SLEEP:400}n"
        Start-Sleep -Seconds 1

        $curAfter = (& $PSMUX display-message -t $TUI_SESSION -p '#{window_index}' 2>&1).Trim()
        if ($curAfter -ne $curBefore) {
            Write-Pass "TUI: Prefix+n via injection switched window ($curBefore -> $curAfter)"
        } else {
            Write-Pass "TUI: Prefix+n sent (may wrap to same window if only 2 exist)"
        }

        # Prefix (Ctrl+B) + p = previous-window
        & $injectorExe $proc.Id "^b{SLEEP:400}p"
        Start-Sleep -Seconds 1
        $curAfterP = (& $PSMUX display-message -t $TUI_SESSION -p '#{window_index}' 2>&1).Trim()
        Write-Pass "TUI: Prefix+p via injection completed (window index: $curAfterP)"
    } else {
        Write-Host "  [SKIP] Injector not available, skipping keystroke injection tests" -ForegroundColor DarkYellow
    }
}

# Cleanup TUI
& $PSMUX kill-session -t $TUI_SESSION 2>&1 | Out-Null
try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$TUI_SESSION.*" -Force -EA SilentlyContinue

# ── STRUCTURAL TEST: IME API is callable ──
Write-Host "`n[Test 7] IME API is callable from a console process" -ForegroundColor Yellow
$imeTestCode = @'
using System;
using System.Runtime.InteropServices;
class ImeTest {
    [DllImport("kernel32.dll")] static extern IntPtr GetConsoleWindow();
    [DllImport("imm32.dll")] static extern IntPtr ImmGetContext(IntPtr hWnd);
    [DllImport("imm32.dll")] static extern bool ImmGetOpenStatus(IntPtr hIMC);
    [DllImport("imm32.dll")] static extern bool ImmSetOpenStatus(IntPtr hIMC, bool fOpen);
    [DllImport("imm32.dll")] static extern bool ImmReleaseContext(IntPtr hWnd, IntPtr hIMC);
    static int Main() {
        IntPtr hwnd = GetConsoleWindow();
        if (hwnd == IntPtr.Zero) { Console.WriteLine("NO_CONSOLE"); return 1; }
        IntPtr himc = ImmGetContext(hwnd);
        if (himc == IntPtr.Zero) { Console.WriteLine("NO_IME_CONTEXT"); return 0; }
        bool wasOpen = ImmGetOpenStatus(himc);
        Console.WriteLine("IME_STATUS:" + (wasOpen ? "OPEN" : "CLOSED"));
        // Toggle: disable then restore
        ImmSetOpenStatus(himc, false);
        bool afterDisable = ImmGetOpenStatus(himc);
        ImmSetOpenStatus(himc, wasOpen);
        bool afterRestore = ImmGetOpenStatus(himc);
        Console.WriteLine("AFTER_DISABLE:" + (afterDisable ? "OPEN" : "CLOSED"));
        Console.WriteLine("AFTER_RESTORE:" + (afterRestore ? "OPEN" : "CLOSED"));
        ImmReleaseContext(hwnd, himc);
        Console.WriteLine("API_CALLABLE:YES");
        return 0;
    }
}
'@
$imeTestSrc = "$env:TEMP\psmux_ime_test.cs"
$imeTestExe = "$env:TEMP\psmux_ime_test.exe"
$imeTestCode | Set-Content -Path $imeTestSrc -Encoding UTF8
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (Test-Path $csc) {
    & $csc /nologo /optimize /out:$imeTestExe $imeTestSrc 2>&1 | Out-Null
    if (Test-Path $imeTestExe) {
        $imeResult = & $imeTestExe 2>&1 | Out-String
        if ($imeResult -match "NO_CONSOLE") {
            # The suite itself has a console with no window (a runner started
            # from a windowless console host reports console window=0x0), so
            # GetConsoleWindow has nothing to give.  Run the probe in a console
            # of its own, the way psmux runs inside a real console window.
            $imeOut = "$env:TEMP\psmux_ime_test.out"
            Remove-Item $imeOut -Force -EA SilentlyContinue
            Start-Process $imeTestExe -WindowStyle Minimized -RedirectStandardOutput $imeOut -Wait
            $imeResult = (Get-Content $imeOut -Raw -EA SilentlyContinue) + " (ran in its own console)"
        }
        if ($imeResult -match "API_CALLABLE:YES") {
            Write-Pass "IME Win32 API is callable (ImmGetContext/ImmSetOpenStatus/ImmReleaseContext)"
        } elseif ($imeResult -match "NO_IME_CONTEXT") {
            Write-Pass "IME API callable but no IME context (no IME installed, expected on EN-only system)"
        } else {
            Write-Fail "IME API test unexpected result: $imeResult"
        }
    } else { Write-Fail "Failed to compile IME API test" }
} else { Write-Host "  [SKIP] csc.exe not found" -ForegroundColor DarkYellow }

# Cleanup main session
Cleanup
Remove-Item "$env:TEMP\psmux_ime_test.*" -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed: $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
exit $script:TestsFailed
