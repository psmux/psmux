# psmux Issue #211: pwsh-mouse-selection Win32 TUI E2E Proof Tests
#
# Validates client-side rsel selection behavior with a REAL psmux TUI window,
# injecting mouse and keyboard events via WriteConsoleInput (subprocess-based
# so our PowerShell host is not disrupted by FreeConsole/AttachConsole).
#
# IMPORTANT: Launches via conhost.exe to guarantee a real conhost window
# (Windows Terminal intercepts mouse events and prevents them from reaching
# crossterm's event loop).
#
# Tests:
#   0:   DIAGNOSTIC: copy-on-release with pwsh-mouse-selection OFF proves
#        mouse events reach psmux.
#   2.1: Set option via TUI command prompt (keybd_event driven)
#   2.2: Left-click drag: copy-on-release with pwsh-mouse-selection ON
#   2.3: Ctrl+Shift+C with no active selection does not clobber clipboard
#   2.4: Smart Ctrl+C with no active selection sends SIGINT (session survives)
#   2.5: Click + Ctrl+C path still leaves session alive
#   2.6: Option roundtrip stays consistent after TUI usage
#
# Run:
#   pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue211_win32_mouse.ps1

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:SessionDead = $false
$script:MouseEventsWork = $false

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red;   $script:TestsFailed++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor Cyan }
function Write-Test($msg) { Write-Host "  [TEST] $msg" -ForegroundColor White }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow }

$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -ErrorAction SilentlyContinue).Path }
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\debug\psmux.exe" -ErrorAction SilentlyContinue).Path }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
Write-Info "Using: $PSMUX"

# Its own -L namespace and PSMUX_DATA_DIR: the suite never touches the
# default server or ~/.psmux, and cleans up with -L kill-server only.
foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "Env:\$v" -EA SilentlyContinue }
$NS = "w32sel211_$PID"
$savedDataDir = $env:PSMUX_DATA_DIR
$dataRoot = Join-Path $env:TEMP "w32sel211_$PID"
$env:PSMUX_DATA_DIR = Join-Path $dataRoot "d"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null

function Psmux { & $PSMUX -L $NS @args 2>&1; Start-Sleep -Milliseconds 300 }

# Seed and read the clipboard with the shared helpers. pwsh's one shot
# Set-Clipboard is a silent no-op and Get-Clipboard returns '' whenever another
# window has the clipboard open at that instant (a clipboard history listener,
# an RDP bridge, the attached psmux client reading the clipboard head after it
# changed). Full sweep 2026-11-16 failed 2.3 with "Ctrl+Shift+C unexpectedly
# changed clipboard (got '')": the read, not psmux.
. (Join-Path $PSScriptRoot "clipboard_test_helpers.ps1")
$savedClip = $null; $clipSaved = $false
try { $savedClip = Get-Clipboard -Raw -EA Stop; $clipSaved = $true } catch {}

