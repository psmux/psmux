// Synchronized output (DECSET 2026, PR #781) query probe: runs inside a psmux
// pane and measures how long the terminal takes to answer a query, both on its
// own and when the query is sent INSIDE an open synchronized update.  A pane
// gate that holds output until ESC[?2026l must not hold the query with it: the
// asking program is blocked on the reply and will not send ESC[?2026l first.
//
// Each line of the log: NAME ms=<latency or -1> text=[reply]
//
// Build: csc /nologo /out:sync_output_probe.exe sync_output_probe.cs
// Usage: sync_output_probe.exe <logfile> [timeout_ms]
using System;
using System.IO;
using System.Text;
using System.Threading;
using System.Diagnostics;
using System.Runtime.InteropServices;

class SyncOutputProbe {
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool GetConsoleMode(IntPtr h, out uint mode);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool SetConsoleMode(IntPtr h, uint mode);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool ReadFile(IntPtr h, byte[] buf, uint n, out uint read, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool WriteFile(IntPtr h, byte[] buf, uint n, out uint written, IntPtr ov);

    static readonly object gate = new object();
    static byte[] buf = new byte[65536];
    static int used = 0;

    static void Reader(IntPtr hIn) {
        byte[] chunk = new byte[4096];
        while (true) {
            uint got;
            if (!ReadFile(hIn, chunk, (uint)chunk.Length, out got, IntPtr.Zero)) return;
            if (got == 0) return;
            lock (gate) {
                if (used + (int)got <= buf.Length) {
                    Array.Copy(chunk, 0, buf, used, (int)got);
                    used += (int)got;
                }
            }
        }
    }

    static string Printable(byte[] b, int from, int to) {
        StringBuilder sb = new StringBuilder();
        for (int i = from; i < to; i++) {
            byte c = b[i];
            if (c == 0x1b) sb.Append("<ESC>");
            else if (c >= 0x20 && c < 0x7f) sb.Append((char)c);
            else sb.Append("<" + c.ToString("x2") + ">");
        }
        return sb.ToString();
    }

    static IntPtr hOut;
    static void Send(string s) {
        byte[] b = Encoding.ASCII.GetBytes(s);
        uint w;
        WriteFile(hOut, b, (uint)b.Length, out w, IntPtr.Zero);
    }

    // Writes `prefix + query`, then waits until a reply ending in `term`
    // arrives or the timeout passes.  Returns the latency in ms or -1.
    static string Ask(string name, string prefix, string query, char term, int timeout) {
        int mark;
        lock (gate) { mark = used; }
        Stopwatch sw = Stopwatch.StartNew();
        Send(prefix + query);
        long ms = -1;
        while (sw.ElapsedMilliseconds < timeout) {
            lock (gate) {
                for (int i = mark; i < used; i++) {
                    if (buf[i] == (byte)term) { ms = sw.ElapsedMilliseconds; break; }
                }
            }
            if (ms >= 0) break;
            Thread.Sleep(2);
        }
        Thread.Sleep(50);
        string text;
        lock (gate) { text = Printable(buf, mark, used); }
        return name + " ms=" + ms + " text=[" + text + "]\n";
    }

    static int Main(string[] args) {
        string log = args[0];
        int timeout = 3000;
        if (args.Length > 1) int.TryParse(args[1], out timeout);

        IntPtr hIn = GetStdHandle(-10);
        hOut = GetStdHandle(-11);
        uint outMode, inMode;
        if (GetConsoleMode(hOut, out outMode)) SetConsoleMode(hOut, outMode | 0x0004);
        if (GetConsoleMode(hIn, out inMode)) {
            uint m = inMode & ~(0x0001u | 0x0002u | 0x0004u);
            SetConsoleMode(hIn, m | 0x0200);
        }
        Thread t = new Thread(delegate() { Reader(hIn); });
        t.IsBackground = true;
        t.Start();
        Thread.Sleep(400);

        StringBuilder o = new StringBuilder();
        o.Append("SYNC_PROBE START\n");
        o.Append(Ask("DA1_PLAIN", "", "\x1b[c", 'c', timeout));
        o.Append(Ask("DECRQM_2026_OUTSIDE", "", "\x1b[?2026$p", 'y', timeout));
        o.Append(Ask("DA1_IN_SYNC", "\x1b[?2026hframe", "\x1b[c", 'c', timeout));
        Send("\x1b[?2026l");
        Thread.Sleep(300);
        o.Append(Ask("DECRQM_2026_IN_SYNC", "\x1b[?2026hframe", "\x1b[?2026$p", 'y', timeout));
        Send("\x1b[?2026l");
        Thread.Sleep(300);
        o.Append(Ask("CPR_IN_SYNC", "\x1b[?2026hframe", "\x1b[6n", 'R', timeout));
        Send("\x1b[?2026l");
        Thread.Sleep(300);
        o.Append(Ask("XTVERSION_IN_SYNC", "\x1b[?2026hframe", "\x1b[>0q", '\\', timeout));
        Send("\x1b[?2026l");
        o.Append("SYNC_PROBE DONE\n");
        File.WriteAllText(log, o.ToString());
        return 0;
    }
}
