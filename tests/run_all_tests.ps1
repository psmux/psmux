# psmux Comprehensive Test Runner
# Runs ALL test suites sequentially with proper cleanup, captures results,
# and produces a full report including performance metrics.
#
# Usage: pwsh -NoProfile -ExecutionPolicy Bypass -File tests\run_all_tests.ps1

param(
    [switch]$SkipPerf,       # Skip long-running perf/stress tests
    [switch]$PerfOnly,       # Run only the perf group ($perfTests), then print the trend
    [switch]$IncludeWSL,     # Include WSL-dependent tests
    [switch]$IncludeInteractive, # Include tests that need interactive TUI
    [int]$DefaultTimeoutSec = 240,  # Per-suite timeout (normal suites)
    [int]$LongTimeoutSec = 900,     # Per-suite timeout (perf/stress/latency suites)
    [string]$Only,           # Regex filter: run only suites whose name matches
    [switch]$Resume,         # Continue the latest run: skip suites that already have a result
    [string]$TestDir         # Override test directory (default: this script's folder)
)

# ── Safety gate: this runner is DESTRUCTIVE to a live psmux ──────────────────
# Before every test it kills ALL psmux processes (by image name) and deletes
# ~/.psmux\*.port, *.key and ~/.psmux.conf / ~/.psmuxrc. That is fine in a
# throwaway sandbox (the Docker dev image / CI) but would wipe a real user's
# running sessions and config. Refuse to run unless the caller has explicitly
# confirmed a sandbox by setting PSMUX_TEST_SANDBOX=1.
if ($env:PSMUX_TEST_SANDBOX -ne '1') {
    Write-Host ''
    Write-Host 'REFUSING TO RUN: this test runner is destructive to a live psmux.' -ForegroundColor Red
    Write-Host 'Between tests it kills ALL psmux processes and deletes' -ForegroundColor Yellow
    Write-Host '~/.psmux\*.port, *.key and ~/.psmux.conf / ~/.psmuxrc.' -ForegroundColor Yellow
    Write-Host ''
    Write-Host 'Run it only in a throwaway/sandbox environment (e.g. the Docker dev' -ForegroundColor Yellow
    Write-Host 'image, which sets this automatically). To confirm a sandbox and run:' -ForegroundColor Yellow
    Write-Host '    $env:PSMUX_TEST_SANDBOX = "1"; pwsh -File tests\run_all_tests.ps1' -ForegroundColor Cyan
    Write-Host ''
    exit 2
}

$ErrorActionPreference = "Continue"
$startTime = Get-Date

# NO_COLOR poisons every color assertion in the tree: psmux honors it and
# strips SGR, so a runner launched from a shell that sets it (AI agent tool
# shells do) fakes a machine-wide "psmux drops all colour" regression across
# the issue2/263/425/451 families. Proven 2026-08-05: 12 suites red with it,
# all green without, identical binary. Scrub it for this process and every
# suite we spawn.
Remove-Item Env:NO_COLOR -ErrorAction SilentlyContinue

