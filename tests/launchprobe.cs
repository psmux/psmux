// launchprobe.exe: launch a program in a NEW console and time, from one QPC
// clock, (a) CreateProcess returning, (b) the first time a pwsh prompt
// ("PS X:\") is visible in that console's screen buffer. Writes "key value"
// lines to --out (QPC ticks: t0 before CreateProcess, created, attached, visible).
//
// This is the launch number a user sees: for psmux it is the attached CLIENT's
// console, so it includes the server, the pane shell, the first frame and the
// client's draw; for a bare shell it is the shell's own console. Polling the
// screen costs one ReadConsoleOutputCharacter per millisecond (timeBeginPeriod
// 1), the same for every cell. Used by tests/test_launch_visible_prompt_gate.ps1.
//
// usage: launchprobe.exe --out f --cmd "full command line" [--timeout 20000]
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;

class P {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct STARTUPINFO { public int cb; public string r; public string d; public string t; public int x, y, xs, ys, xc, yc, fill, flags; public short show, r2; public IntPtr r3, i, o, e; }
    [StructLayout(LayoutKind.Sequential)]
    struct PI { public IntPtr hp, ht; public int pid, tid; }
    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X, Y; }
    [StructLayout(LayoutKind.Sequential)] struct SR { public short L, T, R, B; }
    [StructLayout(LayoutKind.Sequential)] struct CSBI { public COORD size, cur; public short attr; public SR win; public COORD max; }
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CreateProcessW(string app, StringBuilder cmd, IntPtr pa, IntPtr ta, bool inh, uint flags, IntPtr env, string dir, ref STARTUPINFO si, out PI pi);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool AttachConsole(int pid);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateFileW(string n, uint a, uint s, IntPtr sa, uint c, uint f, IntPtr t);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")] static extern bool GetConsoleScreenBufferInfo(IntPtr h, out CSBI i);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    static extern bool ReadConsoleOutputCharacterW(IntPtr h, [Out] char[] b, uint len, COORD at, out uint read);
    [DllImport("winmm.dll")] static extern uint timeBeginPeriod(uint p);

    static int Main(string[] argv) {
        var a = new Dictionary<string, string>();
        for (int i = 0; i + 1 < argv.Length; i += 2) a[argv[i].TrimStart('-')] = argv[i + 1];
        string outf = a["out"]; string cmd = a["cmd"];
        int timeout = a.ContainsKey("timeout") ? int.Parse(a["timeout"]) : 20000;
        var re = new Regex(@"PS [A-Za-z]:\\");
        timeBeginPeriod(1);
        FreeConsole();
        var si = new STARTUPINFO(); si.cb = Marshal.SizeOf(si);
        PI pi;
        long t0 = Stopwatch.GetTimestamp();
        bool ok = CreateProcessW(null, new StringBuilder(cmd), IntPtr.Zero, IntPtr.Zero, false, 0x10 /*CREATE_NEW_CONSOLE*/, IntPtr.Zero, null, ref si, out pi);
        long tc = Stopwatch.GetTimestamp();
        var lines = new List<string>();
        lines.Add("freq " + Stopwatch.Frequency);
        lines.Add("t0 " + t0);
        if (!ok) { lines.Add("error createprocess " + Marshal.GetLastWin32Error()); File.WriteAllLines(outf, lines); return 2; }
        lines.Add("pid " + pi.pid);
        lines.Add("created " + tc);
        long tAttach = 0, tVis = 0;
        var sw = Stopwatch.StartNew();
        IntPtr h = IntPtr.Zero;
        char[] buf = new char[1 << 16];
        while (sw.ElapsedMilliseconds < timeout) {
            if (h == IntPtr.Zero) {
                if (AttachConsole(pi.pid)) {
                    h = CreateFileW("CONOUT$", 0xC0000000, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
                    if (h == new IntPtr(-1)) { h = IntPtr.Zero; FreeConsole(); }
                    else tAttach = Stopwatch.GetTimestamp();
                }
                if (h == IntPtr.Zero) { Thread.Sleep(1); continue; }
            }
            CSBI ci;
            if (GetConsoleScreenBufferInfo(h, out ci)) {
                int w = ci.size.X; int top = ci.win.T; int rows = ci.win.B - ci.win.T + 1;
                int n = Math.Min(buf.Length, w * rows);
                COORD at; at.X = 0; at.Y = (short)top; uint rd;
                if (ReadConsoleOutputCharacterW(h, buf, (uint)n, at, out rd) && rd > 0) {
                    if (re.IsMatch(new string(buf, 0, (int)rd))) { tVis = Stopwatch.GetTimestamp(); break; }
                }
            }
            Thread.Sleep(1);
        }
        lines.Add("attached " + tAttach);
        lines.Add("visible " + tVis);
        if (h != IntPtr.Zero) CloseHandle(h);
        FreeConsole();
        File.WriteAllLines(outf, lines);
        return tVis > 0 ? 0 : 1;
    }
}
