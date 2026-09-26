// Far Manager's VT palette query, as psmux sees it: the stand-in for issue
// #623's "first F10 inserts a backslash" report.
//
// Far Manager 3.0.6364 (far/console.cpp, GetPaletteVT + query_vt) reads the
// terminal palette like this:
//
//   1. add ENABLE_VIRTUAL_TERMINAL_INPUT to its console input mode
//   2. ONE write: CSI 0c, OSC 4;0;?;1;?;...;255;? ST, CSI 0c
//   3. ReadConsole until it holds two DA1 replies (CSI ? ... c)
//   4. restore its own mode (0x01B8) and go back to reading key records
//
// Under a ConPTY the host answers both DA1s itself while it processes that
// write, so step 3 ends at once, and Far is back in 0x01B8 before psmux has
// answered the OSC: measured, psmux found Far's console in 0x01B8 at every one
// of its reply injections (10 of 10 launches).  Whatever psmux injects then is
// read as KEYSTROKES: Far shows a `\` on its command line (every ESC clears
// the line, the final ST's backslash stays) and an autocompletion list that
// eats the next key.
//
// A .NET child cannot reproduce Far's microsecond gap between steps 3 and 4
// reliably, so this child holds the state psmux actually meets: it stays in
// Far's record mode (0x01B8) for the whole query and reads key records.  Every
// character that is not one of ConPTY's two DA1 answers is a reply that was
// typed into a record reader.  Nobody types during the run.
//
// Build: csc /nologo /platform:x64 /out:far_palette_query_child.exe far_palette_query_child.cs
// Usage: far_palette_query_child.exe <log file> [listen ms]
using System;
using System.IO;
using System.Text;
using System.Text.RegularExpressions;
using System.Runtime.InteropServices;

class FarPaletteQueryChild {
    [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetConsoleMode(IntPtr h, out uint mode);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetConsoleMode(IntPtr h, uint mode);
    [DllImport("kernel32.dll", SetLastError=true)] static extern uint WaitForSingleObject(IntPtr h, uint ms);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool ReadConsoleInputW(IntPtr h, [Out] INPUT_RECORD[] buf, uint len, out uint read);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool WriteConsoleW(IntPtr h, string s, uint n, out uint written, IntPtr r);

    const int STD_INPUT_HANDLE = -10, STD_OUTPUT_HANDLE = -11;
    const uint FAR_MODE = 0x01B8;
    const uint ENABLE_VIRTUAL_TERMINAL_PROCESSING = 0x0004;
    const ushort KEY_EVENT = 1;

    [StructLayout(LayoutKind.Sequential)]
    struct KEY_EVENT_RECORD {
        public int bKeyDown; public ushort wRepeatCount; public ushort wVirtualKeyCode;
        public ushort wVirtualScanCode; public ushort UnicodeChar; public uint dwControlKeyState;
    }
    [StructLayout(LayoutKind.Explicit, Size = 20)]
    struct INPUT_RECORD {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public KEY_EVENT_RECORD KeyEvent;
    }

    static string Show(string s) {
        var sb = new StringBuilder();
        foreach (char c in s) {
            if (c == 0x1b) sb.Append("\\e");
            else if (c < 0x20 || c > 0x7e) sb.AppendFormat("\\x{0:x2}", (int)c);
            else sb.Append(c);
        }
        return sb.ToString();
    }

    static int Main(string[] args) {
        string log = args.Length > 0 ? args[0] : Path.Combine(Path.GetTempPath(), "far_palette_query.txt");
        int listen = args.Length > 1 ? int.Parse(args[1]) : 3000;
        File.WriteAllText(log, "FAR_QUERY START\n");

        IntPtr hIn = GetStdHandle(STD_INPUT_HANDLE), hOut = GetStdHandle(STD_OUTPUT_HANDLE);
        uint outMode; if (GetConsoleMode(hOut, out outMode)) SetConsoleMode(hOut, outMode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        SetConsoleMode(hIn, FAR_MODE);
        uint m; GetConsoleMode(hIn, out m);
        File.AppendAllText(log, string.Format("MODE 0x{0:X4}\n", m));

        // Far's exact request, in one write.
        var q = new StringBuilder("\x1b[0c\x1b]4");
        for (int i = 0; i < 256; i++) q.Append(";").Append(i).Append(";?");
        q.Append("\x1b\\\x1b[0c");
        uint w; WriteConsoleW(hOut, q.ToString(), (uint)q.Length, out w, IntPtr.Zero);

        // Read every key record for the listen period.
        var buf = new INPUT_RECORD[256];
        var text = new StringBuilder();
        var stop = DateTime.UtcNow.AddMilliseconds(listen);
        while (DateTime.UtcNow < stop) {
            if (WaitForSingleObject(hIn, 50) != 0) continue;
            uint read;
            if (!ReadConsoleInputW(hIn, buf, (uint)buf.Length, out read)) break;
            for (int i = 0; i < read; i++) {
                if (buf[i].EventType != KEY_EVENT || buf[i].KeyEvent.bKeyDown == 0) continue;
                if (buf[i].KeyEvent.UnicodeChar != 0) text.Append((char)buf[i].KeyEvent.UnicodeChar);
            }
        }
        string all = text.ToString();
        var da1 = new Regex("\x1b\\[\\?[0-9;]*c");
        int daCount = da1.Matches(all).Count;
        string stray = da1.Replace(all, "");
        File.AppendAllText(log, string.Format("DA1 {0}\nSTRAY_KEYS {1} text={2}\nFAR_QUERY END\n",
            daCount, stray.Length, Show(stray)));
        return 0;
    }
}
