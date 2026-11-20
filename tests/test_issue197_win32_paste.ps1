# Issue #197: Win32 TUI proof - real Ctrl+V paste like an actual human
#
# This launches a REAL psmux TUI window, sets the clipboard to the EXACT
# text from the issue reporter, sends a REAL Ctrl+V keystroke, and verifies
# the paste appeared correctly with no freeze, no tilde, no junk.

$ErrorActionPreference = "Stop"
$psmuxDir = "$env:USERPROFILE\.psmux"
$SESSION = "tui_paste_197"

. "$PSScriptRoot\helpers\focus_guard.ps1"

# Win32 keyboard input API + window enumeration for console apps
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public class Win32Paste {
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")]
    public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern int GetWindowTextLength(IntPtr hWnd);

    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("kernel32.dll")]
    public static extern uint GetProcessId(IntPtr hProcess);

    public const byte VK_CONTROL = 0x11;
    public const byte VK_RETURN = 0x0D;
    public const byte VK_SHIFT = 0x10;
    public const byte VK_V = 0x56;
    public const uint KEYEVENTF_KEYUP = 0x0002;

    public static void SendCtrlV() {
        keybd_event(VK_CONTROL, 0, 0, UIntPtr.Zero);
        keybd_event(VK_V, 0, 0, UIntPtr.Zero);
        keybd_event(VK_V, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendCtrlB() {
        keybd_event(VK_CONTROL, 0, 0, UIntPtr.Zero);
        keybd_event(0x42, 0, 0, UIntPtr.Zero);
        keybd_event(0x42, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendEnter() {
        keybd_event(VK_RETURN, 0, 0, UIntPtr.Zero);
        keybd_event(VK_RETURN, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendChar(char c) {
        byte vk = 0; bool shift = false;
        if (c >= 'a' && c <= 'z') vk = (byte)(0x41 + (c - 'a'));
        else if (c >= 'A' && c <= 'Z') { vk = (byte)(0x41 + (c - 'A')); shift = true; }
        else if (c >= '0' && c <= '9') vk = (byte)(0x30 + (c - '0'));
        else if (c == ' ') vk = 0x20;
        else return;
        if (shift) keybd_event(VK_SHIFT, 0, 0, UIntPtr.Zero);
        keybd_event(vk, 0, 0, UIntPtr.Zero);
        keybd_event(vk, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        if (shift) keybd_event(VK_SHIFT, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendString(string s) {
        foreach (char c in s) {
            SendChar(c);
            System.Threading.Thread.Sleep(30);
        }
    }

    // Find the console window hosting a given process ID
    // Console apps are hosted by conhost.exe - but we can find visible
    // windows that appeared right after our process started
    private static List<IntPtr> _foundWindows = new List<IntPtr>();

    public static IntPtr FindConsoleWindowForPid(int pid) {
        _foundWindows.Clear();
        EnumWindows((hWnd, lParam) => {
            uint wPid;
            GetWindowThreadProcessId(hWnd, out wPid);
            if (wPid == (uint)pid && IsWindowVisible(hWnd)) {
                _foundWindows.Add(hWnd);
            }
            return true;
        }, IntPtr.Zero);
        return _foundWindows.Count > 0 ? _foundWindows[0] : IntPtr.Zero;
    }

    // Find ANY new visible console window (conhost) that appeared after launch
    public static IntPtr FindNewestVisibleConsole(HashSet<IntPtr> existingWindows) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((hWnd, lParam) => {
            if (IsWindowVisible(hWnd) && !existingWindows.Contains(hWnd) && GetWindowTextLength(hWnd) > 0) {
                found = hWnd;
                return false; // stop enum
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static HashSet<IntPtr> GetAllVisibleWindows() {
        var windows = new HashSet<IntPtr>();
        EnumWindows((hWnd, lParam) => {
            if (IsWindowVisible(hWnd)) windows.Add(hWnd);
            return true;
        }, IntPtr.Zero);
        return windows;
    }
}
"@

# ── Cleanup old sessions ──────────────────────────────────────────
Write-Host "=== Issue #197: Win32 TUI Real Ctrl+V Paste Test ===" -ForegroundColor Cyan
Get-Process tmux, psmux, pmux -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
Start-Sleep -Seconds 1

# The EXACT texts from the issue report
$testTexts = @(
    @{ Text = 'C:\Users\myusername\Documents\PowerShell\Microsoft.PowerShell_profile.ps1'; Desc = "Exact freeze text" },
    @{ Text = 'C:\Users\myusername\Documents\PowerShell\';                                  Desc = "Shorter path (was OK)" },
    @{ Text = 'ddddddddddd';                                                               Desc = "Simple repeated chars" },
    @{ Text = 'C:\Users\myusername\unity_build.log';                                        Desc = "Short path with dot" }
)

$allPass = $true

foreach ($test in $testTexts) {
    $pasteText = $test.Text
    $desc = $test.Desc
    Write-Host "`n--- Testing: $desc ---" -ForegroundColor Green
    Write-Host "  Clipboard: $pasteText" -ForegroundColor Yellow

    # Kill any old sessions
    Get-Process tmux, psmux, pmux -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
    Start-Sleep -Seconds 1

    # Step 1: Set clipboard to the test text.
    #
    # The Windows clipboard is a single-owner resource: OpenClipboard fails
    # while any other process holds it, and Set-Clipboard swallows that, so a
    # one-shot set-then-read races anything else on the desktop that touches
    # the clipboard. In a full-suite run that is a real collision, and it
    # reported "Clipboard set failed" as a psmux defect when psmux had not even
    # been launched yet. Retry briefly; contention is momentary.
    $clipOk = $false
    for ($attempt = 1; $attempt -le 10; $attempt++) {
        Set-Clipboard -Value $pasteText -EA SilentlyContinue
        Start-Sleep -Milliseconds 150
        $clipCheck = Get-Clipboard -Raw -EA SilentlyContinue
        if ($null -ne $clipCheck -and $clipCheck.Trim() -eq $pasteText) { $clipOk = $true; break }
        Start-Sleep -Milliseconds 200
    }
    if (-not $clipOk) {
        # Still not ours after ~3.5s: another process owns the clipboard. That
        # says nothing about psmux paste handling, so do not call it a failure.
        Write-Host "  [SKIP] could not claim the OS clipboard (held by another process); paste not exercised" -ForegroundColor DarkYellow
        continue
    }
    Write-Host "  Clipboard set OK" -ForegroundColor DarkGray

    # Step 2: Snapshot existing windows BEFORE launching psmux
    $existingWindows = [Win32Paste]::GetAllVisibleWindows()
    Write-Host "  Existing windows: $($existingWindows.Count)" -ForegroundColor DarkGray

    # Step 3: Launch REAL attached psmux session
    $psmuxExe = (Get-Command psmux -EA Stop).Source
    $proc = Start-Process -FilePath $psmuxExe -ArgumentList "new-session", "-s", $SESSION -PassThru
    Write-Host "  Launched PID: $($proc.Id)"

    # Step 4: Wait for session to be ready
    $ready = $false
    for ($i = 0; $i -lt 50; $i++) {
        Start-Sleep -Milliseconds 200
        $portFile = "$psmuxDir\$SESSION.port"
        if (Test-Path $portFile) {
            $port = (Get-Content $portFile -Raw).Trim()
            try {
                $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
                $tcp.Close()
                $ready = $true
                break
            } catch {}
        }
    }
    if (-not $ready) {
        Write-Host "  [FAIL] Session did not start in time" -ForegroundColor Red
        $allPass = $false
        try { $proc.Kill() } catch {}
        continue
    }
    Write-Host "  Session ready (port $port)" -ForegroundColor DarkGray

    # Step 4: Clear the pane first
    Start-Sleep -Seconds 2
    & psmux send-keys -t $SESSION "clear" Enter
    Start-Sleep -Seconds 1

    # Step 6: Focus the window (best effort - console apps owned by conhost)
    $hwnd = [IntPtr]::Zero
    for ($w = 0; $w -lt 15; $w++) {
        $proc.Refresh()
        if ($proc.MainWindowHandle -ne [IntPtr]::Zero) {
            $hwnd = $proc.MainWindowHandle; break
        }
        $hwnd = [Win32Paste]::FindConsoleWindowForPid($proc.Id)
        if ($hwnd -ne [IntPtr]::Zero) { break }
        $hwnd = [Win32Paste]::FindNewestVisibleConsole($existingWindows)
        if ($hwnd -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 200
    }
    # Ctrl+V and the typed marker go to whichever window has the
    # foreground. Confirm it is the psmux window before typing: a refused
    # SetForegroundWindow used to read as "paste text NOT in pane" and sent
    # the keys to somebody else's window.
    $focus = Confirm-TestForeground -Hwnd $hwnd
    if (-not $focus.Ok) {
        Write-Host "  [SKIP] $desc not run: $($focus.Why)" -ForegroundColor Yellow
        try { $proc.Kill() } catch {}
        Start-Sleep -Milliseconds 500
        continue
    }
    Write-Host "  Window focused (hwnd=$hwnd, confirmed foreground)" -ForegroundColor DarkGray
    Start-Sleep -Milliseconds 800

    # Step 6: Send REAL Ctrl+V keystroke!
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    [Win32Paste]::SendCtrlV()
    Start-Sleep -Milliseconds 100
    $sw.Stop()
    $pasteMs = $sw.ElapsedMilliseconds
    Write-Host "  Ctrl+V sent ($pasteMs ms)" -ForegroundColor Yellow

    # Step 7: Wait a moment for paste to be processed, then press Enter
    Start-Sleep -Seconds 1

    # Step 8: Type a marker AFTER the paste to prove terminal is responsive
    [Win32Paste]::SendEnter()
    Start-Sleep -Milliseconds 300

    # Type "echo ALIVE" followed by Enter
    [Win32Paste]::SendString("echo ALIVE")
    Start-Sleep -Milliseconds 200
    [Win32Paste]::SendEnter()
    Start-Sleep -Seconds 1

    $kept = Test-TestForeground -Hwnd $hwnd
    if (-not $kept) { Write-Host "  [INFO] the psmux window lost the foreground while keys were sent; now $([PsmuxFocusGuard]::Describe([PsmuxFocusGuard]::Foreground()))" -ForegroundColor Yellow }

    # Step 9: Capture pane content via CLI
    $capture = & psmux capture-pane -t $SESSION -p
    $captureStr = ($capture | Out-String)
    Write-Host "  --- PANE ---" -ForegroundColor Cyan
    $capture | Where-Object { $_ -ne "" } | Select-Object -First 10 | ForEach-Object { Write-Host "    $_" }
    Write-Host "  --- END ---" -ForegroundColor Cyan

    # Step 10: Verify results
    $escaped = [regex]::Escape($pasteText)

    # Check: text appeared?
    if ($captureStr -match $escaped) {
        Write-Host "  [PASS] Paste text appeared" -ForegroundColor Green
    } else {
        Write-Host "  [FAIL] Paste text NOT in pane!" -ForegroundColor Red
        $allPass = $false
    }

    # Check: no trailing tilde?
    if ($captureStr -match ($escaped + '~')) {
        Write-Host "  [FAIL] Trailing tilde!" -ForegroundColor Red
        $allPass = $false
    } else {
        Write-Host "  [PASS] No trailing tilde" -ForegroundColor Green
    }

    # Check: terminal not frozen (ALIVE appeared)?
    if ($captureStr -match "ALIVE") {
        Write-Host "  [PASS] Terminal responsive after paste" -ForegroundColor Green
    } else {
        Write-Host "  [FAIL] Terminal may be frozen (ALIVE missing)" -ForegroundColor Red
        $allPass = $false
    }

    # Check: no junk/old clipboard before paste text?
    # The issue reporter saw old clipboard contents prepended to their paste
    $lines = $capture | Where-Object { $_ -match $escaped }
    foreach ($line in $lines) {
        $idx = $line.IndexOf($pasteText)
        if ($idx -gt 0) {
            $prefix = $line.Substring(0, $idx)
            # Ignore shell prompt (PS C:\..>)
            if ($prefix -notmatch '^\s*PS\s+[A-Za-z]:\\[^>]*>\s*$' -and $prefix.Trim().Length -gt 3) {
                Write-Host "  [WARN] Possible junk before paste: '$prefix'" -ForegroundColor Yellow
            }
        }
    }

    # Kill session
    try { $proc.Kill() } catch {}
    Start-Sleep -Milliseconds 500
}

# ── Final cleanup ────────────────────────────────────────────────
Get-Process tmux, psmux, pmux -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue

Write-Host ""
if ($allPass) {
    Write-Host "=== ALL Win32 TUI PASTE TESTS PASSED ===" -ForegroundColor Green
} else {
    Write-Host "=== SOME TESTS FAILED ===" -ForegroundColor Red
}

exit $(if ($allPass) { 0 } else { 1 })