# ── Win32 API (window management + keybd_event only) ──────────────────────
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public class W32Sel {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }

    public const byte VK_MENU = 0x12, VK_CONTROL = 0x11, VK_SHIFT = 0x10;
    public const byte VK_RETURN = 0x0D, VK_ESCAPE = 0x1B;
    public const uint UP = 0x0002;

    public static HashSet<IntPtr> Snapshot() {
        var s = new HashSet<IntPtr>();
        EnumWindows((h, l) => { if (IsWindowVisible(h)) s.Add(h); return true; }, IntPtr.Zero);
        return s;
    }

    public static IntPtr FindNewest(HashSet<IntPtr> before) {
        IntPtr f = IntPtr.Zero;
        EnumWindows((h, l) => {
            if (IsWindowVisible(h) && !before.Contains(h) && GetWindowTextLength(h) > 0) {
                var sb2 = new StringBuilder(256);
                GetWindowText(h, sb2, 256);
                string t = sb2.ToString();
                if (!t.Contains("Visual Studio Code") && !t.Contains("Code -")) {
                    f = h; return false;
                }
            }
            return true;
        }, IntPtr.Zero);
        return f;
    }

    public static IntPtr FindByTitle(string needle) {
        IntPtr f = IntPtr.Zero;
        EnumWindows((h, l) => {
            if (IsWindowVisible(h) && GetWindowTextLength(h) > 0) {
                var sb2 = new StringBuilder(512);
                GetWindowText(h, sb2, 512);
                string t = sb2.ToString();
                if (t.IndexOf(needle, StringComparison.OrdinalIgnoreCase) >= 0
                    && !t.Contains("Visual Studio Code") && !t.Contains("Code -")) {
                    f = h; return false;
                }
            }
            return true;
        }, IntPtr.Zero);
        return f;
    }

    public static string Title(IntPtr h) {
        int len = GetWindowTextLength(h); if (len <= 0) return "";
        var sb = new StringBuilder(len + 1); GetWindowText(h, sb, sb.Capacity); return sb.ToString();
    }

    public static bool Focus(IntPtr h) {
        keybd_event(VK_MENU, 0, 0, UIntPtr.Zero);
        ShowWindow(h, 9);
        BringWindowToTop(h);
        SetForegroundWindow(h);
        keybd_event(VK_MENU, 0, UP, UIntPtr.Zero);
        System.Threading.Thread.Sleep(300);
        return GetForegroundWindow() == h;
    }

    public static void Key(byte vk, bool shift) {
        if (shift) keybd_event(VK_SHIFT, 0, 0, UIntPtr.Zero);
        keybd_event(vk, 0, 0, UIntPtr.Zero);
        System.Threading.Thread.Sleep(30);
        keybd_event(vk, 0, UP, UIntPtr.Zero);
        if (shift) { System.Threading.Thread.Sleep(10); keybd_event(VK_SHIFT, 0, UP, UIntPtr.Zero); }
    }

    public static void Enter() { Key(VK_RETURN, false); }

    public static void CtrlB() {
        keybd_event(VK_CONTROL, 0, 0, UIntPtr.Zero);
        System.Threading.Thread.Sleep(20);
        keybd_event(0x42, 0, 0, UIntPtr.Zero);
        System.Threading.Thread.Sleep(40);
        keybd_event(0x42, 0, UP, UIntPtr.Zero);
        System.Threading.Thread.Sleep(10);
        keybd_event(VK_CONTROL, 0, UP, UIntPtr.Zero);
    }

    public static void TypeChar(char c) {
        byte vk = 0; bool shift = false;
        if      (c >= 'a' && c <= 'z') vk = (byte)(0x41 + (c - 'a'));
        else if (c >= 'A' && c <= 'Z') { vk = (byte)(0x41 + (c - 'A')); shift = true; }
        else if (c >= '0' && c <= '9') vk = (byte)(0x30 + (c - '0'));
        else if (c == '-') vk = 0xBD;
        else if (c == ' ') vk = 0x20;
        else if (c == ':') { vk = 0xBA; shift = true; }
        else if (c == ';') vk = 0xBA;
        else if (c == '.') vk = 0xBE;
        else if (c == '/') vk = 0xBF;
        else if (c == '\\') vk = 0xDC;
        else if (c == '=') vk = 0xBB;
        else if (c == ',') vk = 0xBC;
        else if (c == '_') { vk = 0xBD; shift = true; }
        else return;
        Key(vk, shift);
    }

    public static void TypeString(string s) {
        foreach (char c in s) { TypeChar(c); System.Threading.Thread.Sleep(30); }
    }

    public static RECT GetRect(IntPtr h) {
        RECT r; GetWindowRect(h, out r); return r;
    }
}
"@

# ── Subprocess-based console input injection ──────────────────────────────
# We spawn a child pwsh process that does FreeConsole/AttachConsole/
# WriteConsoleInput so our main process is not disrupted.