# Suppress Windows hard-error popups for this process and EVERYTHING it
# spawns (the error mode is inherited through CreateProcess). The runner and
# the suites kill whole process trees constantly, and a console child caught
# mid-initialization while its console/job is being torn down dies with
# STATUS_DLL_INIT_FAILED (0xc0000142). Without this, each such death posts a
# MODAL "pwsh.exe - Application Error" dialog to the desktop; Windows queues
# them, so they keep resurfacing one OK-click at a time for hours after a
# sweep. Measured 2026-08-21: every 0xc0000142 popup in a 7-day window fell
# inside a full-sweep run (bursts of 10-12 around the codex-suite tree
# kills), zero outside them.
#   SEM_FAILCRITICALERRORS (0x1) | SEM_NOGPFAULTERRORBOX (0x2) |
#   SEM_NOOPENFILEERRORBOX (0x8000)
Add-Type -Name ErrMode -Namespace PsmuxRunner -MemberDefinition `
    '[DllImport("kernel32.dll")] public static extern uint SetErrorMode(uint uMode);'
[void][PsmuxRunner.ErrMode]::SetErrorMode(0x1 -bor 0x2 -bor 0x8000)

# ── Abort channel: stop a run WITHOUT needing the runner window ──────────────
#
# WHY A FILE AND NOT JUST Ctrl+C: the suites launch attached psmux clients in
# their own consoles, and those windows TAKE THE FOREGROUND within seconds of a
# run starting. Measured 2026-08-26 by sampling GetForegroundWindow every 300ms
# across a -Only tui_proof run: the runner console held focus for 2 seconds, a
# psmux.exe window took it, and the runner never got it back. Ctrl+C is only
# delivered to the console that HAS focus, so for most of a multi-hour run the
# runner console is simply not reachable from the keyboard. The only exit left
# was killing pwsh from Task Manager, which skips the summary and strands every
# psmux server the in-flight suite had started (measured: 3 orphans).
#
# So the abort signal is a FILE any other shell can create:
#     tests\stop_tests.cmd     (or: New-Item $env:TEMP\psmux-teststop.flag)
# It is polled once a second inside the per-suite wait loop and again before
# each suite starts, so an abort lands within ~1s even in the middle of a 900s
# perf suite, and nothing further is started.
#
# Ctrl+C still works when the window IS reachable, but it is now routed through
# the same flag instead of killing the process. The native handler installed
# here runs BEFORE PowerShell's own (handlers fire in reverse registration
# order) and returns TRUE to swallow the event, so the runner survives long
# enough to kill the suite's process tree, tear down psmux and print the report
# for everything that did run.
$script:StopFile = Join-Path $env:TEMP "psmux-teststop.flag"

# A stop flag left behind by a previous abort would kill this run on its first
# poll. Clear it before the handler can ever look at it.
Remove-Item $script:StopFile -Force -ErrorAction SilentlyContinue

Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;

public static class PsmuxTestAbort {
    delegate bool HandlerRoutine(uint ctrlType);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetConsoleCtrlHandler(HandlerRoutine handler, bool add);

    // The delegate MUST stay rooted. Windows calls it from a thread it injects
    // into this process; if the GC collects it first the call lands on freed
    // memory and the runner dies with an access violation instead of aborting.
    static HandlerRoutine _handler;
    static string _flagFile;

    public static volatile bool Requested;
    public static string Reason = "";

    public static void Install(string flagFile) {
        _flagFile = flagFile;
        _handler = new HandlerRoutine(OnCtrl);
        SetConsoleCtrlHandler(_handler, true);
    }

    static bool OnCtrl(uint t) {
        // 0 = CTRL_C, 1 = CTRL_BREAK, 2 = CTRL_CLOSE, 5 = LOGOFF, 6 = SHUTDOWN
        Reason = (t == 1) ? "Ctrl+Break" : "Ctrl+C";
        Requested = true;
        // Mirror to the flag file so the abort survives even if this handler
        // races the main thread, and so a watcher can see the run is stopping.
        try { File.WriteAllText(_flagFile, Reason); } catch { }
        // Swallow C/BREAK so the runner can clean up. CLOSE/LOGOFF/SHUTDOWN are
        // not ours to veto: Windows kills us shortly after regardless.
        return (t == 0 || t == 1);
    }
}
'@ -ErrorAction SilentlyContinue

[PsmuxTestAbort]::Install($script:StopFile)

$script:AbortReason = $null

# Returns the abort reason, or $null when the run should continue.
function Test-AbortRequested {
    if ($script:AbortReason) { return $script:AbortReason }
    if ([PsmuxTestAbort]::Requested) { return [PsmuxTestAbort]::Reason }
    if (Test-Path $script:StopFile) {
        $why = ""
        try { $why = (Get-Content $script:StopFile -Raw -ErrorAction SilentlyContinue) } catch {}
        if ($why) { $why = $why.Trim() }
        if (-not $why) { $why = "stop file" }
        return $why
    }
    return $null
}

# ── Logging setup ──────────────────────────────────────────────
# All logs go to $env:TEMP\psmux-test-logs\ (never inside the repo).
# Each run gets a timestamped folder with:
#   progress.log   – one-line-per-suite result, flushed immediately (crash-safe)
#   summary.log    – final report (written at end)
#   suites\<name>.log – full stdout/stderr captured from each test file
$script:LogRoot = Join-Path $env:TEMP "psmux-test-logs"
$script:RunId   = $startTime.ToString("yyyy-MM-dd_HH-mm-ss")
$latestFile = Join-Path $script:LogRoot "latest_run.txt"

# -Resume: continue the most recent run instead of starting a new one.
# Suites that already have a line in results.jsonl are skipped.
$script:CompletedSuites = @{}
if ($Resume -and (Test-Path $latestFile)) {
    $prevId = (Get-Content $latestFile -Raw).Trim()
    $prevDir = Join-Path $script:LogRoot $prevId
    if (Test-Path (Join-Path $prevDir "results.jsonl")) {
        $script:RunId = $prevId
        foreach ($line in (Get-Content (Join-Path $prevDir "results.jsonl"))) {
            try {
                $r = $line | ConvertFrom-Json
                $script:CompletedSuites[$r.Name] = $r
            } catch {}
        }
    }
}

$script:RunDir  = Join-Path $script:LogRoot $script:RunId
$script:SuiteDir = Join-Path $script:RunDir "suites"
New-Item -ItemType Directory -Path $script:SuiteDir -Force | Out-Null

$script:ProgressLog = Join-Path $script:RunDir "progress.log"
$script:SummaryLog  = Join-Path $script:RunDir "summary.log"
$script:ResultsJsonl = Join-Path $script:RunDir "results.jsonl"

Set-Content -Path $latestFile -Value $script:RunId -Encoding UTF8

function Write-Log {
    param([string]$Message, [string]$File = $script:ProgressLog)
    $ts = Get-Date -Format "yyyy-MM-dd HH:mm:ss.fff"
    $line = "[$ts] $Message"
    # Append + flush immediately so partial results survive crashes/power loss
    [System.IO.File]::AppendAllText($File, "$line`r`n")
}

Write-Log "=== psmux test run started ==="
Write-Log "Run ID: $script:RunId"
Write-Log "Log directory: $script:RunDir"
if ($script:CompletedSuites.Count -gt 0) {
    Write-Log "Resuming: $($script:CompletedSuites.Count) suites already have results and will be skipped"
}

# ── Windows Job Object: guarantees the WHOLE process tree of a test dies ─────
# Why: the old Start-Job pattern hung FOREVER when a test left behind a child
# that inherited stdout (Stop-Job blocks on the pipe), and orphaned children
# survived across suites. A job object with KILL_ON_JOB_CLOSE kills every
# descendant (even orphans whose parent already exited) in one call.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class PsmuxTestJob {
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern IntPtr CreateJobObject(IntPtr lpJobAttributes, string lpName);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool SetInformationJobObject(IntPtr hJob, int infoClass, IntPtr lpInfo, uint cbInfoLength);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool AssignProcessToJobObject(IntPtr hJob, IntPtr hProcess);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool TerminateJobObject(IntPtr hJob, uint uExitCode);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool CloseHandle(IntPtr hObject);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern IntPtr OpenProcess(uint access, bool inherit, int pid);

    [StructLayout(LayoutKind.Sequential)]
    struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
        public long PerProcessUserTimeLimit;
        public long PerJobUserTimeLimit;
        public uint LimitFlags;
        public UIntPtr MinimumWorkingSetSize;
        public UIntPtr MaximumWorkingSetSize;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass;
        public uint SchedulingClass;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct IO_COUNTERS {
        public ulong ReadOperationCount;
        public ulong WriteOperationCount;
        public ulong OtherOperationCount;
        public ulong ReadTransferBytes;
        public ulong WriteTransferBytes;
        public ulong OtherTransferBytes;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
        public IO_COUNTERS IoInfo;
        public UIntPtr ProcessMemoryLimit;
        public UIntPtr JobMemoryLimit;
        public UIntPtr PeakProcessMemoryUsed;
        public UIntPtr PeakJobMemoryUsed;
    }

    const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x2000;
    const int  JobObjectExtendedLimitInformation  = 9;
    const uint PROCESS_SET_QUOTA_AND_TERMINATE    = 0x0100 | 0x0001;

    public static IntPtr Create() {
        IntPtr job = CreateJobObject(IntPtr.Zero, null);
        if (job == IntPtr.Zero) return IntPtr.Zero;
        var info = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        int len = Marshal.SizeOf(info);
        IntPtr buf = Marshal.AllocHGlobal(len);
        try {
            Marshal.StructureToPtr(info, buf, false);
            if (!SetInformationJobObject(job, JobObjectExtendedLimitInformation, buf, (uint)len)) {
                CloseHandle(job);
                return IntPtr.Zero;
            }
        } finally { Marshal.FreeHGlobal(buf); }
        return job;
    }

    public static bool Assign(IntPtr job, int pid) {
        IntPtr h = OpenProcess(PROCESS_SET_QUOTA_AND_TERMINATE, false, pid);
        if (h == IntPtr.Zero) return false;
        bool ok = AssignProcessToJobObject(job, h);
        CloseHandle(h);
        return ok;
    }

    // Terminate every process in the job, then release it.
    public static void Kill(IntPtr job) {
        if (job == IntPtr.Zero) return;
        TerminateJobObject(job, 0xDEAD);
        CloseHandle(job);
    }
}
'@ -ErrorAction SilentlyContinue

# ── Desktop access witness ──
# Sweep 2026-09-24_12-19-22: from 13:50 to about 14:40 every process in the
# runner's lineage was refused OpenClipboard (ERROR_ACCESS_DENIED, even on a
# freshly created private window station, so no process was holding the
# clipboard) and could not take the foreground, while a process started by the
# Task Scheduler at the same time could do both. Six paste/focus suites scored
# FAIL on it. psmux was not the cause (its clipboard calls are paired, and the
# refusal outlived every psmux process). The refusal lifted on its own.
# Whatever imposes it, the sweep must SAY it happened on the result line, and
# record when it began and ended, instead of scoring it as six product bugs.
# The probe is one OpenClipboard(NULL)/CloseClipboard pair and one
# GetForegroundWindow: no side effects, microseconds, safe between suites.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class PsmuxUiAccess {
    [DllImport("user32.dll", SetLastError=true)] static extern bool OpenClipboard(IntPtr h);
    [DllImport("user32.dll")] static extern bool CloseClipboard();
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    // "" when the desktop is usable, otherwise a short reason.
    public static string Probe() {
        bool ok = OpenClipboard(IntPtr.Zero);
        int err = ok ? 0 : Marshal.GetLastWin32Error();
        if (ok) CloseClipboard();
        string r = "";
        if (!ok) r += "clipboard-denied(err=" + err + ")";
        if (GetForegroundWindow() == IntPtr.Zero) r += (r.Length > 0 ? " " : "") + "no-foreground";
        return r;
    }
}
'@ -ErrorAction SilentlyContinue

$script:UiAccessLog   = Join-Path $script:RunDir "ui_access.log"
$script:UiAccessState = ""
function Test-UiAccess {
    param([string]$When)
    $r = ""
    try { $r = [PsmuxUiAccess]::Probe() } catch { return "" }
    if ($r -ne $script:UiAccessState) {
        $line = "[{0}] {1}: {2}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss.fff'), $When, $(if ($r) { "DESKTOP REFUSED $r" } else { "desktop usable again" })
        try { [System.IO.File]::AppendAllText($script:UiAccessLog, "$line`r`n") } catch { }
        if ($r) { Write-Log "UI-ACCESS $When $r (this lineage cannot use the clipboard or take focus; paste and focus suites will fail for it)" }
        else    { Write-Log "UI-ACCESS $When restored" }
        $script:UiAccessState = $r
    }
    return $r
}

# ── Antivirus: harness binaries quarantined while a suite runs ───────────────
#
# Sweep 2026-10-01_01-53-14: Defender cloud protection issued a FastPath
# verdict (Trojan:Win32/Bearfoos.A!ml) for three freshly compiled copies of
# tests\injector.cs and quarantined each in the middle of its suite (588, 596,
# 612). The suites read the untouched panes as psmux regressions, and the
# -Only rerun passed because the verdict had moved on. Every Defender
# detection (event 1116) inside a suite's window is now written to
# av_detections.log and returned as "<threat> <file>" for the result line.
$script:AvLog = Join-Path $script:RunDir "av_detections.log"
function Get-AvDetections {
    param([datetime]$Since, [string]$Suite)
    $out = @()
    try {
        $ev = Get-WinEvent -ErrorAction Stop -FilterHashtable @{
            LogName = 'Microsoft-Windows-Windows Defender/Operational'; Id = 1116; StartTime = $Since }
    } catch { return @() }   # none found, or the log is not readable here
    foreach ($e in ($ev | Sort-Object TimeCreated)) {
        $name = ([regex]'Name:\s*(\S+)').Match($e.Message).Groups[1].Value
        $path = ([regex]'(?m)^\s*Path:\s*(?:file:_)?(.+?)\s*$').Match($e.Message).Groups[1].Value
        if (-not $path) { $path = '?' }
        $line = "[{0}] {1}: {2} {3}" -f $e.TimeCreated.ToString('yyyy-MM-dd HH:mm:ss'), $Suite, $name, $path
        try { [System.IO.File]::AppendAllText($script:AvLog, "$line`r`n") } catch { }
        $out += ("{0} {1}" -f $name, (Split-Path $path -Leaf))
    }
    $out = @($out | Select-Object -Unique)
    if ($out.Count) { $null = Write-Log "AV-DETECTION $Suite $($out -join ', ') (a harness file was quarantined; this suite's verdict is not about psmux)" }
    return $out
}

# ── Desktop hygiene: console windows a suite leaves behind ───────────────────
#
# THE FAILURE THIS EXISTS FOR
# After three full sweeps the desktop held about 33 Windows Terminal windows,
# most with six dead tabs each, hosting 121 processes whose command line was
# exactly `C:\WINDOWS\system32\cmd.exe /c pause`, every one of them parked at
# "Press any key to continue", plus the launcher windows sitting at their own
# final pause. On Windows 11 every new console is delegated to Windows Terminal,
# so each of those is a visible tab or window a human has to click away.
#
# They are not descendants of any suite: the runner puts each suite in a Job
# Object with KILL_ON_JOB_CLOSE, and the job kill had already run. Anything
# created outside that job (a process that breaks away, or one whose PTY host
# died and left the child orphaned on a real console) survives it. The root
# cause of that particular batch was traced to a Rust test helper that spawned
# `cmd.exe /c pause` dummies under a pseudoconsole and relied on the PTY master
# drop to end them, which it does not, but the runner must not depend on knowing
# the culprit: ANY suite that leaks a console window is a defect, and the run has
# to say which suite did it rather than leaving a pile of anonymous dead tabs.
#
# SO THE RULE IS: a suite may open windows, but it must close them. After each
# suite the runner diffs the desktop against a snapshot taken before it, logs
# what is new with its parent chain, ends it, and closes the window.
#
# WHY A PROCESS SNAPSHOT *AND* A WINDOW SNAPSHOT
# A Windows Terminal window hosts many tabs and exposes ONE window handle, so a
# handle cannot be mapped back to the tab's process. The process snapshot is what
# catches the carrier; the window snapshot is what catches a window whose process
# is already gone (a dead tab) and proves the desktop is actually back to
# baseline. Neither alone is sufficient.
#
# WHAT IS NEVER TOUCHED
#   - every pid in this runner's own ancestry, and the pid that owns this
#     console's window (killing that is how a sweep used to take down the
#     terminal it was launched from),
#   - every window handle that already existed when the run started,
#   - any window whose title matches $script:AuditProtectTitles, and any handle
#     listed in PSMUX_AUDIT_PROTECT_HWND (a comma separated list), which is how a
#     caller pins the window its own session lives in,
#   - WindowsTerminal.exe is never killed as a process. It hosts tabs that are
#     not ours; its windows are closed politely instead.
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class PsmuxWinAudit {
    delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowsProc cb, IntPtr p);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder sb, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder sb, int n);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("kernel32.dll")] static extern IntPtr GetConsoleWindow();

    const uint WM_CLOSE = 0x0010;

    public class Win {
        public long Handle;
        public int  Pid;
        public string Class;
        public string Title;
    }

    // Visible top level console hosts only. CASCADIA_HOSTING_WINDOW_CLASS is a
    // Windows Terminal window (what a delegated console becomes on Windows 11);
    // ConsoleWindowClass is a classic conhost window (what you still get when
    // defterm delegation does not apply, for example for an elevated child).
    public static List<Win> List() {
        var res = new List<Win>();
        EnumWindows((h, l) => {
            if (!IsWindowVisible(h)) { return true; }
            var cn = new StringBuilder(256);
            GetClassNameW(h, cn, cn.Capacity);
            string c = cn.ToString();
            if (c != "CASCADIA_HOSTING_WINDOW_CLASS" && c != "ConsoleWindowClass") { return true; }
            var tb = new StringBuilder(512);
            GetWindowTextW(h, tb, tb.Capacity);
            uint pid;
            GetWindowThreadProcessId(h, out pid);
            res.Add(new Win { Handle = h.ToInt64(), Pid = (int)pid, Class = c, Title = tb.ToString() });
            return true;
        }, IntPtr.Zero);
        return res;
    }

    // PostMessage, not SendMessage: a window whose owning process is wedged (or
    // already gone, leaving a zombie tab) would block a synchronous send for ever
    // and hang the runner between suites.
    public static bool Close(long handle) {
        IntPtr h = new IntPtr(handle);
        if (!IsWindow(h)) { return false; }
        return PostMessageW(h, WM_CLOSE, IntPtr.Zero, IntPtr.Zero);
    }

    public static bool Alive(long handle) {
        IntPtr h = new IntPtr(handle);
        return IsWindow(h) && IsWindowVisible(h);
    }

    // NOT sufficient on its own to identify the runner's own window, see
    // FindByTitle. Kept because it is the right answer for a classic conhost.
    public static long OwnConsoleWindow() { return GetConsoleWindow().ToInt64(); }

    public static int OwnerPid(long handle) {
        uint pid;
        GetWindowThreadProcessId(new IntPtr(handle), out pid);
        return (int)pid;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool SetConsoleTitleW(string title);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern uint GetConsoleTitleW(StringBuilder buf, uint size);

    public static bool SetTitle(string t) { return SetConsoleTitleW(t); }
    public static string GetTitle() {
        var sb = new StringBuilder(1024);
        GetConsoleTitleW(sb, (uint)sb.Capacity);
        return sb.ToString();
    }

    // Find the visible console host window(s) whose title carries a marker.
    //
    // WHY THIS EXISTS RATHER THAN JUST GetConsoleWindow()
    // On Windows 11 a console is delegated to Windows Terminal, and
    // GetConsoleWindow() then returns the handle of the INVISIBLE pseudoconsole
    // host window, not the visible CASCADIA_HOSTING_WINDOW_CLASS window the human
    // sees. Pinning that handle protects nothing. Pid does not help either: one
    // WindowsTerminal.exe owns many windows, so the runner's own window and a
    // window a suite leaked can share an owner pid (observed: both were pid
    // 13928). The only thing that distinguishes them is the title, and the title
    // is something the runner can set itself, which turns a guess into an
    // identity: stamp a unique marker with SetConsoleTitle, then find the window
    // that is showing it.
    public static List<long> FindByTitle(string marker) {
        var res = new List<long>();
        EnumWindows((h, l) => {
            if (!IsWindowVisible(h)) { return true; }
            var cn = new StringBuilder(256);
            GetClassNameW(h, cn, cn.Capacity);
            string c = cn.ToString();
            if (c != "CASCADIA_HOSTING_WINDOW_CLASS" && c != "ConsoleWindowClass") { return true; }
            var tb = new StringBuilder(512);
            GetWindowTextW(h, tb, tb.Capacity);
            if (tb.ToString().IndexOf(marker, StringComparison.Ordinal) >= 0) { res.Add(h.ToInt64()); }
            return true;
        }, IntPtr.Zero);
        return res;
    }
}
'@ -ErrorAction SilentlyContinue

# Titles that are never closed no matter what. The first is the convention for
# the window a human (or an agent session) is working in; the second is this
# runner's own launcher window, which legitimately lives through the whole run.
$script:AuditProtectTitles = @('Shell prompt loading performance', 'psmux FULL test suite')

$script:AuditProtectHwnd = @{}
if ($env:PSMUX_AUDIT_PROTECT_HWND) {
    foreach ($tok in ($env:PSMUX_AUDIT_PROTECT_HWND -split '[,; ]+')) {
        $v = 0L
        if ([long]::TryParse($tok.Trim(), [ref]$v) -and $v -ne 0) { $script:AuditProtectHwnd[$v] = $true }
    }
}

# Every pid from this process up to the session root, plus whoever owns this
# console's window. An earlier cleanup in this repo killed the Windows Terminal
# tab that was hosting the session doing the cleaning; walking our own ancestry
# once, up front, is what makes that impossible here.
$script:AuditOwnPids = @{}
try {
    $cur = $PID
    for ($i = 0; $i -lt 16 -and $cur -gt 0; $i++) {
        if ($script:AuditOwnPids.ContainsKey($cur)) { break }
        $script:AuditOwnPids[$cur] = $true
        $ci = Get-CimInstance -Query "SELECT ParentProcessId FROM Win32_Process WHERE ProcessId=$cur" -ErrorAction SilentlyContinue
        if (-not $ci) { break }
        $cur = [int]$ci.ParentProcessId
    }
} catch { }
try {
    $ownWin = [PsmuxWinAudit]::OwnConsoleWindow()
    if ($ownWin -ne 0) {
        $script:AuditProtectHwnd[$ownWin] = $true
        $op = [PsmuxWinAudit]::OwnerPid($ownWin)
        if ($op -gt 0) { $script:AuditOwnPids[$op] = $true }
    }
} catch { }

# Images that can own a console window, or be the thing parked inside one.
# WindowsTerminal.exe is in the snapshot so a NEW terminal window is noticed, but
# it is on the never-kill list below.
$script:AuditImageFilter = "Name='cmd.exe' OR Name='conhost.exe' OR Name='OpenConsole.exe' OR Name='pwsh.exe' OR Name='powershell.exe' OR Name='WindowsTerminal.exe'"
$script:AuditNeverKill = @('WindowsTerminal.exe')

$script:AuditBaselineWins = @{}
$script:AuditTotalLeftovers = 0
$script:AuditSuitesWithLeftovers = 0
$script:AuditLog = Join-Path $script:RunDir "leftovers.log"

function New-AuditSnapshot {
    $procs = @{}
    try {
        foreach ($p in (Get-CimInstance -Query "SELECT ProcessId FROM Win32_Process WHERE $script:AuditImageFilter" -ErrorAction SilentlyContinue)) {
            $procs[[int]$p.ProcessId] = $true
        }
    } catch { }
    $wins = @{}
    try { foreach ($w in [PsmuxWinAudit]::List()) { $wins[$w.Handle] = $w.Title } } catch { }
    return @{ Procs = $procs; Wins = $wins }
}

function Test-AuditWindowProtected {
    param($W)
    if ($script:AuditProtectHwnd.ContainsKey([long]$W.Handle)) { return $true }
    if ($script:AuditBaselineWins.ContainsKey([long]$W.Handle)) { return $true }
    # A window owned by this runner's own ancestry is never ours to close, even
    # when the handle is new. Pass 1 has always honoured $script:AuditOwnPids for
    # PROCESSES and the header above promises the same for windows, but this
    # function did not consult it, and the gap is reachable in the normal case:
    # launching the runner from a Windows Terminal session opens a NEW tab inside
    # the SAME WindowsTerminal.exe that hosts the caller, so its window handle is
    # new while its owner is our own terminal. Measured 2026-09-16 12:37 in run
    # 2026-09-16_12-24-28: "LEFTOVER-WINDOW test_perf_vs_terminals hwnd=24842416
    # pid=5548 class=CASCADIA_HOSTING_WINDOW_CLASS title=[Administrator: cmd]",
    # where pid 5548 was the Windows Terminal hosting the session that started
    # the run, and pass 2 posted WM_CLOSE to it and then went looking for the
    # "Close all" confirmation. Closing the terminal a sweep was launched from is
    # exactly the incident this audit exists to prevent.
    if ($script:AuditOwnPids.ContainsKey([int]$W.Pid)) { return $true }
    foreach ($t in $script:AuditProtectTitles) {
        if ($W.Title -and $W.Title.Contains($t)) { return $true }
    }
    return $false
}

# Windows Terminal refuses to close a window with several tabs without asking
# first ("Do you want to close all tabs?"). That dialog is a XAML popup with no
# window handle of its own to post to, so the only way past it is the accessibility
# tree: the confirm button has AutomationId PrimaryButton and Name "Close all".
function Invoke-WtCloseAllDialog {
    param([int]$OwnerPid)
    try {
        Add-Type -AssemblyName UIAutomationClient -ErrorAction Stop
        Add-Type -AssemblyName UIAutomationTypes -ErrorAction Stop
    } catch { return $false }
    try {
        $root = [System.Windows.Automation.AutomationElement]::RootElement
        $byPid = New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $OwnerPid)
        $wins = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $byPid)
        foreach ($w in $wins) {
            $byId = New-Object System.Windows.Automation.PropertyCondition(
                [System.Windows.Automation.AutomationElement]::AutomationIdProperty, 'PrimaryButton')
            $btns = $w.FindAll([System.Windows.Automation.TreeScope]::Descendants, $byId)
            foreach ($b in $btns) {
                if ($b.Current.Name -match 'Close all') {
                    $pat = $b.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
                    $pat.Invoke()
                    return $true
                }
            }
        }
    } catch { }
    return $false
}

function Write-AuditLine {
    param([string]$Line)
    Write-Log $Line
    [System.IO.File]::AppendAllText($script:AuditLog, "[$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss.fff')] $Line`r`n")
}

# Diff the desktop against $Before, attribute what is new to $Suite, end it and
# close its window. Returns the number of leftovers found (0 when clean).
#
# Called AFTER the suite has exited and after its job object was torn down, which
# is deliberately after the suite's own cleanup: suites such as
# test_perf_vs_terminals legitimately open Windows Terminal, WezTerm and
# Alacritty windows and audit/close them themselves, and this must only ever
# report what is STILL there once they are done.
function Invoke-LeftoverAudit {
    param([string]$Suite, $Before, [datetime]$SuiteStart)

    if (-not $Before) { return 0 }
    # Let windows the suite itself closed finish going away, so its own cleanup is
    # never miscounted as a leak.
    Start-Sleep -Milliseconds 600

    $found = 0

    # Which handles are new? Their owning pids are the candidates that hold a
    # visible console, including ones whose command line looks innocent.
    $newWinPids = @{}
    $newWins = @()
    try {
        foreach ($w in [PsmuxWinAudit]::List()) {
            if ($Before.Wins.ContainsKey([long]$w.Handle)) { continue }
            if (Test-AuditWindowProtected $w) { continue }
            $newWins += $w
            $newWinPids[[int]$w.Pid] = $true
        }
    } catch { }

    # ── pass 1: new console processes ────────────────────────────────────────
    $killed = @()
    try {
        $now = Get-CimInstance -Query "SELECT ProcessId,ParentProcessId,Name,CommandLine,CreationDate FROM Win32_Process WHERE $script:AuditImageFilter" -ErrorAction SilentlyContinue
        foreach ($p in $now) {
            $procId = [int]$p.ProcessId
            if ($Before.Procs.ContainsKey($procId)) { continue }       # was already there
            if ($script:AuditOwnPids.ContainsKey($procId)) { continue } # our own ancestry
            # Belt and braces against pid reuse: a process that started before the
            # suite did cannot be the suite's leftover.
            try { if ($p.CreationDate -and $p.CreationDate -lt $SuiteStart.AddSeconds(-2)) { continue } } catch { }

            $cl = (([string]$p.CommandLine) -replace '\s+', ' ').Trim()
            $isParked = ($p.Name -eq 'cmd.exe') -and ($cl -match '/c\s+pause\b' -or $cl -match '/k(\s|$)')
            $ownsWin  = $newWinPids.ContainsKey($procId)
            if (-not ($isParked -or $ownsWin)) { continue }

            # Parent chain for attribution. Usually already dead by now, which is
            # exactly why spawn_trace.log (captured at spawn time) exists too.
            $chain = @()
            $cur = [int]$p.ParentProcessId
            for ($i = 0; $i -lt 3 -and $cur -gt 0; $i++) {
                $pf = Get-CimInstance -Query "SELECT ParentProcessId,Name FROM Win32_Process WHERE ProcessId=$cur" -ErrorAction SilentlyContinue
                if (-not $pf) { $chain += "pid=$cur(<exited>)"; break }
                $chain += ("pid={0}({1})" -f $cur, $pf.Name)
                $cur = [int]$pf.ParentProcessId
            }
            $chainStr = if ($chain.Count) { $chain -join '<-' } else { '<none>' }
            $why = if ($isParked) { 'parked-console' } else { 'owns-new-console-window' }

            Write-AuditLine ("LEFTOVER {0} pid={1} image={2} why={3} parent={4} cmd=[{5}]" -f `
                $Suite, $procId, $p.Name, $why, $chainStr, $cl)
            $found++

            if ($script:AuditNeverKill -contains $p.Name) {
                Write-AuditLine ("  not killing {0} pid={1} by design; its window is closed instead" -f $p.Name, $procId)
                continue
            }
            if (-not (Test-KillTargetSafe -TargetPid $procId -Reason "leftover audit, $Suite, $why")) {
                Write-AuditLine ("  REFUSED to end pid={0}: the kill ledger says it is this runner or one of its ancestors" -f $procId)
                continue
            }
            try {
                Stop-Process -Id $procId -Force -ErrorAction Stop
                $killed += $procId
            } catch {
                Write-AuditLine ("  could not end pid={0}: {1}" -f $procId, $_.Exception.Message)
            }
        }
    } catch { }

    if ($killed.Count -gt 0) {
        Start-Sleep -Milliseconds 500
        $still = @($killed | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
        Write-AuditLine ("  ended {0} leftover process(es): {1}{2}" -f $killed.Count, ($killed -join ','),
            $(if ($still.Count) { "  STILL ALIVE: $($still -join ',')" } else { '' }))
    }

    # ── pass 2: windows still standing (dead tabs, or hosts we refused to kill) ──
    foreach ($w in $newWins) {
        if (-not [PsmuxWinAudit]::Alive($w.Handle)) { continue }   # went with its process
        Write-AuditLine ("LEFTOVER-WINDOW {0} hwnd={1} pid={2} class={3} title=[{4}]" -f `
            $Suite, $w.Handle, $w.Pid, $w.Class, $w.Title)
        $found++
        [void][PsmuxWinAudit]::Close($w.Handle)
        $gone = $false
        for ($i = 0; $i -lt 10; $i++) {
            Start-Sleep -Milliseconds 200
            if (-not [PsmuxWinAudit]::Alive($w.Handle)) { $gone = $true; break }
        }
        if (-not $gone -and $w.Class -eq 'CASCADIA_HOSTING_WINDOW_CLASS') {
            # Almost certainly the multi-tab confirmation prompt.
            if (Invoke-WtCloseAllDialog -OwnerPid $w.Pid) {
                Write-AuditLine ("  confirmed Windows Terminal 'Close all' for hwnd={0}" -f $w.Handle)
                for ($i = 0; $i -lt 10; $i++) {
                    Start-Sleep -Milliseconds 200
                    if (-not [PsmuxWinAudit]::Alive($w.Handle)) { $gone = $true; break }
                }
            }
        }
        if ($gone) { Write-AuditLine ("  closed hwnd={0}" -f $w.Handle) }
        else       { Write-AuditLine ("  STUCK-WINDOW hwnd={0} would not close; left on the desktop" -f $w.Handle) }
    }

    if ($found -gt 0) {
        $script:AuditTotalLeftovers += $found
        $script:AuditSuitesWithLeftovers++
        Write-Host ("  [LEFTOVER] {0} left {1} console process(es)/window(s) behind; logged and cleaned" -f $Suite, $found) -ForegroundColor Magenta
    }
    return $found
}

function Get-SuiteTimeout {
    param([string]$Name)
    # Perf/stress/latency suites legitimately run long; everything else gets the default.
    if ($Name -match 'perf|stress|latency|benchmark|extreme|battle|install_speed|e2e|sustained|exhaustive|nsis|installer|realistic_typing|robust_|win32_tui_flag_parity|issue615_wsl_pane_path|issue684_paste_route|issue647_git_commit_in_version|issue693_targets|stalled_server_target_routing|issue835_status_tab_click_drift|issue943_failing_kinds_tcp|ctrlc_private_bridge') { return $LongTimeoutSec }
    return $DefaultTimeoutSec
}

# ── Environment pin ──
# Since #683 psmux seeds default-shell from SHELL the way tmux does. A runner
# started from Git Bash inherits SHELL=C:\Program Files\Git\bin\bash.exe (MSYS
# rewrites it for Windows children), which would turn every suite's default
# pane into bash. The suites are written for the pwsh default; the SHELL cases
# themselves live in test_issue683_shell_env_default_shell.ps1 and set their own.
if ($env:SHELL) {
    Write-Host "Clearing inherited SHELL='$($env:SHELL)' for the run" -ForegroundColor DarkYellow
    Remove-Item env:SHELL -ErrorAction SilentlyContinue
}

# ── Binary discovery ──
$PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -ErrorAction SilentlyContinue).Path
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\debug\psmux.exe" -ErrorAction SilentlyContinue).Path }
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -ErrorAction SilentlyContinue).Source }
if (-not $PSMUX) { Write-Error "psmux binary not found"; exit 1 }

# Every suite that calls bare `psmux` resolves it through PATH, which on the
# owner's box finds the INSTALLED copy in ~\.cargo\bin first.  Put the binary
# this run announces in front, so each suite tests that build: a run from a
# worktree otherwise mixed its own build with whatever was installed (the
# -Only runs of 2026-10-02 drove test_tui_exit_cleanup with the installed
# psmux, whose pane shells then ignored PSMUX_PSREADLINE_HISTORY below).
$env:PATH = (Split-Path $PSMUX) + ';' + $env:PATH
$resolved = (Get-Command psmux -ErrorAction SilentlyContinue).Source

Write-Log "Binary: $PSMUX"
Write-Log "PATH resolves psmux to: $resolved"
Write-Log "Params: SkipPerf=$SkipPerf IncludeWSL=$IncludeWSL IncludeInteractive=$IncludeInteractive DefaultTimeoutSec=$DefaultTimeoutSec LongTimeoutSec=$LongTimeoutSec Only='$Only' Resume=$Resume"

Write-Host "Binary: $PSMUX" -ForegroundColor Cyan
Write-Host "Started: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')" -ForegroundColor Cyan
Write-Host "Logs:    $script:RunDir" -ForegroundColor Cyan
Write-Host ""

# ── Pin the runner's OWN window, by identity ─────────────────────────────────
#
# MEASURED DEFECT THIS FIXES (run 2026-09-12_00-54-17)
# The audit reported the runner's own launcher window as a leftover:
#     LEFTOVER-WINDOW test_perf_vs_terminals hwnd=17698570 pid=13928
#                     class=CASCADIA_HOSTING_WINDOW_CLASS title=[Administrator: cmd]
# and then politely closed the window the run was printing into. Three separate
# protections all failed to catch it:
#   - GetConsoleWindow() returns the INVISIBLE pseudoconsole host window for a
#     console that Windows 11 delegated to Windows Terminal, so the handle it
#     pinned (2031992) was not the visible window at all;
#   - the owner pid is useless, because one WindowsTerminal.exe owns many windows:
#     the runner's window and the user's own window were BOTH pid 13928;
#   - the title filter did not match, because at the moment of the audit the
#     window was showing "Administrator: cmd", not the launcher's title.
#
# So the runner stamps a unique marker into its own console title and finds the
# window displaying it. That is identity rather than inference, and it works the
# same for a classic conhost window and a delegated Terminal window. The title is
# then restored to something a human can read which ALSO matches the protected
# title list, so the window stays protected even if the marker is overwritten.
$script:AuditOwnMarker = "psmux-runner-$PID-" + [guid]::NewGuid().ToString('N').Substring(0, 8)
try {
    if ([PsmuxWinAudit]::SetTitle($script:AuditOwnMarker)) {
        Start-Sleep -Milliseconds 500   # the host has to repaint its title bar
        $mine = @([PsmuxWinAudit]::FindByTitle($script:AuditOwnMarker))
        foreach ($h in $mine) {
            $script:AuditProtectHwnd[[long]$h] = $true
            Write-Log "OWN-WINDOW pinned hwnd=$h (identified by console title marker)"
        }
        if ($mine.Count -eq 0) {
            Write-Log "OWN-WINDOW no visible window carries the marker (headless or redirected host); baseline protection only"
        }
        [void][PsmuxWinAudit]::SetTitle("psmux FULL test suite - run $script:RunId")
    }
} catch { }

# ── Forensics: who we are, what we kill, and a witness outside our own tree ──
# See tests\run_forensics.ps1 for the measurements behind this. Short version:
# issue #680 lost two runs to something that ended the whole tree with nothing
# written anywhere, and the only two mechanisms that reproduce that signature
# (a job object closing on us, a tree kill aimed at an ancestor) are both
# TerminateProcess, so nothing inside this process can ever record them. The
# identity file says in advance whether we are sitting in such a job, the kill
# ledger refuses to be one of those mechanisms ourselves, and the watchdog is
# the only thing that will still be alive to describe the next occurrence.
try {
    . (Join-Path $PSScriptRoot 'run_forensics.ps1')
    Initialize-RunForensics -RunDir $script:RunDir -RunId $script:RunId
} catch {
    Write-Log "Forensics did not load: $_"
    Write-Host "  (forensics did not load: $_)" -ForegroundColor DarkYellow
}
# Never let a missing forensics file change how the run behaves: without it the
# ledger is simply absent and every kill is allowed, exactly as before.
if (-not (Get-Command Test-KillTargetSafe -ErrorAction SilentlyContinue)) {
    function Test-KillTargetSafe { param([int]$TargetPid, [string]$Reason, [switch]$Tree) return $true }
}
if (-not (Get-Command Write-KillNote -ErrorAction SilentlyContinue)) {
    function Write-KillNote { param([string]$Message) }
}
if (-not (Get-Command Complete-RunForensics -ErrorAction SilentlyContinue)) {
    function Complete-RunForensics { param([string]$Status) }
}

# ── Desktop baseline + spawn attribution watcher ──────────────────────────────
# The baseline is the set of console windows that existed BEFORE the run. Nothing
# in it is ever closed, which is what makes the per suite cleanup safe to run on
# a desktop that belongs to a human.
#
# Taken TWICE with a settle in between, and unioned. A console window that the
# shell which launched this run had only just created can take a moment to become
# visible to EnumWindows, and a single snapshot taken inside that gap declares the
# runner's own window "new", which is half of how the defect above happened.
$script:AuditBaselineWins = @{}
foreach ($pass in 1, 2) {
    try {
        foreach ($w in [PsmuxWinAudit]::List()) {
            if ($script:AuditBaselineWins.ContainsKey([long]$w.Handle)) { continue }
            $script:AuditBaselineWins[[long]$w.Handle] = $w.Title
            Write-Log ("BASELINE-WINDOW hwnd={0} pid={1} class={2} title=[{3}] (pass {4})" -f $w.Handle, $w.Pid, $w.Class, $w.Title, $pass)
        }
    } catch { }
    if ($pass -eq 1) { Start-Sleep -Milliseconds 700 }
}
Write-Host ("  Desktop baseline: {0} console window(s) present; they are never touched." -f $script:AuditBaselineWins.Count) -ForegroundColor DarkGray
if ($script:AuditProtectHwnd.Count -gt 0) {
    Write-Host ("  Pinned window handles (never closed): {0}" -f (($script:AuditProtectHwnd.Keys | Sort-Object) -join ', ')) -ForegroundColor DarkGray
}

# Suites detect that they were started by this runner through this variable. It
# lets a suite that deliberately leaks a console (the audit's own proof suite)
# SKIP when a human runs it by hand, so running it standalone never strands a
# window on the desktop.
$env:PSMUX_TEST_RUNNER = '1'

# Every pane shell a suite starts loads PSReadLine, and by default PSReadLine
# reads and writes the owner's ConsoleHost_history.txt: every line a suite types
# landed in the owner's real history, and an Up arrow (or a stray ESC [ A) then
# Enter in a test pane would recall and RUN whatever the owner typed last.
# psmux honours PSMUX_PSREADLINE_HISTORY in its pane init, so the run gets a
# throwaway history file of its own.
$env:PSMUX_PSREADLINE_HISTORY = Join-Path $script:RunDir "psreadline_history.txt"

# Discussion #748: a suite that starts a server without -f loads the machine's
# own config (~/.psmux.conf, ~/.psmuxrc, ~/.tmux.conf or ~/.config/psmux/psmux.conf),
# so `set -g base-index 1` on the machine made every suite that targets `:0`
# fail. The sweep must run on the defaults, but it must NOT get there by
# exporting PSMUX_CONFIG_FILE: a client with a custom config never claims a
# warm standby (src/main.rs, has_custom_config), so every suite would quietly
# test the cold path instead of the one users run. Sweep 2026-10-07_01-38-07
# did exactly that: test_pane_startup_perf measured new-session at 879 ms with
# a standby armed, and test_kill_server_reaps_warm saw its session cold spawned.
# So the variable is cleared. ~/.psmux.conf and ~/.psmuxrc are the runner's to
# remove (Clean-Server does it before every suite, since suites write them);
# the other two are the developer's, and either one stops the run before
# anything starts rather than being deleted.
if ($env:PSMUX_CONFIG_FILE) {
    Write-Log "PSMUX_CONFIG_FILE was $env:PSMUX_CONFIG_FILE in the launching shell; cleared so suites run on the defaults and the warm path"
    Remove-Item Env:PSMUX_CONFIG_FILE -ErrorAction SilentlyContinue
}
$script:DefaultConfigs = @(
    "$env:USERPROFILE\.tmux.conf", "$env:USERPROFILE\.config\psmux\psmux.conf"
) | Where-Object { Test-Path -LiteralPath $_ }
if ($script:DefaultConfigs) {
    Write-Host "This machine has a psmux config at a default path, which every suite would inherit:" -ForegroundColor Red
    $script:DefaultConfigs | ForEach-Object { Write-Host "    $_" -ForegroundColor Red }
    Write-Host "Move it aside for the sweep (discussion #748), then start the run again." -ForegroundColor Red
    Write-Log "ABORT: default path config present: $($script:DefaultConfigs -join ', ')"
    exit 2
}
# Suites run with -NoNewWindow and share the runner's console, and a suite that
# sets [Console]::OutputEncoding or InputEncoding (several read psmux's UTF-8
# that way) calls SetConsoleOutputCP / SetConsoleCP on that console. Every
# suite after it then ran at 65001 instead of the console's own page (OEM 437
# in the launcher's window, as users have it), and a suite's result depended on
# what ran before it (#570 failed under -Only only). Run-TestFile saves both
# code pages before a suite and puts them back after it, whatever way it ends.
function Save-SuiteConsoleEncoding {
    try { return @{ Out = [Console]::OutputEncoding; In = [Console]::InputEncoding } } catch { return $null }
}
function Restore-SuiteConsoleEncoding($Saved) {
    if (-not $Saved) { return }
    try { [Console]::OutputEncoding = $Saved.Out } catch { }
    try { [Console]::InputEncoding = $Saved.In } catch { }
}
$script:RunStartedAt = Get-Date

$script:CurrentSuiteFile = Join-Path $script:RunDir "current_suite.txt"
[System.IO.File]::WriteAllText($script:CurrentSuiteFile, '<starting>')
$script:SpawnTrace = Join-Path $script:RunDir "spawn_trace.log"
$script:SpawnWatcher = $null
$watcherScript = Join-Path $PSScriptRoot 'spawn_trace_watcher.ps1'
if (Test-Path $watcherScript) {
    try {
        $script:SpawnWatcher = Start-Process -FilePath "pwsh" -PassThru -WindowStyle Hidden `
            -ArgumentList "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $watcherScript,
                          "-OutFile", $script:SpawnTrace,
                          "-SuiteFile", $script:CurrentSuiteFile,
                          "-RunnerPid", $PID
        # The watcher must never be mistaken for a leftover by its own audit.
        if ($script:SpawnWatcher) { $script:AuditOwnPids[$script:SpawnWatcher.Id] = $true }
        Write-Host ("  Spawn attribution watcher pid {0} -> {1}" -f $script:SpawnWatcher.Id, $script:SpawnTrace) -ForegroundColor DarkGray
        Write-Log "Spawn watcher pid $($script:SpawnWatcher.Id) tracing to $script:SpawnTrace"
    } catch {
        Write-Host "  (spawn watcher did not start: $_)" -ForegroundColor DarkGray
        Write-Log "Spawn watcher failed to start: $_"
    }
}

# ── Categorize tests ──
# Tests requiring WSL
$wslTests = @(
    "test_wsl_in_pwsh_latency", "test_wsl_in_pwsh_latency2", "test_wsl_latency",
    "test_wsl_pwsh_latency3", "test_wsl_pwsh_latency4", "test_wsl_pwsh_latency5"
)
# Tests requiring interactive TUI / attached session / mouse
$interactiveTests = @(
    "test_claude_mouse", "test_conpty_mouse", "test_mouse_handling", "test_mouse_hover",
    "test_stress_attached", "test_tui_exit_cleanup", "test_claude_cursor_diag",
    "test_issue60_native_tui_mouse", "test_issue15_altgr", "test_cursor_fallback",
    "test_cursor_style", "test_issue52_cursor", "test_perf_vs_wt"
)
# Long-running stress/perf tests
$perfTests = @(
    "test_stress", "test_stress_50", "test_stress_aggressive", "test_extreme_perf",
    "test_e2e_latency", "test_pane_startup_perf", "test_startup_perf", "test_perf",
    # Launches real terminal emulators (Windows Terminal, WezTerm, Alacritty) and
    # times psmux against them: launch to prompt, keystroke to screen, creation
    # latency. Long by nature, and it opens GUI windows, so -SkipPerf skips it.
    "test_launch_to_prompt_gate", "test_keystroke_latency_gate", "test_creation_latency_gate", "test_idle_socket_traffic", "test_perf_vs_terminals",
    # To the prompt VISIBLE in the console the user launched from (warm, cold,
    # bare pwsh), and real typing into an attached client, per character and
    # after Ctrl+C. Both write JSON to the metrics folder perf_summary reads.
    "test_launch_visible_prompt_gate", "test_typing_benchmark"
)

# Results tracking
$results = [System.Collections.ArrayList]::new()

# ── Live dashboard state ──
$script:LivePass = 0; $script:LiveFail = 0; $script:LiveSkip = 0
$script:LivePassTests = 0; $script:LiveFailTests = 0
$script:SuiteDurations = [System.Collections.ArrayList]::new()  # rolling avg for ETA

function Get-Category {
    param([string]$Name)
    if ($wslTests -contains $Name) { return "WSL" }
    if ($interactiveTests -contains $Name) { return "Interactive" }
    if ($perfTests -contains $Name) { return "Perf/Stress" }
    if ($Name -match 'test_issue') { return "Issue Fixes" }
    if ($Name -match 'test_config|test_plugin|test_theme') { return "Config/Plugin" }
    if ($Name -match 'test_copy_mode|test_pane|test_layout|test_split|test_zoom') { return "UI/Layout" }
    if ($Name -match 'test_session|test_kill|test_warm') { return "Session Mgmt" }
    return "General"
}

function Show-ProgressDashboard {
    param([int]$Current, [int]$Total, [string]$SuiteName, [string]$Status)
    $pct = if ($Total -gt 0) { [math]::Round(($Current / $Total) * 100) } else { 0 }
    $elapsed = ((Get-Date) - $startTime).TotalSeconds

    # ETA calculation from rolling average
    $eta = "--:--"
    if ($script:SuiteDurations.Count -gt 0) {
        $avgTime = ($script:SuiteDurations | Measure-Object -Average).Average
        $remaining = ($Total - $Current) * $avgTime
        if ($remaining -gt 3600) {
            $eta = "{0:F0}h {1:F0}m" -f [math]::Floor($remaining/3600), [math]::Floor(($remaining%3600)/60)
        } elseif ($remaining -gt 60) {
            $eta = "{0:F0}m {1:F0}s" -f [math]::Floor($remaining/60), [math]::Floor($remaining%60)
        } else {
            $eta = "{0:F0}s" -f $remaining
        }
    }

    # Progress bar (40 chars wide)
    $barWidth = 40
    $filled = [math]::Max([math]::Round($pct / 100 * $barWidth), 0)
    $empty  = $barWidth - $filled
    $barFill  = [char]0x2588  # full block
    $barEmpty = [char]0x2591  # light shade
    $bar = ($barFill.ToString() * $filled) + ($barEmpty.ToString() * $empty)

    $barColor = if ($script:LiveFail -gt 0) { "Red" } elseif ($pct -ge 80) { "Green" } else { "Yellow" }

    # Status badge
    $badge = switch ($Status) {
        "PASS"    { "[PASS]" }
        "FAIL"    { "[FAIL]" }
        "TIMEOUT" { "[TIME]" }
        "SKIP"    { "[SKIP]" }
        "ERROR"   { "[ERR!]" }
        default   { "[....]" }
    }
    $badgeColor = switch ($Status) {
        "PASS"    { "Green" }
        "FAIL"    { "Red" }
        "TIMEOUT" { "Red" }
        "SKIP"    { "Yellow" }
        "ERROR"   { "Magenta" }
        default   { "DarkGray" }
    }

    Write-Host ""
    Write-Host ("  {0} " -f $bar) -ForegroundColor $barColor -NoNewline
    Write-Host ("{0,3}%" -f $pct) -ForegroundColor White -NoNewline
    Write-Host ("  [{0}/{1}]" -f $Current, $Total) -ForegroundColor DarkGray -NoNewline
    Write-Host ("  ETA: {0}" -f $eta) -ForegroundColor Cyan

    # Live counters
    Write-Host "  " -NoNewline
    Write-Host ("Pass:{0}" -f $script:LivePass) -ForegroundColor Green -NoNewline
    Write-Host " | " -ForegroundColor DarkGray -NoNewline
    Write-Host ("Fail:{0}" -f $script:LiveFail) -ForegroundColor $(if ($script:LiveFail -gt 0) { "Red" } else { "Green" }) -NoNewline
    Write-Host " | " -ForegroundColor DarkGray -NoNewline
    Write-Host ("Skip:{0}" -f $script:LiveSkip) -ForegroundColor Yellow -NoNewline
    Write-Host " | " -ForegroundColor DarkGray -NoNewline
    Write-Host "Tests: " -ForegroundColor DarkGray -NoNewline
    Write-Host ("{0}" -f $script:LivePassTests) -ForegroundColor Green -NoNewline
    Write-Host "/" -ForegroundColor DarkGray -NoNewline
    $fColor = if ($script:LiveFailTests -gt 0) { "Red" } else { "Green" }
    Write-Host ("{0}" -f $script:LiveFailTests) -ForegroundColor $fColor -NoNewline
    $elapsedFmt = if ($elapsed -gt 3600) { "{0:F0}h{1:F0}m" -f [math]::Floor($elapsed/3600),[math]::Floor(($elapsed%3600)/60) } elseif ($elapsed -gt 60) { "{0:F0}m{1:F0}s" -f [math]::Floor($elapsed/60),[math]::Floor($elapsed%60) } else { "{0:F0}s" -f $elapsed }
    Write-Host ("  Elapsed: {0}" -f $elapsedFmt) -ForegroundColor DarkGray

    # Last suite result
    if ($SuiteName) {
        Write-Host "  " -NoNewline
        Write-Host $badge -ForegroundColor $badgeColor -NoNewline
        Write-Host (" {0}" -f $SuiteName) -ForegroundColor White
    }
}

# ── Orphaned pane trees ───────────────────────────────────────────────────────
# A pane shell is a pwsh/powershell whose command line carries psmux's pane init
# ("PredictionSource None").  An ORPHAN is one whose parent (its server) is gone:
# the parent pid is not running, or it now belongs to a process created later
# (pid reuse).  Its tree is every descendant by PID, each link checked by
# creation time (parent created no later than child), so the walk never wanders
# into an unrelated process that inherited a recycled pid.
#
# Never reaped, only logged as KEPT: a tree containing a psmux/tmux/pmux server
# (a detached server started from a pane is a daemon by design), Windows
# Terminal, Claude Code, an editor host, or any process of this runner's own
# ancestry.  Everything is by PID with the start time re-checked at kill time.
$script:OrphanKeepNames = '^(psmux|tmux|pmux|WindowsTerminal|OpenConsole|claude|node|Code|wezterm-gui|alacritty)\.exe$'

# The three image names a psmux server can run under: the crate builds psmux,
# pmux and tmux, and a server is spawned from `std::env::current_exe()`, so a
# suite that calls bare `tmux` or `pmux` leaves a server and a warm standby of
# that name. psmux itself counts all three (src/session.rs
# `PSMUX_SERVER_IMAGE_NAMES`), and so does the orphan reaper above, but
# Clean-Server asked for `psmux` alone: a lone tmux.exe standby made its guard
# false, so the whole teardown, the graceful kill-server included, was skipped.
$script:PsmuxServerNames = @('psmux', 'pmux', 'tmux')

function Get-PsmuxServerProcess {
    Get-Process -Name $script:PsmuxServerNames -ErrorAction SilentlyContinue
}

function Get-ProcSnapshot {
    $all = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue)
    $byId = @{}; $kids = @{}
    foreach ($p in $all) {
        $byId[[int]$p.ProcessId] = $p
        $pp = [int]$p.ParentProcessId
        if (-not $kids.ContainsKey($pp)) { $kids[$pp] = New-Object System.Collections.Generic.List[object] }
        $kids[$pp].Add($p)
    }
    $protect = @{}
    $id = $PID
    while ($byId.ContainsKey($id) -and -not $protect.ContainsKey($id)) {
        $protect[$id] = $true
        $par = [int]$byId[$id].ParentProcessId
        if (-not $byId.ContainsKey($par) -or $byId[$par].CreationDate -gt $byId[$id].CreationDate) { break }
        $id = $par
    }
    [pscustomobject]@{ All = $all; ById = $byId; Kids = $kids; Protect = $protect }
}

