// Issue #503 control harness: run a child under a bare ConPTY, with NO psmux in
// the path, and dump every byte ConPTY hands back to a file.
//
// This isolates the platform from psmux. If the same generator loses bytes here
// too, the loss happens inside conhost/ConPTY before psmux's reader thread ever
// runs, and no change to psmux's reader loop can recover it.
//
// Modelled on tests/conpty_host_0xE.cs, which is the known-good ConPTY host in
// this repo (do not close the PTY-owned pipe ends, drain on a reader thread).
//
// Usage: conptycap.exe <outFile> <cols> <rows> <flags> <command line...>
//   flags: 0 = default (conhost re-renders), 8 = PSEUDOCONSOLE_PASSTHROUGH_MODE
//
// Compile: csc /nologo /optimize /out:conptycap.exe conptycap.cs

using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;

static class ConPtyCap
{
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CreatePipe(out IntPtr hRead, out IntPtr hWrite, IntPtr sa, int size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern int CreatePseudoConsole(COORD size, IntPtr hInput, IntPtr hOutput, uint flags, out IntPtr phPC);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern void ClosePseudoConsole(IntPtr hPC);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool ReadFile(IntPtr h, byte[] buf, uint toRead, out uint read, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint WaitForSingleObject(IntPtr h, uint ms);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool InitializeProcThreadAttributeList(IntPtr lpAttributeList, int dwAttributeCount, int dwFlags, ref IntPtr lpSize);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool UpdateProcThreadAttribute(IntPtr lpAttributeList, uint dwFlags, IntPtr Attribute, IntPtr lpValue, IntPtr cbSize, IntPtr lpPreviousValue, IntPtr lpReturnSize);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CreateProcess(string app, string cmd, IntPtr pa, IntPtr ta, bool inherit, uint flags, IntPtr env, string cwd, ref STARTUPINFOEX si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetStdHandle(int nStdHandle, IntPtr hHandle);

    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X, Y; }
    [StructLayout(LayoutKind.Sequential)]
    struct STARTUPINFO { public int cb; public string r1; public string r2; public string r3; public int dx, dy, dxs, dys, dxc, dyc, fa; public int flags; public short showw; public short r4; public IntPtr r5; public IntPtr si, so, se; }
    [StructLayout(LayoutKind.Sequential)]
    struct STARTUPINFOEX { public STARTUPINFO StartupInfo; public IntPtr lpAttributeList; }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int pid, tid; }

    const uint EXTENDED_STARTUPINFO_PRESENT = 0x00080000;
    static readonly IntPtr PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = new IntPtr(0x00020016);

    static string _logPath;

    // Diagnostics go to a file, never to stderr: this process must be launched
    // with its own console and no redirected handles, otherwise the child ends
    // up inheriting those handles instead of being attached to the pseudoconsole.
    static void Log(string msg)
    {
        try { File.AppendAllText(_logPath, msg + "\r\n"); } catch { }
    }

    static void Main(string[] args)
    {
        if (args.Length < 5)
        {
            Console.Error.WriteLine("usage: conptycap.exe <outFile> <cols> <rows> <flags> <command...>");
            Environment.Exit(2);
        }
        _logPath = args[0] + ".log";
        try { File.WriteAllText(_logPath, ""); } catch { }

        string outFile = args[0];
        short cols = short.Parse(args[1]);
        short rows = short.Parse(args[2]);
        uint ptyFlags = uint.Parse(args[3]);
        string cmd = string.Join(" ", args, 4, args.Length - 4);
        int drainMs = 30000;
        string drainEnv = Environment.GetEnvironmentVariable("CONPTYCAP_DRAIN_MS");
        if (!string.IsNullOrEmpty(drainEnv)) int.TryParse(drainEnv, out drainMs);

        IntPtr inRead, inWrite, outRead, outWrite;
        CreatePipe(out inRead, out inWrite, IntPtr.Zero, 0);
        CreatePipe(out outRead, out outWrite, IntPtr.Zero, 0);

        COORD size; size.X = cols; size.Y = rows;
        IntPtr hPC;
        int hr = CreatePseudoConsole(size, inRead, outWrite, ptyFlags, out hPC);
        Log("CreatePseudoConsole hr=0x" + hr.ToString("X8") + " flags=" + ptyFlags);
        if (hr != 0) Environment.Exit(3);

        IntPtr lpSize = IntPtr.Zero;
        InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref lpSize);
        IntPtr attr = Marshal.AllocHGlobal(lpSize);
        InitializeProcThreadAttributeList(attr, 1, 0, ref lpSize);
        UpdateProcThreadAttribute(attr, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, hPC, (IntPtr)IntPtr.Size, IntPtr.Zero, IntPtr.Zero);

        // This process owns a real console (it is launched from a shell), so
        // CreateProcess would propagate those console std handle VALUES into the
        // ConPTY child, where they bypass the pseudoconsole entirely: the child's
        // writes land on the parent's console and only console-API output such as
        // the title OSC reaches the PTY pipe. NULL them so the child binds fresh
        // handles to the pseudoconsole, matching how a GUI parent behaves.
        SetStdHandle(-10, IntPtr.Zero); // STD_INPUT_HANDLE
        SetStdHandle(-11, IntPtr.Zero); // STD_OUTPUT_HANDLE
        SetStdHandle(-12, IntPtr.Zero); // STD_ERROR_HANDLE

        var siex = new STARTUPINFOEX();
        siex.StartupInfo.cb = Marshal.SizeOf(typeof(STARTUPINFOEX));
        siex.lpAttributeList = attr;
        PROCESS_INFORMATION pi;
        bool ok = CreateProcess(null, cmd, IntPtr.Zero, IntPtr.Zero, false, EXTENDED_STARTUPINFO_PRESENT, IntPtr.Zero, null, ref siex, out pi);
        Log("CreateProcess ok=" + ok + " err=" + Marshal.GetLastWin32Error() + " childPid=" + pi.pid);
        if (!ok) Environment.Exit(4);

        long total = 0;
        var fs = new FileStream(outFile, FileMode.Create, FileAccess.Write);
        var reader = new Thread(() =>
        {
            byte[] buf = new byte[16384];
            while (true)
            {
                uint r;
                if (!ReadFile(outRead, buf, (uint)buf.Length, out r, IntPtr.Zero) || r == 0) break;
                fs.Write(buf, 0, (int)r);
                fs.Flush();
                total += r;
            }
        });
        reader.IsBackground = true;
        reader.Start();

        // Drain for a fixed window then exit hard. ClosePseudoConsole is not used
        // to stop the reader: it blocks indefinitely when conhost still has work
        // queued, which deadlocks the teardown.
        WaitForSingleObject(pi.hProcess, (uint)drainMs);
        Thread.Sleep(2000);
        try { fs.Flush(); fs.Close(); } catch { }
        Log("CONPTYCAP_DONE bytes=" + total);
        // Close the pseudo console before leaving, on a thread that may block,
        // with a bounded wait. Windows 10 (build 19045) keeps the pseudo
        // console's conhost.exe running after its owner exits without this,
        // and a caller that waits for the whole process tree (Start-Process
        // -Wait) then never returns: #626, #639 and #644 hung there. On
        // Windows 11 the conhost goes with its owner either way.
        var closer = new Thread(() => ClosePseudoConsole(hPC));
        closer.IsBackground = true;
        closer.Start();
        bool closed = closer.Join(3000);
        Log("ClosePseudoConsole returned=" + closed);
        Environment.Exit(0);
    }
}
