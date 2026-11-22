// Terminal query probe: runs inside a psmux pane (or any console) and asks the terminal
// the round-trip questions Claude Code asks at startup, logging the raw answer bytes.
//
// Queries, in order: XTVERSION (CSI > 0 q), DA1 (CSI c), DA2 (CSI > c), DSR-CPR (CSI 6 n),
// DECRQM 2026 (CSI ? 2026 $ p), DECRQM 1006 (CSI ? 1006 $ p), XTGETTCAP for "TN".
//
// Each query is written with WriteFile on the raw stdout handle (WriteConsole-written
// escapes are swallowed by conhost), then the probe waits and records every byte that
// arrived on stdin in that window.
//
// Build: csc /nologo /out:query_probe_child.exe query_probe_child.cs
// Usage: query_probe_child.exe <logfile> [wait_ms] [paint]
//
// `paint` writes a `.` right after each query, in the same write. The Windows
// 10 inbox conhost (build 19045) holds a query the program writes on its own
// and hands it to psmux only when it next draws something, ahead of that text,
// so without a letter after it no query reaches psmux in the wait (measured:
// XTVERSION alone, nothing in 4 s; with a letter after it, the reply in 8 ms;
// with a letter a second later, the reply right after the letter). Suites pass
// it on a build before 22000.
using System;
using System.IO;
using System.Text;
using System.Threading;
using System.Runtime.InteropServices;

class QueryProbe {
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

    const int STD_INPUT_HANDLE = -10;
    const int STD_OUTPUT_HANDLE = -11;
    const uint ENABLE_PROCESSED_INPUT = 0x0001;
    const uint ENABLE_LINE_INPUT = 0x0002;
    const uint ENABLE_ECHO_INPUT = 0x0004;
    const uint ENABLE_VIRTUAL_TERMINAL_INPUT = 0x0200;
    const uint ENABLE_VIRTUAL_TERMINAL_PROCESSING = 0x0004;

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

    static string Hex(byte[] b, int from, int to) {
        StringBuilder sb = new StringBuilder();
        for (int i = from; i < to; i++) {
            if (sb.Length > 0) sb.Append(' ');
            sb.Append(b[i].ToString("x2"));
        }
        return sb.ToString();
    }

    static string Printable(byte[] b, int from, int to) {
        StringBuilder sb = new StringBuilder();
        for (int i = from; i < to; i++) {
            byte c = b[i];
            if (c == 0x1b) sb.Append("<ESC>");
            else if (c == 0x9c) sb.Append("<ST>");
            else if (c >= 0x20 && c < 0x7f) sb.Append((char)c);
            else sb.Append("<" + c.ToString("x2") + ">");
        }
        return sb.ToString();
    }

