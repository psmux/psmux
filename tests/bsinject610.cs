// bsinject610.exe <pid> <spec> [spec...]
//
// Injects specific KEY_EVENT records into another process's console input
// buffer with WriteConsoleInput.  Unlike tests/injector.cs this lets the caller
// dictate the EXACT (vk, uChar, dwControlKeyState) triple, which is the whole
// point for issue #610: Ctrl+Backspace differs from Backspace only by the
// control key state and by the uChar a given terminal chooses to put in it.
//
// Specs:
//   cbs        Ctrl+Backspace as Windows Terminal / conhost generate it
//              (VK_BACK 0x08, uChar 0x7F, LEFT_CTRL_PRESSED)
//   cbs08      Ctrl+Backspace with uChar 0x08 instead
//   cbs00      Ctrl+Backspace with uChar 0x00
//   bs         plain Backspace (VK_BACK, uChar 0x08, no modifiers)
//   sbs        Shift+Backspace (VK_BACK, uChar 0x08, SHIFT_PRESSED)
//   abs        Alt+Backspace (VK_BACK, uChar 0x08, LEFT_ALT_PRESSED)
//   ctrlw      Ctrl+W (VK 0x57, uChar 0x17, LEFT_CTRL_PRESSED)
//   ctrlh      Ctrl+H (VK 0x48, uChar 0x08, LEFT_CTRL_PRESSED)
//   ctrlz      Ctrl+Z sentinel
//   text:foo   literal text
//   cr         Enter
//   raw:VK,UCHAR,CTRL   fully explicit, all hex without 0x
//   sleep:ms
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;

class BsInject610
{
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateFileW(string name, uint access, uint share,
        IntPtr sec, uint disp, uint flags, IntPtr tmpl);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool WriteConsoleInput(IntPtr h, INPUT_RECORD[] buf, uint len, out uint written);
    [DllImport("user32.dll")] static extern uint MapVirtualKeyW(uint code, uint mapType);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern short VkKeyScanW(char ch);

    const uint GENERIC_READ = 0x80000000;
    const uint GENERIC_WRITE = 0x40000000;
    const uint FILE_SHARE_READ = 1;
    const uint FILE_SHARE_WRITE = 2;
    const uint OPEN_EXISTING = 3;
    const ushort KEY_EVENT = 1;
    const uint LEFT_CTRL_PRESSED = 0x0008;
    const uint LEFT_ALT_PRESSED = 0x0002;
    const uint SHIFT_PRESSED = 0x0010;

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

    static List<string> log = new List<string>();

    static INPUT_RECORD Rec(bool down, ushort vk, char ch, uint ctrl)
    {
        var r = new INPUT_RECORD();
        r.EventType = KEY_EVENT;
        r.KeyEvent.bKeyDown = down ? 1 : 0;
        r.KeyEvent.wRepeatCount = 1;
        r.KeyEvent.wVirtualKeyCode = vk;
        r.KeyEvent.wVirtualScanCode = (ushort)MapVirtualKeyW(vk, 0);
        r.KeyEvent.UnicodeChar = ch;
        r.KeyEvent.dwControlKeyState = ctrl;
        return r;
    }

    // Sends the modifier key down, the key itself, then releases, exactly the
    // shape conhost produces for a real physical chord.
    static void Chord(IntPtr h, ushort vk, char ch, uint ctrl, string name)
    {
        var recs = new List<INPUT_RECORD>();
        if ((ctrl & LEFT_CTRL_PRESSED) != 0) recs.Add(Rec(true, 0x11, '\0', ctrl));
        if ((ctrl & SHIFT_PRESSED) != 0) recs.Add(Rec(true, 0x10, '\0', ctrl));
        if ((ctrl & LEFT_ALT_PRESSED) != 0) recs.Add(Rec(true, 0x12, '\0', ctrl));
        recs.Add(Rec(true, vk, ch, ctrl));
        recs.Add(Rec(false, vk, ch, ctrl));
        if ((ctrl & LEFT_ALT_PRESSED) != 0) recs.Add(Rec(false, 0x12, '\0', 0));
        if ((ctrl & SHIFT_PRESSED) != 0) recs.Add(Rec(false, 0x10, '\0', 0));
        if ((ctrl & LEFT_CTRL_PRESSED) != 0) recs.Add(Rec(false, 0x11, '\0', 0));
        uint w;
        var arr = recs.ToArray();
        bool ok = WriteConsoleInput(h, arr, (uint)arr.Length, out w);
        log.Add(string.Format("  {0}: vk=0x{1:X2} uChar=0x{2:X4} ctrl=0x{3:X4} n={4} ok={5} w={6} e={7}",
            name, vk, (int)ch, ctrl, arr.Length, ok, w, ok ? 0 : Marshal.GetLastWin32Error()));
    }

