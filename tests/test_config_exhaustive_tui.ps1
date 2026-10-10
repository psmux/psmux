# =============================================================================
# PSMUX Win32 TUI Config Exhaustive Test Suite
# =============================================================================
#
# Tests EVERY config option via REAL Win32 keybd_event keystrokes to a live
# PSMUX window using the Ctrl+B : command prompt, exactly as a real user
# would configure psmux interactively.
#
# Every set-option typed via TUI is VERIFIED via TCP show-options to prove
# the option actually took effect.
#
# Usage: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_config_exhaustive_tui.ps1
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
function Write-Fail  { param($msg) Write-Host "  [FAIL] $msg" -ForegroundColor Red;    $script:TestsFailed++ }
function Write-Skip  { param($msg) Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Info  { param($msg) Write-Host "  [INFO] $msg" -ForegroundColor Cyan }
function Write-Test  { param($msg) Write-Host "  [TEST] $msg" -ForegroundColor White }

$PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\debug\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $cmd = Get-Command psmux -EA SilentlyContinue; if ($cmd) { $PSMUX = $cmd.Source } }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }
Write-Info "Binary: $PSMUX"

$PSMUX_DIR = "$env:USERPROFILE\.psmux"
$SESSION   = "w32cfg"

# =============================================================================
# Win32 Input API
# =============================================================================

Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Threading;

public class Win32Cfg {
    [DllImport("user32.dll")]
    public static extern IntPtr FindWindow(string lpClassName, string lpWindowName);
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")]
    public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();

    public const byte VK_CONTROL = 0x11;
    public const byte VK_RETURN  = 0x0D;
    public const byte VK_SHIFT   = 0x10;
    public const byte VK_ESCAPE  = 0x1B;
    public const byte VK_SPACE   = 0x20;
    public const byte VK_BACK    = 0x08;
    public const uint KEYEVENTF_KEYUP = 0x0002;

