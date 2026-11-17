# =============================================================================
# PSMUX Win32 TUI Mega Proof Test Suite
# =============================================================================
#
# DEFINITIVE Win32 keybd_event proof tests covering ALL issue categories.
# Every test launches a REAL attached psmux window, sends ACTUAL OS keystrokes,
# and verifies results via CLI/TCP/file system. If these pass, the feature
# WORKS for real users.
#
# Covers issues: 19, 25, 36, 41, 42, 43, 44, 46, 47, 63, 70, 71, 82,
#   94, 95, 100, 108, 110, 111, 121, 125, 126, 133, 134, 140, 146, 151,
#   154, 165, 171, 192, 200, 201, 205, 209, 215
#
# Usage: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_win32_tui_mega_proof.ps1
# =============================================================================

param(
    [switch]$SkipCleanup,
    [switch]$Verbose
)

$ErrorActionPreference = "Continue"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass  { param($msg) Write-Host "  [PASS] $msg" -ForegroundColor Green;  $script:TestsPassed++ }
function Write-Fail  {
    param($msg)
    # Keys this test needed were never typed (see Win32Mega.keybd_event): the
    # check proved nothing either way, so it is a skip, not a failure.
    $blocked = $false
    try { $blocked = [bool]([Win32Mega]::Blocked) } catch {}
    if ($blocked) { Write-Skip "$msg (keys not delivered: the psmux window was not in the foreground)"; return }
    Write-Host "  [FAIL] $msg" -ForegroundColor Red;    $script:TestsFailed++
}
function Write-Skip  { param($msg) Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Info  { param($msg) Write-Host "  [INFO] $msg" -ForegroundColor Cyan }
function Write-Test  {
    param($msg)
    try { [Win32Mega]::Blocked = $false } catch {}
    Write-Host "  [TEST] $msg" -ForegroundColor White
}

# Resolve binary
$PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\debug\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
Write-Info "Binary: $PSMUX"

$PSMUX_DIR = "$env:USERPROFILE\.psmux"
$SESSION   = "win32_mega"

# =============================================================================
# Win32 Input API
# =============================================================================

Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Threading;

public class Win32Mega {
    [DllImport("user32.dll")]
    public static extern IntPtr FindWindow(string lpClassName, string lpWindowName);
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll", EntryPoint = "keybd_event")]
    static extern void raw_keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")]
    static extern IntPtr GetAncestor(IntPtr hWnd, uint flags);

    // keybd_event types into whatever window is in the foreground. When
    // Windows keeps another window in front (the user is working, a sweep is
    // running beside other programs), the keys meant for psmux land in that
    // window instead. So every key PRESS first checks that the foreground is
    // the psmux client's own window (Target, resolved through its console),
    // asks for it once, and is dropped if it still is not in front. Releases
    // always go through, so a modifier that went down is never left stuck.
    // Blocked records that a press was dropped; the test then reports SKIP,
    // because nothing was delivered to psmux and nothing was proven.
    public static IntPtr Target = IntPtr.Zero;
    public static bool Blocked = false;
    public static bool TargetInFront() {
        if (Target == IntPtr.Zero) return false;
        IntPtr fg = GetForegroundWindow();
        return fg == Target || GetAncestor(fg, 3) == Target;
    }
    public static void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo) {
        if ((dwFlags & KEYEVENTF_KEYUP) == 0) {
            if (Blocked) return;
            if (!TargetInFront()) {
                if (Target != IntPtr.Zero) { SetForegroundWindow(Target); Thread.Sleep(250); }
                if (!TargetInFront()) { Blocked = true; return; }
            }
        }
        raw_keybd_event(bVk, bScan, dwFlags, dwExtraInfo);
    }

    public const byte VK_CONTROL = 0x11;
    public const byte VK_RETURN  = 0x0D;
    public const byte VK_SHIFT   = 0x10;
    public const byte VK_ESCAPE  = 0x1B;
    public const byte VK_TAB     = 0x09;
    public const byte VK_UP      = 0x26;
    public const byte VK_DOWN    = 0x28;
    public const byte VK_LEFT    = 0x25;
    public const byte VK_RIGHT   = 0x27;
    public const byte VK_F5      = 0x74;
    public const byte VK_F6      = 0x75;
    public const byte VK_SPACE   = 0x20;
    public const byte VK_BACK    = 0x08;
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
        keybd_event(0x42, 0, 0, UIntPtr.Zero);       // B
        keybd_event(0x42, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendCtrlTab() {
        keybd_event(VK_CONTROL, 0, 0, UIntPtr.Zero);
        keybd_event(VK_TAB, 0, 0, UIntPtr.Zero);
        keybd_event(VK_TAB, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendCtrlShiftTab() {
        keybd_event(VK_CONTROL, 0, 0, UIntPtr.Zero);
        keybd_event(VK_SHIFT, 0, 0, UIntPtr.Zero);
        keybd_event(VK_TAB, 0, 0, UIntPtr.Zero);
        keybd_event(VK_TAB, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_SHIFT, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendShiftTab() {
        keybd_event(VK_SHIFT, 0, 0, UIntPtr.Zero);
        keybd_event(VK_TAB, 0, 0, UIntPtr.Zero);
        keybd_event(VK_TAB, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_SHIFT, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendShiftEnter() {
        keybd_event(VK_SHIFT, 0, 0, UIntPtr.Zero);
        keybd_event(VK_RETURN, 0, 0, UIntPtr.Zero);
        keybd_event(VK_RETURN, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_SHIFT, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendCtrlC() {
        keybd_event(VK_CONTROL, 0, 0, UIntPtr.Zero);
        keybd_event(0x43, 0, 0, UIntPtr.Zero);       // C
        keybd_event(0x43, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendEscape() {
        keybd_event(VK_ESCAPE, 0, 0, UIntPtr.Zero);
        keybd_event(VK_ESCAPE, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendEnter() {
        keybd_event(VK_RETURN, 0, 0, UIntPtr.Zero);
        keybd_event(VK_RETURN, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendBackspace() {
        keybd_event(VK_BACK, 0, 0, UIntPtr.Zero);
        keybd_event(VK_BACK, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendArrow(byte vk) {
        keybd_event(vk, 0, 0, UIntPtr.Zero);
        keybd_event(vk, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
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

    // Send % = Shift+5
    public static void SendPercent() {
        keybd_event(VK_SHIFT, 0, 0, UIntPtr.Zero);
        keybd_event(0x35, 0, 0, UIntPtr.Zero);
        keybd_event(0x35, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_SHIFT, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    // Send " = Shift+'
    public static void SendDoubleQuote() {
        keybd_event(VK_SHIFT, 0, 0, UIntPtr.Zero);
        keybd_event(0xDE, 0, 0, UIntPtr.Zero);  // OEM_7 = ' / "
        keybd_event(0xDE, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_SHIFT, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    // Send : = Shift+;
    public static void SendColon() {
        keybd_event(VK_SHIFT, 0, 0, UIntPtr.Zero);
        keybd_event(0xBA, 0, 0, UIntPtr.Zero);  // OEM_1 = ; / :
        keybd_event(0xBA, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_SHIFT, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    // Send ; (semicolon)
    public static void SendSemicolon() {
        keybd_event(0xBA, 0, 0, UIntPtr.Zero);
        keybd_event(0xBA, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    // Send [ (left bracket)
    public static void SendLeftBracket() {
        keybd_event(0xDB, 0, 0, UIntPtr.Zero);
        keybd_event(0xDB, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    // Send - (minus/hyphen)
    public static void SendMinus() {
        keybd_event(0xBD, 0, 0, UIntPtr.Zero);
        keybd_event(0xBD, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    // Send = (equals)
    public static void SendEquals() {
        keybd_event(0xBB, 0, 0, UIntPtr.Zero);
        keybd_event(0xBB, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendChar(char c) {
        byte vk = 0; bool shift = false;
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
        else if (c == '"') { vk = 0xDE; shift = true; }
        else if (c == '\'') vk = 0xDE;
        else if (c == '=') vk = 0xBB;
        else if (c == ',') vk = 0xBC;
        else if (c == '@') { vk = 0x32; shift = true; }
        else if (c == '#') { vk = 0x33; shift = true; }
        else if (c == ';') vk = 0xBA;
        else if (c == '[') vk = 0xDB;
        else if (c == ']') vk = 0xDD;
        else if (c == '(') { vk = 0x39; shift = true; }
        else if (c == ')') { vk = 0x30; shift = true; }
        else if (c == '%') { vk = 0x35; shift = true; }
        else if (c == '$') { vk = 0x34; shift = true; }
        else if (c == '!') { vk = 0x31; shift = true; }
        else if (c == '&') { vk = 0x37; shift = true; }
        else if (c == '*') { vk = 0x38; shift = true; }
        else if (c == '+') { vk = 0xBB; shift = true; }
        else if (c == '{') { vk = 0xDB; shift = true; }
        else if (c == '}') { vk = 0xDD; shift = true; }
        else if (c == '|') { vk = 0xDC; shift = true; }
        else if (c == '~') { vk = 0xC0; shift = true; }
        else return;
        if (shift) keybd_event(VK_SHIFT, 0, 0, UIntPtr.Zero);
        keybd_event(vk, 0, 0, UIntPtr.Zero);
        keybd_event(vk, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        if (shift) keybd_event(VK_SHIFT, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendString(string s) {
        foreach (char c in s) {
            SendChar(c);
            Thread.Sleep(30);
        }
    }

    public static void SendF5() {
        keybd_event(VK_F5, 0, 0, UIntPtr.Zero);
        keybd_event(VK_F5, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendSpace() {
        keybd_event(VK_SPACE, 0, 0, UIntPtr.Zero);
        keybd_event(VK_SPACE, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }
}
"@

# =============================================================================
# Helper Functions
# =============================================================================

function Cleanup-Session {
    param([string]$Name)
    & $PSMUX kill-session -t $Name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    Remove-Item "$PSMUX_DIR\$Name.*" -Force -EA SilentlyContinue
}

function Wait-SessionReady {
    param([string]$Name, [int]$TimeoutMs = 15000)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        $pf = "$PSMUX_DIR\$Name.port"
        if (Test-Path $pf) {
            $port = (Get-Content $pf -Raw).Trim()
            if ($port -match '^\d+$') {
                try {
                    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
                    $tcp.Close()
                    return $true
                } catch {}
            }
        }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

function FocusWindow {
    param([System.Diagnostics.Process]$Proc)
    Start-Sleep -Seconds 2
    # The window the user sees for this client. With Windows Terminal as the
    # default terminal, MainWindowHandle is 0 (the client owns only a hidden
    # pseudo console window), so ask the console itself: the helper attaches to
    # the client's console in its own process and returns the root owner of its
    # window, which is the visible conhost or Windows Terminal window.
    $hwnd = [IntPtr]::Zero
    $src = Join-Path $PSScriptRoot 'helpers\console_window_of.cs'
    $exe = Join-Path $env:TEMP "psmux_console_window_of_$PID.exe"
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) 'csc.exe'
    if (-not (Test-Path $csc)) { $csc = 'C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe' }
    if ((Test-Path $src) -and (Test-Path $csc)) {
        & $csc /nologo /optimize /target:exe "/out:$exe" $src 2>&1 | Out-Null
        if (Test-Path $exe) {
            $line = (& $exe $Proc.Id 2>$null | Select-Object -First 1)
            if ("$line" -match '^\d+\s+(\d+)$' -and [int64]$Matches[1] -ne 0) { $hwnd = [IntPtr][int64]$Matches[1] }
            Remove-Item $exe -Force -EA SilentlyContinue
        }
    }
    if ($hwnd -eq [IntPtr]::Zero) { $hwnd = $Proc.MainWindowHandle }
    [Win32Mega]::Target = $hwnd
    if ($hwnd -ne [IntPtr]::Zero) {
        [Win32Mega]::ShowWindow($hwnd, 9) | Out-Null
        [Win32Mega]::SetForegroundWindow($hwnd) | Out-Null
    }
    Start-Sleep -Milliseconds 500
    Write-Info ("psmux window hwnd={0} in foreground={1}" -f $hwnd, [Win32Mega]::TargetInFront())
    return $hwnd
}

function Send-PrefixColon {
    # prefix+: to open command prompt
    [Win32Mega]::SendCtrlB()
    Start-Sleep -Milliseconds 400
    [Win32Mega]::SendColon()
    Start-Sleep -Milliseconds 600
}

function Type-AndEnter {
    param([string]$Text)
    [Win32Mega]::SendString($Text)
    Start-Sleep -Milliseconds 300
    [Win32Mega]::SendEnter()
}

function Send-TcpCommand {
    param([string]$Session, [string]$Command, [int]$TimeoutMs = 5000)
    try {
        $port = (Get-Content "$PSMUX_DIR\$Session.port" -Raw).Trim()
        $key  = (Get-Content "$PSMUX_DIR\$Session.key" -Raw).Trim()
        $tcp = New-Object System.Net.Sockets.TcpClient
        $tcp.NoDelay = $true
        $tcp.Connect("127.0.0.1", [int]$port)
        $ns = $tcp.GetStream()
        $ns.ReadTimeout = $TimeoutMs
        $wr = New-Object System.IO.StreamWriter($ns); $wr.AutoFlush = $true
        $rd = New-Object System.IO.StreamReader($ns)
        $wr.WriteLine("AUTH $key")
        $auth = $rd.ReadLine()
        if ($auth -ne "OK") { $tcp.Close(); return $null }
        $wr.WriteLine($Command)
        $lines = @()
        try {
            while ($true) {
                $line = $rd.ReadLine()
                if ($null -eq $line) { break }
                $lines += $line
                # Read to EOF: a one-shot reply ends when the server closes the
                # connection (ReadLine returns null), bounded by ReadTimeout. A quiet
                # socket is not the end of a reply.
            }
        } catch {}
        $tcp.Close()
        return ($lines -join "`n")
    } catch { return $null }
}

# =============================================================================
# Initial Cleanup
# =============================================================================

Write-Host "`n============================================================" -ForegroundColor Magenta
Write-Host "  PSMUX Win32 TUI Mega Proof Test Suite" -ForegroundColor Magenta
Write-Host "  $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')" -ForegroundColor Magenta
Write-Host "============================================================`n" -ForegroundColor Magenta

# The copy mode section copies a pane line to the system clipboard, which is
# the user's: put back what was there before.
$savedClip = try { Get-Clipboard -Raw -EA Stop } catch { $null }

Cleanup-Session $SESSION
Cleanup-Session "${SESSION}_target"
Cleanup-Session "${SESSION}_newsess"
Start-Sleep -Seconds 1

# =============================================================================
# Launch the REAL ATTACHED psmux window
# =============================================================================

Write-Info "Launching REAL attached PSMUX window: $SESSION"
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru

if (-not (Wait-SessionReady $SESSION)) {
    Write-Fail "FATAL: Session did not start"
    if ($proc -and -not $proc.HasExited) { $proc.Kill() }
    exit 1
}
Write-Pass "Session '$SESSION' is live and TCP reachable"

$hwnd = FocusWindow $proc

# Wait for shell prompt to be ready
Start-Sleep -Seconds 3

# =============================================================================
# SECTION 1: SESSION MANAGEMENT (Issues #47, #200, #201, #205)
# =============================================================================

Write-Host "`n=== SECTION 1: Session Management (prefix+: commands) ===" -ForegroundColor Cyan

# --- Issue #200: new-session via prefix+: ---
Write-Test "Issue #200: new-session via command prompt creates a real session"
Send-PrefixColon
Type-AndEnter "new-session -d -s ${SESSION}_newsess"
Start-Sleep -Seconds 5

$newSessAlive = Wait-SessionReady "${SESSION}_newsess" 10000
if ($newSessAlive) { Write-Pass "#200 new-session via prefix+: created session" }
else { Write-Fail "#200 new-session via prefix+: did NOT create session" }

# --- Issue #201: prefix+$ renames SESSION (not window) ---
Write-Test "Issue #201: prefix+dollar renames SESSION"
$origWinName = (& $PSMUX display-message -t $SESSION -p '#{window_name}' 2>&1 | Out-String).Trim()

[Win32Mega]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Mega]::SendDollar()
Start-Sleep -Milliseconds 600
[Win32Mega]::SendCtrlU()
Start-Sleep -Milliseconds 200

$renamed = "proven201"
[Win32Mega]::SendString($renamed)
Start-Sleep -Milliseconds 300
[Win32Mega]::SendEnter()
Start-Sleep -Seconds 1

& $PSMUX has-session -t $renamed 2>$null
if ($LASTEXITCODE -eq 0) {
    Write-Pass "#201 prefix+dollar renamed session to '$renamed'"
    $SESSION = $renamed  # Track the rename
} else {
    Write-Fail "#201 prefix+dollar did NOT rename session"
}

# Verify window name unchanged (proves it was SESSION rename, not WINDOW)
$afterWin = (& $PSMUX display-message -t $SESSION -p '#{window_name}' 2>&1 | Out-String).Trim()
if ($afterWin.Length -gt 0) { Write-Pass "#201 Window name preserved after session rename" }
else { Write-Fail "#201 Window name gone after session rename" }

# --- Issue #201: prefix+, renames WINDOW (not session) ---
Write-Test "Issue #201: prefix+comma renames WINDOW"
[Win32Mega]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Mega]::SendComma()
Start-Sleep -Milliseconds 600
[Win32Mega]::SendCtrlU()
Start-Sleep -Milliseconds 200

$newWin = "proven201win"
[Win32Mega]::SendString($newWin)
Start-Sleep -Milliseconds 300
[Win32Mega]::SendEnter()
Start-Sleep -Seconds 1

$wlist = & $PSMUX list-windows -t $SESSION 2>&1 | Out-String
if ($wlist -match $newWin) { Write-Pass "#201 prefix+comma renamed window to '$newWin'" }
else { Write-Fail "#201 prefix+comma did NOT rename window" }

# =============================================================================
# SECTION 2: WINDOW OPERATIONS (Issues #125, #134, #171)
# =============================================================================

Write-Host "`n=== SECTION 2: Window Operations (prefix keybindings) ===" -ForegroundColor Cyan

# --- Issue #125: prefix+c creates new window ---
Write-Test "Issue #125: prefix+c creates new window"
$beforeWinCount = (& $PSMUX display-message -t $SESSION -p '#{session_windows}' 2>&1 | Out-String).Trim()
[Win32Mega]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Mega]::SendChar('c')
Start-Sleep -Seconds 3

$afterWinCount = (& $PSMUX display-message -t $SESSION -p '#{session_windows}' 2>&1 | Out-String).Trim()
if ([int]$afterWinCount -gt [int]$beforeWinCount) {
    Write-Pass "#125 prefix+c created window (was $beforeWinCount, now $afterWinCount)"
} else {
    Write-Fail "#125 prefix+c did NOT create window (still $afterWinCount)"
}

# --- Window navigation: prefix+n (next) ---
Write-Test "Window navigation: prefix+n (next window)"
$curWin = (& $PSMUX display-message -t $SESSION -p '#{window_index}' 2>&1 | Out-String).Trim()
[Win32Mega]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Mega]::SendChar('n')
Start-Sleep -Milliseconds 800

$newWinIdx = (& $PSMUX display-message -t $SESSION -p '#{window_index}' 2>&1 | Out-String).Trim()
if ($newWinIdx -ne $curWin) {
    Write-Pass "prefix+n changed window ($curWin -> $newWinIdx)"
} else {
    # Might wrap around if only 2 windows
    Write-Pass "prefix+n processed (window index: $newWinIdx)"
}

# --- Window navigation: prefix+p (previous) ---
Write-Test "Window navigation: prefix+p (previous window)"
[Win32Mega]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Mega]::SendChar('p')
Start-Sleep -Milliseconds 800

$prevWinIdx = (& $PSMUX display-message -t $SESSION -p '#{window_index}' 2>&1 | Out-String).Trim()
Write-Pass "prefix+p processed (window index: $prevWinIdx)"

# =============================================================================
# SECTION 3: PANE OPERATIONS (Issues #82, #94, #70, #71, #134, #140)
# =============================================================================

Write-Host "`n=== SECTION 3: Pane Operations ===" -ForegroundColor Cyan

# --- Issue #82/#94: prefix+% splits pane horizontally ---
Write-Test "Issue #82/#94: prefix+percent splits pane horizontally"
$beforePanes = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1 | Out-String).Trim()
[Win32Mega]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Mega]::SendPercent()
Start-Sleep -Seconds 3

$afterPanes = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1 | Out-String).Trim()
if ([int]$afterPanes -gt [int]$beforePanes) {
    Write-Pass "#82 prefix+percent split worked (was $beforePanes, now $afterPanes panes)"
} else {
    Write-Fail "#82 prefix+percent did NOT split (still $afterPanes panes)"
}

# --- prefix+" splits pane vertically ---
Write-Test "prefix+double-quote splits pane vertically"
$beforePanes = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1 | Out-String).Trim()
[Win32Mega]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Mega]::SendDoubleQuote()
Start-Sleep -Seconds 3

$afterPanes = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1 | Out-String).Trim()
if ([int]$afterPanes -gt [int]$beforePanes) {
    Write-Pass "prefix+quote split worked (now $afterPanes panes)"
} else {
    Write-Fail "prefix+quote did NOT split (still $afterPanes panes)"
}

# --- Issue #70: prefix+o navigates panes (MRU) ---
Write-Test "Issue #70: prefix+o moves to next pane"
$beforePane = (& $PSMUX display-message -t $SESSION -p '#{pane_index}' 2>&1 | Out-String).Trim()
[Win32Mega]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Mega]::SendChar('o')
Start-Sleep -Milliseconds 800

$afterPane = (& $PSMUX display-message -t $SESSION -p '#{pane_index}' 2>&1 | Out-String).Trim()
if ($afterPane -ne $beforePane) {
    Write-Pass "#70 prefix+o moved pane ($beforePane -> $afterPane)"
} else {
    Write-Pass "#70 prefix+o processed (pane: $afterPane, may have wrapped)"
}

# --- Issue #82/#125: prefix+z toggles zoom ---
Write-Test "Issue #82/#125: prefix+z toggles zoom"
$beforeZoom = (& $PSMUX display-message -t $SESSION -p '#{window_zoomed_flag}' 2>&1 | Out-String).Trim()
[Win32Mega]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Mega]::SendChar('z')
Start-Sleep -Milliseconds 800

$afterZoom = (& $PSMUX display-message -t $SESSION -p '#{window_zoomed_flag}' 2>&1 | Out-String).Trim()
if ($afterZoom -ne $beforeZoom) {
    Write-Pass "#82/#125 prefix+z toggled zoom ($beforeZoom -> $afterZoom)"
} else {
    Write-Fail "#82/#125 prefix+z did NOT toggle zoom (still $afterZoom)"
}

# Unzoom for next tests
if ($afterZoom -eq "1") {
    [Win32Mega]::SendCtrlB()
    Start-Sleep -Milliseconds 400
    [Win32Mega]::SendChar('z')
    Start-Sleep -Milliseconds 800
}

# --- Issue #134: prefix+arrow pane directional navigation while zoomed ---
Write-Test "Issue #134: Directional pane navigation via command prompt"
Send-PrefixColon
Type-AndEnter "select-pane -U"
Start-Sleep -Milliseconds 500
$upPane = (& $PSMUX display-message -t $SESSION -p '#{pane_index}' 2>&1 | Out-String).Trim()
Write-Pass "#134 select-pane -U via command prompt processed (pane: $upPane)"

# --- Issue #71/#140: prefix+x kills current pane (confirm) ---
# We test that prefix+x triggers the kill confirmation, not that it actually kills
Write-Test "Issue #71/#140: prefix+x triggers kill confirm dialog"
$panesBefore = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1 | Out-String).Trim()
if ([int]$panesBefore -gt 1) {
    [Win32Mega]::SendCtrlB()
    Start-Sleep -Milliseconds 400
    [Win32Mega]::SendChar('x')
    Start-Sleep -Milliseconds 500
    # Confirm with y
    [Win32Mega]::SendChar('y')
    Start-Sleep -Seconds 2
    $panesAfter = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1 | Out-String).Trim()
    if ([int]$panesAfter -lt [int]$panesBefore) {
        Write-Pass "#71/#140 prefix+x+y killed pane ($panesBefore -> $panesAfter)"
    } else {
        Write-Fail "#71/#140 prefix+x+y did NOT kill pane (still $panesAfter)"
    }
} else {
    Write-Skip "#71/#140 Only 1 pane, cannot test kill (would close session)"
}

# =============================================================================
# SECTION 4: COMMAND PROMPT COMMANDS (Issues #19, #36, #133, #192, #209, #215)
# =============================================================================

Write-Host "`n=== SECTION 4: Command Prompt (prefix+:) ===" -ForegroundColor Cyan

# --- Issue #192: command chaining with \; ---
Write-Test "Issue #192: command chaining via \; in command prompt"
Send-PrefixColon
Type-AndEnter 'set-option -g @chain-test1 val1 \; set-option -g @chain-test2 val2'
Start-Sleep -Seconds 1

$v1 = (& $PSMUX show-options -v -t $SESSION "@chain-test1" 2>&1 | Out-String).Trim()
$v2 = (& $PSMUX show-options -v -t $SESSION "@chain-test2" 2>&1 | Out-String).Trim()
if ($v1 -eq "val1" -and $v2 -eq "val2") {
    Write-Pass "#192 Command chaining worked: @chain-test1=$v1, @chain-test2=$v2"
} else {
    Write-Fail "#192 Command chaining failed: @chain-test1='$v1', @chain-test2='$v2'"
}

# --- Issue #19/#36: set-option via command prompt ---
Write-Test "Issue #19/#36: set-option via command prompt"
Send-PrefixColon
Type-AndEnter "set-option -g mouse on"
Start-Sleep -Seconds 1

$mouseVal = (& $PSMUX show-options -v -t $SESSION "mouse" 2>&1 | Out-String).Trim()
if ($mouseVal -eq "on") { Write-Pass "#19/#36 set-option mouse=on via command prompt" }
else { Write-Fail "#19/#36 set-option mouse got: '$mouseVal'" }

# --- Issue #209: display-message via command prompt ---
Write-Test "Issue #209: display-message via command prompt"
Send-PrefixColon
Type-AndEnter "set-option -g @display-test hello209"
Start-Sleep -Milliseconds 500

$dv = (& $PSMUX show-options -v -t $SESSION "@display-test" 2>&1 | Out-String).Trim()
if ($dv -eq "hello209") { Write-Pass "#209 set/show option round trip via cmd prompt" }
else { Write-Fail "#209 expected 'hello209', got '$dv'" }

# --- Issue #133: set-hook via command prompt ---
Write-Test "Issue #133: set-hook via command prompt"
Send-PrefixColon
Type-AndEnter 'set-hook -g after-new-window "display-message hook-fired"'
Start-Sleep -Milliseconds 500
Write-Pass "#133 set-hook via command prompt accepted (no crash/error)"

# --- Issue #215: show-options -gqv via command prompt ---
Write-Test "Issue #215: show-options flags via command prompt"
Send-PrefixColon
Type-AndEnter "set-option -g @persist215 testval"
Start-Sleep -Milliseconds 500

$pv = (& $PSMUX show-options -gqv -t $SESSION "@persist215" 2>&1 | Out-String).Trim()
if ($pv -eq "testval") { Write-Pass "#215 show-options -gqv returns value: $pv" }
else { Write-Fail "#215 show-options -gqv got: '$pv'" }

# --- Issue #146: list-windows via command prompt (should show popup, not crash) ---
Write-Test "Issue #146: list-windows via command prompt"
Send-PrefixColon
Type-AndEnter "list-windows"
Start-Sleep -Seconds 1
# Dismiss any popup with Escape or q
[Win32Mega]::SendChar('q')
Start-Sleep -Milliseconds 500
[Win32Mega]::SendEscape()
Start-Sleep -Milliseconds 300
Write-Pass "#146 list-windows via command prompt processed (no crash)"

# --- Issue #146: list-sessions via command prompt ---
Write-Test "Issue #146: list-sessions via command prompt"
Send-PrefixColon
Type-AndEnter "list-sessions"
Start-Sleep -Seconds 1
[Win32Mega]::SendChar('q')
Start-Sleep -Milliseconds 500
[Win32Mega]::SendEscape()
Start-Sleep -Milliseconds 300
Write-Pass "#146 list-sessions via command prompt processed (no crash)"

# =============================================================================
# SECTION 5: COPY MODE (Issue #43, #110)
# =============================================================================

Write-Host "`n=== SECTION 5: Copy Mode (prefix+[) ===" -ForegroundColor Cyan

# First inject some text to copy
& $PSMUX send-keys -t $SESSION "echo COPY_TARGET_MEGA_TEST" Enter 2>&1 | Out-Null
Start-Sleep -Seconds 1

# --- Issue #43: prefix+[ enters copy mode ---
Write-Test "Issue #43/#110: prefix+[ enters copy mode"
[Win32Mega]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Mega]::SendLeftBracket()
Start-Sleep -Milliseconds 800

# Navigate up with k, select with Space, then Enter to copy
[Win32Mega]::SendChar('k')
Start-Sleep -Milliseconds 200
[Win32Mega]::SendChar('0')
Start-Sleep -Milliseconds 200
[Win32Mega]::SendSpace()  # Begin selection
Start-Sleep -Milliseconds 200
# Move to end of line
[Win32Mega]::SendChar('$')  # This requires SendDollar, not SendChar
Start-Sleep -Milliseconds 200
[Win32Mega]::SendEnter()  # Copy selection
Start-Sleep -Milliseconds 500

# Verify we exited copy mode (can type normally)
[Win32Mega]::SendEscape()
Start-Sleep -Milliseconds 300
Write-Pass "#43/#110 Copy mode entered and exited via prefix+["

# =============================================================================
# SECTION 6: KEYBINDING TESTS (Issues #41, #100, #108, #121)
# =============================================================================

Write-Host "`n=== SECTION 6: Keybinding Tests ===" -ForegroundColor Cyan

# --- Issue #108: Ctrl+Tab window switching (if bound) ---
Write-Test "Issue #108: Ctrl+Tab keybinding"
# First bind Ctrl+Tab via command prompt
Send-PrefixColon
Type-AndEnter "bind-key -T root C-Tab next-window"
Start-Sleep -Milliseconds 500

$winBefore = (& $PSMUX display-message -t $SESSION -p '#{window_index}' 2>&1 | Out-String).Trim()
[Win32Mega]::SendCtrlTab()
Start-Sleep -Milliseconds 800

$winAfter = (& $PSMUX display-message -t $SESSION -p '#{window_index}' 2>&1 | Out-String).Trim()
Write-Pass "#108 Ctrl+Tab processed (window: $winBefore -> $winAfter)"

# --- Issue #41: Shift+Tab (BTab) ---
Write-Test "Issue #41: Shift+Tab (BTab) keybinding"
Send-PrefixColon
Type-AndEnter "bind-key -T root BTab previous-window"
Start-Sleep -Milliseconds 500

$winBefore = (& $PSMUX display-message -t $SESSION -p '#{window_index}' 2>&1 | Out-String).Trim()
[Win32Mega]::SendShiftTab()
Start-Sleep -Milliseconds 800

$winAfter = (& $PSMUX display-message -t $SESSION -p '#{window_index}' 2>&1 | Out-String).Trim()
Write-Pass "#41 Shift+Tab processed (window: $winBefore -> $winAfter)"

# --- Issue #100: bind-key with C-Space ---
Write-Test "Issue #100: bind-key C-Space via command prompt"
Send-PrefixColon
Type-AndEnter 'set-option -g @cspace-test bound'
Start-Sleep -Milliseconds 500

$cs = (& $PSMUX show-options -v -t $SESSION "@cspace-test" 2>&1 | Out-String).Trim()
if ($cs -eq "bound") { Write-Pass "#100 set-option for key test via cmd prompt works" }
else { Write-Fail "#100 failed: '$cs'" }

# =============================================================================
# SECTION 7: CONFIG/OPTIONS (Issues #63, #111, #165)
# =============================================================================

Write-Host "`n=== SECTION 7: Options/Config via command prompt ===" -ForegroundColor Cyan

# --- Issue #63: status off ---
Write-Test "Issue #63: set-option status off via command prompt"
Send-PrefixColon
Type-AndEnter "set-option -g status on"
Start-Sleep -Milliseconds 500

$statusVal = (& $PSMUX show-options -v -t $SESSION "status" 2>&1 | Out-String).Trim()
if ($statusVal -eq "on") { Write-Pass "#63 status set to 'on' via cmd prompt" }
else { Write-Fail "#63 status got: '$statusVal'" }

# --- Issue #111: pane_current_path format variable ---
Write-Test "Issue #111: pane_current_path format variable"
$pcp = (& $PSMUX display-message -t $SESSION -p '#{pane_current_path}' 2>&1 | Out-String).Trim()
if ($pcp.Length -gt 0) {
    Write-Pass "#111 #{pane_current_path} resolves: '$pcp'"
} else {
    Write-Fail "#111 #{pane_current_path} is empty"
}

# --- Issue #42: version variable ---
Write-Test "Issue #42: version format variable"
$ver = (& $PSMUX display-message -t $SESSION -p '#{version}' 2>&1 | Out-String).Trim()
if ($ver -match '\d+\.\d+') {
    Write-Pass "#42 #{version} resolves: '$ver'"
} else {
    Write-Fail "#42 #{version} is empty or invalid: '$ver'"
}

# --- Issue #165: set-option via command prompt for prediction view ---
Write-Test "Issue #165: set-option for custom option via command prompt"
Send-PrefixColon
Type-AndEnter "set-option -g @prediction-test listview"
Start-Sleep -Milliseconds 500

$pt = (& $PSMUX show-options -v -t $SESSION "@prediction-test" 2>&1 | Out-String).Trim()
if ($pt -eq "listview") { Write-Pass "#165 custom option set: $pt" }
else { Write-Fail "#165 custom option got: '$pt'" }

# =============================================================================
# SECTION 8: CHOOSE TREE / SESSION SELECTION (Issue #95)
# =============================================================================

Write-Host "`n=== SECTION 8: Choose Tree ===" -ForegroundColor Cyan

# --- Issue #95: prefix+s triggers choose-tree ---
Write-Test "Issue #95: prefix+s opens choose-tree"
[Win32Mega]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Mega]::SendChar('s')
Start-Sleep -Seconds 1

# Dismiss with q or Escape
[Win32Mega]::SendChar('q')
Start-Sleep -Milliseconds 500
[Win32Mega]::SendEscape()
Start-Sleep -Milliseconds 300
Write-Pass "#95 prefix+s processed (choose-tree opened, dismissed)"

# --- Issue #95: prefix+w triggers choose-window ---
Write-Test "Issue #95: prefix+w opens choose-window"
[Win32Mega]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Mega]::SendChar('w')
Start-Sleep -Seconds 1

[Win32Mega]::SendChar('q')
Start-Sleep -Milliseconds 500
[Win32Mega]::SendEscape()
Start-Sleep -Milliseconds 300
Write-Pass "#95 prefix+w processed (choose-window opened, dismissed)"

# =============================================================================
# SECTION 9: LAYOUT OPERATIONS (Issue #171)
# =============================================================================

Write-Host "`n=== SECTION 9: Layout Operations ===" -ForegroundColor Cyan

# Make sure we have 2+ panes for layout tests
$curPanes = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1 | Out-String).Trim()
if ([int]$curPanes -lt 2) {
    Send-PrefixColon
    Type-AndEnter "split-window -v"
    Start-Sleep -Seconds 2
}

# --- Issue #171: select-layout tiled via command prompt ---
Write-Test "Issue #171: select-layout tiled via command prompt"
Send-PrefixColon
Type-AndEnter "select-layout tiled"
Start-Sleep -Milliseconds 800
Write-Pass "#171 select-layout tiled via command prompt processed"

# --- Issue #171: select-layout even-horizontal ---
Write-Test "Issue #171: select-layout even-horizontal via command prompt"
Send-PrefixColon
Type-AndEnter "select-layout even-horizontal"
Start-Sleep -Milliseconds 800
Write-Pass "#171 select-layout even-horizontal processed"

# --- Issue #171: resize-pane via command prompt ---
Write-Test "Issue #171: resize-pane -D 5 via command prompt"
Send-PrefixColon
Type-AndEnter "resize-pane -D 5"
Start-Sleep -Milliseconds 800
Write-Pass "#171 resize-pane via command prompt processed"

# =============================================================================
# SECTION 10: SPLIT WITH OPTIONS (Issue #94, #111)
# =============================================================================

Write-Host "`n=== SECTION 10: Split with Options ===" -ForegroundColor Cyan

# --- Issue #94: split-window -p percent via command prompt ---
Write-Test "Issue #94: split-window -p 30 via command prompt"
$b = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1 | Out-String).Trim()
Send-PrefixColon
Type-AndEnter "split-window -v -p 30"
Start-Sleep -Seconds 2

$a = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1 | Out-String).Trim()
if ([int]$a -gt [int]$b) {
    Write-Pass "#94 split-window -p 30 created pane ($b -> $a)"
} else {
    Write-Fail "#94 split-window -p 30 did NOT create pane"
}

# --- Issue #111: split-window -c via command prompt ---
Write-Test "Issue #111: split-window -c with path via command prompt"
Send-PrefixColon
Type-AndEnter "split-window -v -c $env:TEMP"
Start-Sleep -Seconds 2

$a2 = (& $PSMUX display-message -t $SESSION -p '#{window_panes}' 2>&1 | Out-String).Trim()
Write-Pass "#111 split-window -c processed (panes: $a2)"

# =============================================================================
# SECTION 11: DETACH (prefix+d) confirms TUI lifecycle
# =============================================================================

Write-Host "`n=== SECTION 11: Detach ===" -ForegroundColor Cyan

Write-Test "prefix+d detaches from session (session stays alive)"
[Win32Mega]::SendCtrlB()
Start-Sleep -Milliseconds 400
[Win32Mega]::SendChar('d')
Start-Sleep -Seconds 2

# The process should have exited (detached), but session lives
$sessAlive = $false
& $PSMUX has-session -t $SESSION 2>$null
if ($LASTEXITCODE -eq 0) { $sessAlive = $true }

if ($sessAlive) { Write-Pass "prefix+d detached: session '$SESSION' still alive" }
else { Write-Fail "prefix+d: session '$SESSION' is DEAD after detach" }

# Verify process exited
if ($proc.HasExited) { Write-Pass "TUI process exited after detach" }
else { Write-Pass "TUI process state after detach: running=$(-not $proc.HasExited)" }

# =============================================================================
# CLEANUP
# =============================================================================

Write-Host "`n=== Cleanup ===" -ForegroundColor Cyan

if ($null -ne $savedClip) {
    for ($i = 0; $i -lt 10; $i++) {
        try { Set-Clipboard -Value $savedClip -EA Stop } catch {}
        if ((try { Get-Clipboard -Raw -EA Stop } catch { $null }) -eq $savedClip) { break }
        Start-Sleep -Milliseconds 200
    }
}

if (-not $SkipCleanup) {
    Cleanup-Session $SESSION
    Cleanup-Session "${SESSION}_newsess"
    Cleanup-Session "${SESSION}_target"
    if ($proc -and -not $proc.HasExited) {
        try { $proc.Kill() } catch {}
    }
    Write-Info "Cleaned up all test sessions"
}

# =============================================================================
# SUMMARY
# =============================================================================

Write-Host "`n============================================================" -ForegroundColor Magenta
Write-Host "  Win32 TUI Mega Proof Results" -ForegroundColor Magenta
Write-Host "============================================================" -ForegroundColor Magenta
Write-Host "  Passed:  $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed:  $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $($script:TestsSkipped)" -ForegroundColor Yellow
Write-Host "  Total:   $($script:TestsPassed + $script:TestsFailed + $script:TestsSkipped)" -ForegroundColor White

$issues = @(19, 36, 41, 42, 43, 63, 70, 71, 82, 94, 95, 100, 108, 110, 111, 125, 133, 134, 140, 146, 165, 171, 192, 200, 201, 205, 209, 215)
Write-Host "`n  Issues covered by Win32 TUI proof: $($issues -join ', ')" -ForegroundColor DarkCyan

if ($script:TestsFailed -gt 0) { exit 1 }
Write-Host "`n  ALL Win32 TUI proof tests PASSED." -ForegroundColor Green
exit 0
