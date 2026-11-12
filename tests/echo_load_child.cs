// echo_load_child.exe [spew_lines_per_sec] [line_width]
//
// A pane occupant with no line editor in it. Every byte read from stdin is
// echoed at a FIXED screen position (row 1 column 1 of whatever grid it is in)
// using DECSC/DECRC, so a latency oracle can watch a single cell even while the
// pane is scrolling. That separates two costs that a shell prompt mixes
// together: the multiplexer's own input-to-screen path, and PSReadLine
// redrawing the whole edited line for every keystroke.
//
// With spew_lines_per_sec > 0 it also writes filler lines at that rate (0 or
// omitted means no filler, "max" means as fast as the pipe will take it), which
// is the "typing while the pane is producing heavy output" scenario. Filler
// text is digits and dots only so it can never be mistaken for an injected
// letter.
using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

class EchoLoadChild
{
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetConsoleMode(IntPtr h, out uint mode);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetConsoleMode(IntPtr h, uint mode);

    const int STD_INPUT = -10;
    const int STD_OUTPUT = -11;
    const uint ENABLE_LINE_INPUT = 0x0002;
    const uint ENABLE_ECHO_INPUT = 0x0004;
    const uint ENABLE_PROCESSED_INPUT = 0x0001;
    const uint ENABLE_VIRTUAL_TERMINAL_PROCESSING = 0x0004;

    static readonly object WriteLock = new object();
    static Stream Out;

    static void Emit(string s)
    {
        var b = Encoding.UTF8.GetBytes(s);
        lock (WriteLock) { Out.Write(b, 0, b.Length); Out.Flush(); }
    }

    static int Main(string[] argv)
    {
        double rate = 0;
        bool maxRate = false;
        if (argv.Length > 0)
        {
            if (argv[0] == "max") maxRate = true;
            else double.TryParse(argv[0], out rate);
        }
        int width = 60;
        if (argv.Length > 1) int.TryParse(argv[1], out width);

        IntPtr hi = GetStdHandle(STD_INPUT);
        IntPtr ho = GetStdHandle(STD_OUTPUT);
        uint m;
        if (GetConsoleMode(hi, out m))
            SetConsoleMode(hi, m & ~(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT));
        if (GetConsoleMode(ho, out m))
            SetConsoleMode(ho, m | ENABLE_VIRTUAL_TERMINAL_PROCESSING);

        Out = Console.OpenStandardOutput();
        Emit("\x1b[2J\x1b[H");
        // Freeze row 1 by putting the scrolling region BELOW it. Without this
        // the filler scrolls the echo cell off the top faster than any oracle
        // can read it, and the "typing while the pane floods" scenario measures
        // nothing at all.
        Emit("\x1b[2;200r\x1b[2;1H");
        Emit("ECHOCHILD READY rate=" + (maxRate ? "max" : rate.ToString()) + "\r\n");

        if (maxRate || rate > 0)
        {
            var filler = new string('.', Math.Max(4, width - 12));
            var t = new Thread(() =>
            {
                var sw = Stopwatch.StartNew();
                long i = 0;
                double interval = maxRate ? 0 : 1000.0 / rate;
                for (; ; )
                {
                    i++;
                    // Filler never touches row 1: it is written below, and the
                    // echo cell is parked at row 1 col 1.
                    Emit(string.Format("{0:D8} {1}\r\n", i, filler));
                    if (!maxRate)
                    {
                        double target = i * interval;
                        while (sw.Elapsed.TotalMilliseconds < target) Thread.Sleep(1);
                    }
                }
            });
            t.IsBackground = true;
            t.Start();
        }

        var stdin = Console.OpenStandardInput();
        var buf = new byte[256];
        for (; ; )
        {
            int k = stdin.Read(buf, 0, buf.Length);
            if (k <= 0) break;
            var sb = new StringBuilder();
            for (int i = 0; i < k; i++)
            {
                byte b = buf[i];
                if (b == 3 || b == 4) return 0;
                if (b < 32) continue;
                // Save cursor, park the echo at row 1 col 1, restore. Row 1 is
                // outside the scrolling region set at startup, so the oracle's
                // cell survives however fast the rest of the pane scrolls.
                sb.Append("\x1b7\x1b[1;1H").Append((char)b).Append("\x1b8");
            }
            if (sb.Length > 0) Emit(sb.ToString());
        }
        return 0;
    }
}
