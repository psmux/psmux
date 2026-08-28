using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;

class Injector
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

    // Maps a character to the virtual key plus modifiers that produce it on the
    // current keyboard layout. Low byte is the VK, high byte is the modifier
    // mask (1 Shift, 2 Ctrl, 4 Alt); -1 means the character has no key.
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern short VkKeyScanW(char ch);

    const ushort KEY_EVENT = 1;
    const uint LEFT_CTRL_PRESSED = 0x0008;
    const uint SHIFT_PRESSED = 0x0010;
    const uint LEFT_ALT_PRESSED = 0x0002;

    // CharSet.Unicode is LOAD BEARING on both of these structs, and its absence
    // was a silent injector bug found while testing issue #616.
    //
    // StructLayout defaults to CharSet.Ansi, which makes the marshaller convert
    // `char UnicodeChar` to a SINGLE ANSI byte in the console's input code page
    // (437 here). ASCII survived that by accident: the ANSI byte lands at the
    // low half of the WCHAR field and the padding byte above it is zero, so the
    // native side reads the right character. Everything outside the code page
    // was converted to '?' before WriteConsoleInputW ever saw it, so {U:044B}
    // injected a literal question mark and every non-ASCII injection test was
    // passing on a character it never actually sent. Verified with a
    // ReadConsoleInputW probe: 'ы' in, U+003F out.
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

    static INPUT_RECORD MakeKey(bool down, ushort vk, char ch, uint ctrl)
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

    static bool SendKey(IntPtr h, ushort vk, char ch, uint ctrl, List<string> log)
    {
        var recs = new INPUT_RECORD[] {
            MakeKey(true, vk, ch, ctrl),
            MakeKey(false, vk, ch, 0)
        };
        uint written;
        bool ok = WriteConsoleInput(h, recs, 2, out written);
        int err = ok ? 0 : Marshal.GetLastWin32Error();
        log.Add(string.Format("  '{0}' vk=0x{1:X2} ok={2} w={3} e={4}",
            ch == '\0' ? "NUL" : ch.ToString(), vk, ok, written, err));
        return ok && written == 2;
    }

    static bool SendCtrlCombo(IntPtr h, char letter, List<string> log)
    {
        ushort vk = (ushort)char.ToUpper(letter);
        char ctrlChar = (char)(char.ToUpper(letter) - 'A' + 1);
        var recs = new INPUT_RECORD[] {
            MakeKey(true,  0x11, '\0',     LEFT_CTRL_PRESSED),
            MakeKey(true,  vk,   ctrlChar, LEFT_CTRL_PRESSED),
            MakeKey(false, vk,   ctrlChar, LEFT_CTRL_PRESSED),
            MakeKey(false, 0x11, '\0',     0)
        };
        uint written;
        bool ok = WriteConsoleInput(h, recs, 4, out written);
        int err = ok ? 0 : Marshal.GetLastWin32Error();
        log.Add(string.Format("  Ctrl+{0} ok={1} w={2} e={3}", letter, ok, written, err));
        return ok && written == 4;
    }

    static int Main(string[] args)
    {
        var log = new List<string>();
        string logFile = Path.Combine(Path.GetTempPath(), "psmux_inject.log");

        if (args.Length < 2)
        {
            File.WriteAllText(logFile, "Usage: injector.exe <pid> <keys>\n" +
                "Keys: chars, ^x=Ctrl+x, {ALT:x}, {ENTER}, {ESC}, {LBRACE}, {RBRACE}, {SLEEP:ms}");
            return 99;
        }

        uint pid;
        if (!uint.TryParse(args[0], out pid))
        {
            File.WriteAllText(logFile, "Invalid PID: " + args[0]);
            return 98;
        }

        string keys = string.Join(" ", args, 1, args.Length - 1);
        log.Add("PID=" + pid + " Keys=" + keys);

        // Detach from our console, attach to target
        FreeConsole();
        if (!AttachConsole(pid))
        {
            log.Add("AttachConsole FAILED err=" + Marshal.GetLastWin32Error());
            File.WriteAllText(logFile, string.Join("\n", log));
            return 2;
        }

        // Open the console input buffer directly
        IntPtr handle = CreateFileW("CONIN$", 0xC0000000u, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        if (handle == new IntPtr(-1))
        {
            log.Add("CreateFile(CONIN$) FAILED err=" + Marshal.GetLastWin32Error());
            FreeConsole();
            File.WriteAllText(logFile, string.Join("\n", log));
            return 3;
        }
        log.Add("Handle=" + handle);

        int injected = 0;
        int i = 0;
        while (i < keys.Length)
        {
            if (keys[i] == '^' && i + 1 < keys.Length)
            {
                if (SendCtrlCombo(handle, keys[i + 1], log)) injected++;
                i += 2;
                Thread.Sleep(50);
            }
            else if (keys[i] == '{')
            {
                int end = keys.IndexOf('}', i);
                if (end > i)
                {
                    string token = keys.Substring(i + 1, end - i - 1);
                    if (token == "ENTER")
                    {
                        if (SendKey(handle, 0x0D, '\r', 0, log)) injected++;
                    }
                    else if (token == "ESC" || token == "ESCAPE")
                    {
                        if (SendKey(handle, 0x1B, (char)0x1B, 0, log)) injected++;
                    }
                    else if (token == "UP")
                    {
                        if (SendKey(handle, 0x26, '\0', 0, log)) injected++;
                    }
                    else if (token == "DOWN")
                    {
                        if (SendKey(handle, 0x28, '\0', 0, log)) injected++;
                    }
                    else if (token == "LEFT")
                    {
                        if (SendKey(handle, 0x25, '\0', 0, log)) injected++;
                    }
                    else if (token == "RIGHT")
                    {
                        if (SendKey(handle, 0x27, '\0', 0, log)) injected++;
                    }
                    else if (token == "HOME")
                    {
                        if (SendKey(handle, 0x24, '\0', 0, log)) injected++;
                    }
                    else if (token == "END")
                    {
                        if (SendKey(handle, 0x23, '\0', 0, log)) injected++;
                    }
                    else if (token == "PGUP" || token == "PAGEUP")
                    {
                        if (SendKey(handle, 0x21, '\0', 0, log)) injected++;
                    }
                    else if (token == "PGDN" || token == "PAGEDOWN")
                    {
                        if (SendKey(handle, 0x22, '\0', 0, log)) injected++;
                    }
                    else if (token.StartsWith("ALT:"))
                    {
                        // {ALT:x} — Alt+x, e.g. copy mode jump-to-mark
                        char ac = token[4];
                        ushort avk = (ac >= 'a' && ac <= 'z') ? (ushort)(0x41 + ac - 'a')
                                   : (ac >= 'A' && ac <= 'Z') ? (ushort)(0x41 + ac - 'A')
                                   : (ushort)ac;
                        var arecs = new INPUT_RECORD[] {
                            MakeKey(true,  0x12, '\0', LEFT_ALT_PRESSED),
                            MakeKey(true,  avk,  ac,   LEFT_ALT_PRESSED),
                            MakeKey(false, avk,  ac,   LEFT_ALT_PRESSED),
                            MakeKey(false, 0x12, '\0', 0)
                        };
                        uint aw; bool aok = WriteConsoleInput(handle, arecs, 4, out aw);
                        log.Add(string.Format("  ALT+{0} vk=0x{1:X2} ok={2} w={3}", ac, avk, aok, aw));
                        if (aok) injected++;
                    }
                    else if (token == "LBRACE")
                    {
                        // '{' cannot be written literally: it opens a token.
                        // Shift + the '[' key is how a real keyboard makes it.
                        if (SendKey(handle, 0xDB, '{', SHIFT_PRESSED, log)) injected++;
                    }
                    else if (token == "RBRACE")
                    {
                        if (SendKey(handle, 0xDD, '}', SHIFT_PRESSED, log)) injected++;
                    }
                    else if (token.StartsWith("SLEEP:"))
                    {
                        int ms = int.Parse(token.Substring(6));
                        Thread.Sleep(ms);
                        log.Add("  SLEEP " + ms + "ms");
                    }
                    else if (token.StartsWith("U:"))
                    {
                        // {U:XXXX[,XXXX,...]} — inject one or more Unicode codepoints
                        // (hex BMP) as KEY_EVENT records with vk=0 and UnicodeChar set.
                        // crossterm's ReadConsoleInputW path delivers these as
                        // KeyCode::Char(c) — the only reliable way to inject e.g.
                        // CJK characters into the console input buffer.
                        var hexes = token.Substring(2).Split(',');
                        foreach (var hex in hexes)
                        {
                            char uc = (char)Convert.ToUInt16(hex, 16);
                            if (SendKey(handle, 0, uc, 0, log)) injected++;
                            Thread.Sleep(20);
                        }
                    }
                    else if (token.StartsWith("RAW:"))
                    {
                        // RAW:vkHex:charHex:ctrlHex  e.g. RAW:BF:1F:0008
                        // Sends Ctrl-down + key-down + key-up + Ctrl-up
                        // with the EXACT VK / UnicodeChar / dwControlKeyState
                        // the caller specifies. This lets tests inject
                        // Ctrl+/ etc. with Windows-accurate fields.
                        var parts = token.Substring(4).Split(':');
                        if (parts.Length == 3)
                        {
                            ushort rvk = Convert.ToUInt16(parts[0], 16);
                            char rch = (char)Convert.ToUInt16(parts[1], 16);
                            uint rctrl = Convert.ToUInt32(parts[2], 16);
                            var recs = new INPUT_RECORD[] {
                                MakeKey(true,  0x11, '\0', LEFT_CTRL_PRESSED),
                                MakeKey(true,  rvk,  rch,  rctrl),
                                MakeKey(false, rvk,  rch,  rctrl),
                                MakeKey(false, 0x11, '\0', 0)
                            };
                            uint w; bool ok = WriteConsoleInput(handle, recs, 4, out w);
                            int e = ok ? 0 : Marshal.GetLastWin32Error();
                            log.Add(string.Format("  RAW vk=0x{0:X2} ch=0x{1:X2} ctrl=0x{2:X4} ok={3} w={4} e={5}",
                                rvk, (int)rch, rctrl, ok, w, e));
                            if (ok) injected++;
                        }
                    }
                    i = end + 1;
                    Thread.Sleep(30);
                }
                else { i++; }
            }
            else
            {
                char c = keys[i];
                ushort vk;
                uint ctrl = 0;

                if (c >= 'a' && c <= 'z') vk = (ushort)(0x41 + c - 'a');
                else if (c >= 'A' && c <= 'Z') { vk = (ushort)(0x41 + c - 'A'); ctrl = SHIFT_PRESSED; }
                else if (c >= '0' && c <= '9') vk = (ushort)(0x30 + c - '0');
                else if (c == ' ') vk = 0x20;
                else if (c == '-') vk = 0xBD;
                else if (c == '_') { vk = 0xBD; ctrl = SHIFT_PRESSED; }
                else if (c == ':') { vk = 0xBA; ctrl = SHIFT_PRESSED; }
                else if (c == '.') vk = 0xBE;
                else if (c == ',') vk = 0xBC;
                else if (c == '/') vk = 0xBF;
                else if (c == '\\') vk = 0xDC;
                else if (c == '[') vk = 0xDB;
                else if (c == ']') vk = 0xDD;
                else if (c == '"') { vk = 0xDE; ctrl = SHIFT_PRESSED; }
                else if (c == '\'') vk = 0xDE;
                else if (c == ';') vk = 0xBA;
                else if (c == '=') vk = 0xBB;
                else if (c == '(') { vk = 0x39; ctrl = SHIFT_PRESSED; }
                else if (c == ')') { vk = 0x30; ctrl = SHIFT_PRESSED; }
                else if (c == '%') { vk = 0x35; ctrl = SHIFT_PRESSED; }
                else if (c == '#') { vk = 0x33; ctrl = SHIFT_PRESSED; }
                else if (c == '@') { vk = 0x32; ctrl = SHIFT_PRESSED; }
                else if (c == '!') { vk = 0x31; ctrl = SHIFT_PRESSED; }
                else if (c == '&') { vk = 0x37; ctrl = SHIFT_PRESSED; }
                else if (c == '*') { vk = 0x38; ctrl = SHIFT_PRESSED; }
                else if (c == '+') { vk = 0xBB; ctrl = SHIFT_PRESSED; }
                else if (c == '}') { vk = 0xDD; ctrl = SHIFT_PRESSED; }
                else if (c == '{') { vk = 0xDB; ctrl = SHIFT_PRESSED; }
                else
                {
                    // Anything not in the table above: ask Windows for the key
                    // that produces this character on the CURRENT layout, rather
                    // than assuming the character code IS the virtual key.
                    //
                    // The old fallback was vk = (ushort)c, which is wrong for
                    // every punctuation character it was reached for. '|' is
                    // 0x7C, and 0x7C is VK_F13, so typing a pipe pressed F13:
                    // the character never arrived, and the stray function key was
                    // free to trigger a binding. That silently turned
                    // "set-option -g window-status-separator |" into the same
                    // command with an EMPTY value, and the commands after it
                    // stopped taking effect.
                    //
                    // VkKeyScanW returns the virtual key in the low byte and the
                    // required modifiers in the high byte (1 = Shift, 2 = Ctrl,
                    // 4 = Alt), which also makes shifted punctuation correct
                    // without hand-maintaining a table.
                    short scan = VkKeyScanW(c);
                    if (scan == -1)
                    {
                        log.Add(string.Format("  SKIP '{0}' (U+{1:X4}): no key on this layout", c, (int)c));
                        i++;
                        continue;
                    }
                    vk = (ushort)(scan & 0xFF);
                    int mods = (scan >> 8) & 0xFF;
                    if ((mods & 1) != 0) ctrl |= SHIFT_PRESSED;
                    if ((mods & 2) != 0) ctrl |= LEFT_CTRL_PRESSED;
                    if ((mods & 4) != 0) ctrl |= LEFT_ALT_PRESSED;
                }

                if (SendKey(handle, vk, c, ctrl, log)) injected++;
                i++;
                Thread.Sleep(30);
            }
        }

        log.Add("Injected=" + injected);
        FreeConsole();
        File.WriteAllText(logFile, string.Join("\n", log));
        return 0;
    }
}
