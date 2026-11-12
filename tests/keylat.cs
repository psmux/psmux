// keylat.exe - high resolution keystroke-to-screen latency probe.
//
// Measures the time from a key record being written into a console input
// buffer to the echoed character becoming visible in that console's screen
// buffer. Everything happens in ONE process so both timestamps come from the
// same QueryPerformanceCounter: no cross process clock skew, no 50ms polling
// like the older capture-pane based harnesses.
//
// The same code measures psmux (attach to the psmux CLIENT console) and a bare
// conhost (attach to the shell's own console), which is what makes the
// "psmux overhead over bare console" number honest: identical injection,
// identical oracle, identical timer.
//
// Oracles:
//   cursor      read the cell under the cursor, wait for it to become the
//               injected character. Right for a shell prompt that echoes at
//               the cursor.
//   cell:X,Y    watch one fixed cell. Used with echo_load_child.exe, which
//               echoes every byte at a fixed screen position, so the oracle
//               survives a pane that is scrolling under heavy output.
//
// Modes:
//   single      n independent trials: inject one char, time its appearance,
//               erase, wait. Steady state single keystroke latency.
//   type        inject a string at a fixed rate from one thread while another
//               thread watches the row grow, so every character gets its own
//               latency even when injection does not wait for the echo. This
//               is sustained typing and burst typing.
//   pollcost    calibration: how long one oracle read takes, ie the resolution
//               of every number above.
//
// Output is written to --out because the process detaches from its own console
// (FreeConsole) to attach to the target, so stdout would land on the screen
// being measured.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

class KeyLat
{
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateFileW(string name, uint access, uint share,
        IntPtr sec, uint disp, uint flags, IntPtr tmpl);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool WriteConsoleInput(IntPtr h, INPUT_RECORD[] buf, uint len, out uint written);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetConsoleScreenBufferInfo(IntPtr h, out CSBI info);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool ReadConsoleOutputCharacterW(IntPtr h, [Out] char[] buf,
        uint len, COORD coord, out uint read);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern short VkKeyScanW(char ch);
    [DllImport("user32.dll")]
    static extern uint MapVirtualKeyW(uint code, uint mapType);

