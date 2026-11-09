// A native INPUT_RECORD reader with an optional ssh.exe descendant on its own
// console. Models a TUI spawning an SSH -N helper through a tool subprocess.
// The ssh.exe fixture is this executable copied under that name; no network or
// credentials are involved. Mode "bridge" instead reads VT bytes on this console.
using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

class MouseBridgeConsoleChild {
    [DllImport("kernel32.dll")] static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll")] static extern bool GetConsoleMode(IntPtr h, out uint m);
    [DllImport("kernel32.dll")] static extern bool SetConsoleMode(IntPtr h, uint m);
    [DllImport("kernel32.dll")] static extern uint GetConsoleProcessList(uint[] pids, uint size);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool CreateProcessW(string app, StringBuilder cmd, IntPtr pa,
        IntPtr ta, bool inherit, uint flags, IntPtr env, string cwd,
        ref STARTUPINFO si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode)]
    static extern bool ReadConsoleInputW(IntPtr h, [Out] INPUT_RECORD[] b, uint n, out uint read);
    [DllImport("kernel32.dll")] static extern bool ReadFile(IntPtr h, byte[] b, uint n, out uint read, IntPtr ov);
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
    struct STARTUPINFO {
        public uint cb; public string reserved, desktop, title;
        public uint x,y,xsize,ysize,xchars,ychars,fill,flags;
        public ushort show, reserved2; public IntPtr reservedPtr, stdin, stdout, stderr;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr process, thread; public uint pid, tid; }
    [StructLayout(LayoutKind.Explicit, Size=20)]
    struct INPUT_RECORD {
        [FieldOffset(0)] public ushort type;
        [FieldOffset(4)] public int down;
        [FieldOffset(14)] public ushort character;
    }
    static string log;
    static void Write(string s) { File.AppendAllText(log, s + "\n"); }
    static int Main(string[] args) {
        if (args[0] == "sleep") { System.Threading.Thread.Sleep(120000); return 0; }
        string mode = args[0]; log = args[1];
        File.WriteAllText(log, "START " + mode + "\n");
        IntPtr input = GetStdHandle(-10), output = GetStdHandle(-11);
        uint old;
        if (!GetConsoleMode(input, out old)) return 2;
        // Native reader has mouse records, never VTI. The bridge reads bytes.
        uint next = (old & ~0x0247u) | 0x0098u;
        if (mode == "bridge") next |= 0x0200u;
        if (!SetConsoleMode(input, next)) return 3;
        if (GetConsoleMode(output, out old)) SetConsoleMode(output, old | 4u);
        if (mode == "private-bridge") {
            string helper = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "ssh.exe");
            STARTUPINFO si = new STARTUPINFO();
            si.cb = (uint)Marshal.SizeOf(typeof(STARTUPINFO)); si.flags = 1;
            PROCESS_INFORMATION pi;
            if (!CreateProcessW(helper, new StringBuilder("\"" + helper + "\" sleep"),
                IntPtr.Zero, IntPtr.Zero, false, 0x10, IntPtr.Zero, null, ref si, out pi)) return 4;
            Write("HELPER " + pi.pid);
            CloseHandle(pi.process); CloseHandle(pi.thread);
        }
        uint[] members = new uint[128];
        uint count = GetConsoleProcessList(members, (uint)members.Length);
        for (int i=0; i<Math.Min(count, members.Length); i++) Write("MEMBER " + members[i]);
        Console.Write("\x1b[?1003h\x1b[?1006hREADY\r\n"); Console.Out.Flush();
        Write("READY");
        if (mode == "bridge") {
            byte[] b = new byte[128]; uint read;
            while (ReadFile(input, b, (uint)b.Length, out read, IntPtr.Zero)) {
                if (read > 0) Write("BYTES " + BitConverter.ToString(b, 0, (int)read));
            }
        } else {
            INPUT_RECORD[] b = new INPUT_RECORD[32]; uint read;
            while (ReadConsoleInputW(input, b, (uint)b.Length, out read)) {
                for (int i=0; i<read; i++) {
                    if (b[i].type == 1 && b[i].down != 0) Write("KEY " + b[i].character.ToString("X4"));
                    else if (b[i].type == 2) Write("MOUSE");
                }
            }
        }
        return 0;
    }
}