function New-ProcRec($p) {
    [pscustomobject]@{ Pid = [int]$p.ProcessId; Name = $p.Name; Start = $p.CreationDate; ParentPid = [int]$p.ParentProcessId }
}

# The root and every descendant, as records.  Returns $null when the tree holds
# a process that must never be touched (see OrphanKeepNames) or one of the
# runner's own ancestry.
function Get-ProcTreeRecs($snap, [int]$root) {
    $r = $snap.ById[$root]; if (-not $r) { return @() }
    $out = New-Object System.Collections.Generic.List[object]
    $q = [System.Collections.Queue]::new(); $q.Enqueue($r)
    while ($q.Count) {
        $p = $q.Dequeue()
        if ($snap.Protect.ContainsKey([int]$p.ProcessId) -or $p.Name -match $script:OrphanKeepNames) { return $null }
        $out.Add((New-ProcRec $p))
        if ($snap.Kids.ContainsKey([int]$p.ProcessId)) {
            foreach ($c in $snap.Kids[[int]$p.ProcessId]) {
                if ([int]$c.ProcessId -ne [int]$p.ProcessId -and $c.CreationDate -ge $p.CreationDate) { $q.Enqueue($c) }
            }
        }
    }
    , $out.ToArray()
}

function Get-OrphanPaneTrees {
    param([datetime]$Since = [datetime]::MinValue)
    $snap = Get-ProcSnapshot
    $res = New-Object System.Collections.Generic.List[object]
    foreach ($p in $snap.All) {
        if ($p.Name -notmatch '^(pwsh|powershell)\.exe$') { continue }
        if (-not $p.CommandLine -or $p.CommandLine -notmatch 'PredictionSource None') { continue }
        if ($p.CreationDate -lt $Since) { continue }
        $par = $snap.ById[[int]$p.ParentProcessId]
        if ($par -and $par.CreationDate -le $p.CreationDate) { continue }   # parent alive: not an orphan
        $tree = Get-ProcTreeRecs $snap ([int]$p.ProcessId)
        $res.Add([pscustomobject]@{ Shell = (New-ProcRec $p); Tree = $tree; Keep = ($null -eq $tree) })
    }
    $res.ToArray()
}

