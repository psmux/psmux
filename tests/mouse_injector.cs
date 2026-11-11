using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;

// Injects mouse wheel events into a console process via WriteConsoleInput.
// Usage: mouse_injector.exe <pid> <up|down|left|right> [count] [x] [y]
// left/right send MOUSE_HWHEELED records (a tilt wheel or trackpad sideways
// swipe): a positive high word is right, a negative one left.
class MouseInjector
{
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool FreeConsole();

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AttachConsole(uint pid);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateFileW(string name, uint access, uint share,
        IntPtr sec, uint disp, uint flags, IntPtr tmpl);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool WriteConsoleInput(IntPtr h, INPUT_RECORD[] buf, uint len, out uint written);

    const ushort MOUSE_EVENT = 0x0002;
    const uint MOUSE_WHEELED = 0x0004;
    const uint MOUSE_HWHEELED = 0x0008;
    const uint GENERIC_WRITE = 0x40000000;
    const uint GENERIC_READ = 0x80000000;
    const uint FILE_SHARE_READ = 0x00000001;
    const uint FILE_SHARE_WRITE = 0x00000002;
    const uint OPEN_EXISTING = 3;

    [StructLayout(LayoutKind.Sequential)]
    struct COORD
    {
        public short X;
        public short Y;
    }

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

    static int Main(string[] args)
    {
        string logPath = Path.Combine(Path.GetTempPath(), "psmux_mouse_inject.log");

        if (args.Length < 2)
        {
            Console.Error.WriteLine("Usage: mouse_injector.exe <pid> <up|down|left|right> [count] [x] [y]");
            return 1;
        }

        uint pid = uint.Parse(args[0]);
        string direction = args[1].ToLower();
        int count = args.Length > 2 ? int.Parse(args[2]) : 3;
        short x = args.Length > 3 ? short.Parse(args[3]) : (short)40;
        short y = args.Length > 4 ? short.Parse(args[4]) : (short)15;

        // Scroll delta: positive = up, negative = down
        // The high word of dwButtonState holds the delta (120 = one notch)
        // For left/right: positive = right, negative = left.
        bool horizontal = direction == "left" || direction == "right";
        int delta = (direction == "up" || direction == "right") ? 120 : -120;

        var log = new System.Text.StringBuilder();
        log.AppendLine(string.Format("MouseInjector: pid={0} dir={1} count={2} x={3} y={4} delta={5}", pid, direction, count, x, y, delta));

        FreeConsole();
        if (!AttachConsole(pid))
        {
            int err = Marshal.GetLastWin32Error();
            log.AppendLine(string.Format("AttachConsole FAILED: error={0}", err));
            File.WriteAllText(logPath, log.ToString());
            return 2;
        }
        log.AppendLine("AttachConsole OK");

        IntPtr hInput = CreateFileW("CONIN$",
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);

        if (hInput == IntPtr.Zero || hInput == (IntPtr)(-1))
        {
            int err = Marshal.GetLastWin32Error();
            log.AppendLine(string.Format("CreateFile CONIN$ FAILED: error={0}", err));
            File.WriteAllText(logPath, log.ToString());
            FreeConsole();
            return 3;
        }
        log.AppendLine(string.Format("CONIN$ handle={0}", hInput));

        for (int i = 0; i < count; i++)
        {
            var rec = new INPUT_RECORD();
            rec.EventType = MOUSE_EVENT;
            rec.MouseEvent.dwMousePosition.X = x;
            rec.MouseEvent.dwMousePosition.Y = y;
            // High word = scroll delta, encoded as unsigned
            rec.MouseEvent.dwButtonState = (uint)(delta << 16);
            rec.MouseEvent.dwControlKeyState = 0;
            rec.MouseEvent.dwEventFlags = horizontal ? MOUSE_HWHEELED : MOUSE_WHEELED;

            uint written;
            bool ok = WriteConsoleInput(hInput, new INPUT_RECORD[] { rec }, 1, out written);
            int err = Marshal.GetLastWin32Error();
            log.AppendLine(string.Format("  scroll[{0}] ok={1} written={2} err={3}", i, ok, written, err));
            Thread.Sleep(50);
        }

        FreeConsole();
        File.WriteAllText(logPath, log.ToString());
        log.AppendLine("Done");
        return 0;
    }
}
