// Issue #697 harness: host a child under a real pseudoconsole, drive it from a
// script file, and record every output byte with a timestamp so the caller can
// count what the HOST terminal would receive per keystroke (cursor hide/show,
// cursor moves, bytes).
//
// Usage: conpty697.exe <script> <outFile> <cols> <rows> <flags> <command...>
//   flags: 0 = default (conhost re-renders), 8 = PSEUDOCONSOLE_PASSTHROUGH_MODE
//
// Script verbs, one per line:
//   WAIT <ms>        sleep
//   TEXT <ascii>     write literal bytes (no CR)
//   HEX <hh hh ..>   write raw bytes
//   CR               write 0x0d
//   ESC              write 0x1b
//   MARK <name>      record a marker at the current output offset
//   END              stop: terminate the child and exit
//
// Output: <outFile> holds the raw byte stream, <outFile>.idx holds lines
//   CHUNK <offset> <len> <ms>   one per ReadFile
//   MARK <name> <offset> <ms>
//
// Compile: csc /nologo /optimize /out:conpty697.exe conpty697.cs
using System;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

static class ConPty697
{
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CreatePipe(out IntPtr hRead, out IntPtr hWrite, IntPtr sa, int size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern int CreatePseudoConsole(COORD size, IntPtr hInput, IntPtr hOutput, uint flags, out IntPtr phPC);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool ReadFile(IntPtr h, byte[] buf, uint toRead, out uint read, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool WriteFile(IntPtr h, byte[] buf, uint n, out uint written, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool InitializeProcThreadAttributeList(IntPtr l, int c, int f, ref IntPtr s);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool UpdateProcThreadAttribute(IntPtr l, uint f, IntPtr a, IntPtr v, IntPtr cb, IntPtr p, IntPtr r);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CreateProcess(string app, string cmd, IntPtr pa, IntPtr ta, bool inherit, uint flags, IntPtr env, string cwd, ref STARTUPINFOEX si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetStdHandle(int nStdHandle, IntPtr hHandle);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateProcess(IntPtr h, uint code);

    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X, Y; }
    [StructLayout(LayoutKind.Sequential)]
    struct STARTUPINFO { public int cb; public string r1; public string r2; public string r3; public int dx, dy, dxs, dys, dxc, dyc, fa; public int flags; public short showw; public short r4; public IntPtr r5; public IntPtr si, so, se; }
    [StructLayout(LayoutKind.Sequential)]
    struct STARTUPINFOEX { public STARTUPINFO StartupInfo; public IntPtr lpAttributeList; }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int pid, tid; }

    const uint EXTENDED_STARTUPINFO_PRESENT = 0x00080000;
    static readonly IntPtr PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = new IntPtr(0x00020016);

    static string Ms(Stopwatch sw)
    {
        return sw.Elapsed.TotalMilliseconds.ToString("F1", CultureInfo.InvariantCulture);
    }

    static void Main(string[] args)
    {
        if (args.Length < 6)
        {
            Console.Error.WriteLine("usage: conpty697.exe <script> <outFile> <cols> <rows> <flags> <command...>");
            Environment.Exit(2);
        }
        string[] script = File.ReadAllLines(args[0]);
        string outFile = args[1];
        short cols = short.Parse(args[2]);
        short rows = short.Parse(args[3]);
        uint ptyFlags = uint.Parse(args[4]);
        string cmd = string.Join(" ", args, 5, args.Length - 5);

        var idx = new StreamWriter(outFile + ".idx", false, Encoding.ASCII);
        idx.AutoFlush = true;
        var sw = Stopwatch.StartNew();

        IntPtr inRead, inWrite, outRead, outWrite;
        CreatePipe(out inRead, out inWrite, IntPtr.Zero, 0);
        CreatePipe(out outRead, out outWrite, IntPtr.Zero, 0);
        COORD size; size.X = cols; size.Y = rows;
        IntPtr hPC;
        int hr = CreatePseudoConsole(size, inRead, outWrite, ptyFlags, out hPC);
        idx.WriteLine("INFO CreatePseudoConsole hr=0x" + hr.ToString("X8") + " flags=" + ptyFlags);
        if (hr != 0) Environment.Exit(3);

        IntPtr lpSize = IntPtr.Zero;
        InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref lpSize);
        IntPtr attr = Marshal.AllocHGlobal(lpSize);
        InitializeProcThreadAttributeList(attr, 1, 0, ref lpSize);
        UpdateProcThreadAttribute(attr, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, hPC, (IntPtr)IntPtr.Size, IntPtr.Zero, IntPtr.Zero);
        // Without this the child inherits this process's std handle values and
        // bypasses the pseudoconsole (see tests/conptycap.cs).
        SetStdHandle(-10, IntPtr.Zero);
        SetStdHandle(-11, IntPtr.Zero);
        SetStdHandle(-12, IntPtr.Zero);

        var siex = new STARTUPINFOEX();
        siex.StartupInfo.cb = Marshal.SizeOf(typeof(STARTUPINFOEX));
        siex.lpAttributeList = attr;
        PROCESS_INFORMATION pi;
        bool ok = CreateProcess(null, cmd, IntPtr.Zero, IntPtr.Zero, false, EXTENDED_STARTUPINFO_PRESENT, IntPtr.Zero, null, ref siex, out pi);
        idx.WriteLine("INFO CreateProcess ok=" + ok + " err=" + Marshal.GetLastWin32Error() + " childPid=" + pi.pid);
        if (!ok) Environment.Exit(4);

        long total = 0;
        object gate = new object();
        var fs = new FileStream(outFile, FileMode.Create, FileAccess.Write, FileShare.ReadWrite);
        var reader = new Thread(() =>
        {
            byte[] buf = new byte[65536];
            while (true)
            {
                uint r;
                if (!ReadFile(outRead, buf, (uint)buf.Length, out r, IntPtr.Zero) || r == 0) break;
                lock (gate)
                {
                    try
                    {
                        fs.Write(buf, 0, (int)r);
                        fs.Flush();
                        idx.WriteLine("CHUNK " + total + " " + r + " " + Ms(sw));
                    }
                    catch { return; }
                    total += r;
                }
            }
        });
        reader.IsBackground = true;
        reader.Start();

        foreach (var raw in script)
        {
            string line = raw.TrimEnd('\r');
            if (line.Length == 0 || line.StartsWith("#")) continue;
            byte[] b = null;
            if (line.StartsWith("WAIT ")) { Thread.Sleep(int.Parse(line.Substring(5))); continue; }
            if (line.StartsWith("MARK "))
            {
                lock (gate) idx.WriteLine("MARK " + line.Substring(5) + " " + total + " " + Ms(sw));
                continue;
            }
            if (line == "END") break;
            if (line.StartsWith("TEXT ")) b = Encoding.ASCII.GetBytes(line.Substring(5));
            else if (line.StartsWith("HEX "))
            {
                var parts = line.Substring(4).Split(new char[] { ' ' }, StringSplitOptions.RemoveEmptyEntries);
                b = new byte[parts.Length];
                for (int i = 0; i < parts.Length; i++) b[i] = byte.Parse(parts[i], NumberStyles.HexNumber);
            }
            else if (line == "CR") b = new byte[] { 0x0d };
            else if (line == "ESC") b = new byte[] { 0x1b };
            if (b != null) { uint w; WriteFile(inWrite, b, (uint)b.Length, out w, IntPtr.Zero); }
        }
        lock (gate)
        {
            idx.WriteLine("MARK END " + total + " " + Ms(sw));
            try { fs.Flush(); fs.Close(); } catch { }
        }
        TerminateProcess(pi.hProcess, 0);
        Environment.Exit(0);
    }
}
