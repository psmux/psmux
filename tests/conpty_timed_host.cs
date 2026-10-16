// Timed ConPTY host (issue #841 follow up): plays a terminal emulator in
// front of a child, writing scripted bytes to a real pseudoconsole and
// stamping each write with GetSystemTimePreciseAsFileTime.
// Usage: conpty_timed_host.exe <logfile> <scriptfile> <outbin> -- <cmdline>
// Script lines: S <ms> | H <label> <hex> | T <label> <text>
//   | X <label> <command line>   (runs it through cmd /c, waits, logs its output)
// Each write logs "<unix ms> <label>" to logfile.
using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;

class ConptyTimedHost
{
    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X, Y; }
    [StructLayout(LayoutKind.Sequential)]
    struct STARTUPINFO {
        public int cb; public string lpReserved, lpDesktop, lpTitle;
        public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public short wShowWindow, cbReserved2; public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
    }
    [StructLayout(LayoutKind.Sequential)] struct STARTUPINFOEX { public STARTUPINFO StartupInfo; public IntPtr lpAttributeList; }
    [StructLayout(LayoutKind.Sequential)] struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }
    [DllImport("kernel32.dll")] static extern bool CreatePipe(out IntPtr r, out IntPtr w, IntPtr a, int n);
    [DllImport("kernel32.dll")] static extern int CreatePseudoConsole(COORD s, IntPtr i, IntPtr o, uint f, out IntPtr pc);
    [DllImport("kernel32.dll")] static extern void ClosePseudoConsole(IntPtr pc);
    [DllImport("kernel32.dll")] static extern bool InitializeProcThreadAttributeList(IntPtr l, int c, int f, ref IntPtr s);
    [DllImport("kernel32.dll")] static extern bool UpdateProcThreadAttribute(IntPtr l, uint f, IntPtr a, IntPtr v, IntPtr cb, IntPtr p, IntPtr r);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CreateProcess(string app, string cmd, IntPtr pp, IntPtr pt, bool inh, uint fl, IntPtr env, string dir, ref STARTUPINFOEX si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll")] static extern bool WriteFile(IntPtr h, byte[] b, int n, out int w, IntPtr o);
    [DllImport("kernel32.dll")] static extern bool ReadFile(IntPtr h, byte[] b, int n, out int r, IntPtr o);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr h, uint c);
    [DllImport("kernel32.dll")] static extern void GetSystemTimePreciseAsFileTime(out long ft);
    [DllImport("winmm.dll")] static extern uint timeBeginPeriod(uint p);
    static double Now() { long ft; GetSystemTimePreciseAsFileTime(out ft); return (ft - 116444736000000000L) / 10000.0; }
    static byte[] Hex(string h) { var b = new byte[h.Length / 2]; for (int i = 0; i < b.Length; i++) b[i] = Convert.ToByte(h.Substring(i * 2, 2), 16); return b; }

    static void Main(string[] args)
    {
        timeBeginPeriod(1);
        int sep = Array.IndexOf(args, "--");
        string log = args[0], script = args[1], outbin = args[2];
        string cmdline = string.Join(" ", args, sep + 1, args.Length - sep - 1);
        IntPtr inR, inW, outR, outW;
        CreatePipe(out inR, out inW, IntPtr.Zero, 0);
        CreatePipe(out outR, out outW, IntPtr.Zero, 0);
        COORD sz; sz.X = 120; sz.Y = 30;
        IntPtr pc; CreatePseudoConsole(sz, inR, outW, 0, out pc);
        IntPtr ls = IntPtr.Zero; InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref ls);
        IntPtr al = Marshal.AllocHGlobal(ls); InitializeProcThreadAttributeList(al, 1, 0, ref ls);
        UpdateProcThreadAttribute(al, 0, (IntPtr)0x00020016, pc, (IntPtr)IntPtr.Size, IntPtr.Zero, IntPtr.Zero);
        var si = new STARTUPINFOEX(); si.StartupInfo.cb = Marshal.SizeOf(typeof(STARTUPINFOEX)); si.lpAttributeList = al;
        PROCESS_INFORMATION pi;
        if (!CreateProcess(null, cmdline, IntPtr.Zero, IntPtr.Zero, false, 0x00080000, IntPtr.Zero, null, ref si, out pi))
        { File.WriteAllText(log, "CreateProcess failed " + Marshal.GetLastWin32Error()); Environment.Exit(2); }
        var lines = new List<string>();
        lines.Add(string.Format("{0:F3} CHILD {1}", Now(), pi.dwProcessId));
        var of = new FileStream(outbin, FileMode.Create, FileAccess.Write, FileShare.ReadWrite);
        var drain = new Thread(() => { var b = new byte[8192]; int n; try { while (ReadFile(outR, b, b.Length, out n, IntPtr.Zero) && n > 0) { lock (of) { of.Write(b, 0, n); of.Flush(); } } } catch { } });
        drain.IsBackground = true; drain.Start();
        int w;
        foreach (string raw in File.ReadAllLines(script))
        {
            string line = raw.TrimEnd();
            if (line.Length == 0 || line.StartsWith("#")) continue;
            var p = line.Split(new[] { ' ' }, 3);
            if (p[0] == "S") { Thread.Sleep(int.Parse(p[1])); continue; }
            if (p[0] == "X") {
                var psi = new System.Diagnostics.ProcessStartInfo("cmd.exe", "/c " + p[2]);
                psi.UseShellExecute = false; psi.RedirectStandardOutput = true; psi.CreateNoWindow = true;
                var pr = System.Diagnostics.Process.Start(psi); string o = pr.StandardOutput.ReadToEnd(); pr.WaitForExit();
                lines.Add(string.Format("{0:F3} {1} => {2}", Now(), p[1], o.Trim().Replace("\r", "").Replace("\n", " | ")));
                File.WriteAllLines(log, lines); continue;
            }
            byte[] data = p[0] == "H" ? Hex(p[2]) : System.Text.Encoding.UTF8.GetBytes(p[2]);
            double t = Now();
            WriteFile(inW, data, data.Length, out w, IntPtr.Zero);
            lines.Add(string.Format("{0:F3} {1}", t, p[1]));
            File.WriteAllLines(log, lines);
        }
        Thread.Sleep(500);
        File.WriteAllLines(log, lines);
        TerminateProcess(pi.hProcess, 0);
        try { lock (of) { of.Flush(); of.Close(); } } catch { }
        Environment.Exit(0);
    }
}
