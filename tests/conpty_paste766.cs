// conpty_paste766.exe <childlog> <textfile> <delayMs> <command...>
//
// Issue #766 host: runs a psmux CLIENT under a real pseudoconsole, the way a
// node-pty or xterm.js front end does, and writes into its input pipe exactly
// what the reporter's front end wrote: ESC[200~ <text> ESC[201~ in one write,
// then a lone CR <delayMs> later. <textfile> holds the text as UTF-8. The pane
// program (paste766_child.exe) logs what it receives to <childlog>; the host
// waits for its "ready" line before pasting, and appends its own "write paste"
// and "write CR" lines there so the caller has one timeline.
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

class ConPtyPaste766
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

    static long Now() { return DateTimeOffset.UtcNow.ToUnixTimeMilliseconds(); }

    static int Main(string[] args)
    {
        if (args.Length < 4)
        {
            Console.Error.WriteLine("usage: conpty_paste766 <childlog> <textfile> <delayMs> <command...>");
            return 2;
        }
        string childLog = args[0];
        string text = File.ReadAllText(args[1], Encoding.UTF8);
        int delay = int.Parse(args[2]);
        string cmd = string.Join(" ", args, 3, args.Length - 3);

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

        // Drain the client's output so its writes never block.
        var reader = new Thread(() =>
        {
            byte[] buf = new byte[8192];
            uint r;
            while (ReadFile(outRead, buf, (uint)buf.Length, out r, IntPtr.Zero) && r > 0) { }
        });
        reader.IsBackground = true;
        reader.Start();

        // Wait for the pane program, then let the client settle.
        var deadline = DateTime.UtcNow.AddSeconds(20);
        bool ready = false;
        while (DateTime.UtcNow < deadline)
        {
            try { if (File.Exists(childLog) && File.ReadAllText(childLog).Contains(" ready")) { ready = true; break; } }
            catch { }
            Thread.Sleep(100);
        }
        if (!ready)
        {
            Console.WriteLine("NOREADY");
            TerminateProcess(pi.hProcess, 1);
            ClosePseudoConsole(hPC);
            return 5;
        }
        Thread.Sleep(1500);

        byte[] paste = Encoding.UTF8.GetBytes("\x1b[200~" + text + "\x1b[201~");
        uint w;
        File.AppendAllText(childLog, Now() + " write paste\n");
        WriteFile(inWrite, paste, (uint)paste.Length, out w, IntPtr.Zero);
        Thread.Sleep(delay);
        File.AppendAllText(childLog, Now() + " write CR\n");
        WriteFile(inWrite, new byte[] { 0x0d }, 1, out w, IntPtr.Zero);
        Thread.Sleep(3000);

        // Closing the pseudoconsole ends the client (it sees its console go).
        ClosePseudoConsole(hPC);
        Thread.Sleep(300);
        TerminateProcess(pi.hProcess, 0);
        return 0;
    }
}
