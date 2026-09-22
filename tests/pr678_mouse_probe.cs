using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

// pr678_mouse_probe.exe
//
// Two jobs, both against a REAL attached psmux client console:
//   1. inject MOUSE_EVENT records (press / held move / release), including a
//      "slip + release in one WriteConsoleInput batch" mode, so the client's
//      own drag handling is what runs;
//   2. read back what the client actually PAINTED with
//      ReadConsoleOutputAttribute / ReadConsoleOutputCharacterW, so the
//      highlight can be compared with the paste buffer instead of guessed at.
//
// Usage:
//   pr678_mouse_probe <pid> press  <x> <y>
//   pr678_mouse_probe <pid> move   <x> <y>
//   pr678_mouse_probe <pid> up     <x> <y>
//   pr678_mouse_probe <pid> slipup <sx> <sy> <ux> <uy>   (both in ONE batch)
//   pr678_mouse_probe <pid> row    <y>
//   pr678_mouse_probe <pid> rows   <y0> <y1>
//   pr678_mouse_probe <pid> size
class Pr678MouseProbe
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

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool ReadConsoleOutputAttribute(IntPtr h, [Out] ushort[] attrs,
        uint len, COORD coord, out uint read);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool ReadConsoleOutputCharacterW(IntPtr h, [Out] char[] buf,
        uint len, COORD coord, out uint read);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetConsoleScreenBufferInfo(IntPtr h, out CONSOLE_SCREEN_BUFFER_INFO info);

    const ushort MOUSE_EVENT = 0x0002;
    const uint MOUSE_MOVED = 0x0001;
    const uint LEFT_BUTTON = 0x0001;
    const uint GENERIC_WRITE = 0x40000000;
    const uint GENERIC_READ = 0x80000000;
    const uint FILE_SHARE_READ = 0x00000001;
    const uint FILE_SHARE_WRITE = 0x00000002;
    const uint OPEN_EXISTING = 3;

    [StructLayout(LayoutKind.Sequential)]
    struct COORD { public short X; public short Y; }

    [StructLayout(LayoutKind.Sequential)]
    struct SMALL_RECT { public short Left, Top, Right, Bottom; }

    [StructLayout(LayoutKind.Sequential)]
    struct CONSOLE_SCREEN_BUFFER_INFO
    {
        public COORD dwSize;
        public COORD dwCursorPosition;
        public ushort wAttributes;
        public SMALL_RECT srWindow;
        public COORD dwMaximumWindowSize;
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

    static INPUT_RECORD Rec(short x, short y, uint buttons, uint flags)
    {
        var rec = new INPUT_RECORD();
        rec.EventType = MOUSE_EVENT;
        rec.MouseEvent.dwMousePosition.X = x;
        rec.MouseEvent.dwMousePosition.Y = y;
        rec.MouseEvent.dwButtonState = buttons;
        rec.MouseEvent.dwControlKeyState = 0;
        rec.MouseEvent.dwEventFlags = flags;
        return rec;
    }

    static int Main(string[] args)
    {
        if (args.Length < 2)
        {
            Console.Error.WriteLine("Usage: pr678_mouse_probe <pid> <press|move|up|slipup|row|rows|size> ...");
            return 1;
        }
        uint pid = uint.Parse(args[0]);
        string cmd = args[1].ToLowerInvariant();

        FreeConsole();
        if (!AttachConsole(pid))
        {
            Console.Error.WriteLine("ERR AttachConsole " + Marshal.GetLastWin32Error());
            return 2;
        }

        int rc;
        if (cmd == "row" || cmd == "rows" || cmd == "size")
            rc = Read(cmd, args);
        else
            rc = Inject(cmd, args);

        FreeConsole();
        return rc;
    }

    static int Inject(string cmd, string[] args)
    {
        IntPtr h = CreateFileW("CONIN$", GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
        if (h == IntPtr.Zero || h == (IntPtr)(-1))
        {
            Console.Error.WriteLine("ERR CONIN$ " + Marshal.GetLastWin32Error());
            return 3;
        }
        uint w;
        bool ok;
        if (cmd == "slipup")
        {
            short sx = short.Parse(args[2]), sy = short.Parse(args[3]);
            short ux = short.Parse(args[4]), uy = short.Parse(args[5]);
            var batch = new INPUT_RECORD[] {
                Rec(sx, sy, LEFT_BUTTON, MOUSE_MOVED),
                Rec(ux, uy, 0, 0),
            };
            ok = WriteConsoleInput(h, batch, 2, out w);
        }
        else
        {
            short x = short.Parse(args[2]), y = short.Parse(args[3]);
            INPUT_RECORD r;
            if (cmd == "press") r = Rec(x, y, LEFT_BUTTON, 0);
            else if (cmd == "move") r = Rec(x, y, LEFT_BUTTON, MOUSE_MOVED);
            else if (cmd == "up") r = Rec(x, y, 0, 0);
            else { Console.Error.WriteLine("ERR bad cmd " + cmd); return 4; }
            ok = WriteConsoleInput(h, new INPUT_RECORD[] { r }, 1, out w);
        }
        Console.WriteLine(ok ? ("OK " + w) : ("ERR write " + Marshal.GetLastWin32Error()));
        return ok ? 0 : 5;
    }

    static int Read(string cmd, string[] args)
    {
        IntPtr h = CreateFileW("CONOUT$", GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
        if (h == IntPtr.Zero || h == (IntPtr)(-1))
        {
            Console.Error.WriteLine("ERR CONOUT$ " + Marshal.GetLastWin32Error());
            return 3;
        }
        CONSOLE_SCREEN_BUFFER_INFO info;
        if (!GetConsoleScreenBufferInfo(h, out info))
        {
            Console.Error.WriteLine("ERR GetConsoleScreenBufferInfo " + Marshal.GetLastWin32Error());
            return 4;
        }
        // The visible window is what the user sees; rows are reported relative
        // to its top so the caller can use pane coordinates directly.
        short top = info.srWindow.Top;
        short width = (short)(info.srWindow.Right - info.srWindow.Left + 1);
        if (cmd == "size")
        {
            Console.WriteLine("SIZE w=" + width + " h=" + (info.srWindow.Bottom - info.srWindow.Top + 1)
                + " bufw=" + info.dwSize.X + " bufh=" + info.dwSize.Y + " top=" + top);
            return 0;
        }
        short y0 = short.Parse(args[2]);
        short y1 = cmd == "rows" ? short.Parse(args[3]) : y0;
        for (short y = y0; y <= y1; y++)
        {
            var coord = new COORD();
            coord.X = info.srWindow.Left;
            coord.Y = (short)(top + y);
            var attrs = new ushort[width];
            uint got;
            if (!ReadConsoleOutputAttribute(h, attrs, (uint)width, coord, out got))
            {
                Console.Error.WriteLine("ERR ReadAttr " + Marshal.GetLastWin32Error());
                return 5;
            }
            var chars = new char[width];
            uint gotc;
            if (!ReadConsoleOutputCharacterW(h, chars, (uint)width, coord, out gotc))
            {
                Console.Error.WriteLine("ERR ReadChar " + Marshal.GetLastWin32Error());
                return 6;
            }
            var sb = new StringBuilder();
            sb.Append("ROW ").Append(y).Append(" ATTR ");
            for (int i = 0; i < got; i++)
            {
                if (i > 0) sb.Append(',');
                sb.Append(attrs[i].ToString("X4"));
            }
            Console.WriteLine(sb.ToString());
            Console.WriteLine("ROW " + y + " TEXT " + new string(chars, 0, (int)gotc));
        }
        return 0;
    }
}
