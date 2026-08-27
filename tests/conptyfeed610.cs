// conptyfeed610.exe <ctrlfile> <outfile> <command...>
//
// Hosts a child under a REAL pseudoconsole, exactly the way psmux hosts a pane,
// and lets a control file feed ARBITRARY BYTES into the ConPTY input pipe.  That
// is the only way to learn what ConPTY's input parser turns each candidate byte
// sequence into, which is the contract psmux has to hit for issue #610.
//
// Control file verbs (appended, one per line):
//   HEX 08 7f 1b ...   raw bytes, space separated hex
//   TEXT foo           literal ASCII, no CR
//   LINE foo           literal ASCII plus CR
//   CR
//   QUIT
//
// Everything the child writes out is mirrored to <outfile> so the caller can see
// whether ConPTY negotiated win32-input-mode (ESC [ ?9001 h) with us.
using System;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Threading;

class ConPtyFeed610
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

    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X, Y; }
    [StructLayout(LayoutKind.Sequential)]
    struct STARTUPINFO { public int cb; public string r1; public string r2; public string r3; public int dx, dy, dxs, dys, dxc, dyc, fa; public int flags; public short showw; public short r4; public IntPtr r5; public IntPtr si, so, se; }
    [StructLayout(LayoutKind.Sequential)]
    struct STARTUPINFOEX { public STARTUPINFO StartupInfo; public IntPtr lpAttributeList; }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int pid, tid; }

    const uint EXTENDED_STARTUPINFO_PRESENT = 0x00080000;
    static readonly IntPtr PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = new IntPtr(0x00020016);

    static void Main(string[] args)
    {
        if (args.Length < 3)
        {
            Console.Error.WriteLine("usage: conptyfeed610 <ctrlfile> <outfile> <command...>");
            return;
        }
        string ctrlFile = args[0];
        string outFile = args[1];
        string cmd = string.Join(" ", args, 2, args.Length - 2);
        string logFile = outFile + ".log";
        var log = new System.Text.StringBuilder();

        IntPtr inRead, inWrite, outRead, outWrite;
        CreatePipe(out inRead, out inWrite, IntPtr.Zero, 0);
        CreatePipe(out outRead, out outWrite, IntPtr.Zero, 0);

        COORD size; size.X = 120; size.Y = 30;
        IntPtr hPC;
        int hr = CreatePseudoConsole(size, inRead, outWrite, 0, out hPC);
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
        bool ok = CreateProcess(null, cmd, IntPtr.Zero, IntPtr.Zero, false,
            EXTENDED_STARTUPINFO_PRESENT, IntPtr.Zero, null, ref siex, out pi);
        log.Append("CreateProcess ok=" + ok + " e=" + Marshal.GetLastWin32Error() + " childPid=" + pi.pid + "\r\n");
        System.IO.File.WriteAllText(logFile, log.ToString());
        if (!ok) return;

        var outFs = new System.IO.FileStream(outFile, System.IO.FileMode.Create, System.IO.FileAccess.Write,
            System.IO.FileShare.ReadWrite);
        var reader = new Thread(() =>
        {
            byte[] buf = new byte[4096];
            while (true)
            {
                uint r;
                if (!ReadFile(outRead, buf, (uint)buf.Length, out r, IntPtr.Zero) || r == 0) break;
                lock (outFs) { outFs.Write(buf, 0, (int)r); outFs.Flush(); }
            }
        });
        reader.IsBackground = true;
        reader.Start();

        long lastLen = 0;
        while (true)
        {
            Thread.Sleep(80);
            if (!System.IO.File.Exists(ctrlFile)) continue;
            string content;
            try
            {
                using (var fs = new System.IO.FileStream(ctrlFile, System.IO.FileMode.Open,
                    System.IO.FileAccess.Read, System.IO.FileShare.ReadWrite))
                using (var sr = new System.IO.StreamReader(fs))
                    content = sr.ReadToEnd();
            }
            catch { continue; }
            if (content.Length <= (int)lastLen) continue;
            string tail = content.Substring((int)lastLen);
            lastLen = content.Length;
            foreach (var rawLine in tail.Split('\n'))
            {
                string line = rawLine.Trim();
                if (line.Length == 0) continue;
                byte[] b = null;
                if (line.StartsWith("HEX "))
                {
                    var parts = line.Substring(4).Split(new char[] { ' ' },
                        StringSplitOptions.RemoveEmptyEntries);
                    b = new byte[parts.Length];
                    for (int i = 0; i < parts.Length; i++)
                        b[i] = byte.Parse(parts[i], NumberStyles.HexNumber);
                }
                else if (line.StartsWith("TEXT "))
                    b = System.Text.Encoding.ASCII.GetBytes(line.Substring(5));
                else if (line.StartsWith("LINE "))
                    b = System.Text.Encoding.ASCII.GetBytes(line.Substring(5) + "\r");
                else if (line == "CR")
                    b = new byte[] { 0x0d };
                else if (line.StartsWith("QUIT"))
                {
                    ClosePseudoConsole(hPC);
                    Thread.Sleep(200);
                    return;
                }
                if (b != null)
                {
                    uint w;
                    WriteFile(inWrite, b, (uint)b.Length, out w, IntPtr.Zero);
                }
            }
        }
    }
}
