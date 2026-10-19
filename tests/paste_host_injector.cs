// Emulates what a terminal host does for a clipboard paste, at the console
// input buffer, so the psmux client's Ctrl+V handling can be driven without a
// real keyboard or a real Windows Terminal keybinding.
//
// Windows Terminal binds Ctrl+V itself: the PRESS never reaches the client, the
// clipboard text is injected as ordinary character key events, and only the
// RELEASE of the V key is forwarded (win32 input mode forwards key ups). That
// release is what the psmux client's "Ctrl+V Release fallback" sees, and the
// duplicate paste in PR #667 / PR #676 is that fallback reading the clipboard a
// second time after the characters already arrived.
//
//   paste_host_injector.exe <pid> host  <delay_ms> <text>  chars, then after
//                                    delay_ms the Ctrl+V release only. A hand
//                                    releases the key 50 to 200 ms after the
//                                    press, and the host injects within a few
//                                    ms of the press, so the delay is the gap
//                                    the client's paste detection has to cope
//                                    with.
//   paste_host_injector.exe <pid> plain <delay_ms> <text>  Ctrl+V press, then
//                                    the release after delay_ms, no chars: a
//                                    terminal that does not paste on Ctrl+V.
//                                    Since issue #858 that is NOT a paste: the
//                                    key belongs to a binding or to the pane.
//   paste_host_injector.exe <pid> release <delay_ms> <text>  the Ctrl+V
//                                    release only, no press and no chars: the
//                                    host's binding took the press and pasted
//                                    where the client cannot see it, so the
//                                    client reads the clipboard itself.
//   paste_host_injector.exe <pid> hostv <delay_ms> <text>  host, with the
//                                    Ctrl+V PRESS delivered before the chars.
//   paste_host_injector.exe <pid> drip  <delay_ms> <text>  the same as host,
//                                    except every character is its OWN
//                                    WriteConsoleInputW call with a gap
//                                    (PSMUX_INJECT_GAP_MS, default 2) between
//                                    them.  This is the shape gabri-ns measured
//                                    on Windows 10 19045: the first event batch
//                                    the client drains holds ONE character, not
//                                    the whole clipboard, so a client that
//                                    flushes a short batch as typing puts the
//                                    head of the paste outside the brackets.
//   paste_host_injector.exe <pid> dripv <delay_ms> <text>  drip, with the
//                                    Ctrl+V PRESS delivered first (a host that
//                                    does not swallow the press).
//   paste_host_injector.exe <pid> type  <delay_ms> <text>  no paste at all: a
//                                    hand typing, one character per
//                                    PSMUX_INJECT_GAP_MS (default 120), press
//                                    and release 40 ms apart, no Ctrl anywhere.
//                                    The control case for the paste head hold.
//
// Exit codes: 0 delivered, 2 AttachConsole failed (treat as SKIP), 3 write failed.
// Log: %TEMP%\psmux_paste_host_inject.log
using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;

class PasteHostInjector
{
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool FreeConsole();

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AttachConsole(uint pid);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateFileW(string name, uint access, uint share,
        IntPtr sec, uint disp, uint flags, IntPtr tmpl);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool WriteConsoleInput(IntPtr h, INPUT_RECORD[] buf, uint len, out uint written);

    [DllImport("user32.dll")]
    static extern uint MapVirtualKeyW(uint code, uint mapType);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern short VkKeyScanW(char ch);

    const ushort KEY_EVENT = 1;
    const uint LEFT_CTRL_PRESSED = 0x0008;
    const uint SHIFT_PRESSED = 0x0010;
    const ushort VK_CONTROL = 0x11;
    const ushort VK_V = 0x56;

    // CharSet.Unicode is load bearing (see tests/injector.cs): without it the
    // marshaller narrows UnicodeChar through the console code page and every
    // CJK character arrives as '?'.
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct KEY_EVENT_RECORD
    {
        public int bKeyDown;
        public ushort wRepeatCount;
        public ushort wVirtualKeyCode;
        public ushort wVirtualScanCode;
        public char UnicodeChar;
        public uint dwControlKeyState;
    }

    [StructLayout(LayoutKind.Explicit, CharSet = CharSet.Unicode)]
    struct INPUT_RECORD
    {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public KEY_EVENT_RECORD KeyEvent;
    }

