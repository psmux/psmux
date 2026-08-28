// proc_cwd_probe.cs -- read a Win32 process's PEB CurrentDirectory (the value
// GetCurrentDirectory() returns inside that process) from outside it.
//
// Usage: proc_cwd_probe.exe <pid> [<pid> ...]
//        proc_cwd_probe.exe -tree <root-pid>     (root plus all descendants)
//
// Prints one line per process:  <pid> <exe-name> CWD=<path or ->
//
// This is a MEASUREMENT tool for issue #615: it answers "does the Win32 cwd of
// the pane's foreground process actually follow `cd` in the shell running in
// it?" for pwsh, cygwin bash, git bash and wsl.exe.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;

class ProcCwdProbe
{
    const int ProcessBasicInformation = 0;
    const uint PROCESS_QUERY_INFORMATION = 0x0400;
    const uint PROCESS_VM_READ = 0x0010;

    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_BASIC_INFORMATION
    {
        public IntPtr ExitStatus;
        public IntPtr PebBaseAddress;
        public IntPtr AffinityMask;
        public IntPtr BasePriority;
        public IntPtr UniqueProcessId;
        public IntPtr InheritedFromUniqueProcessId;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct UNICODE_STRING
    {
        public ushort Length;
        public ushort MaximumLength;
        public IntPtr Buffer;
    }

    [DllImport("ntdll.dll")]
    static extern int NtQueryInformationProcess(IntPtr h, int cls, ref PROCESS_BASIC_INFORMATION pbi, int len, out int ret);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr OpenProcess(uint access, bool inherit, int pid);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool ReadProcessMemory(IntPtr h, IntPtr addr, byte[] buf, int size, out IntPtr read);

    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr h);

    static bool Read(IntPtr h, IntPtr addr, byte[] buf)
    {
        IntPtr got;
        return ReadProcessMemory(h, addr, buf, buf.Length, out got) && (long)got == buf.Length;
    }

    static string GetCwd(int pid)
    {
        IntPtr h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid);
        if (h == IntPtr.Zero) return null;
        try
        {
            var pbi = new PROCESS_BASIC_INFORMATION();
            int ret;
            if (NtQueryInformationProcess(h, ProcessBasicInformation, ref pbi, Marshal.SizeOf(pbi), out ret) != 0) return null;
            if (pbi.PebBaseAddress == IntPtr.Zero) return null;

            // PEB (64-bit): +0x20 ProcessParameters
            byte[] p = new byte[8];
            if (!Read(h, (IntPtr)((long)pbi.PebBaseAddress + 0x20), p)) return null;
            long rup = BitConverter.ToInt64(p, 0);
            if (rup == 0) return null;

            // RTL_USER_PROCESS_PARAMETERS (64-bit): +0x38 CurrentDirectory.DosPath (UNICODE_STRING)
            byte[] u = new byte[16];
            if (!Read(h, (IntPtr)(rup + 0x38), u)) return null;
            ushort len = BitConverter.ToUInt16(u, 0);
            long buf = BitConverter.ToInt64(u, 8);
            if (len == 0 || buf == 0 || len > 4096) return null;

            byte[] s = new byte[len];
            if (!Read(h, (IntPtr)buf, s)) return null;
            return System.Text.Encoding.Unicode.GetString(s);
        }
        finally { CloseHandle(h); }
    }

    static Dictionary<int, List<int>> Children()
    {
        var map = new Dictionary<int, List<int>>();
        using (var s = new System.Management.ManagementObjectSearcher("SELECT ProcessId, ParentProcessId FROM Win32_Process"))
        foreach (System.Management.ManagementObject o in s.Get())
        {
            int pid = Convert.ToInt32(o["ProcessId"]);
            int ppid = Convert.ToInt32(o["ParentProcessId"]);
            if (!map.ContainsKey(ppid)) map[ppid] = new List<int>();
            map[ppid].Add(pid);
        }
        return map;
    }

    static void Emit(int pid)
    {
        string name = "?";
        try { name = Process.GetProcessById(pid).ProcessName; } catch { }
        string cwd = null;
        try { cwd = GetCwd(pid); } catch { }
        Console.WriteLine("{0} {1} CWD={2}", pid, name, cwd == null ? "-" : cwd);
    }

    static int Main(string[] a)
    {
        if (a.Length == 0) { Console.Error.WriteLine("usage: proc_cwd_probe <pid>... | -tree <pid>"); return 2; }
        if (a[0] == "-tree")
        {
            int root = int.Parse(a[1]);
            var kids = Children();
            var q = new Queue<int>(); q.Enqueue(root);
            var seen = new HashSet<int>();
            while (q.Count > 0)
            {
                int p = q.Dequeue();
                if (!seen.Add(p)) continue;
                Emit(p);
                if (kids.ContainsKey(p)) foreach (int c in kids[p]) q.Enqueue(c);
            }
            return 0;
        }
        foreach (string s in a) Emit(int.Parse(s));
        return 0;
    }
}
