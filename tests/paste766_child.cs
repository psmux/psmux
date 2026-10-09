// paste766_child.exe <logfile>
//
// The pane program for issue #766: turns on bracketed paste (DECSET 2004) the
// way Claude Code does, puts its console input in VT mode, and logs every
// chunk it reads from stdin as "<unix ms> stdin <n>B <hex bytes>". The first
// line, "ready", tells the host the pane is up.
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

class Paste766Child
{
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetConsoleMode(IntPtr h, out uint mode);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool SetConsoleMode(IntPtr h, uint mode);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool ReadFile(IntPtr h, byte[] buf, uint n, out uint read, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool WriteFile(IntPtr h, byte[] buf, uint n, out uint written, IntPtr ov);

    [DllImport("kernel32.dll", SetLastError = true)] static extern bool SetConsoleCP(uint cp);

    static long Now() { return DateTimeOffset.UtcNow.ToUnixTimeMilliseconds(); }

    static int Main(string[] args)
    {
        string log = args[0];
        IntPtr hin = GetStdHandle(-10);
        IntPtr hout = GetStdHandle(-11);
        uint omode; GetConsoleMode(hout, out omode);
        SetConsoleMode(hout, omode | 0x0004);
        uint mode; GetConsoleMode(hin, out mode);
        // Raw: no line, echo or processed input; VT input so the paste markers
        // the pane's pseudoconsole receives are read as bytes.
        SetConsoleMode(hin, (mode & ~0x0007u) | 0x0200u);
        // ReadFile converts to the input code page; the OEM default has no em
        // dash and would turn it into '?', which is not what psmux delivered.
        SetConsoleCP(65001);
        byte[] on = Encoding.ASCII.GetBytes("\x1b[?2004h");
        uint w; WriteFile(hout, on, (uint)on.Length, out w, IntPtr.Zero);
        File.AppendAllText(log, Now() + " ready\n");
        byte[] buf = new byte[8192];
        while (true)
        {
            uint n;
            if (!ReadFile(hin, buf, (uint)buf.Length, out n, IntPtr.Zero) || n == 0) break;
            var sb = new StringBuilder();
            sb.Append(Now()).Append(" stdin ").Append(n).Append("B");
            for (int i = 0; i < n; i++) sb.Append(' ').Append(buf[i].ToString("x2"));
            sb.Append('\n');
            File.AppendAllText(log, sb.ToString());
        }
        return 0;
    }
}
