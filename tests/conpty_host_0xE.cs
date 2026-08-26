using System;
using System.Runtime.InteropServices;
using System.Threading;

// Hosts a child process under a REAL pseudoconsole, exactly the way Windows
// Terminal hosts psmux, then writes raw VT input bytes to its input pipe.
//
// This is the only faithful way to reproduce keyboard-driven Ctrl+C behaviour
// without an interactive desktop / window focus: WriteConsoleInput-based
// injectors bypass the console mode (ENABLE_PROCESSED_INPUT) that governs
// whether Ctrl+C becomes a key event or a CTRL_C_EVENT signal, so they cannot
// reproduce mode-dependent bugs.  Here the host owns the terminal end of a
// ConPTY and sends 0x03 bytes down the pipe, precisely like WT does on Ctrl+C.
//
// A small file-based control protocol drives it (TEMP\conpty_ctrl.txt):
//   CTRLC <n>   -> write the 0x03 byte n times (one Ctrl+C each)
//   TYPE <s>    -> write literal text, NO carriage return (leaves it on the line)
//   TEXT <s>    -> write literal text followed by CR (submits the line)
//   CR          -> write a bare carriage return
//   QUIT        -> close the pseudoconsole and exit
class ConPtyHostE {
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool CreatePipe(out IntPtr hRead, out IntPtr hWrite, IntPtr sa, uint size);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern int CreatePseudoConsole(COORD size, IntPtr hInput, IntPtr hOutput, uint flags, out IntPtr phPC);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern void ClosePseudoConsole(IntPtr hPC);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool WriteFile(IntPtr h, byte[] buf, uint n, out uint written, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool ReadFile(IntPtr h, byte[] buf, uint n, out uint read, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool CloseHandle(IntPtr h);

    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool InitializeProcThreadAttributeList(IntPtr lpAttributeList, int dwAttributeCount, int dwFlags, ref IntPtr lpSize);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool UpdateProcThreadAttribute(IntPtr lpAttributeList, uint dwFlags, IntPtr Attribute, IntPtr lpValue, IntPtr cbSize, IntPtr lpPreviousValue, IntPtr lpReturnSize);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool CreateProcess(string app, string cmd, IntPtr pa, IntPtr ta, bool inherit, uint flags, IntPtr env, string cwd, ref STARTUPINFOEX si, out PROCESS_INFORMATION pi);

    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X, Y; }
    [StructLayout(LayoutKind.Sequential)]
    struct STARTUPINFO { public int cb; public string r1; public string r2; public string r3; public int dx,dy,dxs,dys,dxc,dyc,fa; public int flags; public short showw; public short r4; public IntPtr r5; public IntPtr si, so, se; }
    [StructLayout(LayoutKind.Sequential)]
    struct STARTUPINFOEX { public STARTUPINFO StartupInfo; public IntPtr lpAttributeList; }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int pid, tid; }

    const uint EXTENDED_STARTUPINFO_PRESENT = 0x00080000;
    static readonly IntPtr PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = new IntPtr(0x00020016);

    static void Main(string[] args) {
        string cmd = args.Length > 0 ? string.Join(" ", args) : "cmd.exe";
        string ctrlFile = Environment.GetEnvironmentVariable("PSMUX_CONPTY_CTRL")
            ?? Environment.GetEnvironmentVariable("TEMP") + "\\conpty_ctrl.txt";
        string outFile = Environment.GetEnvironmentVariable("PSMUX_CONPTY_OUT")
            ?? Environment.GetEnvironmentVariable("TEMP") + "\\conpty_out.bin";
        string logFile = Environment.GetEnvironmentVariable("PSMUX_CONPTY_LOG")
            ?? Environment.GetEnvironmentVariable("TEMP") + "\\conpty_host.log";
        var log = new System.Text.StringBuilder();

        IntPtr inRead, inWrite, outRead, outWrite;
        CreatePipe(out inRead, out inWrite, IntPtr.Zero, 0);   // we write to inWrite -> child stdin
        CreatePipe(out outRead, out outWrite, IntPtr.Zero, 0); // child stdout -> we read outRead

        COORD size; size.X = 120; size.Y = 30;
        IntPtr hPC;
        int hr = CreatePseudoConsole(size, inRead, outWrite, 0xE, out hPC);
        log.Append("CreatePseudoConsole hr=" + hr + "\r\n");
        if (hr != 0) { System.IO.File.WriteAllText(logFile, log.ToString()); return; }

        IntPtr lpSize = IntPtr.Zero;
        InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref lpSize);
        IntPtr attr = Marshal.AllocHGlobal(lpSize);
        InitializeProcThreadAttributeList(attr, 1, 0, ref lpSize);
        UpdateProcThreadAttribute(attr, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, hPC, (IntPtr)IntPtr.Size, IntPtr.Zero, IntPtr.Zero);

