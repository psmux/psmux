using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Threading;

// REALISTIC TYPING BENCHMARK
// Injects keystrokes with proper VK codes + scan codes (like a real keyboard)
// Monitors cursor position movement at 500Hz (2ms) to detect char render
// Cursor advancing = character appeared on screen
//
// Usage: typing_bench.exe <PID> <text> <intra_char_ms> <inter_word_ms>
// Output: CSV of cursor samples + SUMMARY line

class TypingBench {
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr CreateFile(string name, uint access, uint share, IntPtr sa, uint disp, uint flags, IntPtr tmpl);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool WriteConsoleInput(IntPtr h, INPUT_RECORD[] buf, uint len, out uint written);
    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetConsoleScreenBufferInfo(IntPtr h, out CSBI info);
    [DllImport("user32.dll")]
    static extern uint MapVirtualKeyW(uint code, uint mapType);
    [DllImport("user32.dll")]
    static extern short VkKeyScanW(char ch);

    const ushort KEY_EVENT = 1;
    const uint SHIFT_PRESSED = 0x0010;
    const uint LEFT_CTRL_PRESSED = 0x0008;

    [StructLayout(LayoutKind.Explicit)]
    struct INPUT_RECORD {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public KEY_EVENT_RECORD KeyEvent;
    }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct KEY_EVENT_RECORD {
        public int bKeyDown;
        public ushort wRepeatCount;
        public ushort wVirtualKeyCode;
        public ushort wVirtualScanCode;
        public char UnicodeChar;
        public uint dwControlKeyState;
    }
    struct COORD { public short X, Y; }
    struct SMALL_RECT { public short Left, Top, Right, Bottom; }
    struct CSBI {
        public COORD dwSize;
        public COORD dwCursorPosition;
        public ushort wAttributes;
        public SMALL_RECT srWindow;
        public COORD dwMaximumWindowSize;
    }

    static volatile bool injDone = false;
    static volatile int injectedCount = 0;

    static int CursorLinear(CSBI c) {
        return c.dwCursorPosition.Y * c.dwSize.X + c.dwCursorPosition.X;
    }

