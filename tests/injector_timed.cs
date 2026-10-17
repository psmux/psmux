// Timestamped WriteConsoleInput injector (issue #841 latency measurement).
//
// Usage: injector_timed.exe <pid> <outfile> <token> [<token> ...]
// token: a single character, ESC, ENTER, S<ms> (sleep), or D:<key> / U:<key>
// to send only the key down or key up record, or VK:<hex>[:<hex>] for a key
// with no character (VK:26:100 is Up with ENHANCED_KEY, VK:74 is F5, issue
// #845).  Each key write is logged as
// "<unix ms with fraction> <token>" using GetSystemTimePreciseAsFileTime, so a
// reader on the far side can compare arrival times with injection times.
using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;

class InjectorTimed
{
    [DllImport("kernel32.dll")] static extern bool FreeConsole();
    [DllImport("kernel32.dll")] static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateFileW(string n, uint a, uint s, IntPtr sec, uint d, uint f, IntPtr t);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool WriteConsoleInputW(IntPtr h, INPUT_RECORD[] b, uint l, out uint w);
    [DllImport("user32.dll")] static extern uint MapVirtualKeyW(uint c, uint t);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern short VkKeyScanW(char ch);
    [DllImport("kernel32.dll")] static extern void GetSystemTimePreciseAsFileTime(out long ft);
    [DllImport("winmm.dll")] static extern uint timeBeginPeriod(uint p);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct KEY_EVENT_RECORD { public int bKeyDown; public ushort wRepeatCount; public ushort wVirtualKeyCode; public ushort wVirtualScanCode; public char UnicodeChar; public uint dwControlKeyState; }
    [StructLayout(LayoutKind.Explicit, CharSet = CharSet.Unicode)]
    struct INPUT_RECORD { [FieldOffset(0)] public ushort EventType; [FieldOffset(4)] public KEY_EVENT_RECORD KeyEvent; }

    static INPUT_RECORD K(bool down, ushort vk, char ch, uint cs = 0)
    {
        var r = new INPUT_RECORD(); r.EventType = 1; r.KeyEvent.dwControlKeyState = cs;
        r.KeyEvent.bKeyDown = down ? 1 : 0; r.KeyEvent.wRepeatCount = 1;
        r.KeyEvent.wVirtualKeyCode = vk; r.KeyEvent.wVirtualScanCode = (ushort)MapVirtualKeyW(vk, 0);
        r.KeyEvent.UnicodeChar = ch; return r;
    }
    static double Now() { long ft; GetSystemTimePreciseAsFileTime(out ft); return (ft - 116444736000000000L) / 10000.0; }

    static int Main(string[] args)
    {
        timeBeginPeriod(1);
        uint pid = uint.Parse(args[0]);
        var log = new List<string>();
        FreeConsole();
        if (!AttachConsole(pid)) { File.WriteAllText(args[1], "attach failed"); return 2; }
        IntPtr h = CreateFileW("CONIN$", 0xC0000000u, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        for (int i = 2; i < args.Length; i++)
        {
            string t = args[i];
            if (t.StartsWith("S") && t.Length > 1) { Thread.Sleep(int.Parse(t.Substring(1))); continue; }
            int part = 0; if (t.StartsWith("D:")) { part = 1; t = t.Substring(2); } else if (t.StartsWith("U:")) { part = 2; t = t.Substring(2); }
            ushort vk; char ch; uint cs = 0;
            if (t.StartsWith("VK:"))
            {
                var f = t.Substring(3).Split(':');
                vk = Convert.ToUInt16(f[0], 16); ch = (char)0;
                if (f.Length > 1) cs = Convert.ToUInt32(f[1], 16);
            }
            else if (t == "ESC") { vk = 0x1B; ch = (char)0x1B; }
            else if (t == "ENTER") { vk = 0x0D; ch = '\r'; }
            else { ch = t[0]; vk = (ushort)(VkKeyScanW(ch) & 0xFF); }
            var recs = part == 1 ? new INPUT_RECORD[] { K(true, vk, ch, cs) } : part == 2 ? new INPUT_RECORD[] { K(false, vk, ch, cs) } : new INPUT_RECORD[] { K(true, vk, ch, cs), K(false, vk, ch, cs) };
            uint w;
            double ts = Now();
            WriteConsoleInputW(h, recs, (uint)recs.Length, out w);
            if (part != 2) log.Add(string.Format("{0:F3} {1}", ts, t));
        }
        FreeConsole();
        File.WriteAllLines(args[1], log);
        return 0;
    }
}

