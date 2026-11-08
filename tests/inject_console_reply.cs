// inject_console_reply.exe <pid> <text>
//
// Attaches to the console of <pid> and writes <text> (\e for ESC, \a for BEL)
// into its input the way a terminal's reply arrives there with VT input off:
// one key down and one key up record a character, with no virtual key
// (measured on Windows Terminal).
//
// Compile: csc /nologo /optimize /out:inject_console_reply.exe inject_console_reply.cs
using System;
using System.Runtime.InteropServices;

class InjectConsoleReply
{
    [DllImport("kernel32.dll")] static extern bool FreeConsole();
    [DllImport("kernel32.dll")] static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateFileW(string n, uint a, uint s, IntPtr sec, uint d, uint f, IntPtr t);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    static extern bool WriteConsoleInputW(IntPtr h, INPUT_RECORD[] b, uint n, out uint w);
    [StructLayout(LayoutKind.Explicit)] struct INPUT_RECORD {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public int KeyDown;
        [FieldOffset(8)] public ushort Repeat;
        [FieldOffset(10)] public ushort VKey;
        [FieldOffset(12)] public ushort Scan;
        [FieldOffset(14)] public char UChar;
        [FieldOffset(16)] public uint Ctrl;
    }

    static int Main(string[] args)
    {
        uint pid = uint.Parse(args[0]);
        string text = args[1].Replace("\\e", "\x1b").Replace("\\a", "\x07");
        FreeConsole();
        if (!AttachConsole(pid)) { Console.Error.WriteLine("attach failed"); return 2; }
        IntPtr h = CreateFileW("CONIN$", 0xC0000000u, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        var recs = new INPUT_RECORD[text.Length * 2];
        for (int i = 0; i < text.Length; i++)
        {
            recs[2 * i] = new INPUT_RECORD { EventType = 1, KeyDown = 1, Repeat = 1, UChar = text[i] };
            recs[2 * i + 1] = new INPUT_RECORD { EventType = 1, KeyDown = 0, Repeat = 1, UChar = text[i] };
        }
        uint w;
        bool ok = WriteConsoleInputW(h, recs, (uint)recs.Length, out w);
        FreeConsole();
        return ok && w == recs.Length ? 0 : 3;
    }
}