    // Build a proper key event with VK code and scan code
    static INPUT_RECORD MakeKey(bool down, ushort vk, char ch, uint ctrl) {
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

    // Get proper VK code for a character (like a real keyboard would)
    static void CharToVK(char c, out ushort vk, out uint ctrl) {
        ctrl = 0;
        if (c >= 'a' && c <= 'z') { vk = (ushort)(0x41 + c - 'a'); }
        else if (c >= 'A' && c <= 'Z') { vk = (ushort)(0x41 + c - 'A'); ctrl = SHIFT_PRESSED; }
        else if (c >= '0' && c <= '9') { vk = (ushort)(0x30 + c - '0'); }
        else if (c == ' ') vk = 0x20;
        else if (c == '-') vk = 0xBD;
        else if (c == '_') { vk = 0xBD; ctrl = SHIFT_PRESSED; }
        else if (c == '.') vk = 0xBE;
        else if (c == ',') vk = 0xBC;
        else if (c == '/') vk = 0xBF;
        else if (c == '\'') vk = 0xDE;
        else if (c == '"') { vk = 0xDE; ctrl = SHIFT_PRESSED; }
        else if (c == ';') vk = 0xBA;
        else if (c == ':') { vk = 0xBA; ctrl = SHIFT_PRESSED; }
        else if (c == '!') { vk = 0x31; ctrl = SHIFT_PRESSED; }
        else if (c == '?') { vk = 0xBF; ctrl = SHIFT_PRESSED; }
        else {
            // Fallback: use VkKeyScan
            short vks = VkKeyScanW(c);
            if (vks != -1) {
                vk = (ushort)(vks & 0xFF);
                if ((vks & 0x100) != 0) ctrl |= SHIFT_PRESSED;
            } else {
                vk = 0;
            }
        }
    }

    static void InjectChar(IntPtr hIn, char c) {
        ushort vk;
        uint ctrl;
        CharToVK(c, out vk, out ctrl);

        var recs = new INPUT_RECORD[2];
        recs[0] = MakeKey(true, vk, c, ctrl);
        recs[1] = MakeKey(false, vk, c, 0);
        uint written;
        WriteConsoleInput(hIn, recs, 2, out written);
    }

    static void InjectCtrlCombo(IntPtr hIn, char letter) {
        ushort vk = (ushort)char.ToUpper(letter);
        char ctrlChar = (char)(char.ToUpper(letter) - 'A' + 1);
        var recs = new INPUT_RECORD[4];
        recs[0] = MakeKey(true, 0x11, '\0', LEFT_CTRL_PRESSED);
        recs[1] = MakeKey(true, vk, ctrlChar, LEFT_CTRL_PRESSED);
        recs[2] = MakeKey(false, vk, ctrlChar, LEFT_CTRL_PRESSED);
        recs[3] = MakeKey(false, 0x11, '\0', 0);
        uint written;
        WriteConsoleInput(hIn, recs, 4, out written);
    }

    static void InjectEnter(IntPtr hIn) {
        var recs = new INPUT_RECORD[2];
        recs[0] = MakeKey(true, 0x0D, '\r', 0);
        recs[1] = MakeKey(false, 0x0D, '\r', 0);
        uint written;
        WriteConsoleInput(hIn, recs, 2, out written);
    }

    static void InjectEscape(IntPtr hIn) {
        var recs = new INPUT_RECORD[2];
        recs[0] = MakeKey(true, 0x1B, (char)0x1B, 0);
        recs[1] = MakeKey(false, 0x1B, (char)0x1B, 0);
        uint written;
        WriteConsoleInput(hIn, recs, 2, out written);
    }

    static void Main(string[] args) {
        if (args.Length < 4) {
            Console.Error.WriteLine("Usage: typing_bench.exe <PID> <text> <intra_ms> <inter_ms>");
            Console.Error.WriteLine("  intra_ms: delay between chars within a word");
            Console.Error.WriteLine("  inter_ms: delay between words (after space)");
            Environment.Exit(1);
        }

        uint pid = uint.Parse(args[0]);
        string text = args[1];
        int intraMs = int.Parse(args[2]);
        int interMs = int.Parse(args[3]);

        // Optional: mode=clear to just clear the screen and exit
        bool clearOnly = args.Length > 4 && args[4] == "clear";
        // Optional: <burst_at> <burst_len> sends characters burst_at+1 to
        // burst_at+burst_len with no delay after them, the bunch a busy
        // machine hands the client when keys queued while it was not reading.
        int burstAt = -1, burstLen = 0;
        if (args.Length > 5) { burstAt = int.Parse(args[4]); burstLen = int.Parse(args[5]); }

        FreeConsole();
        if (!AttachConsole(pid)) {
            Console.Error.WriteLine("AttachConsole failed: " + Marshal.GetLastWin32Error());
            Environment.Exit(2);
        }

        IntPtr hIn = CreateFile("CONIN$", 0xC0000000u, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        IntPtr hOut = CreateFile("CONOUT$", 0x80000000u, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        if (hIn == (IntPtr)(-1) || hOut == (IntPtr)(-1)) {
            Console.Error.WriteLine("CreateFile failed");
            Environment.Exit(3);
        }

        if (clearOnly) {
            // Send Escape (clear current line), then "cls" + Enter
            InjectEscape(hIn);
            Thread.Sleep(50);
            foreach (char c in "cls") { InjectChar(hIn, c); Thread.Sleep(30); }
            Thread.Sleep(50);
            InjectEnter(hIn);
            Thread.Sleep(500);
            CloseHandle(hIn); CloseHandle(hOut); FreeConsole();
            return;
        }

        // Take baseline cursor position
        CSBI csbi;
        GetConsoleScreenBufferInfo(hOut, out csbi);
        int baseOffset = CursorLinear(csbi);

        // Timing structures
        var timestamps = new List<long>();   // ms when cursor moved
        var positions = new List<int>();      // cursor linear position
        var deltas = new List<int>();         // chars rendered since last sample
        var gaps = new List<int>();           // ms since last cursor movement

        int prevOffset = baseOffset;
        long lastChangeMs = 0;
        long firstChangeMs = 0;
        int maxGap = 0;
        int stallCount = 0;
        int burstCount = 0;

        var sw = Stopwatch.StartNew();

        // Monitor thread: poll cursor at 500Hz (2ms)
        Thread monitor = new Thread(() => {
            while (sw.ElapsedMilliseconds < 60000) {
                CSBI cur;
                GetConsoleScreenBufferInfo(hOut, out cur);
                int curOff = CursorLinear(cur);
                long ts = sw.ElapsedMilliseconds;

                if (curOff != prevOffset) {
                    int delta = curOff - prevOffset;
                    if (delta < 0) delta = 0; // line wrap went backwards, ignore
                    if (firstChangeMs == 0) firstChangeMs = ts;

                    int gap = 0;
                    if (lastChangeMs > 0) {
                        gap = (int)(ts - lastChangeMs);
                        if (gap > maxGap) maxGap = gap;
                        if (gap > 150) stallCount++;
                        if (delta > 8) burstCount++;
                    }

                    timestamps.Add(ts);
                    positions.Add(curOff);
                    deltas.Add(delta);
                    gaps.Add(gap);

                    lastChangeMs = ts;
                    prevOffset = curOff;
                }

                // Stop conditions
                if (injDone && lastChangeMs > 0 && (ts - lastChangeMs) > 2000) break;
                if (ts > 30000 && firstChangeMs == 0) break;

                Thread.Sleep(2);
            }
        });
        monitor.IsBackground = true;
        monitor.Start();

        // Small delay to let monitor start
        Thread.Sleep(20);

        // INJECT: realistic typing with proper delays
        long injStart = sw.ElapsedMilliseconds;
        int charsSent = 0;
        foreach (char c in text) {
            InjectChar(hIn, c);
            charsSent++;
            Interlocked.Exchange(ref injectedCount, charsSent);
            if (burstAt >= 0 && charsSent > burstAt && charsSent <= burstAt + burstLen) continue;

            if (c == ' ') {
                if (interMs > 0) Thread.Sleep(interMs);
            } else {
                if (intraMs > 0) Thread.Sleep(intraMs);
            }
        }
        long injEnd = sw.ElapsedMilliseconds;
        injDone = true;

        monitor.Join(5000);

        CloseHandle(hIn);
        CloseHandle(hOut);
        FreeConsole();

        // Compute statistics from gaps
        var sortedGaps = new List<int>();
        foreach (int g in gaps) { if (g > 0) sortedGaps.Add(g); }
        sortedGaps.Sort();
        int n = sortedGaps.Count;

        int p50 = n > 0 ? sortedGaps[n / 2] : 0;
        int p90 = n > 0 ? sortedGaps[(int)(n * 0.9)] : 0;
        int p95 = n > 0 ? sortedGaps[Math.Min((int)(n * 0.95), n - 1)] : 0;
        int p99 = n > 0 ? sortedGaps[Math.Min((int)(n * 0.99), n - 1)] : 0;

        long avgGap = 0;
        if (n > 0) {
            long sum = 0;
            foreach (int g in sortedGaps) sum += g;
            avgGap = sum / n;
        }

        long renderSpan = (lastChangeMs > 0 && firstChangeMs > 0) ? lastChangeMs - firstChangeMs : 0;
        int totalRendered = prevOffset - baseOffset;
        if (totalRendered < 0) totalRendered = 0;

        // Output CSV
        Console.WriteLine("TS_MS,CURSOR_POS,DELTA,GAP_MS");
        for (int i = 0; i < timestamps.Count; i++) {
            Console.WriteLine("{0},{1},{2},{3}", timestamps[i], positions[i], deltas[i], gaps[i]);
        }

        Console.WriteLine("FIRST_RENDER_AFTER_INJECT_MS={0}", firstChangeMs > 0 ? firstChangeMs - injStart : -1);
        Console.WriteLine("SUMMARY chars={0} inject_ms={1} render_ms={2} samples={3} rendered={4} stalls={5} bursts={6} max_gap={7} avg_gap={8} p50={9} p90={10} p95={11} p99={12}",
            text.Length, injEnd - injStart, renderSpan, timestamps.Count, totalRendered,
            stallCount, burstCount, maxGap, avgGap, p50, p90, p95, p99);
    }
}
