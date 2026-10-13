// screenread.exe <pid> <outFile>
// Attach to the console of <pid> and dump the visible window: one line of text
// per row, then one line of legacy attribute hex per row.
using System;
using System.IO;
using System.Text;
using System.Runtime.InteropServices;

static class ScreenRead
{
    [DllImport("kernel32.dll")] static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool AttachConsole(int pid);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr sa, uint disp, uint flags, IntPtr tmpl);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetConsoleScreenBufferInfo(IntPtr h, out CSBI info);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool ReadConsoleOutputW(IntPtr h, [Out] CHAR_INFO[] buf, COORD size, COORD coord, ref SMALL_RECT region);

    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X, Y; }
    [StructLayout(LayoutKind.Sequential)] struct SMALL_RECT { public short L, T, R, B; }
    [StructLayout(LayoutKind.Sequential)] struct CSBI { public COORD size, cursor; public short attr; public SMALL_RECT win; public COORD max; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] struct CHAR_INFO { public char ch; public short attr; }

    static int Main(string[] a)
    {
        int pid = int.Parse(a[0]);
        FreeConsole();
        if (!AttachConsole(pid)) { File.WriteAllText(a[1], "ATTACH_FAIL " + Marshal.GetLastWin32Error()); return 2; }
        IntPtr h = CreateFileW("CONOUT$", 0xC0000000, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        CSBI info;
        if (!GetConsoleScreenBufferInfo(h, out info)) { File.WriteAllText(a[1], "CSBI_FAIL " + Marshal.GetLastWin32Error()); return 3; }
        int w = info.win.R - info.win.L + 1, hh = info.win.B - info.win.T + 1;
        var buf = new CHAR_INFO[w * hh];
        var r = info.win;
        COORD sz; sz.X = (short)w; sz.Y = (short)hh; COORD z; z.X = 0; z.Y = 0;
        ReadConsoleOutputW(h, buf, sz, z, ref r);
        var sb = new StringBuilder();
        for (int y = 0; y < hh; y++) {
            var line = new StringBuilder();
            for (int x = 0; x < w; x++) line.Append(buf[y * w + x].ch == '\0' ? ' ' : buf[y * w + x].ch);
            sb.Append("T|").Append(line.ToString()).Append("\n");
        }
        for (int y = 0; y < hh; y++) {
            var line = new StringBuilder();
            for (int x = 0; x < w; x++) line.Append((buf[y * w + x].attr & 0xFF).ToString("X2")).Append(' ');
            sb.Append("A|").Append(line.ToString()).Append("\n");
        }
        File.WriteAllText(a[1], sb.ToString(), new UTF8Encoding(false));
        return 0;
    }
}
