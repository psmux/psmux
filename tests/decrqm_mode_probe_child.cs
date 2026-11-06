// DECRQM probe for issue #995: runs in a psmux pane, asks the terminal for
// DECAWM (CSI ? 7 $ p) and IRM (CSI 4 $ p) before and after it changes them,
// and logs each answer as `name = <reply with ESC spelled out>`.
//
// The queries go out with WriteFile on the raw stdout handle; the answers are
// read from stdin in VT input mode.
//
// Build: csc /nologo /out:decrqm_mode_probe_child.exe decrqm_mode_probe_child.cs
// Usage: decrqm_mode_probe_child.exe <logfile>
using System;
using System.IO;
using System.Text;
using System.Threading;
using System.Runtime.InteropServices;

class DecrqmModeProbe {
    [DllImport("kernel32.dll")] static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll")] static extern bool GetConsoleMode(IntPtr h, out uint m);
    [DllImport("kernel32.dll")] static extern bool SetConsoleMode(IntPtr h, uint m);
    [DllImport("kernel32.dll")] static extern bool ReadFile(IntPtr h, byte[] b, uint n, out uint r, IntPtr o);
    [DllImport("kernel32.dll")] static extern bool WriteFile(IntPtr h, byte[] b, uint n, out uint w, IntPtr o);

    static readonly object gate = new object();
    static byte[] buf = new byte[65536];
    static int used;

    static void Main(string[] args) {
        IntPtr hIn = GetStdHandle(-10), hOut = GetStdHandle(-11);
        uint mode;
        // No line input, echo or processed input; VT input on.
        GetConsoleMode(hIn, out mode); SetConsoleMode(hIn, (mode & ~7u) | 0x200);
        GetConsoleMode(hOut, out mode); SetConsoleMode(hOut, mode | 4);
        new Thread(() => {
            byte[] chunk = new byte[4096];
            uint got;
            while (ReadFile(hIn, chunk, (uint)chunk.Length, out got, IntPtr.Zero) && got > 0) {
                lock (gate) {
                    if (used + (int)got <= buf.Length) { Array.Copy(chunk, 0, buf, used, (int)got); used += (int)got; }
                }
            }
        }) { IsBackground = true }.Start();

        string E = "\x1b";
        string[] names = { "awm_default", "awm_off", "awm_on_again", "irm_default", "irm_on", "irm_off_again" };
        string[] queries = {
            E + "[?7$p", E + "[?7l" + E + "[?7$p", E + "[?7h" + E + "[?7$p",
            E + "[4$p", E + "[4h" + E + "[4$p", E + "[4l" + E + "[4$p",
        };
        StringBuilder log = new StringBuilder();
        for (int i = 0; i < queries.Length; i++) {
            int start;
            lock (gate) start = used;
            byte[] q = Encoding.ASCII.GetBytes(queries[i]);
            uint w;
            WriteFile(hOut, q, (uint)q.Length, out w, IntPtr.Zero);
            string reply = "";
            for (int t = 0; t < 40 && reply.IndexOf("$y") < 0; t++) {
                Thread.Sleep(50);
                lock (gate) reply = Encoding.ASCII.GetString(buf, start, used - start);
            }
            log.AppendLine(names[i] + " = " + reply.Replace(E, "ESC"));
        }
        File.WriteAllText(args[0], log.ToString());
        Thread.Sleep(600000);
    }
}
