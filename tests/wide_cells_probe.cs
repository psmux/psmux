// wide_cells_probe.exe <casesFile> <index> <outFile>
//
// Clears its console, writes case <index> of <casesFile> in one write (a line
// "name|bytes", with `\e` standing for ESC), reads the first ten cells of the
// row it wrote back from its own console buffer (ReadConsoleOutputW, with the
// leading and trailing byte flags a wide character's halves carry), writes
// them to <outFile> and waits six seconds, so that the pane can be captured
// while it runs.  A cell is written as its character, `_` for a space, and
// `<` or `>` after the left or right half of a wide character.
//
// Compile: csc /nologo /optimize /out:wide_cells_probe.exe wide_cells_probe.cs
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

class WideCellsProbe
{
    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X, Y; }
    [StructLayout(LayoutKind.Sequential)] struct SMALL_RECT { public short Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] struct CSBI { public COORD Size, Cursor; public ushort Attr; public SMALL_RECT Win; public COORD Max; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] struct CHAR_INFO { public char Ch; public ushort Attr; }
    [DllImport("kernel32.dll")] static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll")] static extern bool GetConsoleMode(IntPtr h, out uint m);
    [DllImport("kernel32.dll")] static extern bool SetConsoleMode(IntPtr h, uint m);
    [DllImport("kernel32.dll")] static extern bool SetConsoleOutputCP(uint cp);
    [DllImport("kernel32.dll")] static extern bool WriteFile(IntPtr h, byte[] b, uint n, out uint w, IntPtr o);
    [DllImport("kernel32.dll")] static extern bool GetConsoleScreenBufferInfo(IntPtr h, out CSBI i);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern bool ReadConsoleOutputW(IntPtr h, [Out] CHAR_INFO[] b, COORD size, COORD at, ref SMALL_RECT r);

    const ushort LeadingByte = 0x100;
    const ushort TrailingByte = 0x200;

    static void Write(IntPtr h, string s)
    {
        byte[] b = Encoding.UTF8.GetBytes(s);
        uint w;
        WriteFile(h, b, (uint)b.Length, out w, IntPtr.Zero);
    }

    static void Main(string[] args)
    {
        string[] lines = File.ReadAllLines(args[0], Encoding.UTF8);
        string[] c = lines[int.Parse(args[1])].Split('|');
        string bytes = c[1].Replace("\\e", "\x1b");
        IntPtr hout = GetStdHandle(-11);
        SetConsoleOutputCP(65001);
        uint mode;
        GetConsoleMode(hout, out mode);
        // ENABLE_VIRTUAL_TERMINAL_PROCESSING
        SetConsoleMode(hout, mode | 0x4);
        Write(hout, "\x1b[H\x1b[2J" + bytes);
        Thread.Sleep(500);
        CSBI info;
        GetConsoleScreenBufferInfo(hout, out info);
        const int cols = 10;
        var buf = new CHAR_INFO[cols];
        var r = new SMALL_RECT { Left = 0, Top = info.Win.Top, Right = cols - 1, Bottom = info.Win.Top };
        ReadConsoleOutputW(hout, buf, new COORD { X = cols, Y = 1 }, new COORD { X = 0, Y = 0 }, ref r);
        var sb = new StringBuilder();
        for (int i = 0; i < cols; i++) {
            sb.Append(buf[i].Ch == ' ' ? '_' : buf[i].Ch);
            if ((buf[i].Attr & LeadingByte) != 0) sb.Append('<');
            else if ((buf[i].Attr & TrailingByte) != 0) sb.Append('>');
        }
        File.WriteAllText(args[2], sb.ToString(), new UTF8Encoding(false));
        Write(hout, "\x1b[6;1H");
        Thread.Sleep(6000);
    }
}