function Stop-ProcRecs($recs) {
    $n = 0
    foreach ($r in $recs) {
        $g = Get-Process -Id $r.Pid -ErrorAction SilentlyContinue
        if (-not $g) { continue }
        $same = $false; try { $same = [Math]::Abs(($g.StartTime - $r.Start).TotalSeconds) -lt 2 } catch { }
        if (-not $same) { continue }   # the pid was reused: never touch the newcomer
        Stop-Process -Id $r.Pid -Force -ErrorAction SilentlyContinue; $n++
    }
    $n
}

# Log and reap orphaned pane trees.  $Since limits it to processes created at or
# after that time (the per suite sweep uses the run start; the pre sweep audit
# uses no limit).  PSMUX_RUN_KEEP_ORPHANS=1 logs without reaping.
function Invoke-OrphanPaneReap {
    param([string]$Phase, [datetime]$Since = [datetime]::MinValue)
    $orph = @(Get-OrphanPaneTrees -Since $Since)
    if (-not $orph.Count) { return 0 }
    $reaped = 0
    foreach ($o in $orph) {
        $desc = if ($o.Keep) { "KEPT (tree holds a server, terminal, agent or runner process)" } else { ($o.Tree | ForEach-Object { "{0}:{1}" -f $_.Name, $_.Pid }) -join ' ' }
        $line = "ORPHAN-PANE {0} shell pid={1} started={2} parent={3}(dead) tree: {4}" -f $Phase, $o.Shell.Pid, $o.Shell.Start.ToString('MM-dd HH:mm:ss'), $o.Shell.ParentPid, $desc
        Write-Log $line; Write-KillNote $line
        if (-not $o.Keep -and $env:PSMUX_RUN_KEEP_ORPHANS -ne '1') { $reaped += Stop-ProcRecs $o.Tree }
    }
    Write-KillNote ("ORPHAN-PANE {0} found {1} orphaned pane tree(s), stopped {2} process(es) by pid" -f $Phase, $orph.Count, $reaped)
    $reaped
}

