// conpty_da1_host.exe <conpty.dll> <outFile> <da1> <flags> <holdMs> <command...>
//
// A terminal in miniature that answers DA1 the way it is told to: runs a psmux
// CLIENT under a pseudoconsole made by the given conpty.dll (an OpenConsole,
// which forwards DA1 to its terminal where the inbox conhost answers it
// itself), writes every byte the client sends to <outFile>, and answers each
// `ESC [ c` it sees with
//
//   yes   ESC [ ? 61;4;6;7;14;21;22;23;24;28;32;42;52 c  (Windows Terminal 1.24)
//   no    ESC [ ? 61;6;7;14;21;22;23;24;28;32;42 c       (the same without 4)
//   none  nothing at all
//
// <flags> goes to CreatePseudoConsole (8 is PSEUDOCONSOLE_PASSTHROUGH_MODE).
// After <holdMs> the client is ended and the pseudoconsole closed.
// Prints "client pid <n>".
//
// Compile: csc /nologo /optimize /out:conpty_da1_host.exe conpty_da1_host.cs
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

class ConPtyDa1Host
{
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr LoadLibraryW(string path);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Ansi)]
    static extern IntPtr GetProcAddress(IntPtr module, string name);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CreatePipe(out IntPtr hRead, out IntPtr hWrite, IntPtr sa, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool WriteFile(IntPtr h, byte[] buf, uint n, out uint written, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool ReadFile(IntPtr h, byte[] buf, uint n, out uint read, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool InitializeProcThreadAttributeList(IntPtr l, int c, int f, ref IntPtr s);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool UpdateProcThreadAttribute(IntPtr l, uint f, IntPtr a, IntPtr v, IntPtr cb, IntPtr p, IntPtr r);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CreateProcessW(string app, string cmd, IntPtr pa, IntPtr ta, bool inherit,
        uint flags, IntPtr env, string cwd, ref STARTUPINFOEX si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateProcess(IntPtr h, uint code);

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    delegate int CreatePseudoConsoleFn(COORD size, IntPtr hInput, IntPtr hOutput, uint flags, out IntPtr phPC);
    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    delegate void ClosePseudoConsoleFn(IntPtr hPC);

    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X, Y; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct STARTUPINFO { public int cb; public string r1; public string r2; public string r3; public int dx, dy, dxs, dys, dxc, dyc, fa; public int flags; public short showw; public short r4; public IntPtr r5; public IntPtr si, so, se; }
    [StructLayout(LayoutKind.Sequential)]
    struct STARTUPINFOEX { public STARTUPINFO StartupInfo; public IntPtr lpAttributeList; }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int pid, tid; }

    const uint EXTENDED_STARTUPINFO_PRESENT = 0x00080000;
    static readonly IntPtr PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = new IntPtr(0x00020016);

    static int Main(string[] args)
    {
        if (args.Length < 6)
        {
            Console.Error.WriteLine("usage: conpty_da1_host <conpty.dll> <outFile> <yes|no|none> <flags> <holdMs> <command...>");
            return 2;
        }
        string dll = args[0], outFile = args[1], da1 = args[2];
        uint flags = uint.Parse(args[3]);
        int holdMs = int.Parse(args[4]);
        string cmd = string.Join(" ", args, 5, args.Length - 5);

        byte[] reply = null;
        if (da1 == "yes") reply = Encoding.ASCII.GetBytes("\x1b[?61;4;6;7;14;21;22;23;24;28;32;42;52c");
        else if (da1 == "no") reply = Encoding.ASCII.GetBytes("\x1b[?61;6;7;14;21;22;23;24;28;32;42c");

        IntPtr mod = LoadLibraryW(dll);
        if (mod == IntPtr.Zero) { Console.Error.WriteLine("LoadLibrary failed: " + Marshal.GetLastWin32Error()); return 1; }
        var create = (CreatePseudoConsoleFn)Marshal.GetDelegateForFunctionPointer(
            GetProcAddress(mod, "CreatePseudoConsole"), typeof(CreatePseudoConsoleFn));
        var close = (ClosePseudoConsoleFn)Marshal.GetDelegateForFunctionPointer(
            GetProcAddress(mod, "ClosePseudoConsole"), typeof(ClosePseudoConsoleFn));

        IntPtr inR, inW, outR, outW;
        CreatePipe(out inR, out inW, IntPtr.Zero, 0);
        CreatePipe(out outR, out outW, IntPtr.Zero, 0);
        IntPtr hpc;
        int hr = create(new COORD { X = 120, Y = 30 }, inR, outW, flags, out hpc);
        if (hr != 0) { Console.Error.WriteLine("CreatePseudoConsole failed: 0x" + hr.ToString("x")); return 1; }

        var output = new MemoryStream();
        object gate = new object();
        var reader = new Thread(() =>
        {
            byte[] buf = new byte[65536];
            int scanned = 0;
            while (true)
            {
                uint n;
                if (!ReadFile(outR, buf, (uint)buf.Length, out n, IntPtr.Zero) || n == 0) break;
                lock (gate)
                {
                    output.Write(buf, 0, (int)n);
                    byte[] all = output.GetBuffer();
                    int len = (int)output.Length;
                    // Answer every DA1 request, `ESC [ c` or `ESC [ 0 c`.
                    for (int i = Math.Max(0, scanned - 3); i + 2 < len; i++)
                    {
                        if (all[i] != 0x1b || all[i + 1] != (byte)'[') continue;
                        int j = i + 2;
                        if (all[j] == (byte)'0' && j + 1 < len) j++;
                        if (all[j] == (byte)'c' && reply != null)
                        {
                            uint w;
                            WriteFile(inW, reply, (uint)reply.Length, out w, IntPtr.Zero);
                        }
                    }
                    scanned = len;
                }
            }
        });
        reader.IsBackground = true;
        reader.Start();

        IntPtr size = IntPtr.Zero;
        InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref size);
        IntPtr attrs = Marshal.AllocHGlobal(size);
        InitializeProcThreadAttributeList(attrs, 1, 0, ref size);
        UpdateProcThreadAttribute(attrs, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, hpc, (IntPtr)IntPtr.Size, IntPtr.Zero, IntPtr.Zero);
        var si = new STARTUPINFOEX();
        si.StartupInfo.cb = Marshal.SizeOf(typeof(STARTUPINFOEX));
        si.lpAttributeList = attrs;
        // Null standard handles, or a child of a redirected parent writes to
        // the parent's pipes instead of the pseudoconsole.
        si.StartupInfo.flags = 0x00000100; // STARTF_USESTDHANDLES
        PROCESS_INFORMATION pi;
        if (!CreateProcessW(null, cmd, IntPtr.Zero, IntPtr.Zero, false, EXTENDED_STARTUPINFO_PRESENT,
            IntPtr.Zero, null, ref si, out pi))
        {
            Console.Error.WriteLine("CreateProcess failed: " + Marshal.GetLastWin32Error());
            return 1;
        }
        Console.WriteLine("client pid " + pi.pid);

        Thread.Sleep(holdMs);
        TerminateProcess(pi.hProcess, 0);
        Thread.Sleep(300);
        lock (gate) { File.WriteAllBytes(outFile, output.ToArray()); }
        close(hpc);
        return 0;
    }
}
