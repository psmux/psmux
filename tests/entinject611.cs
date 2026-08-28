// entinject611.exe <pid> <spec> [spec...]
//
// Console input injector for issue #611.  tests/bsinject610.cs already writes
// an arbitrary (vk, uChar, dwControlKeyState) triple, but it sleeps 60 ms
// between specs, which is fatal for the half of #611 that is a TIMING claim:
// an ESC and a CR that a terminal delivers back to back inside ONE write must
// be measured with no injector-side gap at all, otherwise the harness itself
// manufactures the gap it is trying to detect.
//
// So this injector has an ATOMIC spec: every key inside one `atomic:` group is
// packed into a SINGLE WriteConsoleInput call with zero delay, exactly the way
// conhost's ConPTY input parser hands over the records it decoded from one
// incoming VT write of "\x1b\r".
//
// Specs:
//   sent          Shift+Enter    (VK_RETURN 0x0D, uChar 0x0D, SHIFT_PRESSED)
//   ent           plain Enter    (VK_RETURN, uChar 0x0D, no modifiers)
//   ment          Alt+Enter      (VK_RETURN, uChar 0x0D, LEFT_ALT_PRESSED)
//   cent          Ctrl+Enter     (VK_RETURN, uChar 0x0A, LEFT_CTRL_PRESSED)
//   esc           Escape         (VK_ESCAPE 0x1B, uChar 0x1B)
//   escent        ESC then CR in ONE WriteConsoleInput call, no gap
//   escent:MS     ESC, then MS milliseconds, then CR (two separate writes)
//   text:foo      literal text, one write per character
//   raw:VK,UCHAR,CTRL           explicit triple, hex without 0x
//   atomic:SPEC+SPEC+...        the listed specs packed into one write
//   sleep:MS
//   ctrlz         Ctrl+Z sentinel
//
// Unlike bsinject610.cs there is NO implicit sleep between specs; use sleep:.
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;

class EntInject611
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
    const uint LEFT_ALT_PRESSED = 0x0002;
    const uint LEFT_CTRL_PRESSED = 0x0008;
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

    static IntPtr h;
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

    // The down/up pair for one key.  No modifier key records: conhost's ConPTY
    // input parser does NOT emit separate VK_SHIFT records when it decodes a
    // win32-input-mode sequence, it stamps dwControlKeyState on the key record
    // itself, and that is the shape #611 is about.
    static void Key(List<INPUT_RECORD> into, ushort vk, char ch, uint ctrl)
    {
        into.Add(Rec(true, vk, ch, ctrl));
        into.Add(Rec(false, vk, ch, ctrl));
    }

    static void Flush(List<INPUT_RECORD> recs, string what)
    {
        if (recs.Count == 0) return;
        uint w;
        var arr = recs.ToArray();
        bool ok = WriteConsoleInput(h, arr, (uint)arr.Length, out w);
        log.Add(string.Format("  WRITE {0} n={1} ok={2} w={3} e={4}",
            what, arr.Length, ok, w, ok ? 0 : Marshal.GetLastWin32Error()));
        recs.Clear();
    }

    static bool One(string s, List<INPUT_RECORD> into)
    {
        switch (s)
        {
            case "sent": Key(into, 0x0D, (char)0x0D, SHIFT_PRESSED); return true;
            case "ent": Key(into, 0x0D, (char)0x0D, 0); return true;
            case "ment": Key(into, 0x0D, (char)0x0D, LEFT_ALT_PRESSED); return true;
            case "cent": Key(into, 0x0D, (char)0x0A, LEFT_CTRL_PRESSED); return true;
            case "esc": Key(into, 0x1B, (char)0x1B, 0); return true;
            case "ctrlz": Key(into, 0x5A, (char)0x1A, LEFT_CTRL_PRESSED); return true;
        }
        if (s.StartsWith("raw:"))
        {
            var p = s.Substring(4).Split(',');
            ushort vk = ushort.Parse(p[0], NumberStyles.HexNumber);
            char ch = (char)ushort.Parse(p[1], NumberStyles.HexNumber);
            uint ct = p.Length > 2 ? uint.Parse(p[2], NumberStyles.HexNumber) : 0u;
            Key(into, vk, ch, ct);
            return true;
        }
        return false;
    }

    static int Main(string[] argv)
    {
        string logFile = Path.Combine(Path.GetTempPath(), "entinject611.log");
        if (argv.Length < 2)
        {
            File.WriteAllText(logFile, "usage: entinject611 <pid> <spec>...\r\n");
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
        h = CreateFileW("CONIN$", GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
        if (h == new IntPtr(-1) || h == IntPtr.Zero)
        {
            log.Add("CONIN$ FAILED e=" + Marshal.GetLastWin32Error());
            FreeConsole();
            File.WriteAllText(logFile, string.Join("\r\n", log.ToArray()));
            return 96;
        }

        var buf = new List<INPUT_RECORD>();
        for (int i = 1; i < argv.Length; i++)
        {
            string s = argv[i];
            if (s.StartsWith("sleep:"))
            {
                Thread.Sleep(int.Parse(s.Substring(6)));
                log.Add("  sleep " + s.Substring(6));
            }
            else if (s.StartsWith("atomic:"))
            {
                foreach (string part in s.Substring(7).Split('+'))
                    if (!One(part, buf)) log.Add("  UNKNOWN atomic part " + part);
                Flush(buf, "atomic[" + s.Substring(7) + "]");
            }
            else if (s == "escent")
            {
                One("esc", buf);
                One("ent", buf);
                Flush(buf, "escent(one write)");
            }
            else if (s.StartsWith("escent:"))
            {
                int ms = int.Parse(s.Substring(7));
                One("esc", buf); Flush(buf, "esc");
                Thread.Sleep(ms);
                One("ent", buf); Flush(buf, "cr after " + ms + "ms");
            }
            else if (s.StartsWith("text:"))
            {
                foreach (char c in s.Substring(5))
                {
                    short sc = VkKeyScanW(c);
                    ushort vk = (ushort)(sc & 0xFF);
                    uint ctrl = (sc & 0x100) != 0 ? SHIFT_PRESSED : 0u;
                    Key(buf, vk, c, ctrl);
                    Flush(buf, "char " + c);
                }
            }
            else if (One(s, buf))
            {
                Flush(buf, s);
            }
            else log.Add("  UNKNOWN spec " + s);
        }

        FreeConsole();
        File.WriteAllText(logFile, string.Join("\r\n", log.ToArray()));
        return 0;
    }
}