function Clean-Server {
    # If no psmux processes exist there is nothing to tear down; just clear files.
    $alive = @(Get-PsmuxServerProcess)
    if ($alive.Count -gt 0) {
        # Image name kills, and only ever of a psmux server image. Recorded so
        # the ledger shows what the runner was doing in the seconds around a
        # disappearance: run 2026-09-20_00-19-17 died inside this very function,
        # between "Queuing test_newsession_flags" and "START test_newsession_flags".
        Write-KillNote ("CLEAN-SERVER killing {0} psmux process(es) by image name: {1}" -f $alive.Count, (($alive | ForEach-Object { "{0}:{1}" -f $_.ProcessName, $_.Id }) -join ','))
        # Gracefully ask all servers to exit, but BOUNDED: a wedged server must not
        # hang the runner (the old unbounded `& $PSMUX kill-server` could block forever).
        try {
            $ks = Start-Process -FilePath $PSMUX -ArgumentList "kill-server" -PassThru -NoNewWindow `
                    -RedirectStandardOutput (Join-Path $script:RunDir "ks_out.tmp") `
                    -RedirectStandardError  (Join-Path $script:RunDir "ks_err.tmp")
            if (-not $ks.WaitForExit(5000)) { try { $ks.Kill() } catch {} }
        } catch {}
        # A server killed by image name leaves every pane shell it spawned alive
        # with no parent: 24 `pwsh -NoLogo -NoProfile -NoExit ...` orphans sat
        # on the machine after sweep 2026-09-23_13-54-30, each with a conhost.
        # Record the children of every surviving server BY PID before the kill,
        # and stop them afterwards. Only direct children of a psmux server, and
        # only pwsh/cmd/conhost images, so nothing unrelated is ever touched.
        # The DESCENDANTS of each pane shell are recorded too (sweep 2026-10-02
        # ended with pwsh -> htop -> pstop trees whose pstop each burned half a
        # core), each link checked by creation time.  A tree that holds a
        # psmux server, a terminal or an agent process is left alone.
        $orphanCandidates = @()
        $treeRecs = @()
        $snap = Get-ProcSnapshot
        foreach ($srv in @(Get-PsmuxServerProcess)) {
            $srvP = $snap.ById[$srv.Id]
            if (-not $srvP -or -not $snap.Kids.ContainsKey($srv.Id)) { continue }
            foreach ($c in $snap.Kids[$srv.Id]) {
                if ($c.Name -notmatch '^(pwsh|powershell|cmd|conhost)\.exe$') { continue }
                if ($c.CreationDate -lt $srvP.CreationDate) { continue }
                $orphanCandidates += [int]$c.ProcessId
                $t = Get-ProcTreeRecs $snap ([int]$c.ProcessId)
                if ($null -ne $t) { $treeRecs += $t }
            }
        }
        # Force-kill any lingering processes, then poll (up to 3s) instead of fixed sleeps
        Get-PsmuxServerProcess | Stop-Process -Force -ErrorAction SilentlyContinue
        foreach ($childPid in $orphanCandidates) {
            $c = Get-Process -Id $childPid -ErrorAction SilentlyContinue
            if ($c -and $c.ProcessName -match '^(pwsh|powershell|cmd|conhost)$') {
                Stop-Process -Id $childPid -Force -ErrorAction SilentlyContinue
            }
        }
        $stoppedTree = Stop-ProcRecs $treeRecs
        if ($orphanCandidates.Count -gt 0) { Write-KillNote ("CLEAN-SERVER reaped {0} pane shell(s) of the killed server(s) by pid: {1}; {2} process(es) of their trees were still alive and stopped: {3}" -f $orphanCandidates.Count, ($orphanCandidates -join ','), $stoppedTree, (($treeRecs | Where-Object { $orphanCandidates -notcontains $_.Pid } | ForEach-Object { "{0}:{1}" -f $_.Name, $_.Pid }) -join ',')) }
        $deadline = [DateTime]::Now.AddSeconds(3)
        while ([DateTime]::Now -lt $deadline) {
            if (@(Get-PsmuxServerProcess).Count -eq 0) { break }
            Start-Sleep -Milliseconds 150
        }
        Get-PsmuxServerProcess | Stop-Process -Force -ErrorAction SilentlyContinue
        # Brief settle so the OS releases TCP ports/file handles of killed servers
        Start-Sleep -Milliseconds 500
    }
    # Remove stale port/key files
    Remove-Item "$env:USERPROFILE\.psmux\*.port" -Force -ErrorAction SilentlyContinue
    Remove-Item "$env:USERPROFILE\.psmux\*.key" -Force -ErrorAction SilentlyContinue
    # Remove any test config files (tests should restore originals but may fail)
    Remove-Item "$env:USERPROFILE\.psmux.conf" -Force -ErrorAction SilentlyContinue
    Remove-Item "$env:USERPROFILE\.psmuxrc" -Force -ErrorAction SilentlyContinue
    Remove-Item (Join-Path $script:RunDir "ks_*.tmp") -Force -ErrorAction SilentlyContinue
    # The reap above only sees the panes of servers still ALIVE when it runs.
    # A pane whose server had already died (killed by the suite itself by pid,
    # crashed, or ended in a namespace the by-name kill raced) was invisible to
    # it: sweep 2026-10-02 ended with 51 idle pane shells nobody reaped.  So
    # every orphaned pane tree created since this run started is reaped here,
    # whether or not a psmux process was alive.
    if ($script:RunStartedAt) { [void](Invoke-OrphanPaneReap -Phase "before-suite" -Since $script:RunStartedAt) }
}

