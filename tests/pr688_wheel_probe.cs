// pr688_wheel_probe.cs
//
// Writes real Windows MOUSE_EVENT wheel records into an attached psmux
// client's console input buffer, which is how a user's wheel actually reaches
// copy mode.  Issue #687 is not a scroll key defect: anything that moves the
// view off the live bottom reaches it, and the wheel is the route most people
// take.  tests/pr678_mouse_probe.cs covers press, move and release but has no
// wheel command, so this is its sibling.
//
// Usage:
//   pr688_wheel_probe <pid> up   <x> <y> <notches>
//   pr688_wheel_probe <pid> down <x> <y> <notches>
//
// Build:
//   csc /nologo /optimize /out:pr688_wheel_probe.exe pr688_wheel_probe.cs

using System;
using System.Runtime.InteropServices;

class WheelProbe
{
    const uint GENERIC_READ = 0x80000000;
    const uint GENERIC_WRITE = 0x40000000;
    const uint FILE_SHARE_READ = 0x00000001;
    const uint FILE_SHARE_WRITE = 0x00000002;
    const uint OPEN_EXISTING = 3;

    const ushort MOUSE_EVENT = 0x0002;
    const uint MOUSE_WHEELED = 0x0004;

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AttachConsole(uint dwProcessId);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool FreeConsole();

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr CreateFileW(string lpFileName, uint dwDesiredAccess,
        uint dwShareMode, IntPtr lpSecurityAttributes, uint dwCreationDisposition,
        uint dwFlagsAndAttributes, IntPtr hTemplateFile);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool WriteConsoleInput(IntPtr hConsoleInput, INPUT_RECORD[] lpBuffer,
        uint nLength, out uint lpNumberOfEventsWritten);

    [StructLayout(LayoutKind.Sequential)]
    struct COORD { public short X; public short Y; }

    [StructLayout(LayoutKind.Sequential)]
    struct MOUSE_EVENT_RECORD
    {
        public COORD dwMousePosition;
        public uint dwButtonState;
        public uint dwControlKeyState;
        public uint dwEventFlags;
    }

    [StructLayout(LayoutKind.Explicit, Size = 20)]
    struct INPUT_RECORD
    {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public MOUSE_EVENT_RECORD MouseEvent;
    }

    static INPUT_RECORD Wheel(short x, short y, short delta)
    {
        var rec = new INPUT_RECORD();
        rec.EventType = MOUSE_EVENT;
        rec.MouseEvent.dwMousePosition.X = x;
        rec.MouseEvent.dwMousePosition.Y = y;
        // The wheel delta lives in the HIGH word of dwButtonState, signed:
        // +120 is one notch away from the user, -120 one notch towards them.
        rec.MouseEvent.dwButtonState = unchecked((uint)((int)delta << 16));
        rec.MouseEvent.dwControlKeyState = 0;
        rec.MouseEvent.dwEventFlags = MOUSE_WHEELED;
        return rec;
    }

    static int Main(string[] args)
    {
        if (args.Length < 5)
        {
            Console.Error.WriteLine("Usage: pr688_wheel_probe <pid> <up|down> <x> <y> <notches>");
            return 1;
        }
        uint pid = uint.Parse(args[0]);
        string dir = args[1].ToLowerInvariant();
        short x = short.Parse(args[2]);
        short y = short.Parse(args[3]);
        int notches = int.Parse(args[4]);
        if (dir != "up" && dir != "down")
        {
            Console.Error.WriteLine("ERR bad direction " + dir);
            return 1;
        }
        short delta = (short)(dir == "up" ? 120 : -120);

        FreeConsole();
        if (!AttachConsole(pid))
        {
            Console.Error.WriteLine("ERR AttachConsole " + Marshal.GetLastWin32Error());
            return 2;
        }

        IntPtr h = CreateFileW("CONIN$", GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
        if (h == IntPtr.Zero || h == (IntPtr)(-1))
        {
            Console.Error.WriteLine("ERR CONIN$ " + Marshal.GetLastWin32Error());
            FreeConsole();
            return 3;
        }

        var batch = new INPUT_RECORD[notches];
        for (int i = 0; i < notches; i++) batch[i] = Wheel(x, y, delta);
        uint written;
        bool ok = WriteConsoleInput(h, batch, (uint)notches, out written);
        Console.WriteLine(ok ? ("OK " + written) : ("ERR write " + Marshal.GetLastWin32Error()));
        FreeConsole();
        return ok ? 0 : 5;
    }
}
