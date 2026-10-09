// conread.exe <pid> [rows]
//
// Reads the VISIBLE SCREEN of another process's console and prints it as text.
// This is the "what did the user actually see" oracle: capture-pane returns pane
// CONTENT, so it cannot see the status bar, popups, menus or borders, which are
// drawn by the client around the panes. Anything rendered by psmux itself rather
// than by the shell inside a pane is invisible to every other check we have.
//
// Same attach trick as the key injector: FreeConsole, AttachConsole(pid), then
// open the screen buffer FRESH with CreateFile("CONOUT$"). GetStdHandle would
// hand back a handle to the console we just detached from, which looks valid and
// silently reads the wrong screen.
using System;
using System.Runtime.InteropServices;
using System.Text;

class ConRead
{
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool FreeConsole();

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AttachConsole(uint dwProcessId);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateFileW(string lpFileName, uint dwDesiredAccess,
        uint dwShareMode, IntPtr lpSecurityAttributes, uint dwCreationDisposition,
        uint dwFlagsAndAttributes, IntPtr hTemplateFile);

    [StructLayout(LayoutKind.Sequential)]
    struct COORD { public short X; public short Y; }

    [StructLayout(LayoutKind.Sequential)]
    struct SMALL_RECT { public short Left, Top, Right, Bottom; }

    [StructLayout(LayoutKind.Sequential)]
    struct CONSOLE_SCREEN_BUFFER_INFO
    {
        public COORD dwSize;
        public COORD dwCursorPosition;
        public short wAttributes;
        public SMALL_RECT srWindow;
        public COORD dwMaximumWindowSize;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetConsoleScreenBufferInfo(IntPtr h, out CONSOLE_SCREEN_BUFFER_INFO info);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool ReadConsoleOutputCharacterW(IntPtr h, [Out] char[] buf,
        uint len, COORD coord, out uint read);

    // Characters alone are not enough. A selection highlight, an active pane
    // border, a status style: all of those are ATTRIBUTES (colour), and the text
    // is identical whether or not a row is highlighted. Reading only characters
    // made a chooser whose selection was moving correctly look frozen.
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool ReadConsoleOutputAttribute(IntPtr h, [Out] ushort[] buf,
        uint len, COORD coord, out uint read);

    [StructLayout(LayoutKind.Sequential)]
    struct CONSOLE_CURSOR_INFO { public uint dwSize; public bool bVisible; }

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetConsoleCursorInfo(IntPtr h, out CONSOLE_CURSOR_INFO info);

    static int Main(string[] argv)
    {
        if (argv.Length < 1)
        {
            Console.Error.WriteLine("usage: conread <pid> [maxrows]");
            return 1;
        }
        uint pid = uint.Parse(argv[0]);
        int maxRows = 0;
        bool withAttrs = false;
        bool withCursor = false;
        for (int a = 1; a < argv.Length; a++)
        {
            if (argv[a] == "-a" || argv[a] == "--attrs") withAttrs = true;
            else if (argv[a] == "-c" || argv[a] == "--cursor") withCursor = true;
            else int.TryParse(argv[a], out maxRows);
        }

        // Buffer the output: once attached to the target console, writing to our
        // own stdout would land on the TARGET's screen and corrupt the very thing
        // being measured.
        var sb = new StringBuilder();

        FreeConsole();
        if (!AttachConsole(pid))
        {
            // Reattach to a console so the error is visible to the caller.
            Console.Error.WriteLine("AttachConsole failed err=" + Marshal.GetLastWin32Error());
            return 2;
        }

        IntPtr h = CreateFileW("CONOUT$", 0xC0000000u, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        if (h == new IntPtr(-1))
        {
            int e = Marshal.GetLastWin32Error();
            FreeConsole();
            Console.Error.WriteLine("CreateFile(CONOUT$) failed err=" + e);
            return 3;
        }

        CONSOLE_SCREEN_BUFFER_INFO info;
        if (!GetConsoleScreenBufferInfo(h, out info))
        {
            int e = Marshal.GetLastWin32Error();
            FreeConsole();
            Console.Error.WriteLine("GetConsoleScreenBufferInfo failed err=" + e);
            return 4;
        }

        // Read the WINDOW (what is on screen), not the whole scrollback buffer.
        int top = info.srWindow.Top;
        int height = info.srWindow.Bottom - info.srWindow.Top + 1;
        int width = info.dwSize.X;
        if (maxRows > 0 && maxRows < height) { top = info.srWindow.Bottom - maxRows + 1; height = maxRows; }

        // -c: first line is the console cursor, window relative, plus its
        // visibility flag. This is the cell the user sees blinking, so it is the
        // oracle for "which pane is the cursor in" (#767).
        if (withCursor)
        {
            CONSOLE_CURSOR_INFO ci;
            bool vis = GetConsoleCursorInfo(h, out ci) && ci.bVisible;
            sb.Append("CURSOR x=").Append(info.dwCursorPosition.X)
              .Append(" y=").Append(info.dwCursorPosition.Y - info.srWindow.Top)
              .Append(" visible=").Append(vis ? 1 : 0).AppendLine();
        }

        var line = new char[width];
        var attrs = new ushort[width];
        for (int row = 0; row < height; row++)
        {
            uint read;
            COORD at; at.X = 0; at.Y = (short)(top + row);
            if (!ReadConsoleOutputCharacterW(h, line, (uint)width, at, out read)) break;
            string text = new string(line, 0, (int)read).TrimEnd();

            if (withAttrs)
            {
                // Run-length encode the row's attributes: "7x40,112x18,7x62".
                // A highlighted row differs from its neighbours here even when the
                // text is identical, which is exactly how a moving selection is
                // detected without guessing at colours.
                uint aread;
                if (ReadConsoleOutputAttribute(h, attrs, (uint)width, at, out aread) && aread > 0)
                {
                    var rle = new StringBuilder();
                    ushort cur = attrs[0];
                    int run = 1;
                    for (int i = 1; i < (int)aread; i++)
                    {
                        if (attrs[i] == cur) { run++; continue; }
                        if (rle.Length > 0) rle.Append(',');
                        rle.Append(cur).Append('x').Append(run);
                        cur = attrs[i]; run = 1;
                    }
                    if (rle.Length > 0) rle.Append(',');
                    rle.Append(cur).Append('x').Append(run);
                    sb.Append("[attr ").Append(rle).Append("] ");
                }
            }
            sb.AppendLine(text);
        }

        FreeConsole();
        Console.Out.Write(sb.ToString());
        return 0;
    }
}