function Run-TestFile {
    param([string]$FilePath)

    $name = [System.IO.Path]::GetFileNameWithoutExtension($FilePath)
    $baseName = $name
    $suiteLog = Join-Path $script:SuiteDir "$baseName.log"

    # Check skip categories
    if ($wslTests -contains $baseName -and -not $IncludeWSL) {
        Write-Log "SKIP  $baseName  (WSL required)"
        return @{ Name = $baseName; Status = "SKIP"; Reason = "WSL required"; Passed = 0; Failed = 0; Leftovers = 0; Duration = 0 }
    }
    if ($interactiveTests -contains $baseName -and -not $IncludeInteractive) {
        Write-Log "SKIP  $baseName  (Interactive TUI required)"
        return @{ Name = $baseName; Status = "SKIP"; Reason = "Interactive TUI required"; Passed = 0; Failed = 0; Leftovers = 0; Duration = 0 }
    }
    if ($perfTests -contains $baseName -and $SkipPerf) {
        Write-Log "SKIP  $baseName  (Perf test, -SkipPerf active)"
        return @{ Name = $baseName; Status = "SKIP"; Reason = "Perf test (use -SkipPerf to skip)"; Passed = 0; Failed = 0; Leftovers = 0; Duration = 0 }
    }

    Clean-Server

    $uiBefore = Test-UiAccess "before $baseName"
    Write-Log "START $baseName"

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Write-Host "`n$('=' * 60)" -ForegroundColor DarkGray
    Write-Host "  RUNNING: $baseName" -ForegroundColor White
    Write-Host "$('=' * 60)" -ForegroundColor DarkGray

    $consoleEncoding = Save-SuiteConsoleEncoding
    try {
        # Run the test as a real child process inside a Windows Job Object.
        #  - stdout/stderr go straight to files: nothing ever blocks on a pipe,
        #    and the log is tail-able in real time while the test runs.
        #  - On timeout (or after completion) the job object kills the ENTIRE
        #    process tree, including orphans whose parent already exited.
        $timeoutSec = Get-SuiteTimeout $baseName
        $outFile = Join-Path $script:SuiteDir "$baseName.out.tmp"
        $errFile = Join-Path $script:SuiteDir "$baseName.err.tmp"
        Remove-Item $outFile, $errFile -Force -ErrorAction SilentlyContinue

        # Desktop snapshot for the leftover audit, taken as late as possible so
        # anything already on screen is this suite's problem only if IT created it.
        $auditBefore = New-AuditSnapshot
        $auditStart  = Get-Date
        # Tell the spawn watcher which suite owns the next burst of console starts.
        try { [System.IO.File]::WriteAllText($script:CurrentSuiteFile, $baseName) } catch { }

        $job = [PsmuxTestJob]::Create()
        # Pin the suite's working directory to the repo root: several suites
        # resolve `.\target\release\psmux.exe` relative to CWD, so inheriting
        # whatever directory the runner was launched from silently breaks them.
        $repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
        $proc = Start-Process -FilePath "pwsh" `
            -ArgumentList "-NoProfile","-ExecutionPolicy","Bypass","-File",$FilePath `
            -PassThru -NoNewWindow -WorkingDirectory $repoRoot `
            -RedirectStandardOutput $outFile -RedirectStandardError $errFile
        $inJob = $false
        if ($job -ne [IntPtr]::Zero) { $inJob = [PsmuxTestJob]::Assign($job, $proc.Id) }
        # Say so when the job object could not take the suite. AssignProcessToJobObject
        # fails with ERROR_ACCESS_DENIED when the runner itself already sits in a job
        # that forbids nesting (an agent tool shell, a CI container, an sshd session),
        # and in that case the normal completion path has NOTHING that reaps the
        # suite's children: every orphan it leaves survives. That silence is half the
        # reason a leaked console could not be attributed to a suite before, so it is
        # logged, and the desktop audit below is what actually cleans up after it.
        if (-not $inJob) {
            Write-Log "WARN  $baseName is NOT in a job object (assignment failed); leaked children will only be caught by the desktop audit"
            Write-Host "  (no job object for this suite: process-tree teardown is unavailable, audit only)" -ForegroundColor DarkYellow
        }

        # Wait with a heartbeat so long/hung tests are visible while they run.
        # The same 1s tick polls the abort channel, so a stop request lands
        # within a second even inside a 900s perf suite.
        $timedOut = $false
        $aborted = $false
        $lastBeat = [DateTime]::Now
        while (-not $proc.WaitForExit(1000)) {
            $why = Test-AbortRequested
            if ($why) { $aborted = $true; $script:AbortReason = $why; break }
            if ($sw.Elapsed.TotalSeconds -ge $timeoutSec) { $timedOut = $true; break }
            if (([DateTime]::Now - $lastBeat).TotalSeconds -ge 10) {
                $lastBeat = [DateTime]::Now
                $lastLine = ""
                try { $lastLine = (Get-Content $outFile -Tail 1 -ErrorAction SilentlyContinue) } catch {}
                if ($lastLine) { $lastLine = ($lastLine | Out-String).Trim() }
                if ($lastLine.Length -gt 80) { $lastLine = $lastLine.Substring(0, 80) }
                Write-Host ("  ... {0,4:F0}s / {1}s  {2}" -f $sw.Elapsed.TotalSeconds, $timeoutSec, $lastLine) -ForegroundColor DarkGray
                Write-Log ("HEARTBEAT $baseName {0:F0}s/{1}s" -f $sw.Elapsed.TotalSeconds, $timeoutSec)
            }
        }

        # A real Ctrl+C reaches the suite as well as the runner: the child was
        # started -NoNewWindow so it shares this console, and it dies at the same
        # instant we are signalled. The wait loop then exits NORMALLY, and
        # without this re-check the half-executed suite gets scored on its
        # truncated output - exit code 0, no assertions printed, therefore
        # "PASS". Measured 2026-08-26: test_fake_1 was killed 8s into a 15s body
        # and recorded PASS 0P/0F exit=0, and that bogus pass went into
        # results.jsonl where -Resume would skip the suite as already done.
        if (-not $aborted -and -not $timedOut) {
            $why = Test-AbortRequested
            # Deliberately biased towards calling it an abort: re-running a suite
            # that had genuinely just finished costs one suite, whereas trusting
            # a truncated pass loses coverage silently.
            if ($why) { $aborted = $true; $script:AbortReason = $why }
        }

        if ($aborted) {
            # Same teardown as a timeout: the job object kills the whole tree,
            # including the psmux servers the suite started, so an abort does not
            # strand processes the way a Task Manager kill of the runner did.
            Write-Host "`n  [ABORT] Stop requested ($script:AbortReason). Killing $baseName process tree." -ForegroundColor Yellow
            Write-Log "ABORT $baseName - stop requested ($script:AbortReason), killing process tree"
            if ($inJob) {
                Write-KillNote ("JOBKILL abort suite={0} assigned pid={1}" -f $baseName, $proc.Id)
                [PsmuxTestJob]::Kill($job)
            } elseif (Test-KillTargetSafe -TargetPid $proc.Id -Reason "abort teardown of $baseName" -Tree) {
                & taskkill /F /T /PID $proc.Id 2>&1 | Out-Null
            }
            try { $proc.WaitForExit(5000) | Out-Null } catch {}
            $sw.Stop()
            Remove-Item $outFile, $errFile -Force -ErrorAction SilentlyContinue
            # An interrupted suite is the MOST likely one to strand a window, so
            # the audit runs here too.
            $leftovers = Invoke-LeftoverAudit -Suite $baseName -Before $auditBefore -SuiteStart $auditStart
            return @{
                Name = $baseName
                Status = "ABORT"
                ExitCode = -3
                Passed = 0; Failed = 0; Skipped = 0
                Leftovers = $leftovers
                Duration = [math]::Round($sw.Elapsed.TotalSeconds, 1)
                Reason = "interrupted ($script:AbortReason)"
                Output = ""
            }
        }

        if ($timedOut) {
            Write-Host "  [TIMEOUT] Killing $baseName process tree after ${timeoutSec}s" -ForegroundColor Red
            Write-Log "TIMEOUT $baseName after ${timeoutSec}s, killing process tree"
            if ($inJob) {
                Write-KillNote ("JOBKILL timeout suite={0} assigned pid={1}" -f $baseName, $proc.Id)
                [PsmuxTestJob]::Kill($job)   # kills every descendant, even orphans
            } elseif (Test-KillTargetSafe -TargetPid $proc.Id -Reason "timeout teardown of $baseName after ${timeoutSec}s" -Tree) {
                # Fallback: taskkill the tree if job-object assignment failed
                & taskkill /F /T /PID $proc.Id 2>&1 | Out-Null
            }
            try { $proc.WaitForExit(5000) | Out-Null } catch {}
            $exitCode = -2
        } else {
            try { $proc.WaitForExit() } catch {}  # ensure ExitCode is available
            $exitCode = $proc.ExitCode
            # Suite finished: reap anything it left behind (leaked children would
            # otherwise accumulate across 541 suites and poison later tests)
            if ($inJob) {
                Write-KillNote ("JOBKILL completion suite={0} assigned pid={1} exit={2}" -f $baseName, $proc.Id, $exitCode)
                [PsmuxTestJob]::Kill($job)
            }
        }
        $sw.Stop()

        # Desktop audit. Runs after the job teardown AND after the suite's own
        # cleanup, so a suite that opens terminals and closes them itself (see
        # test_perf_vs_terminals) is not double counted: only what is STILL on the
        # desktop is reported.
        $leftovers = Invoke-LeftoverAudit -Suite $baseName -Before $auditBefore -SuiteStart $auditStart

        # Collect output from the redirect files (out first, then err)
        $output = ""
        try { $output = [System.IO.File]::ReadAllText($outFile) } catch {}
        try {
            $errText = [System.IO.File]::ReadAllText($errFile)
            if ($errText.Trim()) { $output += "`r`n--- STDERR ---`r`n$errText" }
        } catch {}
        if ($timedOut) {
            $output += "`r`n[TIMEOUT] Test $baseName exceeded $timeoutSec seconds; process tree was killed`r`n"
        }
        Remove-Item $outFile, $errFile -Force -ErrorAction SilentlyContinue

        # Write full output to per-suite log file
        $suiteHeader = "Suite: $baseName`r`nFile:  $FilePath`r`nStart: $(($startTime + $sw.Elapsed - $sw.Elapsed).ToString('yyyy-MM-dd HH:mm:ss'))`r`nEnd:   $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')`r`nExit:  $exitCode`r`nDuration: $([math]::Round($sw.Elapsed.TotalSeconds,1))s`r`n$('=' * 70)`r`n"
        [System.IO.File]::WriteAllText($suiteLog, "$suiteHeader$output", [System.Text.Encoding]::UTF8)

        # Count PASS/FAIL from output (multiple patterns used by different test scripts).
        # The colon form ("  PASS: msg" / "FAIL: msg") is used by the agent-teams
        # suites; without it their results recorded as 0P/0F despite real outcomes.
        $passCount = ([regex]::Matches($output, '\[PASS\]')).Count
        $passCount += ([regex]::Matches($output, '(?m)^PASS\s')).Count
        $passCount += ([regex]::Matches($output, '(?m)^\s*PASS:(?!\s*\d+\s*$)')).Count
        $passCount += ([regex]::Matches($output, '=> PASS$', [System.Text.RegularExpressions.RegexOptions]::Multiline)).Count
        $failCount = ([regex]::Matches($output, '\[FAIL\]')).Count
        $failCount += ([regex]::Matches($output, '(?m)^FAIL\s')).Count
        # Negative lookahead: a bare number after the colon is a summary line
        # ("FAIL: 0"), not an assertion result, and must not be counted.
        $failCount += ([regex]::Matches($output, '(?m)^\s*FAIL:(?!\s*\d+\s*$)')).Count
        $failCount += ([regex]::Matches($output, '=> FAIL$', [System.Text.RegularExpressions.RegexOptions]::Multiline)).Count
        $skipCount = ([regex]::Matches($output, '\[SKIP\]')).Count

        # Show output
        Write-Host $output

        $status = if ($timedOut) { "TIMEOUT" }
                  elseif ($exitCode -eq 0 -and $failCount -eq 0) { "PASS" }
                  else { "FAIL" }

        # A refusal seen either side of the suite is stamped on its result line,
        # so a FAIL that happened while the desktop was unusable is never read
        # as a product bug, and a PASS under it is not mistaken for coverage.
        $uiAfter = Test-UiAccess "after $baseName"
        $uiNote = ''
        if ($uiBefore -or $uiAfter) { $uiNote = "  UI-ACCESS-DENIED(" + $(if ($uiBefore) { "before" } else { "after" }) + ")" }
        # Same idea for antivirus: a harness exe quarantined mid suite (sweep
        # 2026-10-01_01-53-14, three injectors) is stamped on the result line.
        $avHits = @(Get-AvDetections -Since $auditStart -Suite $baseName)
        if ($avHits.Count) { $uiNote += "  AV-QUARANTINED(" + ($avHits -join ', ') + ")" }

        # Storage Sense (StorSvc) deletes empty directories (and stale files) in %TEMP%
        # when the disk runs low, on its own schedule (about every 6 h 10 min on
        # the dev box). In sweep 2026-10-01_01-53-14 it ran at 06:15:28, in the
        # middle of test_issue600_bash_rehome, and took that suite's fixture
        # directories, which read as three product failures. Stamp any run that
        # overlaps a suite, so a fixture that vanished is never read as psmux.
        try {
            $suiteStart = (Get-Date).Add(-$sw.Elapsed).AddSeconds(-1)
            $ss = @(Get-WinEvent -FilterHashtable @{ LogName = 'Microsoft-Windows-Storsvc/Diagnostic'; Id = 1003; StartTime = $suiteStart } -ErrorAction Stop)
            if ($ss.Count -gt 0) {
                $when = ($ss | ForEach-Object { $_.TimeCreated.ToString('HH:mm:ss.fff') }) -join ','
                $uiNote += "  STORAGE-SENSE-RAN($when)"
                [System.IO.File]::AppendAllText($suiteLog, "`r`n[RUNNER] Storage Sense (Storsvc event 1003) ran at $when while this suite ran; it deletes unheld files under %TEMP%, so a missing fixture here is external.`r`n")
            }
        } catch { }

        Write-Log ("{0,-7} {1,-45} {2}P/{3}F  exit={4}  {5}s{6}{7}" -f $status, $baseName, $passCount, $failCount, $exitCode, [math]::Round($sw.Elapsed.TotalSeconds,1), $(if ($leftovers -gt 0) { "  LEFTOVERS=$leftovers" } else { '' }), $uiNote)

        return @{
            Name = $baseName
            Status = $status
            ExitCode = $exitCode
            Passed = $passCount
            Failed = $failCount
            Skipped = $skipCount
            Leftovers = $leftovers
            Duration = [math]::Round($sw.Elapsed.TotalSeconds, 1)
            Output = $output
        }
    } catch {
        $sw.Stop()
        Write-Host "  ERROR: $_" -ForegroundColor Red
        [System.IO.File]::WriteAllText($suiteLog, "Suite: $baseName`r`nERROR: $_`r`n", [System.Text.Encoding]::UTF8)
        Write-Log "ERROR $baseName  $_"
        return @{
            Name = $baseName
            Status = "ERROR"
            Passed = 0
            Failed = 1
            Leftovers = 0
            Duration = [math]::Round($sw.Elapsed.TotalSeconds, 1)
            Output = $_.ToString()
        }
    } finally {
        Restore-SuiteConsoleEncoding $consoleEncoding
    }
}

