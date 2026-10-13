// conpty_vt_keys.exe <steps> <command...>
//
// A VT terminal host in miniature, for the Alacritty input route: runs a psmux
// CLIENT under a real pseudoconsole (kernel32's CreatePseudoConsole, the one
// Alacritty uses unless a conpty.dll sits beside it) and writes raw bytes into
// its input pipe, the way Alacritty writes a key. Nothing answers the client's
// terminal queries, so its colour drain simply runs to its deadline.
//
// <steps> is a comma separated list, run in order:
//   w<ms>   wait
//   <hex>   write one byte, e.g. 0a for Ctrl+J, 02 for Ctrl+B, 0d for Enter
//   h<ms>   hold the pseudoconsole open, then close it and end the client
//
// The child inherits this process's environment, so the caller sets
// ALACRITTY_LOG (or not) before starting it. Prints "client pid <n>".
using System;
using System.Runtime.InteropServices;
using System.Threading;

class ConPtyVtKeys
{
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CreatePipe(out IntPtr hRead, out IntPtr hWrite, IntPtr sa, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern int CreatePseudoConsole(COORD size, IntPtr hInput, IntPtr hOutput, uint flags, out IntPtr phPC);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern void ClosePseudoConsole(IntPtr hPC);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool WriteFile(IntPtr h, byte[] buf, uint n, out uint written, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool ReadFile(IntPtr h, byte[] buf, uint n, out uint read, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool InitializeProcThreadAttributeList(IntPtr l, int c, int f, ref IntPtr s);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool UpdateProcThreadAttribute(IntPtr l, uint f, IntPtr a, IntPtr v, IntPtr cb, IntPtr p, IntPtr r);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CreateProcess(string app, string cmd, IntPtr pa, IntPtr ta, bool inherit,
        uint flags, IntPtr env, string cwd, ref STARTUPINFOEX si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateProcess(IntPtr h, uint code);

    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X, Y; }
    [StructLayout(LayoutKind.Sequential)]
    struct STARTUPINFO { public int cb; public string r1; public string r2; public string r3; public int dx, dy, dxs, dys, dxc, dyc, fa; public int flags; public short showw; public short r4; public IntPtr r5; public IntPtr si, so, se; }
    [StructLayout(LayoutKind.Sequential)]
    struct STARTUPINFOEX { public STARTUPINFO StartupInfo; public IntPtr lpAttributeList; }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int pid, tid; }

    const uint EXTENDED_STARTUPINFO_PRESENT = 0x00080000;
    static readonly IntPtr PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = new IntPtr(0x00020016);

    static int Main(string[] args)
    {
        if (args.Length < 2)
        {
            Console.Error.WriteLine("usage: conpty_vt_keys <steps> <command...>");
            return 2;
        }
        string[] steps = args[0].Split(',');
        string cmd = string.Join(" ", args, 1, args.Length - 1);

        IntPtr inRead, inWrite, outRead, outWrite;
        CreatePipe(out inRead, out inWrite, IntPtr.Zero, 0);
        CreatePipe(out outRead, out outWrite, IntPtr.Zero, 0);
        COORD size; size.X = 120; size.Y = 30;
        IntPtr hPC;
        int hr = CreatePseudoConsole(size, inRead, outWrite, 0, out hPC);
        if (hr != 0) { Console.Error.WriteLine("CreatePseudoConsole hr=" + hr); return 3; }

        IntPtr lpSize = IntPtr.Zero;
        InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref lpSize);
        IntPtr attr = Marshal.AllocHGlobal(lpSize);
        InitializeProcThreadAttributeList(attr, 1, 0, ref lpSize);
        UpdateProcThreadAttribute(attr, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, hPC, (IntPtr)IntPtr.Size, IntPtr.Zero, IntPtr.Zero);
        var siex = new STARTUPINFOEX();
        siex.StartupInfo.cb = Marshal.SizeOf(typeof(STARTUPINFOEX));
        siex.lpAttributeList = attr;
        PROCESS_INFORMATION pi;
        if (!CreateProcess(null, cmd, IntPtr.Zero, IntPtr.Zero, false,
            EXTENDED_STARTUPINFO_PRESENT, IntPtr.Zero, null, ref siex, out pi))
        {
            Console.Error.WriteLine("CreateProcess failed e=" + Marshal.GetLastWin32Error());
            return 4;
        }
        Console.WriteLine("client pid " + pi.pid);
        Console.Out.Flush();

        // Drain the client's output so its writes never block.
        var reader = new Thread(() =>
        {
            byte[] buf = new byte[8192];
            uint r;
            while (ReadFile(outRead, buf, (uint)buf.Length, out r, IntPtr.Zero) && r > 0) { }
        });
        reader.IsBackground = true;
        reader.Start();

        uint w;
        foreach (string raw in steps)
        {
            string s = raw.Trim();
            if (s.Length == 0) continue;
            if (s[0] == 'w') { Thread.Sleep(int.Parse(s.Substring(1))); continue; }
            if (s[0] == 'h') { Thread.Sleep(int.Parse(s.Substring(1))); break; }
            byte b = Convert.ToByte(s, 16);
            WriteFile(inWrite, new byte[] { b }, 1, out w, IntPtr.Zero);
        }

        // Closing the pseudoconsole ends the client (it sees its console go).
        ClosePseudoConsole(hPC);
        Thread.Sleep(300);
        TerminateProcess(pi.hProcess, 0);
        return 0;
    }
}