    static INPUT_RECORD Key(bool down, ushort vk, char ch, uint ctrl)
    {
        var r = new INPUT_RECORD();
        r.EventType = KEY_EVENT;
        r.KeyEvent.bKeyDown = down ? 1 : 0;
        r.KeyEvent.wRepeatCount = 1;
        r.KeyEvent.wVirtualKeyCode = vk;
        r.KeyEvent.wVirtualScanCode = vk == 0 ? (ushort)0 : (ushort)MapVirtualKeyW(vk, 0);
        r.KeyEvent.UnicodeChar = ch;
        r.KeyEvent.dwControlKeyState = ctrl;
        return r;
    }

    static void Spin(int ms)
    {
        var sw = System.Diagnostics.Stopwatch.StartNew();
        while (sw.Elapsed.TotalMilliseconds < ms) { Thread.SpinWait(200); }
    }

    static bool Write(IntPtr h, List<INPUT_RECORD> recs, List<string> log, string what)
    {
        uint written;
        var arr = recs.ToArray();
        bool ok = WriteConsoleInput(h, arr, (uint)arr.Length, out written);
        int err = ok ? 0 : Marshal.GetLastWin32Error();
        log.Add(string.Format("  {0}: records={1} ok={2} written={3} err={4}", what, arr.Length, ok, written, err));
        return ok && written == arr.Length;
    }

