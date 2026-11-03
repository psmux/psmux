// seq_emit980.exe <log file> <sequence> [listen ms] [hold seconds]
//
// Issue #980 pane child.  Writes <sequence> to its console output, with the
// two character spellings \e as ESC, \s as ';', \a as BEL and \b as a
// backslash (so no shell between psmux and the child splits or eats them),
// and \p as a pause of one second,
// then reads console input records for [listen ms] (default 3000) and
// appends every character it reads to
// <log file>, so an OSC 4 query reply that psmux types into the pane can be
// read back.  Then it idles [hold seconds] (default 40) so the pane stays
// alive for a client capture.
//
// The console input mode gets ENABLE_VIRTUAL_TERMINAL_INPUT so a reply that
// arrives as VT text is delivered as characters.
//
// Build: csc /nologo /platform:x64 /out:seq_emit980.exe seq_emit980.cs
using System;
using System.IO;
using System.Text;
using System.Threading;
using System.Runtime.InteropServices;

class SeqEmit980
{
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetConsoleMode(IntPtr h, out uint mode);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool SetConsoleMode(IntPtr h, uint mode);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint WaitForSingleObject(IntPtr h, uint ms);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool ReadConsoleInputW(IntPtr h, [Out] INPUT_RECORD[] buf, uint len, out uint read);

    [StructLayout(LayoutKind.Explicit, CharSet = CharSet.Unicode)]
    struct INPUT_RECORD
    {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public int bKeyDown;
        [FieldOffset(8)] public ushort wRepeatCount;
        [FieldOffset(10)] public ushort wVirtualKeyCode;
        [FieldOffset(12)] public ushort wVirtualScanCode;
        [FieldOffset(14)] public char UnicodeChar;
        [FieldOffset(16)] public uint dwControlKeyState;
    }

    static void Main(string[] args)
    {
        string log = args[0];
        string seq = args[1].Replace("\\e", "\u001b").Replace("\\s", ";")
            .Replace("\\a", "\u0007").Replace("\\b", "\\");
        int listen = args.Length > 2 ? int.Parse(args[2]) : 3000;
        int hold = args.Length > 3 ? int.Parse(args[3]) : 40;

        IntPtr hin = GetStdHandle(-10);
        uint mode;
        if (GetConsoleMode(hin, out mode))
            SetConsoleMode(hin, (mode | 0x0200) & ~0x0006u);

        Stream o = Console.OpenStandardOutput();
        Thread.Sleep(300);
        // \p in the sequence is a pause of one second between writes.
        foreach (string part in seq.Split(new string[] { "\\p" }, StringSplitOptions.None))
        {
            if (part.Length == 0) { Thread.Sleep(1000); continue; }
            byte[] b = Encoding.ASCII.GetBytes(part);
            o.Write(b, 0, b.Length);
            o.Flush();
            Thread.Sleep(1000);
        }

        StringBuilder got = new StringBuilder();
        INPUT_RECORD[] recs = new INPUT_RECORD[64];
        DateTime end = DateTime.UtcNow.AddMilliseconds(listen);
        while (DateTime.UtcNow < end)
        {
            int left = (int)(end - DateTime.UtcNow).TotalMilliseconds;
            if (left <= 0) break;
            if (WaitForSingleObject(hin, (uint)left) != 0) break;
            uint n;
            if (!ReadConsoleInputW(hin, recs, (uint)recs.Length, out n)) break;
            for (int i = 0; i < n; i++)
                if (recs[i].EventType == 1 && recs[i].bKeyDown != 0 && recs[i].UnicodeChar != 0)
                    got.Append(recs[i].UnicodeChar);
        }
        File.WriteAllText(log, got.ToString().Replace("\u001b", "ESC"));
        Thread.Sleep(hold * 1000);
    }
}
