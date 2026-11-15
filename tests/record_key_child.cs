// Record-reading key logger: the Far Manager stand-in for issue #623.
//
// Far Manager reads raw INPUT_RECORDs with ReadConsoleInput and dispatches on
// wVirtualKeyCode plus dwControlKeyState.  This child configures its console
// input mode to EXACTLY the shape Far uses (measured 0x01B8: WINDOW_INPUT |
// MOUSE_INPUT | INSERT_MODE | EXTENDED_FLAGS | AUTO_POSITION, with LINE_INPUT,
// ECHO_INPUT, PROCESSED_INPUT and VIRTUAL_TERMINAL_INPUT all clear) and writes
// one line per record so a test can assert the virtual key and the control key
// state psmux actually delivered, not just the character.
//
// A Ctrl+digit or a Ctrl+function key that arrives as a bare character record
// (vk=0x00, or a plain '1' with no LEFT_CTRL_PRESSED) is the #623 symptom: Far
// cannot tell it from the unmodified key.
//
// Build: csc /nologo /platform:x64 /out:record_key_child.exe record_key_child.cs
// Log:   %PSMUX_RECORD_LOG%, else %TEMP%\psmux_record_key.txt
using System;
using System.IO;
using System.Runtime.InteropServices;

class RecordKeyChild {
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool GetConsoleMode(IntPtr h, out uint mode);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool SetConsoleMode(IntPtr h, uint mode);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool ReadConsoleInputW(IntPtr h, [Out] INPUT_RECORD[] buf, uint len, out uint read);

    const int STD_INPUT_HANDLE = -10;
    const ushort KEY_EVENT = 0x0001;
    const ushort MOUSE_EVENT = 0x0002;

    // Far's measured input mode, from the #623 investigation.
    const uint FAR_MODE = 0x01B8;

    // PSMUX_RECORD_MODE (hex) replaces it, so a test can take another reader's
    // mode, such as PSReadLine's 0x01E4 (issue #1048).
    static uint ChosenMode() {
        string m = Environment.GetEnvironmentVariable("PSMUX_RECORD_MODE");
        uint v;
        if (!string.IsNullOrEmpty(m) && uint.TryParse(m, System.Globalization.NumberStyles.HexNumber, null, out v)) return v;
        return FAR_MODE;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct COORD { public short X; public short Y; }

    [StructLayout(LayoutKind.Sequential)]
    struct MOUSE_EVENT_RECORD {
        public COORD dwMousePosition;
        public uint dwButtonState;
        public uint dwControlKeyState;
        public uint dwEventFlags;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct KEY_EVENT_RECORD {
        public int bKeyDown;
        public ushort wRepeatCount;
        public ushort wVirtualKeyCode;
        public ushort wVirtualScanCode;
        public ushort UnicodeChar;
        public uint dwControlKeyState;
    }

    [StructLayout(LayoutKind.Explicit, Size = 20)]
    struct INPUT_RECORD {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public MOUSE_EVENT_RECORD MouseEvent;
        [FieldOffset(4)] public KEY_EVENT_RECORD KeyEvent;
    }

    static int Main() {
        string log = Environment.GetEnvironmentVariable("PSMUX_RECORD_LOG");
        if (string.IsNullOrEmpty(log)) {
            log = Path.Combine(Environment.GetEnvironmentVariable("TEMP"), "psmux_record_key.txt");
        }
        File.WriteAllText(log, "RECORD_KEY START\n");

        IntPtr h = GetStdHandle(STD_INPUT_HANDLE);
        uint mode;
        if (GetConsoleMode(h, out mode)) {
            SetConsoleMode(h, ChosenMode());
            uint after;
            GetConsoleMode(h, out after);
            File.AppendAllText(log, string.Format("mode 0x{0:X4} -> 0x{1:X4}\n", mode, after));
        } else {
            File.AppendAllText(log, "GetConsoleMode failed\n");
        }

        Console.Out.Write("RECORD_KEY_READY\r\n");
        Console.Out.Flush();

        var buf = new INPUT_RECORD[64];
        while (true) {
            uint read;
            if (!ReadConsoleInputW(h, buf, (uint)buf.Length, out read)) {
                System.Threading.Thread.Sleep(20);
                continue;
            }
            for (int i = 0; i < read; i++) {
                if (buf[i].EventType == KEY_EVENT) {
                    var k = buf[i].KeyEvent;
                    File.AppendAllText(log, string.Format(
                        "KEY {0} vk=0x{1:X2} sc=0x{2:X2} ch=0x{3:X4} ctrl=0x{4:X4}\n",
                        k.bKeyDown != 0 ? "down" : "up  ",
                        k.wVirtualKeyCode, k.wVirtualScanCode, k.UnicodeChar, k.dwControlKeyState));
                    // Ctrl+Z ends the logger cleanly.
                    if (k.bKeyDown != 0 && k.UnicodeChar == 0x1A) {
                        File.AppendAllText(log, "RECORD_KEY END\n");
                        return 0;
                    }
                } else if (buf[i].EventType == MOUSE_EVENT) {
                    var m = buf[i].MouseEvent;
                    File.AppendAllText(log, string.Format(
                        "MOUSE x={0} y={1} buttons=0x{2:X} flags=0x{3:X}\n",
                        m.dwMousePosition.X, m.dwMousePosition.Y, m.dwButtonState, m.dwEventFlags));
                }
            }
        }
    }
}
