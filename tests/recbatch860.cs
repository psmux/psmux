// recbatch860.exe <pid> <step>...
// Issue #860 record injector: writes exact KEY_EVENT records into a console.
// step: sleep:<ms> | wt:<text>  (Windows Terminal sendInput shape: every char a
//       vk=0 scan=0 record, ALL in one WriteConsoleInput) | g:<vk>.<ch>.<ctrl>[+...]
//       (hex; each a down+up pair, the whole group in one WriteConsoleInput)
// wt: text understands \e for ESC.
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading;

class RecBatch
{
    [DllImport("kernel32.dll")] static extern bool FreeConsole();
    [DllImport("kernel32.dll")] static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern IntPtr CreateFileW(string n, uint a, uint s, IntPtr sec, uint d, uint f, IntPtr t);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern bool WriteConsoleInputW(IntPtr h, IR[] b, uint l, out uint w);
    [DllImport("user32.dll")] static extern uint MapVirtualKeyW(uint c, uint t);
    [StructLayout(LayoutKind.Explicit, CharSet = CharSet.Unicode)]
    struct IR
    {
        [FieldOffset(0)] public ushort T; [FieldOffset(4)] public int Down; [FieldOffset(8)] public ushort Rep;
        [FieldOffset(10)] public ushort Vk; [FieldOffset(12)] public ushort Sc; [FieldOffset(14)] public ushort Ch; [FieldOffset(16)] public uint Ctrl;
    }
    static IR R(bool down, ushort vk, ushort sc, ushort ch, uint ctrl)
    {
        var r = new IR(); r.T = 1; r.Down = down ? 1 : 0; r.Rep = 1; r.Vk = vk; r.Sc = sc; r.Ch = ch; r.Ctrl = ctrl; return r;
    }
    static int Main(string[] a)
    {
        FreeConsole();
        if (!AttachConsole(uint.Parse(a[0]))) { Console.Error.WriteLine("ATTACH_FAIL"); return 2; }
        IntPtr h = CreateFileW("CONIN$", 0xC0000000u, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        for (int i = 1; i < a.Length; i++)
        {
            string s = a[i];
            var recs = new List<IR>();
            if (s.StartsWith("sleep:")) { Thread.Sleep(int.Parse(s.Substring(6))); continue; }
            if (s.StartsWith("wt:"))
            {
                string t = s.Substring(3).Replace("\\e", "\x1b");
                foreach (char c in t) { recs.Add(R(true, 0, 0, c, 0)); recs.Add(R(false, 0, 0, c, 0)); }
            }
            else if (s.StartsWith("g:"))
            {
                foreach (var part in s.Substring(2).Split('+'))
                {
                    var p = part.Split('.');
                    ushort vk = Convert.ToUInt16(p[0], 16); ushort ch = Convert.ToUInt16(p[1], 16); uint ctrl = Convert.ToUInt32(p[2], 16);
                    ushort sc = (ushort)MapVirtualKeyW(vk, 0);
                    recs.Add(R(true, vk, sc, ch, ctrl)); recs.Add(R(false, vk, sc, ch, ctrl));
                }
            }
            uint w; WriteConsoleInputW(h, recs.ToArray(), (uint)recs.Count, out w);
            Thread.Sleep(30);
        }
        return 0;
    }
}