    static int Main(string[] args) {
        string log = args.Length > 0 ? args[0]
            : Path.Combine(Environment.GetEnvironmentVariable("TEMP"), "psmux_query_probe.txt");
        int wait = 1500;
        if (args.Length > 1) int.TryParse(args[1], out wait);
        bool paint = args.Length > 2 && args[2] == "paint";

        IntPtr hIn = GetStdHandle(STD_INPUT_HANDLE);
        IntPtr hOut = GetStdHandle(STD_OUTPUT_HANDLE);

        StringBuilder outp = new StringBuilder();
        outp.Append("QUERY_PROBE START wait=" + wait + (paint ? " paint" : "") + "\n");

        uint outMode, inMode;
        bool haveOut = GetConsoleMode(hOut, out outMode);
        bool haveIn = GetConsoleMode(hIn, out inMode);
        outp.Append("outMode=" + (haveOut ? "0x" + outMode.ToString("X4") : "NONE")
            + " inMode=" + (haveIn ? "0x" + inMode.ToString("X4") : "NONE") + "\n");
        if (haveOut) SetConsoleMode(hOut, outMode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        if (haveIn) {
            uint m = inMode & ~(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT);
            m |= ENABLE_VIRTUAL_TERMINAL_INPUT;
            SetConsoleMode(hIn, m);
            uint after;
            GetConsoleMode(hIn, out after);
            outp.Append("inMode after=0x" + after.ToString("X4") + "\n");
        }

        Thread t = new Thread(delegate() { Reader(hIn); });
        t.IsBackground = true;
        t.Start();

        // Give the terminal a moment to settle before the first question.
        Thread.Sleep(400);
        lock (gate) { used = 0; }

        string[][] queries = new string[][] {
            new string[] { "XTVERSION", "\x1b[>0q" },
            new string[] { "XTVERSION_NOPARAM", "\x1b[>q" },
            // Negative control: `CSI > 1 q` is a cursor-style request, not a
            // version request. xterm and tmux both leave it unanswered.
            new string[] { "XTVERSION_PARAM1", "\x1b[>1q" },
            new string[] { "DA1", "\x1b[c" },
            new string[] { "DA2", "\x1b[>c" },
            new string[] { "DSR_CPR", "\x1b[6n" },
            // Issue #597 follow up: `CSI 5 n` is the one of the four psmux
            // started answering that the inbox host was never measured on, so
            // the probe asks it explicitly.  Exactly one reply must come back,
            // whoever sends it.
            new string[] { "DSR_STATUS", "\x1b[5n" },
            new string[] { "DECRQM_2026", "\x1b[?2026$p" },
            new string[] { "DECRQM_1006", "\x1b[?1006$p" },
            new string[] { "XTGETTCAP_TN", "\x1bP+q544e\x1b\\" },
            // Issue #597 follow up: the colour queries a reporter on 19045
            // measured as 0 bytes.  psmux answers these from server/helpers.rs
            // (issue #473/#556) when they reach it, so a 0 here says either the
            // ConPTY host ate the QUERY on the way out or the reply could not be
            // delivered, and the mouse debug log tells those two apart.
            new string[] { "OSC_FG", "\x1b]10;?\x1b\\" },
            new string[] { "OSC_BG", "\x1b]11;?\x1b\\" },
            new string[] { "OSC_COLOR1", "\x1b]4;1;?\x1b\\" },
            // The light/dark scheme query psmux answers with a CSI reply rather
            // than an OSC one.  CSI is documented to survive a plain write to
            // the pane's input pipe where OSC does not, so this is the control
            // that tells "psmux could not deliver" apart from "ConPTY ate an
            // OSC reply on the pipe".
            new string[] { "CSI_SCHEME", "\x1b[?996n" },
            // Issue #841: the XTWINOPS size reports.  tmux 3.8 holds every
            // lone Escape for 500 ms until its `CSI 14 t` is answered; the
            // console host answers `CSI 18 t` itself but forwards 14 and 16.
            new string[] { "XTWINOPS_14", "\x1b[14t" },
            new string[] { "XTWINOPS_16", "\x1b[16t" },
            new string[] { "XTWINOPS_18", "\x1b[18t" },
        };

        foreach (string[] q in queries) {
            int mark;
            lock (gate) { mark = used; }
            byte[] qb = Encoding.ASCII.GetBytes(q[1]);
            byte[] wb = paint ? Encoding.ASCII.GetBytes(q[1] + ".") : qb;
            uint w;
            WriteFile(hOut, wb, (uint)wb.Length, out w, IntPtr.Zero);
            Thread.Sleep(wait);
            int now;
            byte[] snap;
            lock (gate) { now = used; snap = (byte[])buf.Clone(); }
            int n = now - mark;
            outp.Append(q[0] + " sent=" + Printable(qb, 0, qb.Length)
                + " got=" + n + " bytes"
                + " hex=[" + Hex(snap, mark, now) + "]"
                + " text=[" + Printable(snap, mark, now) + "]\n");
        }

        outp.Append("QUERY_PROBE DONE\n");
        File.WriteAllText(log, outp.ToString());
        return 0;
    }
}
