// palette_reply_probe.exe <outFile> <queries>
//
// Writes `queries` to the console in one write, `\e` standing for ESC and
// `\a` for BEL, reads console input for two seconds, and writes every OSC 4
// reply that came back, one a line, as `index rgb:RRRR/GGGG/BBBB` in the
// order they arrived (`ESC ] 4 ; index ; rgb:... ESC \` or BEL). Used by the
// pane-colours suite to read what a pane is told for its palette.
//
// Compile: csc /nologo /optimize /out:palette_reply_probe.exe palette_reply_probe.cs
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;

class PaletteReplyProbe
{
    [DllImport("kernel32.dll")] static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll")] static extern bool GetConsoleMode(IntPtr h, out uint m);
    [DllImport("kernel32.dll")] static extern bool SetConsoleMode(IntPtr h, uint m);
    [DllImport("kernel32.dll")] static extern bool GetNumberOfConsoleInputEvents(IntPtr h, out uint n);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern bool ReadConsoleInputW(IntPtr h, [Out] INPUT_RECORD[] b, uint n, out uint r);
    [DllImport("kernel32.dll")] static extern bool WriteFile(IntPtr h, byte[] b, uint n, out uint w, IntPtr o);
    [StructLayout(LayoutKind.Explicit)] struct INPUT_RECORD {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public int KeyDown;
        [FieldOffset(14)] public char UChar;
        [FieldOffset(16)] public uint ControlKeyState;
    }

    static void Main(string[] args)
    {
        IntPtr hin = GetStdHandle(-10), hout = GetStdHandle(-11);
        uint orig; GetConsoleMode(hin, out orig);
        // Raw, with VT input, as a program that reads replies sets it.
        SetConsoleMode(hin, (orig & ~0x7u) | 0x200);
        uint om; GetConsoleMode(hout, out om); SetConsoleMode(hout, om | 0x4);
        string queries = args[1].Replace("\\e", "\x1b").Replace("\\a", "\x07");
        byte[] q = Encoding.ASCII.GetBytes(queries);
        uint w; WriteFile(hout, q, (uint)q.Length, out w, IntPtr.Zero);
        var got = new StringBuilder();
        var recs = new INPUT_RECORD[64];
        var end = DateTime.Now.AddSeconds(2);
        while (DateTime.Now < end)
        {
            uint n; GetNumberOfConsoleInputEvents(hin, out n);
            if (n == 0) { Thread.Sleep(2); continue; }
            uint r; ReadConsoleInputW(hin, recs, 64, out r);
            for (int i = 0; i < r; i++)
                if (recs[i].EventType == 1 && recs[i].KeyDown != 0 && recs[i].UChar != 0) got.Append(recs[i].UChar);
        }
        SetConsoleMode(hin, orig);
        var lines = new StringBuilder();
        foreach (Match m in Regex.Matches(got.ToString(), "\x1b\\]4;(\\d+);(rgb:[0-9a-fA-F/]+)(\x1b\\\\|\x07)"))
            lines.Append(m.Groups[1].Value).Append(' ').Append(m.Groups[2].Value.ToLowerInvariant()).Append('\n');
        File.WriteAllText(args[0], lines.ToString());
    }
}