        var siex = new STARTUPINFOEX();
        siex.StartupInfo.cb = Marshal.SizeOf(typeof(STARTUPINFOEX));
        siex.lpAttributeList = attr;
        PROCESS_INFORMATION pi;
        bool ok = CreateProcess(null, cmd, IntPtr.Zero, IntPtr.Zero, false, EXTENDED_STARTUPINFO_PRESENT, IntPtr.Zero, null, ref siex, out pi);
        log.Append("CreateProcess ok=" + ok + " e=" + Marshal.GetLastWin32Error() + " childPid=" + pi.pid + "\r\n");
        System.IO.File.WriteAllText(logFile, log.ToString());
        if (!ok) return;
        string pidFile = Environment.GetEnvironmentVariable("PSMUX_CONPTY_PID")
            ?? Environment.GetEnvironmentVariable("TEMP") + "\\conpty_childpid.txt";
        System.IO.File.WriteAllText(pidFile, pi.pid.ToString());

        // Reader thread: drain child output so the pipe never blocks.
        var outFs = new System.IO.FileStream(outFile, System.IO.FileMode.Create, System.IO.FileAccess.Write);
        var reader = new Thread(() => {
            byte[] buf = new byte[4096];
            while (true) {
                uint r;
                if (!ReadFile(outRead, buf, (uint)buf.Length, out r, IntPtr.Zero) || r == 0) break;
                lock (outFs) { outFs.Write(buf, 0, (int)r); outFs.Flush(); }
            }
        });
        reader.IsBackground = true;
        reader.Start();

        long lastLen = 0;
        while (true) {
            Thread.Sleep(100);
            if (!System.IO.File.Exists(ctrlFile)) continue;
            string content;
            try { content = System.IO.File.ReadAllText(ctrlFile); } catch { continue; }
            if (content.Length == (int)lastLen) continue;
            string tail = content.Substring((int)lastLen);
            lastLen = content.Length;
            foreach (var rawLine in tail.Split('\n')) {
                string line = rawLine.Trim();
                if (line.Length == 0) continue;
                if (line.StartsWith("CTRLC")) {
                    int n = 1; var parts = line.Split(' ');
                    if (parts.Length > 1) int.TryParse(parts[1], out n);
                    for (int k = 0; k < n; k++) {
                        uint w; WriteFile(inWrite, new byte[] { 0x03 }, 1, out w, IntPtr.Zero);
                    }
                } else if (line.StartsWith("TEXT ")) {
                    string s = line.Substring(5) + "\r";
                    byte[] b = System.Text.Encoding.ASCII.GetBytes(s);
                    uint w; WriteFile(inWrite, b, (uint)b.Length, out w, IntPtr.Zero);
                } else if (line.StartsWith("TYPE ")) {
                    string s = line.Substring(5); // no CR
                    byte[] b = System.Text.Encoding.ASCII.GetBytes(s);
                    uint w; WriteFile(inWrite, b, (uint)b.Length, out w, IntPtr.Zero);
                } else if (line.StartsWith("CR")) {
                    uint w; WriteFile(inWrite, new byte[] { 0x0d }, 1, out w, IntPtr.Zero);
                } else if (line.StartsWith("QUIT")) {
                    ClosePseudoConsole(hPC);
                    return;
                }
            }
        }
    }
}
