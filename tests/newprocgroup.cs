// Launch a command in a NEW PROCESS GROUP without a new console, which is how
// Windows sets the inheritable RTL_USER_PROCESS_PARAMETERS.ConsoleFlags bit 0
// ("Ctrl+C is disabled for this process").  Node's child_process
// spawn({detached:true}) does exactly this, so any tool that backgrounds a
// shell that way hands the bit to everything it starts, psmux included.
//
// Used by test_issue668_inherited_ctrlc_ignore.ps1 to launch the psmux CLI from
// a lineage that holds the bit.  `--plain` drops CREATE_NEW_PROCESS_GROUP and
// is the control arm.
//
// Usage: newprocgroup.exe [--plain] <exe> [args...]
using System;
using System.Runtime.InteropServices;
using System.Text;

class NewProcGroup {
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
    struct STARTUPINFO {
        public int cb; public string r1, desktop, title;
        public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public short wShowWindow, cbReserved2; public IntPtr lpReserved2;
        public IntPtr hStdInput, hStdOutput, hStdError;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }

    [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
    static extern bool CreateProcess(string app, StringBuilder cmd, IntPtr pa, IntPtr ta, bool inherit,
        uint flags, IntPtr env, string cwd, ref STARTUPINFO si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr h, uint ms);
    [DllImport("kernel32.dll")] static extern bool GetExitCodeProcess(IntPtr h, out uint code);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);

    const uint CREATE_NEW_PROCESS_GROUP = 0x00000200;
    const uint CREATE_NO_WINDOW = 0x08000000;

    static int Main(string[] args) {
        bool plain = args.Length > 0 && args[0] == "--plain";
        int start = plain ? 1 : 0;
        if (args.Length <= start) { Console.WriteLine("usage: newprocgroup.exe [--plain] <exe> [args...]"); return 2; }
        var sb = new StringBuilder();
        for (int i = start; i < args.Length; i++) {
            if (sb.Length > 0) sb.Append(' ');
            if (args[i].Contains(" ")) sb.Append('"').Append(args[i]).Append('"'); else sb.Append(args[i]);
        }
        uint flags = plain ? CREATE_NO_WINDOW : (CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        var si = new STARTUPINFO(); si.cb = Marshal.SizeOf(typeof(STARTUPINFO));
        PROCESS_INFORMATION pi;
        if (!CreateProcess(null, sb, IntPtr.Zero, IntPtr.Zero, false, flags, IntPtr.Zero, null, ref si, out pi)) {
            Console.WriteLine("CreateProcess FAILED err=" + Marshal.GetLastWin32Error());
            return 1;
        }
        Console.WriteLine("spawned pid=" + pi.dwProcessId + " newProcessGroup=" + (!plain));
        WaitForSingleObject(pi.hProcess, 120000);
        uint code; GetExitCodeProcess(pi.hProcess, out code);
        CloseHandle(pi.hProcess); CloseHandle(pi.hThread);
        Console.WriteLine("exit=" + code);
        return 0;
    }
}
