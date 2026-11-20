# Focus guard for suites that type with keybd_event / SendInput.
#
# Synthesised keys go to whichever window is in the foreground, not to the
# window the suite launched. SetForegroundWindow can be refused (the
# foreground lock), so a suite that only calls it and types anyway reports a
# lost focus as a psmux bug ("paste text NOT in pane", "session was NOT
# created") and, worse, types into somebody else's window.
#
# Usage:
#   . "$PSScriptRoot\helpers\focus_guard.ps1"
#   $focus = Confirm-TestForeground -Hwnd $hwnd
#   if (-not $focus.Ok) { Write-Host "  [SKIP] $($focus.Why)"; ... do not type ... }
#
# Confirm-TestForeground restores the window, takes the foreground with the
# Alt key trick (a pressed Alt lifts the foreground lock for the caller), and
# retries until GetForegroundWindow() really is the target. It returns
# @{ Ok = $true } or @{ Ok = $false; Why = "<who holds the foreground>" }.

if (-not ('PsmuxFocusGuard' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class PsmuxFocusGuard {
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr h);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] static extern int GetWindowText(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    const byte VK_MENU = 0x12;
    const uint KEYUP = 0x0002;

    public static IntPtr Foreground() { return GetForegroundWindow(); }

    public static bool Take(IntPtr h) {
        if (IsIconic(h)) ShowWindow(h, 9);
        keybd_event(VK_MENU, 0, 0, UIntPtr.Zero);
        BringWindowToTop(h);
        SetForegroundWindow(h);
        keybd_event(VK_MENU, 0, KEYUP, UIntPtr.Zero);
        System.Threading.Thread.Sleep(150);
        return GetForegroundWindow() == h;
    }

    public static string Describe(IntPtr h) {
        if (h == IntPtr.Zero) return "no window (secure desktop, lock screen or a desktop switch)";
        var sb = new StringBuilder(256);
        GetWindowText(h, sb, sb.Capacity);
        uint pid; GetWindowThreadProcessId(h, out pid);
        string name = "?";
        try { name = System.Diagnostics.Process.GetProcessById((int)pid).ProcessName; } catch { }
        return string.Format("hwnd={0} pid={1} {2} '{3}'", h, pid, name, sb.ToString());
    }
}
'@
}

function Confirm-TestForeground {
    param([IntPtr]$Hwnd, [int]$Attempts = 10)
    if ($Hwnd -eq [IntPtr]::Zero) {
        return @{ Ok = $false; Why = "no window handle for the psmux window, so typing would go to whatever has focus ($([PsmuxFocusGuard]::Describe([PsmuxFocusGuard]::Foreground())))" }
    }
    for ($i = 0; $i -lt $Attempts; $i++) {
        if ([PsmuxFocusGuard]::Take($Hwnd)) { return @{ Ok = $true } }
        Start-Sleep -Milliseconds 200
    }
    return @{ Ok = $false; Why = "the psmux window (hwnd=$Hwnd) never got the foreground after $Attempts attempts; it is held by $([PsmuxFocusGuard]::Describe([PsmuxFocusGuard]::Foreground())). Keys were not sent." }
}

# True while the target still owns the foreground. Call it after typing to
# tell a key that went elsewhere from a key psmux dropped.
function Test-TestForeground {
    param([IntPtr]$Hwnd)
    return ([PsmuxFocusGuard]::Foreground() -eq $Hwnd)
}