    [StructLayout(LayoutKind.Sequential)]
    struct COORD { public short X, Y; }
    [StructLayout(LayoutKind.Sequential)]
    struct SMALL_RECT { public short Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)]
    struct CSBI
    {
        public COORD dwSize;
        public COORD dwCursorPosition;
        public ushort wAttributes;
        public SMALL_RECT srWindow;
        public COORD dwMaximumWindowSize;
    }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct KEY_EVENT_RECORD
    {
        public int bKeyDown;
        public ushort wRepeatCount;
        public ushort wVirtualKeyCode;
        public ushort wVirtualScanCode;
        public char UnicodeChar;
        public uint dwControlKeyState;
    }
    [StructLayout(LayoutKind.Explicit, CharSet = CharSet.Unicode)]
    struct INPUT_RECORD
    {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public KEY_EVENT_RECORD KeyEvent;
    }

    const ushort KEY_EVENT = 1;
    const uint SHIFT_PRESSED = 0x0010;
    const uint LEFT_CTRL_PRESSED = 0x0008;
    const uint LEFT_ALT_PRESSED = 0x0002;

    static IntPtr hIn = IntPtr.Zero;
    static IntPtr hOut = IntPtr.Zero;
    static readonly List<string> Log = new List<string>();

    static INPUT_RECORD Rec(bool down, ushort vk, char ch, uint ctrl)
    {
        var r = new INPUT_RECORD();
        r.EventType = KEY_EVENT;
        r.KeyEvent.bKeyDown = down ? 1 : 0;
        r.KeyEvent.wRepeatCount = 1;
        r.KeyEvent.wVirtualKeyCode = vk;
        r.KeyEvent.wVirtualScanCode = (ushort)MapVirtualKeyW(vk, 0);
        r.KeyEvent.UnicodeChar = ch;
        r.KeyEvent.dwControlKeyState = ctrl;
        return r;
    }

    static void KeyFor(char c, out ushort vk, out uint ctrl)
    {
        ctrl = 0;
        short scan = VkKeyScanW(c);
        if (scan == -1) { vk = 0; return; }
        vk = (ushort)(scan & 0xFF);
        int mods = (scan >> 8) & 0xFF;
        if ((mods & 1) != 0) ctrl |= SHIFT_PRESSED;
        if ((mods & 2) != 0) ctrl |= LEFT_CTRL_PRESSED;
        if ((mods & 4) != 0) ctrl |= LEFT_ALT_PRESSED;
    }

    static bool InjectChar(char c)
    {
        ushort vk; uint ctrl;
        KeyFor(c, out vk, out ctrl);
        var recs = new INPUT_RECORD[] { Rec(true, vk, c, ctrl), Rec(false, vk, c, 0) };
        uint w;
        return WriteConsoleInput(hIn, recs, 2, out w) && w == 2;
    }

    static bool InjectVk(ushort vk, char c)
    {
        var recs = new INPUT_RECORD[] { Rec(true, vk, c, 0), Rec(false, vk, c, 0) };
        uint w;
        return WriteConsoleInput(hIn, recs, 2, out w) && w == 2;
    }

    static readonly char[] Cell1 = new char[1];
    static char ReadCell(short x, short y)
    {
        COORD at; at.X = x; at.Y = y;
        uint read;
        if (!ReadConsoleOutputCharacterW(hOut, Cell1, 1, at, out read) || read == 0) return '\0';
        return Cell1[0];
    }

    static string ReadRun(short x, short y, int len, char[] buf)
    {
        COORD at; at.X = x; at.Y = y;
        uint read;
        if (!ReadConsoleOutputCharacterW(hOut, buf, (uint)len, at, out read) || read == 0) return "";
        return new string(buf, 0, (int)read);
    }

    static CSBI Info()
    {
        CSBI i;
        GetConsoleScreenBufferInfo(hOut, out i);
        return i;
    }

    // Busy wait that does not hand the console lock to nobody for too long.
    static void SpinUs(double us, Stopwatch sw)
    {
        if (us <= 0) return;
        double target = sw.Elapsed.TotalMilliseconds + us / 1000.0;
        while (sw.Elapsed.TotalMilliseconds < target) Thread.SpinWait(40);
    }

    static void WaitUntilMs(Stopwatch sw, double ms)
    {
        for (; ; )
        {
            double left = ms - sw.Elapsed.TotalMilliseconds;
            if (left <= 0) return;
            if (left > 3) Thread.Sleep(1);
            else Thread.SpinWait(60);
        }
    }

    static double Pct(List<double> sorted, double p)
    {
        if (sorted.Count == 0) return 0;
        double idx = (sorted.Count - 1) * p;
        int lo = (int)Math.Floor(idx), hi = (int)Math.Ceiling(idx);
        if (lo == hi) return sorted[lo];
        return sorted[lo] + (sorted[hi] - sorted[lo]) * (idx - lo);
    }

    static void Summarize(string label, List<double> samples)
    {
        var s = new List<double>(samples);
        s.Sort();
        double sum = 0; foreach (var v in s) sum += v;
        Log.Add(string.Format(CultureInfo.InvariantCulture,
            "SUMMARY {0} n={1} min={2:F2} p25={3:F2} median={4:F2} mean={5:F2} p90={6:F2} p99={7:F2} max={8:F2}",
            label, s.Count,
            s.Count > 0 ? s[0] : 0, Pct(s, 0.25), Pct(s, 0.50),
            s.Count > 0 ? sum / s.Count : 0, Pct(s, 0.90), Pct(s, 0.99),
            s.Count > 0 ? s[s.Count - 1] : 0));
    }

    static int Main(string[] argv)
    {
        var a = new Dictionary<string, string>();
        for (int i = 0; i < argv.Length; i++)
        {
            if (argv[i].StartsWith("--"))
            {
                string k = argv[i].Substring(2);
                string v = (i + 1 < argv.Length && !argv[i + 1].StartsWith("--")) ? argv[++i] : "1";
                a[k] = v;
            }
        }
        string outFile = a.ContainsKey("out") ? a["out"] : Path.Combine(Path.GetTempPath(), "keylat.txt");
        string mode = a.ContainsKey("mode") ? a["mode"] : "single";
        string label = a.ContainsKey("label") ? a["label"] : mode;
        int n = a.ContainsKey("n") ? int.Parse(a["n"]) : 40;
        int warmup = a.ContainsKey("warmup") ? int.Parse(a["warmup"]) : 5;
        double gapMs = a.ContainsKey("gap") ? double.Parse(a["gap"], CultureInfo.InvariantCulture) : 150;
        double spinUs = a.ContainsKey("spinus") ? double.Parse(a["spinus"], CultureInfo.InvariantCulture) : 0;
        int timeoutMs = a.ContainsKey("timeout") ? int.Parse(a["timeout"]) : 3000;
        string oracle = a.ContainsKey("oracle") ? a["oracle"] : "cursor";
        bool noErase = a.ContainsKey("noerase");
        string text = a.ContainsKey("text") ? a["text"] : "the quick brown fox jumps over the lazy dog";
        double cps = a.ContainsKey("cps") ? double.Parse(a["cps"], CultureInfo.InvariantCulture) : 10;
        uint pid = a.ContainsKey("pid") ? uint.Parse(a["pid"]) : 0;

        if (pid == 0) { File.WriteAllText(outFile, "ERROR no --pid\n"); return 90; }

        Process.GetCurrentProcess().PriorityClass = ProcessPriorityClass.High;
        Thread.CurrentThread.Priority = ThreadPriority.Highest;

        FreeConsole();
        if (!AttachConsole(pid))
        {
            File.WriteAllText(outFile, "ERROR AttachConsole " + pid + " err=" + Marshal.GetLastWin32Error() + "\n");
            return 2;
        }
        hIn = CreateFileW("CONIN$", 0xC0000000u, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        hOut = CreateFileW("CONOUT$", 0xC0000000u, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        if (hIn == new IntPtr(-1) || hOut == new IntPtr(-1))
        {
            int e = Marshal.GetLastWin32Error();
            FreeConsole();
            File.WriteAllText(outFile, "ERROR CreateFile CONIN/CONOUT err=" + e + "\n");
            return 3;
        }

        int rc = 0;
        try
        {
            if (mode == "pollcost") rc = PollCost(label);
            else if (mode == "single") rc = Single(label, n, warmup, gapMs, spinUs, timeoutMs, oracle, noErase);
            else if (mode == "type") rc = TypeMode(label, text, cps, spinUs, timeoutMs, oracle);
            else { Log.Add("ERROR unknown mode " + mode); rc = 91; }
        }
        catch (Exception ex)
        {
            Log.Add("EXCEPTION " + ex.GetType().Name + " " + ex.Message);
            rc = 92;
        }

        FreeConsole();
        File.WriteAllText(outFile, string.Join("\n", Log.ToArray()) + "\n");
        return rc;
    }

    static int PollCost(string label)
    {
        var info = Info();
        var sw = Stopwatch.StartNew();
        // one cell
        int iters = 20000;
        double t0 = sw.Elapsed.TotalMilliseconds;
        for (int i = 0; i < iters; i++) ReadCell(info.dwCursorPosition.X, info.dwCursorPosition.Y);
        double t1 = sw.Elapsed.TotalMilliseconds;
        Log.Add(string.Format(CultureInfo.InvariantCulture,
            "POLLCOST {0} cell1_us={1:F2} iters={2}", label, (t1 - t0) * 1000.0 / iters, iters));
        // CSBI
        t0 = sw.Elapsed.TotalMilliseconds;
        for (int i = 0; i < iters; i++) Info();
        t1 = sw.Elapsed.TotalMilliseconds;
        Log.Add(string.Format(CultureInfo.InvariantCulture,
            "POLLCOST {0} csbi_us={1:F2}", label, (t1 - t0) * 1000.0 / iters));
        return 0;
    }

    // One keystroke at a time, each trial independent, prompt idle in between.
    static int Single(string label, int n, int warmup, double gapMs, double spinUs, int timeoutMs, string oracle, bool noErase)
    {
        short fx = -1, fy = -1;
        if (oracle.StartsWith("cell:"))
        {
            var p = oracle.Substring(5).Split(',');
            fx = short.Parse(p[0]); fy = short.Parse(p[1]);
        }
        var sw = Stopwatch.StartNew();
        var samples = new List<double>();
        var raw = new List<string>();
        int total = n + warmup;
        string alphabet = "abcdefghijklmnopqrstuvwxyz";
        int timeouts = 0;

        for (int t = 0; t < total; t++)
        {
            char c = alphabet[t % alphabet.Length];
            short x, y;
            if (fx >= 0) { x = fx; y = fy; }
            else { var i0 = Info(); x = i0.dwCursorPosition.X; y = i0.dwCursorPosition.Y; }

            char before = ReadCell(x, y);
            if (before == c) { c = alphabet[(t + 7) % alphabet.Length]; }

            double t0 = sw.Elapsed.TotalMilliseconds;
            if (!InjectChar(c)) { Log.Add("inject failed"); return 4; }
            double appear = -1;
            for (; ; )
            {
                char cur = ReadCell(x, y);
                double now = sw.Elapsed.TotalMilliseconds;
                if (cur == c) { appear = now; break; }
                if (now - t0 > timeoutMs) break;
                SpinUs(spinUs, sw);
            }
            if (appear < 0) { timeouts++; }
            else if (t >= warmup)
            {
                samples.Add(appear - t0);
                raw.Add((appear - t0).ToString("F3", CultureInfo.InvariantCulture));
            }

            if (!noErase)
            {
                // backspace, and wait for the cell to stop showing our char
                InjectVk(0x08, '\b');
                double te = sw.Elapsed.TotalMilliseconds;
                for (; ; )
                {
                    if (ReadCell(x, y) != c) break;
                    if (sw.Elapsed.TotalMilliseconds - te > 1000) break;
                    Thread.Sleep(0);
                }
            }
            WaitUntilMs(sw, sw.Elapsed.TotalMilliseconds + gapMs);
        }

        Log.Add("RAW " + label + " " + string.Join(",", raw.ToArray()));
        Log.Add("TIMEOUTS " + label + " " + timeouts);
        Summarize(label, samples);
        return samples.Count == 0 ? 5 : 0;
    }

    static volatile int gVisible = 0;
    static double[] gAppear;
    static volatile bool gStop = false;

    // Sustained / burst typing: injection runs on its own schedule, a watcher
    // thread timestamps every character that shows up. Latency per character is
    // appear[i] - inject[i], which stays meaningful even when the pipeline is
    // behind, because it measures the queue too.
    static int TypeMode(string label, string text, double cps, double spinUs, int timeoutMs, string oracle)
    {
        short x, y;
        if (oracle.StartsWith("cell:"))
        {
            var p = oracle.Substring(5).Split(',');
            x = short.Parse(p[0]); y = short.Parse(p[1]);
        }
        else { var i0 = Info(); x = i0.dwCursorPosition.X; y = i0.dwCursorPosition.Y; }

        // Start from a genuinely empty line. PSReadLine's Escape clears whatever
        // is being edited; without this the watcher can match text left over
        // from the previous run (or, worse, an inline history prediction that
        // fills the WHOLE line from the first keypress) and report every
        // character as having appeared before it was typed.
        InjectVk(0x1B, (char)0x1B);
        Thread.Sleep(250);

        int n = text.Length;
        gAppear = new double[n];
        for (int i = 0; i < n; i++) gAppear[i] = -1;
        gVisible = 0;
        gStop = false;
        var inject = new double[n];
        var sw = Stopwatch.StartNew();
        var buf = new char[n + 8];

        // Refuse to measure a line that already reads back as the text we are
        // about to type: every latency would be negative and the run would look
        // impossibly fast rather than broken.
        {
            string pre = ReadRun(x, y, n, buf);
            if (pre.Length > 0 && pre[0] == text[0])
            {
                Log.Add("PREMATCHED " + label + " row already starts with the target text, aborting");
                return 6;
            }
        }

        var watcher = new Thread(() =>
        {
            while (!gStop)
            {
                int vis = gVisible;
                if (vis >= n) break;
                string s = ReadRun(x, y, n, buf);
                double now = sw.Elapsed.TotalMilliseconds;
                int m = 0;
                int lim = Math.Min(s.Length, n);
                while (m < lim && s[m] == text[m]) m++;
                if (m > vis)
                {
                    for (int i = vis; i < m; i++) gAppear[i] = now;
                    gVisible = m;
                }
                SpinUs(spinUs, sw);
            }
        });
        watcher.Priority = ThreadPriority.Highest;
        watcher.IsBackground = true;
        watcher.Start();

        double interval = 1000.0 / cps;
        double start = sw.Elapsed.TotalMilliseconds + 5;
        for (int i = 0; i < n; i++)
        {
            WaitUntilMs(sw, start + i * interval);
            inject[i] = sw.Elapsed.TotalMilliseconds;
            InjectChar(text[i]);
        }
        double deadline = sw.Elapsed.TotalMilliseconds + timeoutMs;
        while (gVisible < n && sw.Elapsed.TotalMilliseconds < deadline) Thread.Sleep(1);
        gStop = true;
        watcher.Join(500);

        var samples = new List<double>();
        var raw = new List<string>();
        int missing = 0;
        for (int i = 0; i < n; i++)
        {
            if (gAppear[i] < 0) { missing++; continue; }
            double d = gAppear[i] - inject[i];
            samples.Add(d);
            raw.Add(d.ToString("F3", CultureInfo.InvariantCulture));
        }
        // clear the line so the next run starts from a clean prompt
        for (int i = 0; i < gVisible + 2; i++) { InjectVk(0x08, '\b'); Thread.Sleep(2); }

        Log.Add("RAW " + label + " " + string.Join(",", raw.ToArray()));
        Log.Add("MISSING " + label + " " + missing + " of " + n);
        Log.Add(string.Format(CultureInfo.InvariantCulture,
            "SPAN {0} inject_span_ms={1:F1} appear_span_ms={2:F1}",
            label, inject[n - 1] - inject[0],
            gAppear[n - 1] > 0 && gAppear[0] > 0 ? gAppear[n - 1] - gAppear[0] : -1));
        Summarize(label, samples);
        return samples.Count == 0 ? 5 : 0;
    }
}
