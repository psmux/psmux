using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

// Per character typing latency, measured where the user looks: the console
// buffer of the process that owns the window (the psmux CLIENT, or a plain
// shell for the baseline).
//
// One process both types and watches, so both timestamps come from the same
// QueryPerformanceCounter clock: latency(i) = first time char i is visible
// minus the time char i was written with WriteConsoleInput. There is no poll
// period hidden in the number other than the 1 ms watch loop.
//
// Modes:
//   type   <pid> <text> <interval_ms>
//          Read the cursor cell first, then type <text>; char i counts as
//          visible when the screen, read from that cell onward, starts with
//          the first i chars of <text>.
//   ctrlc  <pid> <text> <interval_ms> <delay_ms>
//          Press Ctrl+C, wait <delay_ms>, then type <text>. The prompt is
//          redrawn on a new line, so the start cell is unknown in advance:
//          char i counts as visible when the first i chars of <text> appear
//          anywhere in the window. Use a <text> that starts with characters
//          the screen does not already hold.
//   line   <pid> <text>
//          Type <text> and Enter. No measurement (used to run `clear`).
//
// Output: "LAT <i> <ms>" per visible char, "MISS <i>" per char never seen
// within the timeout, then one "SUMMARY n=.. seen=.. p50=.. p99=.. max=..".
class TypingLatencyProbe {
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr CreateFile(string name, uint access, uint share, IntPtr sa, uint disp, uint flags, IntPtr tmpl);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool WriteConsoleInput(IntPtr h, INPUT_RECORD[] buf, uint len, out uint written);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool ReadConsoleOutputCharacter(IntPtr h, StringBuilder sb, uint n, COORD c, out uint read);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetConsoleScreenBufferInfo(IntPtr h, out CSBI i);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("winmm.dll")] static extern uint timeBeginPeriod(uint p);

    [StructLayout(LayoutKind.Explicit)] struct INPUT_RECORD { [FieldOffset(0)] public ushort EventType; [FieldOffset(4)] public KEY_EVENT_RECORD KeyEvent; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct KEY_EVENT_RECORD { public int bKeyDown; public ushort wRepeatCount; public ushort wVirtualKeyCode; public ushort wVirtualScanCode; public char UnicodeChar; public uint dwControlKeyState; }
    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X, Y; public COORD(short x, short y) { X = x; Y = y; } }
    [StructLayout(LayoutKind.Sequential)] struct SR { public short L, T, R, B; }
    [StructLayout(LayoutKind.Sequential)] struct CSBI { public COORD size; public COORD cur; public ushort attr; public SR win; public COORD max; }

    static IntPtr hIn, hOut;

    static void Key(char c, ushort vk, uint ctrl) {
        var r = new INPUT_RECORD[2];
        r[0].EventType = 1; r[0].KeyEvent.bKeyDown = 1; r[0].KeyEvent.wRepeatCount = 1;
        r[0].KeyEvent.UnicodeChar = c; r[0].KeyEvent.wVirtualKeyCode = vk; r[0].KeyEvent.dwControlKeyState = ctrl;
        r[1] = r[0]; r[1].KeyEvent.bKeyDown = 0;
        uint w; WriteConsoleInput(hIn, r, 2, out w);
    }

    static string ReadRows(short y0, short x0, short y1, int width) {
        var all = new StringBuilder();
        for (short y = y0; y < y1; y++) {
            short x = (y == y0) ? x0 : (short)0;
            var sb = new StringBuilder(width); uint rd;
            ReadConsoleOutputCharacter(hOut, sb, (uint)(width - x), new COORD(x, y), out rd);
            all.Append(sb.ToString(0, (int)rd));
        }
        return all.ToString();
    }

    static int Main(string[] a) {
        if (a.Length < 3) { Console.Error.WriteLine("usage: type|ctrlc|line <pid> <text> [interval_ms] [delay_ms]"); return 1; }
        string mode = a[0]; uint pid = uint.Parse(a[1]); string text = a[2];
        int interval = a.Length > 3 ? int.Parse(a[3]) : 40;
        int delay = a.Length > 4 ? int.Parse(a[4]) : 0;
        timeBeginPeriod(1);
        FreeConsole();
        if (!AttachConsole(pid)) { Console.Error.WriteLine("AttachConsole failed: " + Marshal.GetLastWin32Error()); return 2; }
        hIn = CreateFile("CONIN$", 0xC0000000u, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        hOut = CreateFile("CONOUT$", 0xC0000000u, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        if (hIn == (IntPtr)(-1) || hOut == (IntPtr)(-1)) { Console.Error.WriteLine("CreateFile failed: " + Marshal.GetLastWin32Error()); return 3; }

        if (mode == "line") {
            foreach (char c in text) { Key(c, (ushort)(c == (char)27 ? 0x1B : 0), 0); Thread.Sleep(5); }
            Key('\r', 0x0D, 0);
            FreeConsole();
            return 0;
        }

        bool anywhere = mode == "ctrlc";
        CSBI c0; GetConsoleScreenBufferInfo(hOut, out c0);
        short sx = c0.cur.X, sy = c0.cur.Y; int width = c0.size.X;
        var injected = new long[text.Length + 1];
        var seen = new long[text.Length + 1];
        int typed = 0;

        var typer = new Thread(() => {
            if (anywhere) { Key((char)3, 0x43, 0x0008); Thread.Sleep(delay); }
            for (int i = 0; i < text.Length; i++) {
                injected[i + 1] = Stopwatch.GetTimestamp();
                Key(text[i], 0, 0);
                Volatile.Write(ref typed, i + 1);
                if (interval > 0) Thread.Sleep(interval);
            }
        });
        typer.Priority = ThreadPriority.AboveNormal;
        typer.Start();

        long deadline = Stopwatch.GetTimestamp() + Stopwatch.Frequency * ((long)text.Length * interval / 1000 + delay / 1000 + 4);
        int best = 0;
        while (best < text.Length && Stopwatch.GetTimestamp() < deadline) {
            string s = anywhere
                ? ReadRows(c0.win.T, 0, (short)(c0.win.B + 1), width)
                : ReadRows(sy, sx, (short)Math.Min(sy + 4, (int)c0.size.Y), width);
            int k = best;
            if (anywhere) { while (k < text.Length && s.Contains(text.Substring(0, k + 1))) k++; }
            else { k = 0; while (k < text.Length && k < s.Length && s[k] == text[k]) k++; }
            // A char cannot be visible before it was typed; anything else is
            // the screen already holding the text, and must not count.
            k = Math.Min(k, Volatile.Read(ref typed));
            if (k > best) {
                long t = Stopwatch.GetTimestamp();
                for (int j = best + 1; j <= k; j++) seen[j] = t;
                best = k;
            }
            Thread.Sleep(1);
        }
        typer.Join();
        FreeConsole();

        var lat = new List<double>();
        for (int i = 1; i <= text.Length; i++) {
            if (seen[i] == 0) { Console.WriteLine("MISS " + i); continue; }
            double ms = (seen[i] - injected[i]) * 1000.0 / Stopwatch.Frequency;
            lat.Add(ms);
            Console.WriteLine("LAT " + i + " " + ms.ToString("F1", System.Globalization.CultureInfo.InvariantCulture));
        }
        var sorted = new List<double>(lat); sorted.Sort();
        Func<double, string> pct = p => sorted.Count == 0 ? "-1" :
            sorted[Math.Min(sorted.Count - 1, (int)Math.Ceiling(p * sorted.Count) - 1 < 0 ? 0 : (int)Math.Ceiling(p * sorted.Count) - 1)].ToString("F1", System.Globalization.CultureInfo.InvariantCulture);
        Console.WriteLine("SUMMARY n=" + text.Length + " seen=" + lat.Count + " p50=" + pct(0.5) + " p99=" + pct(0.99) + " max=" + pct(1.0));
        return 0;
    }
}