    static void Text(IntPtr h, string s)
    {
        foreach (char c in s)
        {
            short sc = VkKeyScanW(c);
            ushort vk = (ushort)(sc & 0xFF);
            uint ctrl = 0;
            if ((sc & 0x100) != 0) ctrl |= SHIFT_PRESSED;
            var recs = new List<INPUT_RECORD>();
            if ((ctrl & SHIFT_PRESSED) != 0) recs.Add(Rec(true, 0x10, '\0', ctrl));
            recs.Add(Rec(true, vk, c, ctrl));
            recs.Add(Rec(false, vk, c, ctrl));
            if ((ctrl & SHIFT_PRESSED) != 0) recs.Add(Rec(false, 0x10, '\0', 0));
            uint w;
            var arr = recs.ToArray();
            WriteConsoleInput(h, arr, (uint)arr.Length, out w);
        }
        log.Add("  text: [" + s + "]");
    }

    static int Main(string[] argv)
    {
        string logFile = Path.Combine(Path.GetTempPath(), "bsinject610.log");
        if (argv.Length < 2)
        {
            File.WriteAllText(logFile, "usage: bsinject610 <pid> <spec>...\r\n");
            return 99;
        }
        uint pid;
        if (!uint.TryParse(argv[0], out pid))
        {
            File.WriteAllText(logFile, "bad pid\r\n");
            return 98;
        }
        log.Add("PID=" + pid);

        FreeConsole();
        if (!AttachConsole(pid))
        {
            log.Add("AttachConsole FAILED e=" + Marshal.GetLastWin32Error());
            File.WriteAllText(logFile, string.Join("\r\n", log.ToArray()));
            return 97;
        }
        IntPtr h = CreateFileW("CONIN$", GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
        if (h == new IntPtr(-1) || h == IntPtr.Zero)
        {
            log.Add("CONIN$ FAILED e=" + Marshal.GetLastWin32Error());
            FreeConsole();
            File.WriteAllText(logFile, string.Join("\r\n", log.ToArray()));
            return 96;
        }

        for (int i = 1; i < argv.Length; i++)
        {
            string s = argv[i];
            if (s == "cbs") Chord(h, 0x08, (char)0x7F, LEFT_CTRL_PRESSED, "Ctrl+BS(uChar 7F)");
            else if (s == "cbs08") Chord(h, 0x08, (char)0x08, LEFT_CTRL_PRESSED, "Ctrl+BS(uChar 08)");
            else if (s == "cbs00") Chord(h, 0x08, (char)0x00, LEFT_CTRL_PRESSED, "Ctrl+BS(uChar 00)");
            else if (s == "bs") Chord(h, 0x08, (char)0x08, 0, "Backspace");
            else if (s == "sbs") Chord(h, 0x08, (char)0x08, SHIFT_PRESSED, "Shift+BS");
            else if (s == "abs") Chord(h, 0x08, (char)0x08, LEFT_ALT_PRESSED, "Alt+BS");
            else if (s == "ctrlw") Chord(h, 0x57, (char)0x17, LEFT_CTRL_PRESSED, "Ctrl+W");
            else if (s == "ctrlh") Chord(h, 0x48, (char)0x08, LEFT_CTRL_PRESSED, "Ctrl+H");
            else if (s == "ctrlz") Chord(h, 0x5A, (char)0x1A, LEFT_CTRL_PRESSED, "Ctrl+Z");
            else if (s == "cr") Chord(h, 0x0D, (char)0x0D, 0, "Enter");
            else if (s.StartsWith("text:")) Text(h, s.Substring(5));
            else if (s.StartsWith("sleep:"))
            {
                int ms = int.Parse(s.Substring(6));
                Thread.Sleep(ms);
                log.Add("  sleep " + ms);
            }
            else if (s.StartsWith("raw:"))
            {
                var p = s.Substring(4).Split(',');
                ushort vk = ushort.Parse(p[0], NumberStyles.HexNumber);
                char ch = (char)ushort.Parse(p[1], NumberStyles.HexNumber);
                uint ct = uint.Parse(p[2], NumberStyles.HexNumber);
                Chord(h, vk, ch, ct, "raw");
            }
            else log.Add("  UNKNOWN spec " + s);
            Thread.Sleep(60);
        }

        FreeConsole();
        File.WriteAllText(logFile, string.Join("\r\n", log.ToArray()));
        return 0;
    }
}