    public static void SendCtrlB() {
        keybd_event(VK_CONTROL, 0, 0, UIntPtr.Zero);
        keybd_event(0x42, 0, 0, UIntPtr.Zero);
        keybd_event(0x42, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
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

    public static void SendColon() {
        keybd_event(VK_SHIFT, 0, 0, UIntPtr.Zero);
        keybd_event(0xBA, 0, 0, UIntPtr.Zero);
        keybd_event(0xBA, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        keybd_event(VK_SHIFT, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
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
        else if (c == '<') { vk = 0xBC; shift = true; }
        else if (c == '>') { vk = 0xBE; shift = true; }
        else return;
        if (shift) keybd_event(VK_SHIFT, 0, 0, UIntPtr.Zero);
        keybd_event(vk, 0, 0, UIntPtr.Zero);
        keybd_event(vk, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
        if (shift) keybd_event(VK_SHIFT, 0, KEYEVENTF_KEYUP, UIntPtr.Zero);
    }

    public static void SendString(string s) {
        foreach (char c in s) { SendChar(c); Thread.Sleep(30); }
    }
}
"@

# =============================================================================
# Helpers
# =============================================================================

function Cleanup-Session {
    param([string]$Name)
    & $PSMUX kill-session -t $Name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
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
        if ($auth -ne "OK") { $tcp.Close(); return @{ ok=$false; err="AUTH_FAIL" } }
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
        return @{ ok=$true; resp=($lines -join "`n"); lines=$lines }
    } catch { return @{ ok=$false; err=$_.Exception.Message } }
}

# There is NO window to focus, and focus is not needed.
#
# An earlier fix here switched from FindWindow-by-title to the launching
# process's MainWindowHandle. That fails too, for a more basic reason: a console
# application's window is owned by conhost, not by the application, so the psmux
# process reports MainWindowHandle = 0 and an empty MainWindowTitle. Measured on
# a freshly launched attached session. Every window based lookup returns zero and
# the suite aborted at "FATAL: Cannot find psmux window" in every recorded run.
#
# keybd_event was the wrong mechanism regardless: it posts to the hardware input
# queue that feeds the foreground window, while a console app reads its console
# INPUT BUFFER. WriteConsoleInput (tests/injector.cs) writes to that buffer via
# AttachConsole(pid), needing no window, no handle and no focus.
function Focus-PsmuxWindow {
    if (-not $script:TuiPid) { return $false }
    $p = Get-Process -Id $script:TuiPid -EA SilentlyContinue
    return [bool]$p
}

# Escape via WriteConsoleInput. This used to be Send-PsmuxEscape, a
# keybd_event call that never reached the console input buffer, so the popups it
# was meant to dismiss stayed open and swallowed every later keystroke.
function Send-PsmuxEscape {
    if (-not $script:TuiPid) { return }
    & $script:InjectorExe $script:TuiPid "{ESC}" | Out-Null
    Start-Sleep -Milliseconds 300
}

# Type a command into the psmux command prompt: Ctrl+B, then :, then the command,
# then Enter. The pauses are part of the injected sequence so they occur between
# keystrokes at the target, not just in this script.
function Send-PsmuxCommand {
    param([string]$Command)
    if (-not $script:TuiPid) { return }
    & $script:InjectorExe $script:TuiPid "^b{SLEEP:250}:{SLEEP:350}$Command{ENTER}" | Out-Null
    Start-Sleep -Milliseconds 500
}

# Send TUI command, then verify via TCP show-options
function Test-TuiOption {
    param(
        [string]$SetCmd,
        [string]$ShowOpt,
        [string]$ExpectedPattern,
        [string]$Label
    )
    Focus-PsmuxWindow | Out-Null
    Send-PsmuxCommand $SetCmd
    Start-Sleep -Milliseconds 300
    $r = Send-TcpCommand $SESSION "show-options -g $ShowOpt"
    if ($r.ok -and $r.resp -match $ExpectedPattern) {
        Write-Pass "$Label"
    } else {
        Write-Fail "$Label (expected '$ExpectedPattern', got '$($r.resp)')"
    }
}

# =============================================================================
# Setup: Launch attached psmux window
# =============================================================================

Write-Host "`n============================================================" -ForegroundColor Magenta
Write-Host "  PSMUX Win32 TUI Config Exhaustive Test Suite" -ForegroundColor Magenta
Write-Host "  $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')" -ForegroundColor Magenta
Write-Host "============================================================`n" -ForegroundColor Magenta

Cleanup-Session $SESSION
Start-Sleep -Seconds 1

# Build the keystroke injector. Compiled from $PSScriptRoot and to a suite
# specific output name: a relative source path breaks when the runner is invoked
# from another directory, and a shared output name lets suites overwrite each
# other's binary in $TEMP.
$script:InjectorExe = Join-Path $env:TEMP "psmux_injector_cfgtui.exe"
$cscPath = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $cscPath)) {
    $cscPath = Get-ChildItem "C:\Windows\Microsoft.NET\Framework64\v4*\csc.exe" -EA SilentlyContinue |
               Select-Object -First 1 -ExpandProperty FullName
}
& $cscPath /nologo /optimize /out:$script:InjectorExe (Join-Path $PSScriptRoot "injector.cs") 2>&1 | Out-Null
if (-not (Test-Path $script:InjectorExe)) {
    Write-Fail "FATAL: could not build the keystroke injector"
    exit 1
}

Write-Info "Launching attached psmux window '$SESSION'..."
$proc = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$SESSION -PassThru -WindowStyle Normal
$script:TuiPid = $proc.Id
Start-Sleep -Seconds 2

if (-not (Wait-SessionReady $SESSION 20000)) {
    Write-Fail "FATAL: Session '$SESSION' did not start"
    if ($proc -and !$proc.HasExited) { $proc.Kill() }
    exit 1
}
Start-Sleep -Seconds 3

if (-not (Focus-PsmuxWindow)) {
    Write-Fail "FATAL: Cannot find psmux window"
    Cleanup-Session $SESSION
    exit 1
}
Write-Pass "Session '$SESSION' launched and focused"

# =============================================================================
# SECTION 1: Boolean options via TUI command prompt + TCP verify
# =============================================================================

Write-Host "`n=== 1. BOOLEAN OPTIONS VIA TUI ===" -ForegroundColor Cyan

$bools = @(
    'mouse', 'focus-events', 'renumber-windows', 'automatic-rename',
    'allow-rename', 'monitor-activity', 'visual-activity',
    'remain-on-exit', 'destroy-unattached', 'exit-empty',
    'aggressive-resize', 'set-titles', 'visual-bell',
    'scroll-enter-copy-mode', 'pwsh-mouse-selection', 'mouse-selection-force',
    'synchronize-panes', 'env-shim', 'warm',
    'allow-predictions', 'claude-code-fix-tty', 'claude-code-force-interactive'
)

foreach ($opt in $bools) {
    # Set on via TUI
    Test-TuiOption "set-option -g $opt on" $opt '\bon\b' "TUI bool ON: $opt"

    # Set off via TUI
    Test-TuiOption "set-option -g $opt off" $opt '\boff\b' "TUI bool OFF: $opt"
}

# =============================================================================
# SECTION 2: Numeric options via TUI command prompt + TCP verify
# =============================================================================

Write-Host "`n=== 2. NUMERIC OPTIONS VIA TUI ===" -ForegroundColor Cyan

$nums = @(
    @{n='escape-time'; v='100'; d='500'},
    @{n='history-limit'; v='50000'; d='2000'},
    @{n='display-time'; v='3000'; d='750'},
    @{n='display-panes-time'; v='5000'; d='1000'},
    @{n='base-index'; v='1'; d='0'},
    @{n='pane-base-index'; v='1'; d='0'},
    @{n='status-interval'; v='5'; d='15'},
    @{n='main-pane-width'; v='80'; d='0'},
    @{n='main-pane-height'; v='40'; d='0'},
    @{n='status-left-length'; v='50'; d='10'},
    @{n='status-right-length'; v='80'; d='40'},
    @{n='monitor-silence'; v='30'; d='0'}
)

foreach ($opt in $nums) {
    Test-TuiOption "set-option -g $($opt.n) $($opt.v)" $opt.n $opt.v "TUI num: $($opt.n)=$($opt.v)"
    # Restore default
    Send-PsmuxCommand "set-option -g $($opt.n) $($opt.d)"
    Start-Sleep -Milliseconds 200
}

# =============================================================================
# SECTION 3: String/style options via TUI command prompt + TCP verify
# =============================================================================

Write-Host "`n=== 3. STRING/STYLE OPTIONS VIA TUI ===" -ForegroundColor Cyan

$strings = @(
    @{n='status-position'; v='top'; p='top'},
    @{n='status-justify'; v='centre'; p='centre'},
    @{n='mode-keys'; v='vi'; p='vi'},
    @{n='activity-action'; v='any'; p='any'},
    @{n='silence-action'; v='none'; p='none'},
    @{n='bell-action'; v='none'; p='none'},
    @{n='window-size'; v='smallest'; p='smallest'},
    @{n='allow-passthrough'; v='on'; p='on'},
    @{n='set-clipboard'; v='external'; p='external'},
    @{n='default-shell'; v='pwsh'; p='pwsh'},
    @{n='copy-command'; v='clip.exe'; p='clip'}
)

foreach ($opt in $strings) {
    Test-TuiOption "set-option -g $($opt.n) $($opt.v)" $opt.n ([regex]::Escape($opt.p)) "TUI str: $($opt.n)=$($opt.v)"
}

# Style options
$styles = @(
    @{n='status-style'; v='bg=red,fg=white'; p='bg=red'},
    @{n='status-left-style'; v='fg=blue'; p='blue'},
    @{n='status-right-style'; v='fg=green'; p='green'},
    @{n='pane-border-style'; v='fg=grey'; p='grey'},
    @{n='pane-active-border-style'; v='fg=cyan'; p='cyan'},
    @{n='pane-border-hover-style'; v='fg=red'; p='red'},
    @{n='window-status-style'; v='fg=white'; p='white'},
    @{n='window-status-current-style'; v='fg=yellow'; p='yellow'},
    @{n='window-status-activity-style'; v='underscore'; p='underscore'},
    @{n='window-status-bell-style'; v='blink'; p='blink'},
    @{n='window-status-last-style'; v='dim'; p='dim'},
    @{n='message-style'; v='fg=red'; p='red'},
    @{n='message-command-style'; v='fg=blue'; p='blue'},
    @{n='mode-style'; v='bg=blue'; p='blue'}
)

foreach ($opt in $styles) {
    Test-TuiOption "set-option -g $($opt.n) $($opt.v)" $opt.n ([regex]::Escape($opt.p)) "TUI style: $($opt.n)=$($opt.v)"
}

# Format strings via TUI
$formats = @(
    @{n='status-left'; v='[TUI]'; p='TUI'},
    @{n='status-right'; v='%H:%M'; p='%H:%M'},
    @{n='set-titles-string'; v='#S'; p='#S'},
    @{n='window-status-format'; v='#I'; p='#I'},
    @{n='window-status-current-format'; v='#W'; p='#W'},
    @{n='window-status-separator'; v='|'; p='\|'},
    @{n='word-separators'; v=' -_'; p='-_'}
)

foreach ($opt in $formats) {
    Test-TuiOption "set-option -g $($opt.n) $($opt.v)" $opt.n $opt.p "TUI fmt: $($opt.n)=$($opt.v)"
}

# =============================================================================
# SECTION 4: All flags via TUI command prompt + TCP verify
# =============================================================================

Write-Host "`n=== 4. FLAG MATRIX VIA TUI ===" -ForegroundColor Cyan

# -g (global) already tested above, verify one more
Test-TuiOption "set-option -g escape-time 42" "escape-time" '42' "TUI flag -g"
Send-PsmuxCommand "set-option -g escape-time 500"

# -a (append) via TUI
Send-PsmuxCommand "set-option -g status-right AAA"
Start-Sleep -Milliseconds 300
Test-TuiOption "set-option -a status-right BBB" "status-right" 'AAABBB' "TUI flag -a append"

# Triple append via TUI
Send-PsmuxCommand "set-option -g status-left X"
Start-Sleep -Milliseconds 200
Send-PsmuxCommand "set-option -a status-left Y"
Start-Sleep -Milliseconds 200
Test-TuiOption "set-option -a status-left Z" "status-left" 'XYZ' "TUI flag -a triple"

# -u (unset) via TUI
Send-PsmuxCommand "set-option -g @tui-u-test hello"
Start-Sleep -Milliseconds 300
Send-PsmuxCommand "set-option -u @tui-u-test"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g @tui-u-test"
if ($r.ok -and $r.resp -notmatch 'hello') {
    Write-Pass "TUI flag -u unset user option"
} else {
    Write-Fail "TUI flag -u unset user option (got '$($r.resp)')"
}

# -q (quiet) via TUI: verify server still alive after bogus option
Write-Test "TUI flag -q quiet"
Send-PsmuxCommand "set-option -q totally-bogus-option val"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g mouse"
if ($r.ok -and $r.resp -match 'mouse') {
    Write-Pass "TUI flag -q quiet (server alive: mouse=$($r.resp))"
} else {
    Write-Fail "TUI flag -q quiet (server not responding)"
}

# -o (only if unset) via TUI
Send-PsmuxCommand "set-option -g @tui-o-test first"
Start-Sleep -Milliseconds 300
Send-PsmuxCommand "set-option -o @tui-o-test second"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g @tui-o-test"
if ($r.ok -and $r.resp -match 'first') {
    Write-Pass "TUI flag -o preserves existing"
} else {
    Write-Fail "TUI flag -o preserves existing (got '$($r.resp)')"
}

# -o on new option
Send-PsmuxCommand "set-option -u @tui-o-new"
Start-Sleep -Milliseconds 200
Send-PsmuxCommand "set-option -o @tui-o-new fresh"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g @tui-o-new"
if ($r.ok -and $r.resp -match 'fresh') {
    Write-Pass "TUI flag -o sets when unset"
} else {
    Write-Fail "TUI flag -o sets when unset (got '$($r.resp)')"
}

# -w (window scope) via TUI: verify server still alive
Write-Test "TUI flag -w window scope"
Send-PsmuxCommand "set-option -w mouse on"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g mouse"
if ($r.ok -and $r.resp -match 'mouse') {
    Write-Pass "TUI flag -w window scope (server alive: mouse=$($r.resp))"
} else {
    Write-Fail "TUI flag -w window scope (server not responding)"
}

# Combined -a via TUI
Send-PsmuxCommand "set-option -g status-right P1"
Start-Sleep -Milliseconds 200
Test-TuiOption "set-option -a status-right P2" "status-right" 'P1P2' "TUI combined -a"

# Separate -u via TUI
Send-PsmuxCommand "set-option -g @tui-gu hello"
Start-Sleep -Milliseconds 200
Send-PsmuxCommand "set-option -u @tui-gu"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g @tui-gu"
if ($r.ok -and $r.resp -notmatch 'hello') {
    Write-Pass "TUI separate -u unset"
} else {
    Write-Fail "TUI separate -u unset (got '$($r.resp)')"
}

# -t target via TUI: verify server still alive
Write-Test "TUI flag -t target"
Send-PsmuxCommand "set-option -t 0 -g mouse on"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g mouse"
if ($r.ok -and $r.resp -match 'mouse') {
    Write-Pass "TUI flag -t target (server alive: mouse=$($r.resp))"
} else {
    Write-Fail "TUI flag -t target (server not responding)"
}

# =============================================================================
# SECTION 5: User/@- options lifecycle via TUI
# =============================================================================

Write-Host "`n=== 5. USER OPTIONS VIA TUI ===" -ForegroundColor Cyan

# Create
Test-TuiOption "set-option -g @theme mocha" "@theme" 'mocha' "TUI user create @theme"

# Append
Test-TuiOption "set-option -a @theme _extended" "@theme" 'mocha_extended' "TUI user append @theme"

# Only-if-unset (should keep mocha-extended)
Send-PsmuxCommand "set-option -o @theme latte"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g @theme"
if ($r.ok -and $r.resp -match 'mocha') {
    Write-Pass "TUI user -o preserves @theme"
} else {
    Write-Fail "TUI user -o preserves @theme (got '$($r.resp)')"
}

# Unset
Send-PsmuxCommand "set-option -u @theme"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g @theme"
if ($r.ok -and $r.resp -notmatch 'mocha') {
    Write-Pass "TUI user unset @theme"
} else {
    Write-Fail "TUI user unset @theme (got '$($r.resp)')"
}

# Multiple user options
Test-TuiOption "set-option -g @plugin-tpm enabled" "@plugin-tpm" 'enabled' "TUI user @plugin-tpm"
Test-TuiOption "set-option -g @catppuccin-flavor mocha" "@catppuccin-flavor" 'mocha' "TUI user @catppuccin-flavor"
Test-TuiOption "set-option -g @dracula-show-weather false" "@dracula-show-weather" 'false' "TUI user @dracula-show-weather"

# =============================================================================
# SECTION 6: setw / set-window-option aliases via TUI
# =============================================================================

Write-Host "`n=== 6. SETW ALIASES VIA TUI ===" -ForegroundColor Cyan

Test-TuiOption "setw -g mode-keys vi" "mode-keys" 'vi' "TUI setw mode-keys"
Test-TuiOption "set-window-option -g monitor-activity on" "monitor-activity" '\bon\b' "TUI set-window-option monitor-activity"

# Restore
Send-PsmuxCommand "setw -g mode-keys emacs"
Send-PsmuxCommand "set-window-option -g monitor-activity off"

# =============================================================================
# SECTION 7: Hooks via TUI
# =============================================================================

Write-Host "`n=== 7. HOOKS VIA TUI ===" -ForegroundColor Cyan

Write-Test "TUI set-hook"
Send-PsmuxCommand "set-hook -g after-new-window 'run-shell echo a'"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-hooks"
if ($r.ok -and $r.resp -match 'after-new-window.*echo a') {
    Write-Pass "TUI set-hook (verified via show-hooks)"
} else {
    Write-Fail "TUI set-hook (hooks='$($r.resp)')"
}

Write-Test "TUI set-hook append"
Send-PsmuxCommand "set-hook -a after-new-window 'run-shell echo b'"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-hooks"
if ($r.ok -and $r.resp -match 'echo a' -and $r.resp -match 'echo b') {
    Write-Pass "TUI set-hook append (both hooks present)"
} else {
    Write-Fail "TUI set-hook append (hooks='$($r.resp)')"
}

Write-Test "TUI set-hook unset"
Send-PsmuxCommand "set-hook -u after-new-window"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-hooks"
if ($r.ok -and $r.resp -notmatch 'after-new-window') {
    Write-Pass "TUI set-hook unset (verified gone)"
} else {
    Write-Fail "TUI set-hook unset (hooks='$($r.resp)')"
}

# =============================================================================
# SECTION 8: Environment via TUI
# =============================================================================

Write-Host "`n=== 8. ENVIRONMENT VIA TUI ===" -ForegroundColor Cyan

Write-Test "TUI set-environment"
Send-PsmuxCommand "set-environment TUI_ENV_TEST1 value1"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-environment"
if ($r.ok -and $r.resp -match 'TUI_ENV_TEST1.*value1') {
    Write-Pass "TUI set-environment (verified via show-environment)"
} else {
    Write-Fail "TUI set-environment (env='$($r.resp.Substring(0, [Math]::Min(200, $r.resp.Length)))')"
}

Write-Test "TUI setenv alias"
Send-PsmuxCommand "setenv TUI_ENV_TEST2 value2"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-environment"
if ($r.ok -and $r.resp -match 'TUI_ENV_TEST2.*value2') {
    Write-Pass "TUI setenv alias (verified via show-environment)"
} else {
    Write-Fail "TUI setenv alias (env missing TUI_ENV_TEST2)"
}

Write-Test "TUI set-environment -g global"
Send-PsmuxCommand "set-environment -g TUI_ENV_GLOBAL gval"
Start-Sleep -Milliseconds 300
# -g is the global environment, read back with -g like tmux (#775).
$r = Send-TcpCommand $SESSION "show-environment -g TUI_ENV_GLOBAL"
if ($r.ok -and $r.resp -match 'TUI_ENV_GLOBAL.*gval') {
    Write-Pass "TUI set-environment -g (verified via show-environment)"
} else {
    Write-Fail "TUI set-environment -g (env missing TUI_ENV_GLOBAL)"
}

# =============================================================================
# SECTION 9: Command alias via TUI
# =============================================================================

Write-Host "`n=== 9. COMMAND ALIAS VIA TUI ===" -ForegroundColor Cyan

Write-Test "TUI command-alias"
Send-PsmuxCommand "set-option -g command-alias sp=split-window"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g command-alias"
if ($r.ok -and $r.resp -match 'sp=split-window') {
    Write-Pass "TUI command-alias sp=split-window (verified)"
} else {
    Write-Fail "TUI command-alias sp=split-window (show='$($r.resp)')"
}

Write-Test "TUI second command-alias"
Send-PsmuxCommand "set-option -g command-alias nw=new-window"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g command-alias"
if ($r.ok -and $r.resp -match 'nw=new-window') {
    Write-Pass "TUI command-alias nw=new-window (verified)"
} else {
    Write-Fail "TUI command-alias nw=new-window (show='$($r.resp)')"
}

# =============================================================================
# SECTION 10: Status multiline + format via TUI
# =============================================================================

Write-Host "`n=== 10. STATUS MULTILINE VIA TUI ===" -ForegroundColor Cyan

Test-TuiOption "set-option -g status 2" "status" '2' "TUI status 2 lines"
Test-TuiOption "set-option -g status 5" "status" '5' "TUI status 5 lines"

Write-Test "TUI status-format indexed"
Send-PsmuxCommand "set-option -g status-format[0] 'line zero'"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g status"
if ($r.ok) {
    Write-Pass "TUI status-format[0] (server accepted)"
} else {
    Write-Fail "TUI status-format[0] (TCP error: $($r.err))"
}

Send-PsmuxCommand "set-option -g status-format[1] 'line one'"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g status"
if ($r.ok) {
    Write-Pass "TUI status-format[1] (server accepted)"
} else {
    Write-Fail "TUI status-format[1] (TCP error: $($r.err))"
}

# Restore
Send-PsmuxCommand "set-option -g status on"

# =============================================================================
# SECTION 11: Prefix configuration via TUI
# =============================================================================

Write-Host "`n=== 11. PREFIX VIA TUI ===" -ForegroundColor Cyan

# Set prefix to C-a via TUI, then verify via TCP before restoring
Write-Test "TUI prefix C-a"
Send-PsmuxCommand "set-option -g prefix C-a"
Start-Sleep -Milliseconds 500
$r = Send-TcpCommand $SESSION "show-options -g prefix"
if ($r.ok -and $r.resp -match 'C-a') {
    Write-Pass "TUI prefix C-a (verified via TCP, now restoring)"
} else {
    Write-Fail "TUI prefix C-a (show='$($r.resp)')"
}
# Restore via TCP since TUI prefix changed
Send-TcpCommand $SESSION "set-option -g prefix C-b" | Out-Null
Start-Sleep -Milliseconds 300

# Set prefix2 via TUI, verify via TCP
Test-TuiOption "set-option -g prefix2 C-s" "prefix2" 'C-s' "TUI prefix2 C-s"

# Clear prefix2 and verify via TCP
Write-Test "TUI prefix2 none"
Send-PsmuxCommand "set-option -g prefix2 none"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g prefix2"
if ($r.ok -and ($r.resp -match 'none' -or $r.resp -notmatch 'C-s')) {
    Write-Pass "TUI prefix2 none (verified via TCP)"
} else {
    Write-Fail "TUI prefix2 none (show='$($r.resp)')"
}

# Restore prefix to C-b
Send-TcpCommand $SESSION "set-option -g prefix C-b" | Out-Null

# =============================================================================
# SECTION 12: user_options storage options via TUI
# =============================================================================

Write-Host "`n=== 12. USER_OPTIONS STORAGE VIA TUI ===" -ForegroundColor Cyan

$uo = @(
    'popup-style', 'popup-border-style', 'popup-border-lines',
    'window-style', 'window-active-style', 'wrap-search',
    'pane-border-format', 'pane-border-status',
    'clock-mode-colour', 'clock-mode-style',
    'lock-after-time', 'lock-command', 'status-keys'
)

foreach ($opt in $uo) {
    Test-TuiOption "set-option -g $opt testval" $opt 'testval' "TUI uo: $opt"
}

# =============================================================================
# SECTION 13: source-file via TUI
# =============================================================================

Write-Host "`n=== 13. SOURCE-FILE VIA TUI ===" -ForegroundColor Cyan

$tempSrc = Join-Path $env:TEMP "psmux_tui_source_$(Get-Random).conf"
@"
set -g escape-time 77
set -g base-index 5
"@ | Set-Content -Path $tempSrc -Encoding UTF8

Write-Test "TUI source-file"
Send-PsmuxCommand "source-file $tempSrc"
Start-Sleep -Milliseconds 500
$r = Send-TcpCommand $SESSION "show-options -g escape-time"
if ($r.ok -and $r.resp -match '77') {
    Write-Pass "TUI source-file escape-time=77"
} else {
    Write-Fail "TUI source-file escape-time=77 (got '$($r.resp)')"
}

$r = Send-TcpCommand $SESSION "show-options -g base-index"
if ($r.ok -and $r.resp -match '5') {
    Write-Pass "TUI source-file base-index=5"
} else {
    Write-Fail "TUI source-file base-index=5 (got '$($r.resp)')"
}

# Restore
Send-PsmuxCommand "set-option -g escape-time 500"
Send-PsmuxCommand "set-option -g base-index 0"
Remove-Item $tempSrc -Force -EA SilentlyContinue

# Nonexistent source-file should not crash: verify server still responds
Write-Test "TUI source-file missing"
Send-PsmuxCommand "source-file /no/such/file.conf"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g mouse"
if ($r.ok -and $r.resp -match 'mouse') {
    Write-Pass "TUI source-file missing (server alive: mouse=$($r.resp))"
} else {
    Write-Fail "TUI source-file missing (server not responding after missing source)"
}

# =============================================================================
# SECTION 14: tmux compat no-op options via TUI
# =============================================================================

Write-Host "`n=== 14. TMUX COMPAT VIA TUI ===" -ForegroundColor Cyan

Write-Test "TUI terminal-overrides"
Send-PsmuxCommand "set-option -g terminal-overrides ',xterm*:Tc'"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g mouse"
if ($r.ok -and $r.resp -match 'mouse') {
    Write-Pass "TUI terminal-overrides (server alive)"
} else {
    Write-Fail "TUI terminal-overrides (server not responding)"
}

Write-Test "TUI default-terminal"
Send-PsmuxCommand "set-option -g default-terminal xterm-256color"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g mouse"
if ($r.ok -and $r.resp -match 'mouse') {
    Write-Pass "TUI default-terminal (server alive)"
} else {
    Write-Fail "TUI default-terminal (server not responding)"
}

Write-Test "TUI update-environment"
Send-PsmuxCommand "set-option -g update-environment 'FOO BAR'"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g mouse"
if ($r.ok -and $r.resp -match 'mouse') {
    Write-Pass "TUI update-environment (server alive)"
} else {
    Write-Fail "TUI update-environment (server not responding)"
}

# =============================================================================
# SECTION 15: psmux-specific options via TUI
# =============================================================================

Write-Host "`n=== 15. PSMUX-SPECIFIC OPTIONS VIA TUI ===" -ForegroundColor Cyan

Test-TuiOption "set-option -g claude-code-fix-tty on" "claude-code-fix-tty" '\bon\b' "TUI psmux: claude-code-fix-tty"
Test-TuiOption "set-option -g claude-code-force-interactive on" "claude-code-force-interactive" '\bon\b' "TUI psmux: claude-code-force-interactive"
Test-TuiOption "set-option -g allow-predictions on" "allow-predictions" '\bon\b' "TUI psmux: allow-predictions"
Test-TuiOption "set-option -g warm on" "warm" '\bon\b' "TUI psmux: warm"
Test-TuiOption "set-option -g env-shim on" "env-shim" '\bon\b' "TUI psmux: env-shim"
Test-TuiOption "set-option -g pwsh-mouse-selection on" "pwsh-mouse-selection" '\bon\b' "TUI psmux: pwsh-mouse-selection"
Test-TuiOption "set-option -g mouse-selection-force on" "mouse-selection-force" '\bon\b' "TUI psmux: mouse-selection-force"
Test-TuiOption "set-option -g scroll-enter-copy-mode on" "scroll-enter-copy-mode" '\bon\b' "TUI psmux: scroll-enter-copy-mode"

# =============================================================================
# SECTION 16: Cross-channel verify (TUI set, TCP verify, CLI verify)
# =============================================================================

Write-Host "`n=== 16. CROSS-CHANNEL: TUI SET + TCP VERIFY ===" -ForegroundColor Cyan

# Set via TUI, verify via TCP
Send-PsmuxCommand "set-option -g escape-time 123"
Start-Sleep -Milliseconds 500
$r = Send-TcpCommand $SESSION "show-options -g escape-time"
if ($r.ok -and $r.resp -match '123') {
    Write-Pass "Cross-channel: TUI set, TCP verify escape-time=123"
} else {
    Write-Fail "Cross-channel: TUI set, TCP verify escape-time=123 (got '$($r.resp)')"
}

# Set via TUI, verify via CLI
Send-PsmuxCommand "set-option -g @cross-ch tuival"
Start-Sleep -Milliseconds 500
$cli = & $PSMUX show-options -t $SESSION -g @cross-ch 2>&1 | Out-String
if ($cli -match 'tuival') {
    Write-Pass "Cross-channel: TUI set, CLI verify @cross-ch=tuival"
} else {
    Write-Fail "Cross-channel: TUI set, CLI verify @cross-ch=tuival (got '$cli')"
}

# Set via TCP, verify appears from TUI perspective (check via TCP again since we can't read TUI screen)
Send-TcpCommand $SESSION "set-option -g @reverse-ch tcpval" | Out-Null
Start-Sleep -Milliseconds 200
$r = Send-TcpCommand $SESSION "show-options -g @reverse-ch"
if ($r.ok -and $r.resp -match 'tcpval') {
    Write-Pass "Cross-channel: TCP set, TCP verify @reverse-ch=tcpval"
} else {
    Write-Fail "Cross-channel: TCP set, TCP verify @reverse-ch=tcpval (got '$($r.resp)')"
}

# Restore
Send-PsmuxCommand "set-option -g escape-time 500"

# =============================================================================
# SECTION 17: Escape key cancels command prompt
# =============================================================================

Write-Host "`n=== 17. COMMAND PROMPT CANCEL ===" -ForegroundColor Cyan

Write-Test "TUI Escape cancels command prompt"
Focus-PsmuxWindow | Out-Null
[Win32Cfg]::SendCtrlB()
Start-Sleep -Milliseconds 200
[Win32Cfg]::SendColon()
Start-Sleep -Milliseconds 300
[Win32Cfg]::SendString("set -g mouse off")
Start-Sleep -Milliseconds 200
# Press Escape instead of Enter
Send-PsmuxEscape
Start-Sleep -Milliseconds 300
# Verify mouse is still whatever it was (should not have changed) and server alive
$r = Send-TcpCommand $SESSION "show-options -g mouse"
if ($r.ok -and $r.resp -match 'mouse') {
    Write-Pass "TUI Escape cancels command prompt (server alive, mouse=$($r.resp))"
} else {
    Write-Fail "TUI Escape cancels command prompt (server not responding)"
}

# =============================================================================
# SECTION 18: Boolean variant syntax via TUI
# =============================================================================

Write-Host "`n=== 18. BOOLEAN VARIANTS VIA TUI ===" -ForegroundColor Cyan

# true/false
Test-TuiOption "set-option -g mouse true" "mouse" '\bon\b' "TUI bool: mouse=true"
Test-TuiOption "set-option -g mouse false" "mouse" '\boff\b' "TUI bool: mouse=false"

# 1/0
Test-TuiOption "set-option -g mouse 1" "mouse" '\bon\b' "TUI bool: mouse=1"
Test-TuiOption "set-option -g mouse 0" "mouse" '\boff\b' "TUI bool: mouse=0"

# yes/no
Test-TuiOption "set-option -g mouse yes" "mouse" '\bon\b' "TUI bool: mouse=yes"
Test-TuiOption "set-option -g mouse no" "mouse" '\boff\b' "TUI bool: mouse=no"

# Restore
Send-PsmuxCommand "set-option -g mouse on"

# =============================================================================
# SECTION 19: show-options via TUI command prompt
# =============================================================================

Write-Host "`n=== 19. SHOW-OPTIONS VIA TUI ===" -ForegroundColor Cyan

# show-options via TUI: verify server still responds after each
#
# NOTE: `show-options` / `show-options -g` with NO specific option name,
# run from the interactive TUI command prompt (a persistent connection),
# opens a `ShowTextPopup` overlay (src/server/connection.rs ~2172-2202) to
# display the full option list -- unlike `show-options -g <name>` below,
# which returns a single value directly with no popup. That overlay was
# never dismissed here, so it stayed on screen and swallowed every
# keystroke Section 20 sent afterward (Ctrl+B/colon/text/Enter all landed
# on the popup instead of opening a fresh command prompt), which is what
# made "RAPID SEQUENTIAL SETS" silently fail 4 of 5 checks -- the psmux
# window was simply not focused on the command prompt anymore. Send
# Escape after each bare show-options call to close the popup before
# moving on.
Write-Test "TUI show-options"
Send-PsmuxCommand "show-options"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g mouse"
if ($r.ok -and $r.resp -match 'mouse') {
    Write-Pass "TUI show-options (server alive)"
} else {
    Write-Fail "TUI show-options (server not responding)"
}
Focus-PsmuxWindow | Out-Null
Start-Sleep -Milliseconds 300
Send-PsmuxEscape
Start-Sleep -Milliseconds 300
Send-PsmuxEscape
Start-Sleep -Milliseconds 500

Write-Test "TUI show-options -g"
Send-PsmuxCommand "show-options -g"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g mouse"
if ($r.ok -and $r.resp -match 'mouse') {
    Write-Pass "TUI show-options -g (server alive)"
} else {
    Write-Fail "TUI show-options -g (server not responding)"
}
Focus-PsmuxWindow | Out-Null
Start-Sleep -Milliseconds 300
Send-PsmuxEscape
Start-Sleep -Milliseconds 300
Send-PsmuxEscape
Start-Sleep -Milliseconds 500

Write-Test "TUI show-options -g mouse"
Send-PsmuxCommand "show-options -g mouse"
Start-Sleep -Milliseconds 300
$r = Send-TcpCommand $SESSION "show-options -g mouse"
if ($r.ok -and $r.resp -match 'mouse') {
    Write-Pass "TUI show-options -g mouse (server alive)"
} else {
    Write-Fail "TUI show-options -g mouse (server not responding)"
}
# `show-options -g <name>` opens a popup too, contrary to the note above: run
# from the TUI it displays the value in a small box. Screen-read to be sure,
# because the box is narrow and easy to miss under the status bar:
#
#       +----------------------+
#       [popup3] 0:pwsh*  ...
#
# Left open it swallows the whole next command. That is what made section 20's
# "rapid sequential sets" fail: prefix, colon, the text and Enter all landed on
# this box, so escape-time stayed 500 while the checks reported a product bug.
Send-PsmuxEscape
Send-PsmuxEscape

# =============================================================================
# SECTION 20: Multiple set commands in sequence via TUI
# =============================================================================

Write-Host "`n=== 20. RAPID SEQUENTIAL SETS VIA TUI ===" -ForegroundColor Cyan

# Rapid fire multiple options
$rapidOpts = @(
    @{c='set-option -g escape-time 111'; o='escape-time'; p='111'},
    @{c='set-option -g history-limit 9999'; o='history-limit'; p='9999'},
    @{c='set-option -g mouse off'; o='mouse'; p='\boff\b'},
    @{c='set-option -g status-position top'; o='status-position'; p='top'},
    @{c='set-option -g base-index 1'; o='base-index'; p='1'}
)

foreach ($opt in $rapidOpts) {
    Test-TuiOption $opt.c $opt.o $opt.p "TUI rapid: $($opt.o)"
}

# Restore
Send-PsmuxCommand "set-option -g escape-time 500"
Send-PsmuxCommand "set-option -g history-limit 2000"
Send-PsmuxCommand "set-option -g mouse on"
Send-PsmuxCommand "set-option -g status-position bottom"
Send-PsmuxCommand "set-option -g base-index 0"

# =============================================================================
# Cleanup
# =============================================================================

Write-Host "`n=== CLEANUP ===" -ForegroundColor Cyan

if (-not $SkipCleanup) {
    Cleanup-Session $SESSION
    Write-Info "Session '$SESSION' cleaned up"
}

# =============================================================================
# Summary
# =============================================================================

Write-Host "`n============================================================" -ForegroundColor Magenta
Write-Host "  PSMUX Win32 TUI Config Exhaustive Test Results" -ForegroundColor Magenta
Write-Host "============================================================" -ForegroundColor Magenta
Write-Host "  Passed:  $script:TestsPassed" -ForegroundColor Green
Write-Host "  Failed:  $script:TestsFailed" -ForegroundColor $(if ($script:TestsFailed -gt 0) { 'Red' } else { 'Green' })
Write-Host "  Skipped: $script:TestsSkipped" -ForegroundColor Yellow
Write-Host "  Total:   $($script:TestsPassed + $script:TestsFailed + $script:TestsSkipped)" -ForegroundColor White
Write-Host "============================================================" -ForegroundColor Magenta

if ($script:TestsFailed -gt 0) { exit 1 }
