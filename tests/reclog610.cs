// reclog610.exe <mode> <logpath>
//
// Console input oracle for issue #610 (Ctrl+Backspace).
//
//   mode "rec" : opens CONIN$ fresh, strips ENABLE_VIRTUAL_TERMINAL_INPUT and
//                every line/echo/processed flag, then logs every INPUT_RECORD it
//                reads with ReadConsoleInputW.  This is what a record reading
//                app (PSReadLine, anything using ReadConsoleInput) actually
//                sees, so it answers "did the Ctrl modifier survive".
//
//   mode "vt"  : opens CONIN$ fresh, sets ENABLE_VIRTUAL_TERMINAL_INPUT and
//                strips line/echo/processed, then logs the raw BYTES it reads.
//                This is what a VT reading app (node in raw mode, vim) sees.
//
// Both modes print READY into the log first, and both stop on a Ctrl+Z
// (record VK 0x5A with Ctrl, or byte 0x1A).
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

class RecLog610
{
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateFileW(string name, uint access, uint share,
        IntPtr sec, uint disp, uint flags, IntPtr tmpl);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetConsoleMode(IntPtr h, out uint mode);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetConsoleMode(IntPtr h, uint mode);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool ReadConsoleInputW(IntPtr h, [Out] INPUT_RECORD[] buf, uint len, out uint read);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool ReadFile(IntPtr h, [Out] byte[] buf, uint n, out uint read, IntPtr ov);

    const uint GENERIC_READ = 0x80000000;
    const uint GENERIC_WRITE = 0x40000000;
    const uint FILE_SHARE_READ = 1;
    const uint FILE_SHARE_WRITE = 2;
    const uint OPEN_EXISTING = 3;

    const uint ENABLE_PROCESSED_INPUT = 0x0001;
    const uint ENABLE_LINE_INPUT = 0x0002;
    const uint ENABLE_ECHO_INPUT = 0x0004;
    const uint ENABLE_WINDOW_INPUT = 0x0008;
    const uint ENABLE_MOUSE_INPUT = 0x0010;
    const uint ENABLE_VIRTUAL_TERMINAL_INPUT = 0x0200;

    [StructLayout(LayoutKind.Sequential)]
    struct KEY_EVENT_RECORD
    {
        public int bKeyDown;
        public ushort wRepeatCount;
        public ushort wVirtualKeyCode;
        public ushort wVirtualScanCode;
        public char UnicodeChar;
        public uint dwControlKeyState;
    }

    [StructLayout(LayoutKind.Explicit)]
    struct INPUT_RECORD
    {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public KEY_EVENT_RECORD KeyEvent;
    }

    static string log;
    static object gate = new object();

    static void W(string s)
    {
        lock (gate)
        {
            for (int i = 0; i < 10; i++)
            {
                try { File.AppendAllText(log, s + "\r\n"); return; }
                catch { System.Threading.Thread.Sleep(10); }
            }
        }
    }

    static string Mods(uint c)
    {
        var sb = new StringBuilder();
        if ((c & 0x0008) != 0) sb.Append("LCTRL|");
        if ((c & 0x0004) != 0) sb.Append("RCTRL|");
        if ((c & 0x0002) != 0) sb.Append("LALT|");
        if ((c & 0x0001) != 0) sb.Append("RALT|");
        if ((c & 0x0010) != 0) sb.Append("SHIFT|");
        if (sb.Length == 0) return "none";
        return sb.ToString(0, sb.Length - 1);
    }

    static int Main(string[] argv)
    {
        string mode = argv.Length > 0 ? argv[0] : "rec";
        log = argv.Length > 1
            ? argv[1]
            : Path.Combine(Path.GetTempPath(), "reclog610.txt");
        try { File.WriteAllText(log, ""); } catch { }

        IntPtr h = CreateFileW("CONIN$", GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
        if (h == IntPtr.Zero || h == new IntPtr(-1))
        {
            W("ERROR CONIN$ open failed e=" + Marshal.GetLastWin32Error());
            return 2;
        }

        uint orig;
        GetConsoleMode(h, out orig);
        W(string.Format("MODE-ORIG 0x{0:X4} vt_input={1}", orig,
            (orig & ENABLE_VIRTUAL_TERMINAL_INPUT) != 0));

        uint want;
        if (mode == "vt")
        {
            want = (orig & ~(ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT))
                   | ENABLE_VIRTUAL_TERMINAL_INPUT;
        }
        else
        {
            want = (orig & ~(ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT
                             | ENABLE_VIRTUAL_TERMINAL_INPUT));
        }
        bool setOk = SetConsoleMode(h, want);
        uint after;
        GetConsoleMode(h, out after);
        W(string.Format("MODE-SET ok={0} want=0x{1:X4} after=0x{2:X4}", setOk, want, after));
        W("READY mode=" + mode);

        if (mode == "vt")
        {
            byte[] buf = new byte[512];
            while (true)
            {
                uint n;
                if (!ReadFile(h, buf, (uint)buf.Length, out n, IntPtr.Zero))
                {
                    W("READ-ERR e=" + Marshal.GetLastWin32Error());
                    return 3;
                }
                if (n == 0) { W("EOF"); return 0; }
                var hex = new StringBuilder();
                var pretty = new StringBuilder();
                bool quit = false;
                for (uint i = 0; i < n; i++)
                {
                    hex.AppendFormat("{0:X2} ", buf[i]);
                    byte b = buf[i];
                    if (b == 0x1A) quit = true;
                    if (b == 0x1B) pretty.Append("<ESC>");
                    else if (b == 0x7F) pretty.Append("<DEL>");
                    else if (b == 0x08) pretty.Append("<BS>");
                    else if (b == 0x17) pretty.Append("<C-W>");
                    else if (b == 0x0D) pretty.Append("<CR>");
                    else if (b < 0x20) pretty.AppendFormat("<{0:X2}>", b);
                    else pretty.Append((char)b);
                }
                W("BYTES n=" + n + " hex=[ " + hex.ToString().Trim() + " ] str=[" + pretty + "]");
                if (quit) { W("DONE"); return 0; }
            }
        }
        else
        {
            var recs = new INPUT_RECORD[32];
            while (true)
            {
                uint n;
                if (!ReadConsoleInputW(h, recs, (uint)recs.Length, out n))
                {
                    W("READ-ERR e=" + Marshal.GetLastWin32Error());
                    return 3;
                }
                for (uint i = 0; i < n; i++)
                {
                    if (recs[i].EventType != 1) continue;
                    var k = recs[i].KeyEvent;
                    W(string.Format(
                        "REC {0} vk=0x{1:X2} scan=0x{2:X2} uChar=0x{3:X4} ctrl=0x{4:X4} [{5}]",
                        k.bKeyDown != 0 ? "DOWN" : "UP  ",
                        k.wVirtualKeyCode, k.wVirtualScanCode, (int)k.UnicodeChar,
                        k.dwControlKeyState, Mods(k.dwControlKeyState)));
                    if (k.bKeyDown != 0 && k.wVirtualKeyCode == 0x5A
                        && (k.dwControlKeyState & 0x000C) != 0)
                    {
                        W("DONE");
                        return 0;
                    }
                    if (k.bKeyDown != 0 && (int)k.UnicodeChar == 0x1A)
                    {
                        W("DONE");
                        return 0;
                    }
                }
            }
        }
    }
}
