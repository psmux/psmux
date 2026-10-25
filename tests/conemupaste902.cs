// conemupaste902.exe <pid> <mode> <text>
// Writes a paste into <pid>'s console input buffer the way ConEmu does it
// (one WriteConsoleInput batch of plain key records, one down/up pair per char).
// mode "bracketed": wraps with ESC[200~ ... ESC[201~ (what ConEmu does once the
// app has asked for ?2004h). mode "plain": the text only.
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

class ConEmuPaste902
{
    [StructLayout(LayoutKind.Explicit, CharSet = CharSet.Unicode)]
    struct KEY_EVENT_RECORD
    {
        [FieldOffset(0)] public int bKeyDown;
        [FieldOffset(4)] public ushort wRepeatCount;
        [FieldOffset(6)] public ushort wVirtualKeyCode;
        [FieldOffset(8)] public ushort wVirtualScanCode;
        [FieldOffset(10)] public char UnicodeChar;
        [FieldOffset(12)] public uint dwControlKeyState;
    }
    [StructLayout(LayoutKind.Explicit)]
    struct INPUT_RECORD
    {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public KEY_EVENT_RECORD KeyEvent;
    }
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr CreateFileW(string n, uint a, uint s, IntPtr sa, uint c, uint f, IntPtr t);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool WriteConsoleInputW(IntPtr h, INPUT_RECORD[] r, uint n, out uint w);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern short VkKeyScanW(char c);

    static INPUT_RECORD Rec(bool down, ushort vk, char c, uint ctrl)
    {
        var r = new INPUT_RECORD();
        r.EventType = 1;
        r.KeyEvent.bKeyDown = down ? 1 : 0;
        r.KeyEvent.wRepeatCount = 1;
        r.KeyEvent.wVirtualKeyCode = vk;
        r.KeyEvent.UnicodeChar = c;
        r.KeyEvent.dwControlKeyState = ctrl;
        return r;
    }

    static int Main(string[] a)
    {
        uint pid = uint.Parse(a[0]);
        string s = a[2];
        if (a[1] == "bracketed") s = "\x1b[200~" + s + "\x1b[201~";
        var recs = new List<INPUT_RECORD>();
        foreach (char c in s)
        {
            ushort vk; uint ctrl = 0;
            if (c == '\x1b') vk = 0x1B;
            else
            {
                short sc = VkKeyScanW(c);
                vk = (ushort)(sc & 0xFF);
                if ((sc & 0x100) != 0) ctrl = 0x10;
            }
            recs.Add(Rec(true, vk, c, ctrl));
            recs.Add(Rec(false, vk, c, ctrl));
        }
        FreeConsole();
        if (!AttachConsole(pid)) { Console.Error.WriteLine("attach failed " + Marshal.GetLastWin32Error()); return 2; }
        IntPtr h = CreateFileW("CONIN$", 0xC0000000, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        uint w;
        bool ok = WriteConsoleInputW(h, recs.ToArray(), (uint)recs.Count, out w);
        FreeConsole();
        return ok ? 0 : 3;
    }
}
