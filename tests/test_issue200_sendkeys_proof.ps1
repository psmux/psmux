# Issue #200 TRUE END TO END PROOF
# This test launches a REAL psmux session in a console window,
# sends ACTUAL KEYSTROKES (Ctrl+B, :, "new-session -d -s ...", Enter)
# and verifies the session was ACTUALLY created on disk.
#
# This is the DEFINITIVE proof that the command prompt code path works.

param(
    [string]$SessionName = "e2e200_real",
    [string]$TargetSession = "e2e200_target"
)

$ErrorActionPreference = "Stop"
$psmuxDir = "$env:USERPROFILE\.psmux"

# Cleanup from previous runs
Get-Process psmux -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
Start-Sleep -Seconds 1
Remove-Item "$psmuxDir\$SessionName.*" -Force -EA SilentlyContinue
Remove-Item "$psmuxDir\$TargetSession.*" -Force -EA SilentlyContinue

Add-Type @"
using System;
using System.Runtime.InteropServices;

public class Win32Input {
    [DllImport("user32.dll", SetLastError = true)]
    public static extern IntPtr FindWindow(string lpClassName, string lpWindowName);
    
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);
    
    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    
    [DllImport("user32.dll", SetLastError = true)]
    public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
    
    public const byte VK_CONTROL = 0x11;
    public const byte VK_RETURN = 0x0D;
    public const uint KEYEVENTF_KEYUP = 0x0002;
    
    public static void SendCtrlB() {
        keybd_event(VK_CONTROL, 0, 0, UIntPtr.Zero);
        keybd_event(0x42, 0, 0, UIntPtr.Zero); // 'B'
        keybd_event(0x42, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }
    
    public static void SendChar(char c) {
        // Use SendInput for proper character input
        byte vk = 0;
        bool shift = false;
        
        if (c >= 'a' && c <= 'z') vk = (byte)(0x41 + (c - 'a'));
        else if (c >= 'A' && c <= 'Z') { vk = (byte)(0x41 + (c - 'A')); shift = true; }
        else if (c >= '0' && c <= '9') vk = (byte)(0x30 + (c - '0'));
        else if (c == '-') vk = 0xBD;
        else if (c == '_') { vk = 0xBD; shift = true; }
        else if (c == ' ') vk = 0x20;
        else if (c == ':') { vk = 0xBA; shift = true; }
        else if (c == '.') vk = 0xBE;
        else if (c == '/') vk = 0xBF;
        else if (c == '\\') vk = 0xDC;
        else return;
        
        if (shift) keybd_event(0x10, 0, 0, UIntPtr.Zero);
        keybd_event(vk, 0, 0, UIntPtr.Zero);
        keybd_event(vk, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        if (shift) keybd_event(0x10, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }
    
    public static void SendEnter() {
        keybd_event(VK_RETURN, 0, 0, UIntPtr.Zero);
        keybd_event(VK_RETURN, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }
    
    public static void SendString(string s) {
        foreach (char c in s) {
            SendChar(c);
            System.Threading.Thread.Sleep(30);
        }
    }
}
"@

Write-Host "=== Issue #200 TRUE END TO END PROOF ===" -ForegroundColor Cyan
Write-Host ""

# Step 1: Launch psmux in a new console window (ATTACHED session)
Write-Host "[1/5] Launching psmux in a new console window..." -ForegroundColor Yellow
$psmuxExe = (Get-Command psmux -EA Stop).Source
$proc = Start-Process -FilePath $psmuxExe -ArgumentList "new-session","-s",$SessionName -PassThru
Write-Host "  PID: $($proc.Id)"

# Wait for session to be fully ready
$ready = $false
for ($i = 0; $i -lt 50; $i++) {
    Start-Sleep -Milliseconds 200
    if (Test-Path "$psmuxDir\$SessionName.port") {
        $port = (Get-Content "$psmuxDir\$SessionName.port" -Raw).Trim()
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
    exit 1
}
Write-Host "  [OK] Session '$SessionName' is alive on port $port" -ForegroundColor Green

# Step 2: Find the console window and bring it to foreground
Write-Host "[2/5] Finding console window..." -ForegroundColor Yellow
Start-Sleep -Seconds 2  # Let the TUI fully render

# Try to find the window by process
$hwnd = $proc.MainWindowHandle
if ($hwnd -eq [IntPtr]::Zero) {
    # Try enumerating
    Start-Sleep -Seconds 1
    $hwnd = $proc.MainWindowHandle
}

if ($hwnd -eq [IntPtr]::Zero) {
    Write-Host "  [WARN] Could not get window handle directly, trying FindWindow..." -ForegroundColor Yellow
    # Fallback: use process window title
    $proc.Refresh()
    $title = $proc.MainWindowTitle
    Write-Host "  Window title: '$title'"
}

# Bring window to front, and confirm it really is the foreground: the keys
# below go to whichever window holds it, so a refused SetForegroundWindow used
# to read as "the command prompt code path may still be broken" and typed the
# command into somebody else's window.
. "$PSScriptRoot\helpers\focus_guard.ps1"
$focus = Confirm-TestForeground -Hwnd $hwnd
$focusSkip = -not $focus.Ok
if ($focusSkip) {
    Write-Host "  [SKIP] keystroke proof not run: $($focus.Why)" -ForegroundColor Yellow
} else {
    Write-Host "  [OK] Window focused (handle: $hwnd, confirmed foreground)" -ForegroundColor Green
}

Start-Sleep -Milliseconds 500
if (-not $focusSkip) {

# Step 3: Send Ctrl+B (prefix), then : to open command prompt
Write-Host "[3/5] Sending prefix (Ctrl+B) then ':' to open command prompt..." -ForegroundColor Yellow
[Win32Input]::SendCtrlB()
Start-Sleep -Milliseconds 300
[Win32Input]::SendChar(':')
Start-Sleep -Milliseconds 500
Write-Host "  [OK] Command prompt should be open" -ForegroundColor Green

# Step 4: Type the new-session command and press Enter
$cmd = "new-session -d -s $TargetSession"
Write-Host "[4/5] Typing: '$cmd' and pressing Enter..." -ForegroundColor Yellow
[Win32Input]::SendString($cmd)
Start-Sleep -Milliseconds 300
[Win32Input]::SendEnter()
Write-Host "  [OK] Command sent" -ForegroundColor Green

# Step 5: Wait and check if the session was created
Write-Host "[5/5] Waiting for session '$TargetSession' to appear..." -ForegroundColor Yellow
$created = $false
for ($i = 0; $i -lt 80; $i++) {
    Start-Sleep -Milliseconds 250
    if (Test-Path "$psmuxDir\$TargetSession.port") {
        $tp = (Get-Content "$psmuxDir\$TargetSession.port" -Raw).Trim()
        try {
            $tcp2 = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$tp)
            $tcp2.Close()
            $created = $true
            break
        } catch {}
    }
}
$keptFocus = Test-TestForeground -Hwnd $hwnd
}

Write-Host ""
Write-Host "=========================================" -ForegroundColor Cyan
if ($focusSkip) {
    Write-Host "  SKIP: the psmux window never got the keyboard focus, so nothing was typed." -ForegroundColor Yellow
} elseif ($created) {
    Write-Host "  PASS: Session '$TargetSession' was CREATED!" -ForegroundColor Green
    Write-Host "  Port file: $psmuxDir\$TargetSession.port" -ForegroundColor Green
    Write-Host "  Port: $tp" -ForegroundColor Green
    Write-Host "" 
    Write-Host "  This PROVES the command prompt (prefix+:) code path" -ForegroundColor Green
    Write-Host "  in execute_command_string_single() correctly spawns" -ForegroundColor Green
    Write-Host "  a new session. Issue #200 is FIXED." -ForegroundColor Green
} else {
    Write-Host "  FAIL: Session '$TargetSession' was NOT created!" -ForegroundColor Red
    Write-Host "  Port file not found at: $psmuxDir\$TargetSession.port" -ForegroundColor Red
    Write-Host "  The command prompt code path may still be broken." -ForegroundColor Red
    if (-not $keptFocus) {
        Write-Host "  Note: the psmux window lost the foreground while the keys were sent; it is now held by $([PsmuxFocusGuard]::Describe([PsmuxFocusGuard]::Foreground()))" -ForegroundColor Yellow
    }
}
Write-Host "=========================================" -ForegroundColor Cyan

# Cleanup
Write-Host "`nCleaning up..." -ForegroundColor Yellow
# Kill target session
if (Test-Path "$psmuxDir\$TargetSession.port") {
    $tk = if (Test-Path "$psmuxDir\$TargetSession.key") { (Get-Content "$psmuxDir\$TargetSession.key" -Raw).Trim() } else { "" }
    if ($tk -and $tp) {
        try {
            $tc = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$tp)
            $s = $tc.GetStream(); $w = [System.IO.StreamWriter]::new($s)
            $w.Write("AUTH $tk`n"); $w.Flush()
            $w.Write("kill-server`n"); $w.Flush()
            $tc.Close()
        } catch {}
    }
}
# Kill main session
if (Test-Path "$psmuxDir\$SessionName.port") {
    $mk = if (Test-Path "$psmuxDir\$SessionName.key") { (Get-Content "$psmuxDir\$SessionName.key" -Raw).Trim() } else { "" }
    if ($mk -and $port) {
        try {
            $tc2 = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
            $s2 = $tc2.GetStream(); $w2 = [System.IO.StreamWriter]::new($s2)
            $w2.Write("AUTH $mk`n"); $w2.Flush()
            $w2.Write("kill-server`n"); $w2.Flush()
            $tc2.Close()
        } catch {}
    }
}
Start-Sleep -Seconds 1
Remove-Item "$psmuxDir\$SessionName.*" -Force -EA SilentlyContinue
Remove-Item "$psmuxDir\$TargetSession.*" -Force -EA SilentlyContinue

if ($focusSkip -or $created) { exit 0 } else { exit 1 }
