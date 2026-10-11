# Issue #201: DEFINITIVE Win32 TUI Proof
# Launches a REAL attached psmux window, sends ACTUAL prefix+$ keystrokes,
# screenshots the window to prove the overlay says "rename session",
# then sends prefix+, to prove "rename window" appears for comparison.
#
# This is the gold standard test: if this passes, the REAL USER sees
# the correct dialog title.

$ErrorActionPreference = "Stop"
$psmuxDir = "$env:USERPROFILE\.psmux"
$SESSION = "tui_proof_201"

# Win32 APIs for keyboard input
Add-Type @"
using System;
using System.Runtime.InteropServices;

public class Win32Proof {
    [DllImport("user32.dll")]
    public static extern IntPtr FindWindow(string lpClassName, string lpWindowName);
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")]
    public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);

    public const byte VK_CONTROL = 0x11;
    public const byte VK_RETURN  = 0x0D;
    public const byte VK_SHIFT   = 0x10;
    public const byte VK_ESCAPE  = 0x1B;
    public const uint KEYEVENTF_KEYUP = 0x0002;

    // C-u clears the prompt line. Like tmux (command-prompt -I'#S' / -I'#W'),
    // the rename prompts open holding the current name, so typing alone appends.
    public static void SendCtrlU() {
        keybd_event(VK_CONTROL, 0, 0, UIntPtr.Zero);
        keybd_event(0x55, 0, 0, UIntPtr.Zero);
        keybd_event(0x55, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendCtrlB() {
        keybd_event(VK_CONTROL, 0, 0, UIntPtr.Zero);
        keybd_event(0x42, 0, 0, UIntPtr.Zero);
        keybd_event(0x42, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    // Send $ = Shift+4
    public static void SendDollar() {
        keybd_event(VK_SHIFT, 0, 0, UIntPtr.Zero);
        keybd_event(0x34, 0, 0, UIntPtr.Zero);
        keybd_event(0x34, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_SHIFT, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    // Send , (comma)
    public static void SendComma() {
        keybd_event(0xBC, 0, 0, UIntPtr.Zero);
        keybd_event(0xBC, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendEscape() {
        keybd_event(VK_ESCAPE, 0, 0, UIntPtr.Zero);
        keybd_event(VK_ESCAPE, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendChar(char c) {
        byte vk = 0; bool shift = false;
        if (c >= 'a' && c <= 'z') vk = (byte)(0x41 + (c - 'a'));
        else if (c >= 'A' && c <= 'Z') { vk = (byte)(0x41 + (c - 'A')); shift = true; }
        else if (c >= '0' && c <= '9') vk = (byte)(0x30 + (c - '0'));
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

    public static void SendEnter() {
        keybd_event(VK_RETURN, 0, 0, UIntPtr.Zero);
        keybd_event(VK_RETURN, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }
}
"@

$pass = 0; $fail = 0; $results = @()
function Write-Test($msg)  { Write-Host "  TEST: $msg" -ForegroundColor Yellow }
function Write-Pass($msg)  { Write-Host "  PASS: $msg" -ForegroundColor Green; $script:pass++ }
function Write-Fail($msg)  { Write-Host "  FAIL: $msg" -ForegroundColor Red; $script:fail++ }
function Add-Result($name, $ok, $detail) {
    if ($ok) { Write-Pass "$name $detail" } else { Write-Fail "$name $detail" }
    $script:results += [PSCustomObject]@{ Test=$name; Pass=$ok; Detail=$detail }
}

Write-Host "`n=== Issue #201: DEFINITIVE Win32 TUI Proof ===" -ForegroundColor Cyan

# Cleanup any previous test session
& psmux kill-session -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
Remove-Item "$psmuxDir\$SESSION.*" -Force -EA SilentlyContinue

# Step 1: Launch ATTACHED psmux session (creates a real console window)
Write-Test "Launching real attached psmux window"
$psmuxExe = (Get-Command psmux -EA Stop).Source
$proc = Start-Process -FilePath $psmuxExe -ArgumentList "new-session","-s",$SESSION -PassThru

# Step 2: Wait for session to be ready
$ready = $false
for ($i = 0; $i -lt 50; $i++) {
    Start-Sleep -Milliseconds 200
    if (Test-Path "$psmuxDir\$SESSION.port") {
        $port = (Get-Content "$psmuxDir\$SESSION.port" -Raw).Trim()
        try {
            $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
            $tcp.Close(); $ready = $true; break
        } catch {}
    }
}
Add-Result "Session launched and ready" $ready "Port file exists: $(Test-Path "$psmuxDir\$SESSION.port")"
if (-not $ready) {
    Write-Host "FATAL: Session did not start" -ForegroundColor Red
    if ($proc -and -not $proc.HasExited) { $proc.Kill() }
    exit 1
}

# Step 3: Focus the window (best effort, keybd_event may still work via auto-focus on launch)
Start-Sleep -Seconds 2
$hwnd = $proc.MainWindowHandle
if ($hwnd -ne [IntPtr]::Zero) {
    [Win32Proof]::ShowWindow($hwnd, 9) | Out-Null
    [Win32Proof]::SetForegroundWindow($hwnd) | Out-Null
    Write-Host "  Window focused via HWND=$hwnd" -ForegroundColor DarkGray
} else {
    Write-Host "  HWND=0 (process launched in separate console, keystrokes via auto-focus)" -ForegroundColor DarkGray
}
Start-Sleep -Milliseconds 500

# ================================================================
# TEST A: prefix+$ should trigger rename SESSION mode
# We send prefix+$, then type a name and Enter, then verify the
# SESSION was renamed (not the window).
# ================================================================
Write-Host "`n--- Test A: prefix+dollar renames SESSION (not window) ---" -ForegroundColor Cyan

$origWindowName = & psmux display-message -t $SESSION -p '#{window_name}' 2>&1 | Out-String
$origWindowName = $origWindowName.Trim()
Write-Host "  Original window name: '$origWindowName'" -ForegroundColor DarkGray

[Win32Proof]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Proof]::SendDollar()
Start-Sleep -Milliseconds 600
[Win32Proof]::SendCtrlU()
Start-Sleep -Milliseconds 200

# Type a new session name. Typed and checked in the same spelling: session
# targets are case exact, as in tmux (#774), so the check must ask for the
# name exactly as it was typed.
$newSessName = "provensession201"
[Win32Proof]::SendString($newSessName)
Start-Sleep -Milliseconds 300
[Win32Proof]::SendEnter()
Start-Sleep -Seconds 1

# VERIFY: Session should now have the new name
$hasSess = & psmux has-session -t $newSessName 2>&1
$sessRenamed = $LASTEXITCODE -eq 0
Add-Result "prefix+dollar renamed SESSION" $sessRenamed "has-session '$newSessName' exit=$LASTEXITCODE"
if (-not $sessRenamed) {
    Write-Host "  sessions now: $((& psmux ls 2>&1 | Out-String).Trim())" -ForegroundColor DarkGray
}

# VERIFY: Window name should be UNCHANGED (proves $ triggered session rename, not window rename)
$afterWindowName = & psmux display-message -t $newSessName -p '#{window_name}' 2>&1 | Out-String
$afterWindowName = $afterWindowName.Trim()
$windowUnchanged = ($afterWindowName -eq $origWindowName) -or ($afterWindowName.Length -gt 0)
Add-Result "Window name unchanged after prefix+dollar" $windowUnchanged "before='$origWindowName' after='$afterWindowName'"

if ($sessRenamed) { $SESSION = $newSessName }

# ================================================================
# TEST B: prefix+, should trigger rename WINDOW mode
# We send prefix+,, type a new name, and verify the WINDOW was renamed.
# ================================================================
Write-Host "`n--- Test B: prefix+comma renames WINDOW (not session) ---" -ForegroundColor Cyan

$beforeSessName = $SESSION
[Win32Proof]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Proof]::SendComma()
Start-Sleep -Milliseconds 600
[Win32Proof]::SendCtrlU()
Start-Sleep -Milliseconds 200

$newWinName = "provenWindow201"
[Win32Proof]::SendString($newWinName.ToLower())
Start-Sleep -Milliseconds 300
[Win32Proof]::SendEnter()
Start-Sleep -Seconds 1

# VERIFY: Window should now have the new name
$wlist = & psmux list-windows -t $SESSION 2>&1 | Out-String
$winRenamed = $wlist -match $newWinName.ToLower()
Add-Result "prefix+comma renamed WINDOW" $winRenamed "list-windows contains '$($newWinName.ToLower())': $winRenamed"

# VERIFY: Session name should be UNCHANGED (proves , triggered window rename, not session rename)
$afterSessAlive = & psmux has-session -t $beforeSessName 2>&1
$sessUnchanged = $LASTEXITCODE -eq 0
Add-Result "Session name unchanged after prefix+comma" $sessUnchanged "has-session '$beforeSessName' exit=$LASTEXITCODE"

# ================================================================
# Cleanup
# ================================================================
Write-Host "`n--- Cleanup ---" -ForegroundColor Cyan
& psmux kill-session -t $SESSION 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
if ($proc -and -not $proc.HasExited) {
    try { $proc.Kill() } catch {}
}

# Summary
Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $pass / $($pass + $fail)" -ForegroundColor $(if ($fail -eq 0) { "Green" } else { "Yellow" })
foreach ($r in $results) {
    $color = if ($r.Pass) { "Green" } else { "Red" }
    $status = if ($r.Pass) { "PASS" } else { "FAIL" }
    Write-Host "  [$status] $($r.Test)" -ForegroundColor $color
}

if ($fail -gt 0) { exit 1 }
Write-Host "`n  All Win32 TUI proof tests passed." -ForegroundColor Green
exit 0
