// Native-mouse child: enables mouse input the Win32 way (ENABLE_MOUSE_INPUT +
// ReadConsoleInput) and NEVER emits a DECSET mouse sequence. This is the model a
// console app using the Windows console API directly uses (the issue #285 case).
//
// It exists to check that psmux still delivers mouse events to such an app after
// the 3.3.7 click/motion gating changes (d134533, 57813d1), which key off explicit
// DECSET mouse modes.
//
// Build: csc /nologo /out:native_mouse_child.exe native_mouse_child.cs
// Log:   %TEMP%\psmux_native_mouse.txt
//
// Optional argument `vti`: also turn ENABLE_VIRTUAL_TERMINAL_INPUT on, the
// Bubble Tea shape (keys as VT, mouse still read as MOUSE_EVENT records).
using System;
using System.IO;
using System.Runtime.InteropServices;

class NativeMouse {
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool GetConsoleMode(IntPtr h, out uint mode);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool SetConsoleMode(IntPtr h, uint mode);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool ReadConsoleInputW(IntPtr h, [Out] INPUT_RECORD[] buf, uint len, out uint read);

    const int STD_INPUT_HANDLE = -10;
    const uint ENABLE_PROCESSED_INPUT = 0x0001;
    const uint ENABLE_LINE_INPUT = 0x0002;
    const uint ENABLE_ECHO_INPUT = 0x0004;
    const uint ENABLE_MOUSE_INPUT = 0x0010;
    const uint ENABLE_EXTENDED_FLAGS = 0x0080;
    const uint ENABLE_QUICK_EDIT_MODE = 0x0040;
    const ushort KEY_EVENT = 0x0001;
    const ushort MOUSE_EVENT = 0x0002;

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

    static int Main(string[] args) {
        bool vti = args.Length > 0 && args[0] == "vti";
        string log = Path.Combine(Environment.GetEnvironmentVariable("TEMP"), "psmux_native_mouse.txt");
        File.WriteAllText(log, "NATIVE_MOUSE START\n");

        IntPtr h = GetStdHandle(STD_INPUT_HANDLE);
        uint mode;
        if (GetConsoleMode(h, out mode)) {
            uint newMode = (mode & ~(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT | ENABLE_QUICK_EDIT_MODE))
                           | ENABLE_MOUSE_INPUT | ENABLE_EXTENDED_FLAGS
                           | (vti ? 0x0200u : 0u);
            SetConsoleMode(h, newMode);
            File.AppendAllText(log, string.Format("mode {0:X} -> {1:X}\n", mode, newMode));
        } else {
            File.AppendAllText(log, "GetConsoleMode failed\n");
        }

        Console.Out.Write("NATIVE_MOUSE_READY\r\n");
        Console.Out.Flush();

        var buf = new INPUT_RECORD[32];
        while (true) {
            uint read;
            if (!ReadConsoleInputW(h, buf, (uint)buf.Length, out read)) {
                System.Threading.Thread.Sleep(20);
                continue;
            }
            for (int i = 0; i < read; i++) {
                if (buf[i].EventType == MOUSE_EVENT) {
                    var m = buf[i].MouseEvent;
                    short wheel = (short)((m.dwButtonState >> 16) & 0xFFFF);
                    File.AppendAllText(log, string.Format(
                        "MOUSE x={0} y={1} buttons=0x{2:X} flags=0x{3:X} wheel={4}\n",
                        m.dwMousePosition.X, m.dwMousePosition.Y, m.dwButtonState, m.dwEventFlags, wheel));
                } else if (buf[i].EventType == KEY_EVENT && buf[i].KeyEvent.bKeyDown != 0) {
                    ushort c = buf[i].KeyEvent.UnicodeChar;
                    File.AppendAllText(log, string.Format("KEY 0x{0:X2}\n", c));
                    if (c == 0x1a) { File.AppendAllText(log, "NATIVE_MOUSE END\n"); return 0; }
                }
            }
        }
    }
}