    static int Main(string[] args)
    {
        var log = new List<string>();
        string logFile = Path.Combine(Path.GetTempPath(), "psmux_paste_host_inject.log");
        if (args.Length < 4)
        {
            File.WriteAllText(logFile, "Usage: paste_host_injector.exe <pid> host|plain <delay_ms> <text>\n");
            return 99;
        }
        uint pid;
        if (!uint.TryParse(args[0], out pid)) { File.WriteAllText(logFile, "bad pid"); return 98; }
        string mode = args[1];
        int delay;
        if (!int.TryParse(args[2], out delay)) { File.WriteAllText(logFile, "bad delay"); return 97; }
        string text = string.Join(" ", args, 3, args.Length - 3);
        log.Add("PID=" + pid + " mode=" + mode + " delay=" + delay + " chars=" + text.Length);

        FreeConsole();
        if (!AttachConsole(pid))
        {
            log.Add("AttachConsole FAILED err=" + Marshal.GetLastWin32Error());
            File.WriteAllText(logFile, string.Join("\n", log));
            return 2;
        }
        IntPtr h = CreateFileW("CONIN$", 0xC0000000, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        if (h == IntPtr.Zero || h.ToInt64() == -1)
        {
            log.Add("CONIN$ open FAILED err=" + Marshal.GetLastWin32Error());
            File.WriteAllText(logFile, string.Join("\n", log));
            return 2;
        }

        bool ok = true;

        if (mode == "type")
        {
            // Not a paste at all: a hand typing, one character at a time, with
            // a human gap (PSMUX_INJECT_GAP_MS, default 120) and no Ctrl and no
            // Ctrl+V anywhere.  This is the control case for the paste head
            // hold: every one of these characters must still be forwarded as
            // typing the instant it arrives.
            int tgap = 120;
            string tgapEnv = Environment.GetEnvironmentVariable("PSMUX_INJECT_GAP_MS");
            if (!string.IsNullOrEmpty(tgapEnv)) int.TryParse(tgapEnv, out tgap);
            foreach (char c in text)
            {
                short scan = VkKeyScanW(c);
                ushort vk = 0; uint mods = 0;
                if (scan != -1)
                {
                    vk = (ushort)(scan & 0xFF);
                    if ((scan & 0x100) != 0) mods |= SHIFT_PRESSED;
                }
                ok &= Write(h, new List<INPUT_RECORD> { Key(true, vk, c, mods) }, log, "type down");
                Thread.Sleep(40);
                ok &= Write(h, new List<INPUT_RECORD> { Key(false, vk, c, mods) }, log, "type up");
                Thread.Sleep(tgap);
            }
            File.WriteAllText(logFile, string.Join("\n", log));
            return ok ? 0 : 3;
        }

        // The user holds Ctrl: the host forwards the modifier itself.
        ok &= Write(h, new List<INPUT_RECORD> { Key(true, VK_CONTROL, '\0', LEFT_CTRL_PRESSED) }, log, "ctrl down");

        if (mode == "release")
        {
            // The route the client's clipboard read back is for: the host's
            // Ctrl+V binding swallows the press and pastes where the console
            // input buffer never sees it, so the V release is all that arrives.
            Thread.Sleep(delay);
            ok &= Write(h, new List<INPUT_RECORD> { Key(false, VK_V, '\x16', LEFT_CTRL_PRESSED) }, log, "ctrl+v release only, no chars");
        }
        else if (mode == "plain")
        {
            // A host that does not paste on Ctrl+V: the client sees the press
            // and the release and reads the clipboard itself.
            ok &= Write(h, new List<INPUT_RECORD> { Key(true, VK_V, '\x16', LEFT_CTRL_PRESSED) }, log, "ctrl+v press");
            Thread.Sleep(delay);
            ok &= Write(h, new List<INPUT_RECORD> { Key(false, VK_V, '\x16', LEFT_CTRL_PRESSED) }, log, "ctrl+v release");
        }
        else if (mode == "drip" || mode == "dripv")
        {
            // The 19045 shape: the host does NOT hand the console input buffer
            // the clipboard in one write.  Each character lands on its own,
            // milliseconds apart, so a client that drains the queue between
            // them sees a batch of one.
            int gap = 2;
            string gapEnv = Environment.GetEnvironmentVariable("PSMUX_INJECT_GAP_MS");
            if (!string.IsNullOrEmpty(gapEnv)) int.TryParse(gapEnv, out gap);
            if (mode == "dripv")
            {
                ok &= Write(h, new List<INPUT_RECORD> { Key(true, VK_V, '\x16', LEFT_CTRL_PRESSED) }, log, "ctrl+v press");
            }
            int i = 0;
            foreach (char c in text)
            {
                short scan = VkKeyScanW(c);
                ushort vk = 0; uint mods = 0;
                if (scan != -1)
                {
                    vk = (ushort)(scan & 0xFF);
                    if ((scan & 0x100) != 0) mods |= SHIFT_PRESSED;
                }
                var one = new List<INPUT_RECORD> { Key(true, vk, c, mods), Key(false, vk, c, mods) };
                uint written;
                var arr = one.ToArray();
                bool w = WriteConsoleInput(h, arr, (uint)arr.Length, out written);
                if (!w) { ok = false; log.Add("  drip char " + i + " FAILED err=" + Marshal.GetLastWin32Error()); }
                i++;
                // Thread.Sleep(2) is not 2 ms: without timeBeginPeriod the
                // scheduler rounds it up to the 15.6 ms tick, which drips far
                // slower than any real host and makes every character its own
                // batch.  Spin on the performance counter instead so the gap
                // is the gap that was asked for.
                if (gap > 0) Spin(gap);
            }
            log.Add(string.Format("  dripped characters: {0} chars, gap={1}ms", i, gap));
            Thread.Sleep(delay);
            ok &= Write(h, new List<INPUT_RECORD> { Key(false, VK_V, '\x16', LEFT_CTRL_PRESSED) }, log, "ctrl+v release only");
        }
        else
        {
            // Windows Terminal: the press is consumed by its paste binding,
            // the clipboard text is injected as one burst of character
            // events (Ctrl is not held for those, the host synthesises them),
            // then the V key release is forwarded.  `hostv` is the same with
            // the press delivered first (a host that passes the press through
            // and then pastes as key records, issue #858).
            if (mode == "hostv")
            {
                ok &= Write(h, new List<INPUT_RECORD> { Key(true, VK_V, '\x16', LEFT_CTRL_PRESSED) }, log, "ctrl+v press");
            }
            var chars = new List<INPUT_RECORD>();
            foreach (char c in text)
            {
                short scan = VkKeyScanW(c);
                ushort vk = 0; uint mods = 0;
                if (scan != -1)
                {
                    vk = (ushort)(scan & 0xFF);
                    if ((scan & 0x100) != 0) mods |= SHIFT_PRESSED;
                }
                chars.Add(Key(true, vk, c, mods));
                chars.Add(Key(false, vk, c, mods));
            }
            ok &= Write(h, chars, log, "pasted characters");
            Thread.Sleep(delay);
            ok &= Write(h, new List<INPUT_RECORD> { Key(false, VK_V, '\x16', LEFT_CTRL_PRESSED) }, log, "ctrl+v release only");
        }
        ok &= Write(h, new List<INPUT_RECORD> { Key(false, VK_CONTROL, '\0', 0) }, log, "ctrl up");

        File.WriteAllText(logFile, string.Join("\n", log));
        return ok ? 0 : 3;
    }
}