$script:InjectorCs = @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public class Injector {
    [DllImport("kernel32.dll")] public static extern bool FreeConsole();
    [DllImport("kernel32.dll")] public static extern bool AttachConsole(uint dwProcessId);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern IntPtr CreateFileW(
        [MarshalAs(UnmanagedType.LPWStr)] string lpFileName,
        uint dwDesiredAccess, uint dwShareMode, IntPtr lpSecurityAttributes,
        uint dwCreationDisposition, uint dwFlagsAndAttributes, IntPtr hTemplateFile);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool WriteConsoleInputW(IntPtr hConsoleInput, INPUT_RECORD[] lpBuffer, uint nLength, out uint lpNumberOfEventsWritten);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr hObject);

    [StructLayout(LayoutKind.Explicit)]
    public struct INPUT_RECORD {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public int bKeyDown;
        [FieldOffset(8)] public ushort wRepeatCount;
        [FieldOffset(10)] public ushort wVirtualKeyCode;
        [FieldOffset(12)] public ushort wVirtualScanCode;
        [FieldOffset(14)] public char UnicodeChar;
        [FieldOffset(16)] public uint dwControlKeyState;
        [FieldOffset(4)] public short MouseX;
        [FieldOffset(6)] public short MouseY;
        [FieldOffset(8)] public uint MouseButtonState;
        [FieldOffset(12)] public uint MouseControlKeyState;
        [FieldOffset(16)] public uint MouseEventFlags;
    }

    public const ushort KEY_EVENT = 0x0001;
    public const ushort MOUSE_EVENT = 0x0002;
    public const uint FROM_LEFT_1ST_BUTTON_PRESSED = 0x0001;
    public const uint MOUSE_MOVED = 0x0001;
    public const uint LEFT_CTRL  = 0x0008;
    public const uint SHIFT_PRESSED = 0x0010;

    public static IntPtr OpenConIn() {
        return CreateFileW("CONIN$", 0xC0000000, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
    }

    public static bool WriteMouse(IntPtr h, short col, short row, uint btnState, uint flags) {
        var rec = new INPUT_RECORD();
        rec.EventType = MOUSE_EVENT;
        rec.MouseX = col; rec.MouseY = row;
        rec.MouseButtonState = btnState;
        rec.MouseControlKeyState = 0;
        rec.MouseEventFlags = flags;
        uint w; return WriteConsoleInputW(h, new[] { rec }, 1, out w) && w == 1;
    }

    public static bool WriteKey(IntPtr h, ushort vk, char uchar, uint ctrlState, bool down) {
        var rec = new INPUT_RECORD();
        rec.EventType = KEY_EVENT;
        rec.bKeyDown = down ? 1 : 0;
        rec.wRepeatCount = 1;
        rec.wVirtualKeyCode = vk;
        rec.wVirtualScanCode = 0x2E;
        rec.UnicodeChar = uchar;
        rec.dwControlKeyState = ctrlState;
        uint w; return WriteConsoleInputW(h, new[] { rec }, 1, out w) && w == 1;
    }
}
'@