# ── Collect all test files ──
$testRoot = if ($TestDir) { $TestDir } else { $PSScriptRoot }
$allTests = Get-ChildItem "$testRoot\test_*.ps1" | Sort-Object Name
if ($Only) {
    $allTests = @($allTests | Where-Object { $_.BaseName -match $Only })
    Write-Log "Filter -Only '$Only' matched $($allTests.Count) suites"
}
if ($PerfOnly) {
    if ($SkipPerf) { Write-Host "  -PerfOnly and -SkipPerf together run nothing; -SkipPerf ignored" -ForegroundColor Yellow; $SkipPerf = $false }
    $allTests = @($allTests | Where-Object { $perfTests -contains $_.BaseName })
    Write-Log "Filter -PerfOnly matched $($allTests.Count) suites"
}
$totalSuites = $allTests.Count
Write-Host ""
Write-Host ("  {0} test suites discovered" -f $totalSuites) -ForegroundColor Cyan
Write-Log "Found $totalSuites test files"

# Category header
$catGroups = @{}
foreach ($t in $allTests) {
    $cat = Get-Category $t.BaseName
    if (-not $catGroups.ContainsKey($cat)) { $catGroups[$cat] = 0 }
    $catGroups[$cat]++
}
Write-Host "  Categories: " -ForegroundColor DarkGray -NoNewline
$catNames = ($catGroups.GetEnumerator() | Sort-Object Value -Descending | ForEach-Object { "{0}({1})" -f $_.Key,$_.Value })
Write-Host ($catNames -join "  ") -ForegroundColor DarkGray
Write-Host ""

# ── Pre sweep audit: orphaned pane trees left by EARLIER runs ──
# Sweep 2026-10-02 failed five perf gates because 48 pstop processes left by
# earlier standalone runs were spinning.  Before the first suite, every orphaned
# pane tree on the machine (any age) is logged, and reaped by PID unless it holds
# a server, a terminal, an agent or this runner (see Get-ProcTreeRecs).
$preOrphans = @(Get-OrphanPaneTrees)
if ($preOrphans.Count) {
    Write-Host ("  Pre sweep audit: {0} orphaned pane tree(s) from earlier runs" -f $preOrphans.Count) -ForegroundColor Yellow
    $preReaped = Invoke-OrphanPaneReap -Phase "pre-sweep"
    Write-Log ("PRE-SWEEP orphaned pane trees: {0}, processes stopped: {1}" -f $preOrphans.Count, $preReaped)
} else {
    Write-Log "PRE-SWEEP orphaned pane trees: 0"
}

# ── Run each test ──
$suiteIndex = 0
$script:RunAborted = $false
$script:NotRunCount = 0
try {
foreach ($testFile in $allTests) {
    $suiteIndex++

    # Abort check BEFORE anything is started, so a stop request never launches
    # one more suite. Covers the Clean-Server gap between suites too.
    $why = Test-AbortRequested
    if ($why) {
        $script:AbortReason = $why
        $script:RunAborted = $true
        $script:NotRunCount = $totalSuites - $suiteIndex + 1
        Write-Log "ABORT requested ($why) before [$suiteIndex/$totalSuites] $($testFile.BaseName); $script:NotRunCount suites not run"
        break
    }

    # Resume: skip suites that already completed in the run being resumed
    if ($script:CompletedSuites.ContainsKey($testFile.BaseName)) {
        $prev = $script:CompletedSuites[$testFile.BaseName]
        $result = @{
            Name = $prev.Name; Status = $prev.Status
            Passed = [int]$prev.Passed; Failed = [int]$prev.Failed
            Leftovers = $(if ($prev.PSObject.Properties['Leftovers']) { [int]$prev.Leftovers } else { 0 })
            Duration = [double]$prev.Duration; Reason = "already completed (resume)"
        }
        [void]$results.Add($result)
        Write-Host ("  [RESUME] {0,-45} {1} (from previous run)" -f $testFile.BaseName, $prev.Status) -ForegroundColor DarkCyan
        switch ($result.Status) {
            "PASS"    { $script:LivePass++ }
            "FAIL"    { $script:LiveFail++ }
            "TIMEOUT" { $script:LiveFail++ }
            "ERROR"   { $script:LiveFail++ }
            "SKIP"    { $script:LiveSkip++ }
        }
        $script:LivePassTests += $result.Passed
        $script:LiveFailTests += $result.Failed
        continue
    }

    Write-Log "--- [$suiteIndex/$totalSuites] Queuing $($testFile.BaseName) ---"

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $result = Run-TestFile -FilePath $testFile.FullName
    $sw.Stop()
    [void]$results.Add($result)
    [void]$script:SuiteDurations.Add($sw.Elapsed.TotalSeconds)

    # An interrupted suite has NO verdict: it was killed part way through, so its
    # pass/fail counts are meaningless. Deliberately skip the results.jsonl
    # record here - that file is what -Resume replays, and writing an ABORT into
    # it would make the resumed run treat a half-executed suite as done and
    # silently skip it forever.
    if ($result.Status -eq "ABORT") {
        $script:RunAborted = $true
        $script:NotRunCount = $totalSuites - $suiteIndex
        Write-Log "ABORT during [$suiteIndex/$totalSuites] $($testFile.BaseName); $script:NotRunCount further suites not run"
        break
    }

    # Crash-safe per-suite result record (also powers -Resume)
    $rec = @{ Name=$result.Name; Status=$result.Status; Passed=$result.Passed;
              Failed=$result.Failed; Duration=$result.Duration; ExitCode=$result.ExitCode;
              Leftovers=$(if ($result.Leftovers) { [int]$result.Leftovers } else { 0 }) } | ConvertTo-Json -Compress
    [System.IO.File]::AppendAllText($script:ResultsJsonl, "$rec`r`n")

    # Update live counters
    switch ($result.Status) {
        "PASS"    { $script:LivePass++ }
        "FAIL"    { $script:LiveFail++ }
        "TIMEOUT" { $script:LiveFail++ }
        "ERROR"   { $script:LiveFail++ }
        "SKIP"    { $script:LiveSkip++ }
    }
    $script:LivePassTests += $result.Passed
    $script:LiveFailTests += $result.Failed

    Show-ProgressDashboard -Current $suiteIndex -Total $totalSuites -SuiteName $testFile.BaseName -Status $result.Status
}
} catch {
    # A terminating error in the loop is a death the runner CAN describe, so it
    # says so and marks the run ended. That keeps the watchdog's snapshot for the
    # one case nobody can describe from the inside.
    Write-Log "FATAL in the suite loop: $_"
    Write-Host "  FATAL: $_" -ForegroundColor Red
    Complete-RunForensics -Status "fatal: $_"
    throw
}

# ── Final cleanup ──
# Runs on the abort path too: this is what stops an interrupted run from leaving
# live psmux servers behind.
Clean-Server

# The flag has been consumed. Clear it so the next run is not aborted on its
# first poll by a stale file.
Remove-Item $script:StopFile -Force -ErrorAction SilentlyContinue

# ── Stop the spawn watcher, then state the desktop outcome ───────────────────
# Stopping it by the pid we started is the only kill here; it is never looked up
# by image name, because another session's pwsh is not ours to end.
if ($script:SpawnWatcher) {
    try { [System.IO.File]::WriteAllText($script:CurrentSuiteFile, '<run finished>') } catch { }
    if (Test-KillTargetSafe -TargetPid $script:SpawnWatcher.Id -Reason 'end of run, stopping the spawn watcher we started') {
        try { Stop-Process -Id $script:SpawnWatcher.Id -Force -ErrorAction Stop } catch { }
    }
    Write-Log "Spawn watcher pid $($script:SpawnWatcher.Id) stopped"
}

# The run reached its own end. The watchdog reads this and leaves quietly; a run
# that disappears without it is the event #680 is about, and the watchdog will
# say so in runner_vanished.log.
Complete-RunForensics -Status $(if ($script:RunAborted) { "interrupted ($script:AbortReason)" } else { 'finished' })

# Final desktop reconciliation: anything left that was not in the baseline is
# reported by handle, so a stuck window is visible in the summary rather than
# discovered by a human the next morning.
$script:AuditStrayWindows = @()
try {
    foreach ($w in [PsmuxWinAudit]::List()) {
        if ($script:AuditBaselineWins.ContainsKey([long]$w.Handle)) { continue }
        if (Test-AuditWindowProtected $w) { continue }
        $script:AuditStrayWindows += $w
        Write-AuditLine ("RUN-END-STRAY hwnd={0} pid={1} class={2} title=[{3}]" -f $w.Handle, $w.Pid, $w.Class, $w.Title)
    }
} catch { }

# ── Generate Report ──
$endTime = Get-Date
$totalDuration = ($endTime - $startTime).TotalSeconds

$bullet = [char]0x25CF  # ●

Write-Host "`n"
if ($script:RunAborted) {
    Write-Host ("=" * 80) -ForegroundColor Yellow
    Write-Host "  RUN INTERRUPTED ($script:AbortReason)" -ForegroundColor Yellow
    Write-Host ("  {0} suites did not run. Results below cover only what completed." -f $script:NotRunCount) -ForegroundColor Yellow
    Write-Host "  Resume where this left off:  tests\run_full_interactive.cmd -Resume" -ForegroundColor Yellow
    Write-Host ("=" * 80) -ForegroundColor Yellow
}
Write-Host ("=" * 80) -ForegroundColor White
Write-Host "  COMPREHENSIVE TEST REPORT" -ForegroundColor White
Write-Host "  $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')" -ForegroundColor DarkGray
Write-Host ("=" * 80) -ForegroundColor White

$passed = @($results | Where-Object { $_.Status -eq "PASS" })
$failed = @($results | Where-Object { $_.Status -in @("FAIL","ERROR","TIMEOUT") })
$skipped = @($results | Where-Object { $_.Status -eq "SKIP" })

$totalTests = 0; $totalPassed = 0; $totalFailed = 0
foreach ($r in $results) { $totalTests += ($r.Passed + $r.Failed); $totalPassed += $r.Passed; $totalFailed += $r.Failed }

# ── Suite & Test Counters ──
Write-Host ""
Write-Host "  SUITE SUMMARY" -ForegroundColor Cyan
Write-Host "  -------------------------------------------------------"
Write-Host ("  $bullet Suites PASSED:  {0}" -f $passed.Count) -ForegroundColor Green
Write-Host ("  $bullet Suites FAILED:  {0}" -f $failed.Count) -ForegroundColor $(if ($failed.Count -gt 0) { "Red" } else { "Green" })
Write-Host ("  $bullet Suites SKIPPED: {0}" -f $skipped.Count) -ForegroundColor Yellow
Write-Host ""
Write-Host "  INDIVIDUAL TEST SUMMARY" -ForegroundColor Cyan
Write-Host "  -------------------------------------------------------"
Write-Host ("  $bullet Tests PASSED:   {0}" -f $totalPassed) -ForegroundColor Green
Write-Host ("  $bullet Tests FAILED:   {0}" -f $totalFailed) -ForegroundColor $(if ($totalFailed -gt 0) { "Red" } else { "Green" })
Write-Host ("  $bullet Total Duration: {0:F1}s ({1:F1} min)" -f $totalDuration, ($totalDuration / 60))

