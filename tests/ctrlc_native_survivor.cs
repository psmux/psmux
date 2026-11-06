// Helper for tests/test_issue989_detach_ctrlc.ps1.
//
// A plain native (non-MSYS) console program.  In a pane under Git bash its mere
// presence makes psmux's Ctrl+C router take the "keep PROCESSED_INPUT" branch,
// so it SURVIVES Ctrl+C (its handler returns TRUE) to stay on the console for
// every press of a burst.  What else it does depends on the mode:
//
//   attach <log-file> [max-seconds]
//     Watches its console's process list and, the moment a psmux server joins
//     it, raises ONE CTRL_C_EVENT on the console: an event in flight while the
//     server is attached, at every press.  Appends "fire" to <log-file> for each
//     event raised and "recv" for each CTRL_C_EVENT received.
//
//   storm <interval-ms> <log-file> [max-seconds]
//     Raises CTRL_C_EVENT on the console every <interval-ms>, attached or not.
//     Dispatch is asynchronous, so some of these straddle the moment the server
//     detaches and reach it just after.  Appends a "fire" line per 100 events.
//
//   bit <log-file>
//     Not a pane program: started by run-shell.  Reports whether it was born
//     holding the "ignore Ctrl+C" bit, by raising CTRL_C_EVENT on its own console
//     with a handler registered.  The bit stops the handler running.  Appends
//     "clear" or "IGNORE".
//
// Pane modes print READY once the handler is installed, exit when the console
// goes away so they can never linger, and exit after max-seconds (default 60).
using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;

static class CtrlcNativeSurvivor
{
    delegate bool HandlerRoutine(uint ctrlType);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetConsoleCtrlHandler(HandlerRoutine handler, bool add);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GenerateConsoleCtrlEvent(uint ctrlEvent, uint processGroupId);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint GetConsoleProcessList(uint[] list, uint count);
    [DllImport("winmm.dll")]
    static extern uint timeBeginPeriod(uint ms);

    static string logFile;
    static HandlerRoutine keep;   // keep the delegate alive for the process lifetime
    static readonly object fileLock = new object();
    static int received;

    static void Log(string what)
    {
        lock (fileLock)
        {
            try { File.AppendAllText(logFile, what + "\n"); } catch { }
        }
    }

    static bool Handler(uint ctrlType)
    {
        if (ctrlType == 0 /* CTRL_C_EVENT */ || ctrlType == 1 /* CTRL_BREAK_EVENT */)
        {
            Interlocked.Increment(ref received);
            if (logFile != null) Log("recv");
            return true;   // handled: stay alive
        }
        return false;      // close / logoff / shutdown: default handler exits
    }

    static bool IsPsmux(uint pid)
    {
        try
        {
            string n = Process.GetProcessById((int)pid).ProcessName.ToLowerInvariant();
            // The test names a renamed build (an A/B binary) in this variable.
            string extra = (Environment.GetEnvironmentVariable("PSMUX_T989_SERVER_NAME") ?? "").ToLowerInvariant();
            return n == "psmux" || n == "pmux" || n == "tmux" || (extra.Length > 0 && n == extra);
        }
        catch { return false; }
    }

    static int Attach(int maxSeconds)
    {
        var cache = new System.Collections.Generic.Dictionary<uint, bool>();
        var list = new uint[64];
        bool attached = false;
        var clock = Stopwatch.StartNew();
        while (clock.Elapsed.TotalSeconds < maxSeconds)
        {
            uint n = GetConsoleProcessList(list, (uint)list.Length);
            if (n == 0) return 0;
            bool now = false;
            for (uint i = 0; i < n && i < list.Length; i++)
            {
                bool v;
                if (!cache.TryGetValue(list[i], out v)) { v = IsPsmux(list[i]); cache[list[i]] = v; }
                if (v) { now = true; break; }
            }
            if (now && !attached && GenerateConsoleCtrlEvent(0 /* CTRL_C_EVENT */, 0)) Log("fire");
            attached = now;
            // No sleep: since the Ctrl+C router answers from one process
            // table, the server stays attached for about a millisecond, and a
            // 1 ms sleep let most attaches pass unseen (20 of 20 caught before,
            // 4 to 13 after).  Yield instead, for at most max-seconds.
            Thread.Yield();
        }
        return 0;
    }

    static int Storm(int intervalMs, int maxSeconds)
    {
        string log = logFile;
        logFile = null;   // the handler must not slow the storm with file writes
        var list = new uint[64];
        int raised = 0;
        var clock = Stopwatch.StartNew();
        while (clock.Elapsed.TotalSeconds < maxSeconds)
        {
            if (GetConsoleProcessList(list, (uint)list.Length) == 0) return 0;
            if (GenerateConsoleCtrlEvent(0 /* CTRL_C_EVENT */, 0) && ++raised % 100 == 0)
                File.AppendAllText(log, "fire\n");
            Thread.Sleep(intervalMs);
        }
        return 0;
    }

    static int Bit()
    {
        string log = logFile;
        logFile = null;
        keep = Handler;
        SetConsoleCtrlHandler(keep, true);
        if (!GenerateConsoleCtrlEvent(0 /* CTRL_C_EVENT */, 0)) { File.AppendAllText(log, "unknown\n"); return 0; }
        Thread.Sleep(150);
        File.AppendAllText(log, (received > 0 ? "clear" : "IGNORE") + "\n");
        return 0;
    }

    static int Usage()
    {
        Console.Error.WriteLine("usage: ctrlc_native_survivor attach <log> [max-s] | storm <ms> <log> [max-s] | bit <log>");
        return 2;
    }

    static int Main(string[] args)
    {
        if (args.Length < 2) return Usage();
        string mode = args[0];
        if (mode == "bit") { logFile = args[1]; return Bit(); }
        if (mode != "attach" && mode != "storm") return Usage();

        keep = Handler;
        SetConsoleCtrlHandler(keep, true);
        timeBeginPeriod(1);   // 1 ms polls and sleeps
        Console.WriteLine("READY");
        Console.Out.Flush();

        if (mode == "attach")
        {
            logFile = args[1];
            return Attach(args.Length > 2 ? int.Parse(args[2]) : 60);
        }
        if (args.Length < 3) return Usage();
        logFile = args[2];
        return Storm(int.Parse(args[1]), args.Length > 3 ? int.Parse(args[3]) : 60);
    }
}
