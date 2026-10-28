// reply_order_probe.exe <outFile>
//
// Asks the console what a terminal probe asks (yazi sends these at start up)
// in one write: XTVERSION `ESC [ > q`, OSC 11 `ESC ] 11 ; ? ESC \`, the cell
// size `ESC [ 16 t` and DA1 `ESC [ c`, the last being what such programs take
// as the end of the replies.  Reads console input for two seconds and writes
// the replies in the order they arrived, one name per line:
//
//   XTVERSION  ESC P > | ... ESC \
//   OSC11      ESC ] 11 ; ... ESC \
//   CELL       ESC [ 6 ; h ; w t
//   DA1        ESC [ ? ... c
//
// Compile: csc /nologo /optimize /out:reply_order_probe.exe reply_order_probe.cs
using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;

class ReplyOrderProbe
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
        byte[] q = Encoding.ASCII.GetBytes("\x1b[>q\x1b]11;?\x1b\\\x1b[16t\x1b[c");
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
        var names = new List<string>();
        foreach (Match m in Regex.Matches(got.ToString(), "\x1bP>\\|[^\x1b]*\x1b\\\\|\x1b\\]11;[^\x1b\x07]*(\x1b\\\\|\x07)|\x1b\\[6;\\d+;\\d+t|\x1b\\[\\?[\\d;]*c"))
        {
            string v = m.Value;
            names.Add(v.StartsWith("\x1bP") ? "XTVERSION" : v.StartsWith("\x1b]") ? "OSC11" : v.EndsWith("t") ? "CELL" : "DA1");
        }
        File.WriteAllText(args[0], string.Join("\n", names) + "\n");
    }
}
