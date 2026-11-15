using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;

// Injects a left-button CLICK (press then release) at a console cell via
// WriteConsoleInput MOUSE_EVENT records.
// Usage: click_injector.exe <pid> <x> <y> [holdMs] [buttonMask]
// buttonMask is dwButtonState: 1 left (the default), 2 right, 4 middle.
class ClickInjector
{
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr sec, uint disp, uint flags, IntPtr tmpl);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool WriteConsoleInput(IntPtr h, INPUT_RECORD[] buf, uint len, out uint written);

    const ushort MOUSE_EVENT = 0x0002;
    const uint MOUSE_MOVED = 0x0001;
    const uint FROM_LEFT_1ST_BUTTON_PRESSED = 0x0001;
    const uint GENERIC_WRITE = 0x40000000;
    const uint GENERIC_READ = 0x80000000;
    const uint FILE_SHARE_READ = 0x00000001;
    const uint FILE_SHARE_WRITE = 0x00000002;
    const uint OPEN_EXISTING = 3;

    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X; public short Y; }
    [StructLayout(LayoutKind.Sequential)] struct MOUSE_EVENT_RECORD {
        public COORD dwMousePosition; public uint dwButtonState;
        public uint dwControlKeyState; public uint dwEventFlags; }
    [StructLayout(LayoutKind.Explicit, Size = 20)] struct INPUT_RECORD {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public MOUSE_EVENT_RECORD MouseEvent; }

    static INPUT_RECORD Rec(short x, short y, uint buttons, uint flags)
    {
        var r = new INPUT_RECORD();
        r.EventType = MOUSE_EVENT;
        r.MouseEvent.dwMousePosition.X = x;
        r.MouseEvent.dwMousePosition.Y = y;
        r.MouseEvent.dwButtonState = buttons;
        r.MouseEvent.dwControlKeyState = 0;
        r.MouseEvent.dwEventFlags = flags;
        return r;
    }

    static int Main(string[] args)
    {
        string logPath = Path.Combine(Path.GetTempPath(), "psmux_click_inject.log");
        if (args.Length < 3) { Console.Error.WriteLine("Usage: click_injector.exe <pid> <x> <y> [holdMs] [buttonMask]"); return 1; }
        uint pid = uint.Parse(args[0]);
        short x = short.Parse(args[1]);
        short y = short.Parse(args[2]);
        int hold = args.Length > 3 ? int.Parse(args[3]) : 80; uint button = args.Length > 4 ? uint.Parse(args[4]) : FROM_LEFT_1ST_BUTTON_PRESSED;

        var log = new System.Text.StringBuilder();
        log.AppendLine(string.Format("ClickInjector pid={0} x={1} y={2}", pid, x, y));
        FreeConsole();
        if (!AttachConsole(pid)) { log.AppendLine("AttachConsole FAILED " + Marshal.GetLastWin32Error()); File.WriteAllText(logPath, log.ToString()); return 2; }
        IntPtr h = CreateFileW("CONIN$", GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
        if (h == IntPtr.Zero || h == (IntPtr)(-1)) { log.AppendLine("CONIN$ FAILED " + Marshal.GetLastWin32Error()); File.WriteAllText(logPath, log.ToString()); FreeConsole(); return 3; }
        uint w;
        bool ok1 = WriteConsoleInput(h, new INPUT_RECORD[] { Rec(x, y, 0, MOUSE_MOVED) }, 1, out w);
        Thread.Sleep(30);
        bool ok2 = WriteConsoleInput(h, new INPUT_RECORD[] { Rec(x, y, button, 0) }, 1, out w);
        Thread.Sleep(hold);
        bool ok3 = WriteConsoleInput(h, new INPUT_RECORD[] { Rec(x, y, 0, 0) }, 1, out w);
        log.AppendLine(string.Format("move={0} press={1} release={2}", ok1, ok2, ok3));
        FreeConsole();
        File.WriteAllText(logPath, log.ToString());
        return 0;
    }
}