function Invoke-ConsoleInject {
    param(
        [uint32]$TargetPid,
        [string]$Action,       # "drag", "drag-ctrlshiftc", "drag-ctrlc", "click", "ctrlc", "ctrlshiftc"
        [int]$Col1 = 0, [int]$Row1 = 0,
        [int]$Col2 = 0, [int]$Row2 = 0,
        [int]$Steps = 5
    )

    $injScript = @"
`$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
$($script:InjectorCs)
'@

[Injector]::FreeConsole() | Out-Null
if (-not [Injector]::AttachConsole($TargetPid)) {
    Write-Error 'AttachConsole failed'
    exit 1
}
`$h = [Injector]::OpenConIn()
if (`$h -eq [IntPtr]::new(-1)) {
    Write-Error 'OpenConIn failed'
    exit 1
}

`$action = '$Action'
`$col1 = $Col1; `$row1 = $Row1; `$col2 = $Col2; `$row2 = $Row2; `$steps = $Steps

if (`$action -match 'drag') {
    # Mouse down at (col1, row1)
    [Injector]::WriteMouse(`$h, [int16]`$col1, [int16]`$row1, 1, 0) | Out-Null
    Start-Sleep -Milliseconds 50
    # Drag steps
    for (`$i = 1; `$i -le `$steps; `$i++) {
        `$mx = [int16](`$col1 + (`$col2 - `$col1) * `$i / `$steps)
        `$my = [int16](`$row1 + (`$row2 - `$row1) * `$i / `$steps)
        [Injector]::WriteMouse(`$h, `$mx, `$my, 1, 1) | Out-Null
        Start-Sleep -Milliseconds 30
    }
    # Mouse up
    [Injector]::WriteMouse(`$h, [int16]`$col2, [int16]`$row2, 0, 0) | Out-Null
    Start-Sleep -Milliseconds 200
}

if (`$action -eq 'click') {
    [Injector]::WriteMouse(`$h, [int16]`$col1, [int16]`$row1, 1, 0) | Out-Null
    Start-Sleep -Milliseconds 50
    [Injector]::WriteMouse(`$h, [int16]`$col1, [int16]`$row1, 0, 0) | Out-Null
    Start-Sleep -Milliseconds 100
}

if (`$action -match 'ctrlshiftc') {
    Start-Sleep -Milliseconds 100
    [Injector]::WriteKey(`$h, 0x43, 'C', 0x0018, `$true) | Out-Null
    Start-Sleep -Milliseconds 50
    [Injector]::WriteKey(`$h, 0x43, 'C', 0x0018, `$false) | Out-Null
    Start-Sleep -Milliseconds 100
} elseif (`$action -match 'ctrlc') {
    Start-Sleep -Milliseconds 100
    [Injector]::WriteKey(`$h, 0x43, [char]3, 0x0008, `$true) | Out-Null
    Start-Sleep -Milliseconds 50
    [Injector]::WriteKey(`$h, 0x43, [char]3, 0x0008, `$false) | Out-Null
    Start-Sleep -Milliseconds 100
}

[Injector]::CloseHandle(`$h) | Out-Null
exit 0
"@

    $tmpFile = "$env:TEMP\psmux_inject_$([guid]::NewGuid().ToString('N').Substring(0,8)).ps1"
    Set-Content -Path $tmpFile -Value $injScript -Encoding UTF8

    $result = Start-Process pwsh -ArgumentList "-NoProfile","-ExecutionPolicy","Bypass","-File",$tmpFile `
        -Wait -PassThru -WindowStyle Hidden
    Remove-Item $tmpFile -Force -EA SilentlyContinue
    return ($result.ExitCode -eq 0)
}

# ── Helpers ────────────────────────────────────────────────────────────────
function Ensure-Focus {
    if ($null -eq $script:hwnd -or $script:hwnd -eq [IntPtr]::Zero) { return $false }
    for ($i = 0; $i -lt 5; $i++) {
        if ([W32Sel]::Focus($script:hwnd)) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Verify-Focus { return ([W32Sel]::GetForegroundWindow() -eq $script:hwnd) }

function Skip-IfDead([string]$Name) {
    if ($script:SessionDead) { Write-Fail "$Name (SKIPPED: session dead)"; return $true }
    if ($null -ne $script:proc -and $script:proc.HasExited) {
        $script:SessionDead = $true
        Write-Fail "$Name (process exited)"
        return $true
    }
    return $false
}

$SESSION = "w32sel_211"

# ══════════════════════════════════════════════════════════════════════════
Write-Host "`n==========================================" -ForegroundColor Cyan
Write-Host "  ISSUE #211: Win32 TUI Mouse Selection   " -ForegroundColor Cyan
Write-Host "==========================================`n" -ForegroundColor Cyan

# Snapshot windows BEFORE launch
$snap = [W32Sel]::Snapshot()
Write-Info "$($snap.Count) windows before launch"

# Launch via conhost.exe for a real conhost window (not WT)
$script:proc = Start-Process -FilePath "conhost.exe" `
    -ArgumentList "$PSMUX","-L",$NS,"new-session","-s",$SESSION `
    -PassThru
Start-Sleep -Seconds 4

# Wait for session
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$ready = $false
while ($sw.ElapsedMilliseconds -lt 15000) {
    & $PSMUX -L $NS has-session -t $SESSION 2>$null
    if ($LASTEXITCODE -eq 0) { $ready = $true; break }
    Start-Sleep -Milliseconds 300
}
if (-not $ready) { Write-Host "FATAL: Session did not start" -ForegroundColor Red; exit 1 }
Write-Info "Session ready, PID=$($script:proc.Id)"

# Find the conhost window
Start-Sleep -Seconds 1
$script:hwnd = [W32Sel]::FindNewest($snap)
if ($script:hwnd -eq [IntPtr]::Zero) {
    Start-Sleep -Seconds 2
    $script:hwnd = [W32Sel]::FindByTitle("psmux")
}
if ($script:hwnd -eq [IntPtr]::Zero) {
    $script:hwnd = [W32Sel]::FindByTitle($SESSION)
}
if ($script:hwnd -ne [IntPtr]::Zero) {
    Write-Info "Console: HWND=$($script:hwnd) '$([W32Sel]::Title($script:hwnd))'"
} else {
    Write-Host "WARNING: No console window found." -ForegroundColor Yellow
}

# Enable mouse + pwsh-mouse-selection via CLI
Psmux set -g mouse on -t $SESSION
Psmux set -g pwsh-mouse-selection on -t $SESSION

$val = (Psmux show-options -t $SESSION -gv pwsh-mouse-selection | Out-String).Trim()
Write-Info "pwsh-mouse-selection = $val"

# Find the actual psmux.exe child PID (conhost is the parent, psmux is the child)
# AttachConsole needs a process that owns the console, not conhost itself.
$conhostPid = $script:proc.Id
$psmuxChild = Get-CimInstance Win32_Process -Filter "ParentProcessId = $conhostPid" -EA SilentlyContinue |
    Where-Object { $_.Name -match 'psmux' } | Select-Object -First 1
if ($psmuxChild) {
    $script:TargetPid = [uint32]$psmuxChild.ProcessId
    Write-Info "psmux child PID: $($script:TargetPid)"
} else {
    # Fallback: try the conhost PID itself
    $script:TargetPid = [uint32]$conhostPid
    Write-Info "No psmux child found, using conhost PID: $($script:TargetPid)"
}

# Seed known text for selection verification
$marker = "SELTEST_ABCDEFGHIJKLMNOP_12345"
& $PSMUX -L $NS send-keys -t $SESSION "echo $marker" Enter
Start-Sleep -Seconds 1
& $PSMUX -L $NS send-keys -t $SESSION "echo THE_QUICK_BROWN_FOX_JUMPS" Enter
Start-Sleep -Seconds 1

# ══════════════════════════════════════════════════════════════════════════
# TEST 0: DIAGNOSTIC: copy-on-release proves mouse events reach psmux
# ══════════════════════════════════════════════════════════════════════════
Write-Test "0: DIAGNOSTIC: copy-on-release (pwsh-mouse-selection OFF)"

if (Skip-IfDead "0") {} else {
    # Temporarily turn OFF pwsh-mouse-selection so copy-on-release is active
    Psmux set -g pwsh-mouse-selection off -t $SESSION
    Start-Sleep -Milliseconds 800
    [void](Set-ClipboardVerified "DIAGNOSTIC_MARKER")
    Start-Sleep -Milliseconds 200

    # Inject a drag from col 2 row 2 to col 30 row 2 (console cell coords)
    $ok = Invoke-ConsoleInject -TargetPid $script:TargetPid -Action "drag" `
        -Col1 2 -Row1 2 -Col2 30 -Row2 2
    if (-not $ok) {
        Write-Fail "0: WriteConsoleInput injection failed"
    } else {
        Start-Sleep -Milliseconds 800
        $clip = Get-ClipboardSettled
        if ($null -ne $clip -and $clip -ne "DIAGNOSTIC_MARKER" -and $clip.Length -gt 0) {
            Write-Pass "0: Mouse events reach psmux! Clipboard: '$($clip.Substring(0, [Math]::Min(50, $clip.Length)))'"
            $script:MouseEventsWork = $true
        } else {
            Write-Fail "0: Mouse events did NOT reach psmux (clipboard: '$clip')"
            Write-Info "All subsequent mouse selection tests will be skipped."
        }
    }
    # Re-enable
    Psmux set -g pwsh-mouse-selection on -t $SESSION
    Start-Sleep -Milliseconds 500
}

# ══════════════════════════════════════════════════════════════════════════
# TEST 2.1: Set option via TUI command prompt
# ══════════════════════════════════════════════════════════════════════════
Write-Test "2.1: Set pwsh-mouse-selection via TUI command prompt"

if (Skip-IfDead "2.1") {} elseif ($script:hwnd -eq [IntPtr]::Zero) {
    Write-Fail "2.1: No window handle"
} else {
    Ensure-Focus | Out-Null
    if (Verify-Focus) {
        Psmux set -g pwsh-mouse-selection off -t $SESSION
        Start-Sleep -Milliseconds 300

        [W32Sel]::CtrlB()
        Start-Sleep -Milliseconds 500
        [W32Sel]::TypeChar(':')
        Start-Sleep -Milliseconds 800
        [W32Sel]::TypeString("set -g pwsh-mouse-selection on")
        Start-Sleep -Milliseconds 400
        [W32Sel]::Enter()
        Start-Sleep -Seconds 2

        $val = (Psmux show-options -t $SESSION -gv pwsh-mouse-selection | Out-String).Trim()
        if ($val -eq "on") { Write-Pass "2.1: Command prompt set option to on" }
        else { Write-Fail "2.1: Expected 'on', got '$val'" }
    } else {
        # keybd_event types into whatever window is in front; never type
        # into another one (the pattern of test_win32_tui_mega_proof).
        Write-Skip "2.1: the psmux window could not be brought to the front; no keys typed"
    }
}

# ══════════════════════════════════════════════════════════════════════════
# TEST 2.2: Left-click drag with pwsh-mouse-selection ON: copy-on-release
# ══════════════════════════════════════════════════════════════════════════
Write-Test "2.2: Left-click drag (copy-on-release)"

if (Skip-IfDead "2.2") {} elseif (-not $script:MouseEventsWork) {
    Write-Skip "2.2: Mouse injection did not work (skipped)"
} else {
    [void](Set-ClipboardVerified "UNTOUCHED_MARKER")
    Start-Sleep -Milliseconds 200

    $ok = Invoke-ConsoleInject -TargetPid $script:TargetPid -Action "drag" `
        -Col1 2 -Row1 3 -Col2 30 -Row2 3
    Start-Sleep -Milliseconds 800

    $clip = Get-ClipboardSettled
    if ($null -ne $clip -and $clip.Length -gt 0 -and $clip -ne "UNTOUCHED_MARKER") {
        Write-Pass "2.2: Drag copied on release: '$($clip.Substring(0, [Math]::Min(60, $clip.Length)))'"
    } else {
        Write-Fail "2.2: Expected copy-on-release with pwsh-mouse-selection on (clipboard: '$clip')"
    }
}

# ══════════════════════════════════════════════════════════════════════════
# TEST 2.3: Ctrl+Shift+C with no active selection does not clobber clipboard
# ══════════════════════════════════════════════════════════════════════════
Write-Test "2.3: Ctrl+Shift+C with no active selection"

if (Skip-IfDead "2.3") {} elseif (-not $script:MouseEventsWork) {
    Write-Skip "2.3: Mouse injection did not work (skipped)"
} else {
    [void](Set-ClipboardVerified "BEFORE_COPY")
    Start-Sleep -Milliseconds 200

    # No selection is active here (2.2 clears selection after release).
    # Ctrl+Shift+C should be a no-op for clipboard contents.
    $ok = Invoke-ConsoleInject -TargetPid $script:TargetPid -Action "ctrlshiftc"
    Start-Sleep -Milliseconds 500

    $clip = Get-ClipboardSettled
    if ($clip -eq "BEFORE_COPY") {
        Write-Pass "2.3: Ctrl+Shift+C left clipboard unchanged without active selection"
    } else {
        Write-Fail "2.3: Ctrl+Shift+C unexpectedly changed clipboard (got '$clip')"
    }
}

# ══════════════════════════════════════════════════════════════════════════
# TEST 2.4: Smart Ctrl+C with no active selection sends SIGINT
# ══════════════════════════════════════════════════════════════════════════
Write-Test "2.4: Smart Ctrl+C falls back to SIGINT when no selection"

if (Skip-IfDead "2.4") {} elseif (-not $script:MouseEventsWork) {
    Write-Skip "2.4: Mouse injection did not work (skipped)"
} else {
    & $PSMUX -L $NS send-keys -t $SESSION "ping -n 50 127.0.0.1" Enter
    Start-Sleep -Seconds 2

    Invoke-ConsoleInject -TargetPid $script:TargetPid -Action "ctrlc" | Out-Null
    Start-Sleep -Seconds 2

    & $PSMUX -L $NS has-session -t $SESSION 2>$null
    if ($LASTEXITCODE -eq 0) { Write-Pass "2.4: Session alive after smart Ctrl+C" }
    else { Write-Fail "2.4: Session died after Ctrl+C"; $script:SessionDead = $true }
}

# ══════════════════════════════════════════════════════════════════════════
# TEST 2.5: Click + Ctrl+C path still leaves session alive
# ══════════════════════════════════════════════════════════════════════════
Write-Test "2.5: Click + Ctrl+C keeps session alive"

if (Skip-IfDead "2.5") {} elseif (-not $script:MouseEventsWork) {
    Write-Skip "2.5: Mouse injection did not work (skipped)"
} else {
    & $PSMUX -L $NS send-keys -t $SESSION "ping -n 50 127.0.0.1" Enter
    Start-Sleep -Seconds 2

    # Click to dismiss any selection
    Invoke-ConsoleInject -TargetPid $script:TargetPid -Action "click" -Col1 5 -Row1 5 | Out-Null
    Start-Sleep -Milliseconds 500

    # Ctrl+C with no selection = SIGINT to kill ping
    Invoke-ConsoleInject -TargetPid $script:TargetPid -Action "ctrlc" | Out-Null
    Start-Sleep -Seconds 2

    & $PSMUX -L $NS has-session -t $SESSION 2>$null
    if ($LASTEXITCODE -eq 0) { Write-Pass "2.5: Session alive after SIGINT Ctrl+C" }
    else { Write-Fail "2.5: Session died"; $script:SessionDead = $true }
}

# ══════════════════════════════════════════════════════════════════════════
# TEST 2.6: Option roundtrip after TUI usage
# ══════════════════════════════════════════════════════════════════════════
Write-Test "2.6: Option consistent after TUI interaction"

if (Skip-IfDead "2.6") {} else {
    $val = (Psmux show-options -t $SESSION -gv pwsh-mouse-selection | Out-String).Trim()
    if ($val -eq "on") { Write-Pass "2.6: Option still 'on' after all TUI tests" }
    else { Write-Fail "2.6: Expected 'on', got '$val'" }
}

# ══════════════════════════════════════════════════════════════════════════
# CLEANUP
# ══════════════════════════════════════════════════════════════════════════
Write-Host "`n[Cleanup]" -ForegroundColor Yellow
try { if (-not $script:proc.HasExited) { $script:proc.Kill() } } catch {}
& $PSMUX -L $NS kill-server 2>$null
Start-Sleep -Seconds 1
$env:PSMUX_DATA_DIR = $savedDataDir
Remove-Item -Recurse -Force $dataRoot -EA SilentlyContinue
if ($clipSaved -and $null -ne $savedClip) { [void](Set-ClipboardVerified $savedClip) }

# Summary
Write-Host "`n==========================================" -ForegroundColor Cyan
$color = if ($script:TestsFailed -gt 0) { "Red" } else { "Green" }
Write-Host "  RESULTS: $($script:TestsPassed) passed, $($script:TestsFailed) failed" -ForegroundColor $color
Write-Host "==========================================`n" -ForegroundColor Cyan

if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