# ── Category-Wise Breakdown ──
Write-Host ""
Write-Host ("=" * 80) -ForegroundColor White
Write-Host "  CATEGORY BREAKDOWN" -ForegroundColor White
Write-Host ("=" * 80) -ForegroundColor White
Write-Host ""
Write-Host ("  {0,-16} {1,6} {2,6} {3,6} {4,10}" -f "Category", "Pass", "Fail", "Skip", "Time") -ForegroundColor White
Write-Host ("  " + ("-" * 50)) -ForegroundColor DarkGray

$catStats = @{}
foreach ($r in $results) {
    $cat = Get-Category $r.Name
    if (-not $catStats.ContainsKey($cat)) {
        $catStats[$cat] = @{ Pass=0; Fail=0; Skip=0; Time=[double]0 }
    }
    switch ($r.Status) {
        "PASS"    { $catStats[$cat].Pass++ }
        "FAIL"    { $catStats[$cat].Fail++ }
        "TIMEOUT" { $catStats[$cat].Fail++ }
        "ERROR"   { $catStats[$cat].Fail++ }
        "SKIP"    { $catStats[$cat].Skip++ }
    }
    $catStats[$cat].Time += $r.Duration
}

foreach ($kv in ($catStats.GetEnumerator() | Sort-Object { $_.Value.Fail } -Descending)) {
    $c = $kv.Value
    $catColor = if ($c.Fail -gt 0) { "Red" } elseif ($c.Skip -gt 0 -and $c.Pass -eq 0) { "Yellow" } else { "Green" }
    $timeFmt = if ($c.Time -ge 60) { "{0:F0}m{1:F0}s" -f [math]::Floor($c.Time/60),[math]::Floor($c.Time%60) } else { "{0:F1}s" -f $c.Time }
    Write-Host ("  {0,-16} {1,6} {2,6} {3,6} {4,10}" -f $kv.Key, $c.Pass, $c.Fail, $c.Skip, $timeFmt) -ForegroundColor $catColor
}

# ── Failures first, then passed, then skipped ──
if ($failed.Count -gt 0) {
    Write-Host ""
    Write-Host ("  " + ("-" * 55)) -ForegroundColor Red
    Write-Host "  FAILED SUITES" -ForegroundColor Red
    foreach ($r in $failed) {
        $lv = if ($r.Leftovers) { "  LEFT:$($r.Leftovers)" } else { "" }
        Write-Host ("    $bullet [{0}] {1,-42} {2,3}P/{3}F  ({4}s){5}" -f $r.Status, $r.Name, $r.Passed, $r.Failed, $r.Duration, $lv) -ForegroundColor Red
    }
}

if ($passed.Count -gt 0) {
    Write-Host ""
    Write-Host "  PASSED SUITES" -ForegroundColor Green
    foreach ($r in $passed) {
        $lv = if ($r.Leftovers) { "  LEFT:$($r.Leftovers)" } else { "" }
        Write-Host ("    $bullet [PASS] {0,-42} {1,3}P/{2}F  ({3}s){4}" -f $r.Name, $r.Passed, $r.Failed, $r.Duration, $lv) -ForegroundColor $(if ($r.Leftovers) { "Magenta" } else { "Green" })
    }
}

if ($skipped.Count -gt 0) {
    Write-Host ""
    Write-Host "  SKIPPED SUITES" -ForegroundColor Yellow
    foreach ($r in $skipped) {
        Write-Host ("    $bullet [SKIP] {0,-42} {1}" -f $r.Name, $r.Reason) -ForegroundColor Yellow
    }
}

# ── Desktop hygiene ────────────────────────────────────────────────────────
# A suite must leave the desktop as it found it. This block is the whole point of
# the per suite audit: it names the suites that did not, so the next person does
# not have to guess which of 650 suites produced the dead tabs.
$leftoverSuites = @($results | Where-Object { $_.Leftovers -and $_.Leftovers -gt 0 })
Write-Host ""
Write-Host ("=" * 80) -ForegroundColor White
Write-Host "  DESKTOP HYGIENE (console windows / parked consoles left behind)" -ForegroundColor White
Write-Host ("=" * 80) -ForegroundColor White
Write-Host ""
if ($leftoverSuites.Count -eq 0) {
    Write-Host "  No suite left a console process or window behind. Desktop is clean." -ForegroundColor Green
} else {
    Write-Host ("  {0,-48} {1,10} {2,8}" -f "Suite", "Leftovers", "Verdict") -ForegroundColor White
    Write-Host ("  " + ("-" * 70)) -ForegroundColor DarkGray
    foreach ($r in ($leftoverSuites | Sort-Object { -$_.Leftovers })) {
        Write-Host ("  {0,-48} {1,10} {2,8}" -f $r.Name, $r.Leftovers, $r.Status) -ForegroundColor Magenta
    }
    Write-Host ""
    Write-Host ("  {0} leftover process(es)/window(s) across {1} suite(s); all were logged and cleaned." -f `
        $script:AuditTotalLeftovers, $script:AuditSuitesWithLeftovers) -ForegroundColor Magenta
    Write-Host ("  Per leftover detail (pid, parent chain, command line): {0}" -f $script:AuditLog) -ForegroundColor DarkGray
    Write-Host ("  Who spawned it, captured at spawn time:                {0}" -f $script:SpawnTrace) -ForegroundColor DarkGray
}
if ($script:AuditStrayWindows.Count -gt 0) {
    Write-Host ""
    Write-Host ("  WARNING: {0} window(s) would not close and are STILL on the desktop:" -f $script:AuditStrayWindows.Count) -ForegroundColor Red
    foreach ($w in $script:AuditStrayWindows) {
        Write-Host ("    hwnd={0} pid={1} class={2} title=[{3}]" -f $w.Handle, $w.Pid, $w.Class, $w.Title) -ForegroundColor Red
    }
}

# ── Performance chart (top 15 slowest, visual bar) ──
Write-Host "`n"
Write-Host ("=" * 80) -ForegroundColor White
Write-Host "  PERFORMANCE METRICS (top 15 slowest suites)" -ForegroundColor White
Write-Host ("=" * 80) -ForegroundColor White
Write-Host ""
$perfResults = $results | Where-Object { $_.Status -ne "SKIP" } | Sort-Object { $_.Duration } -Descending | Select-Object -First 15
$maxDur = ($perfResults | Measure-Object -Property Duration -Maximum).Maximum
if ($maxDur -lt 1) { $maxDur = 1 }
$barBlock = [char]0x2588
foreach ($r in $perfResults) {
    $barLen = [math]::Max([math]::Round(($r.Duration / $maxDur) * 30), 1)
    $bar = $barBlock.ToString() * $barLen
    $color = if ($r.Status -eq "PASS") { "Green" } elseif ($r.Status -eq "FAIL") { "Red" } else { "Yellow" }
    Write-Host ("  {0,-42} {1,7:F1}s " -f $r.Name, $r.Duration) -ForegroundColor DarkGray -NoNewline
    Write-Host $bar -ForegroundColor $color
}

Write-Host "`n"
Write-Host ("=" * 80) -ForegroundColor White
# An interrupted run has no verdict. Reporting "ALL TESTS PASSED" here because
# nothing had failed yet at the moment of the abort would be a lie in the one
# place people grep for a result.
if ($script:RunAborted) {
    Write-Host "  RESULT: INTERRUPTED ($script:AbortReason) - $script:NotRunCount suites did not run" -ForegroundColor Yellow
    Write-Log "=== FINAL RESULT: INTERRUPTED ($script:AbortReason) - $script:NotRunCount of $totalSuites suites did not run ==="
} elseif ($totalFailed -gt 0 -or $failed.Count -gt 0) {
    Write-Host "  RESULT: FAILURES DETECTED ($totalFailed tests failed, $($failed.Count) suites failed/timed out)" -ForegroundColor Red
    Write-Log "=== FINAL RESULT: FAILURES DETECTED ($totalFailed tests failed across $($failed.Count) suites) ==="
} else {
    Write-Host "  RESULT: ALL TESTS PASSED ($totalPassed tests across $($passed.Count) suites)" -ForegroundColor Green
    Write-Log "=== FINAL RESULT: ALL TESTS PASSED ($totalPassed tests across $($passed.Count) suites) ==="
}

# ── Write comprehensive summary.log ──────────────────────────────
$summaryLines = [System.Collections.ArrayList]::new()
[void]$summaryLines.Add("psmux Test Run Summary")
[void]$summaryLines.Add("Run ID:   $script:RunId")
[void]$summaryLines.Add("Started:  $($startTime.ToString('yyyy-MM-dd HH:mm:ss'))")
[void]$summaryLines.Add("Finished: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')")
[void]$summaryLines.Add("Duration: $([math]::Round($totalDuration,1))s ($([math]::Round($totalDuration/60,1)) min)")
[void]$summaryLines.Add("Binary:   $PSMUX")
[void]$summaryLines.Add("Params:   SkipPerf=$SkipPerf PerfOnly=$PerfOnly IncludeWSL=$IncludeWSL IncludeInteractive=$IncludeInteractive")
[void]$summaryLines.Add("")
[void]$summaryLines.Add("Suites PASSED:  $($passed.Count)")
[void]$summaryLines.Add("Suites FAILED:  $($failed.Count)")
[void]$summaryLines.Add("Suites SKIPPED: $($skipped.Count)")
[void]$summaryLines.Add("Tests PASSED:   $totalPassed")
[void]$summaryLines.Add("Tests FAILED:   $totalFailed")
[void]$summaryLines.Add("Leftover console processes/windows: $script:AuditTotalLeftovers (across $script:AuditSuitesWithLeftovers suite(s))")
if ($leftoverSuites.Count -gt 0) {
    foreach ($r in ($leftoverSuites | Sort-Object { -$_.Leftovers })) {
        [void]$summaryLines.Add("  LEFTOVERS $($r.Leftovers)  $($r.Name)  [$($r.Status)]")
    }
}
if ($script:AuditStrayWindows.Count -gt 0) {
    [void]$summaryLines.Add("STILL ON DESKTOP: $($script:AuditStrayWindows.Count) window(s) would not close")
}
[void]$summaryLines.Add("")
[void]$summaryLines.Add("=" * 70)
foreach ($r in $results) {
    $line = "[{0,-5}] {1,-45} {2,3}P/{3}F  {4,7:F1}s  LEFT:{5}" -f $r.Status, $r.Name, $r.Passed, $r.Failed, $r.Duration, $(if ($r.Leftovers) { [int]$r.Leftovers } else { 0 })
    if ($r.Reason) { $line += "  ($($r.Reason))" }
    [void]$summaryLines.Add($line)
}
[void]$summaryLines.Add("=" * 70)
if ($script:RunAborted) {
    [void]$summaryLines.Add("RESULT: INTERRUPTED ($script:AbortReason) - $script:NotRunCount suites did not run")
    [void]$summaryLines.Add("Resume with: tests\run_full_interactive.cmd -Resume")
} elseif ($totalFailed -gt 0 -or $failed.Count -gt 0) {
    [void]$summaryLines.Add("RESULT: FAILURES DETECTED")
} else {
    [void]$summaryLines.Add("RESULT: ALL TESTS PASSED")
}
[System.IO.File]::WriteAllText($script:SummaryLog, ($summaryLines -join "`r`n"), [System.Text.Encoding]::UTF8)

Write-Log "Summary written to: $script:SummaryLog"
Write-Log "Suite logs in:      $script:SuiteDir"
Write-Log "=== Run finished ==="

# Every perf suite writes its numbers as JSON under
# %USERPROFILE%\.psmux-test-data\metrics (or PSMUX_METRICS_DIR). When this run
# included any of them, read the history back and print the trend: each
# headline number's newest value against the median of the five runs before
# it. A reading, not a verdict: it never changes the exit code.
$perfRan = @($results | Where-Object { $perfTests -contains $_.Name -and $_.Status -ne "SKIP" })
if ($perfRan.Count -gt 0 -and (Test-Path "$PSScriptRoot\perf_summary.ps1")) {
    $trendFile = Join-Path $script:RunDir "perf_trend.txt"
    try {
        & pwsh -NoProfile -File "$PSScriptRoot\perf_summary.ps1" -Metric T 2>&1 | Tee-Object -FilePath $trendFile | Out-Host
        Write-Log "Perf trend written to: $trendFile"
    } catch { Write-Log "perf_summary.ps1 failed: $_" }
}

Write-Host ""
Write-Host "  Logs saved to: $script:RunDir" -ForegroundColor Cyan

# 130 is the conventional "terminated by SIGINT" code. It is deliberately NOT 1:
# an interrupted run has no verdict, and reporting it as a failure would make an
# aborted sweep look like a red one in any wrapper that keys off the exit code.
if ($script:RunAborted) {
    Write-Host ""
    Write-Host "  RUN INTERRUPTED - $script:NotRunCount suites did not run." -ForegroundColor Yellow
    Write-Host "  Resume with: tests\run_full_interactive.cmd -Resume" -ForegroundColor Yellow
    exit 130
} elseif ($totalFailed -gt 0 -or $failed.Count -gt 0) {
    exit 1
} else {
    exit 0
}
