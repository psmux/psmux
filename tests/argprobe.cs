using System;
using System.IO;
using System.Management;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

// Writes what it was started with to %ARGPROBE_DIR%\<pid>.txt, then waits.
// Line 1: the raw command line. Then one "arg[i]=<value>" line per argv entry
// as the C runtime split it (CommandLineToArgvW rules). Then the parent
// process name and pid.
class ArgProbe
{
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    static extern IntPtr GetCommandLineW();

    static void Main(string[] args)
    {
        string dir = Environment.GetEnvironmentVariable("ARGPROBE_DIR");
        if (string.IsNullOrEmpty(dir)) dir = Path.Combine(Path.GetTempPath(), "argprobe");
        Directory.CreateDirectory(dir);
        int pid = System.Diagnostics.Process.GetCurrentProcess().Id;
        var sb = new StringBuilder();
        sb.AppendLine("cmdline=" + Marshal.PtrToStringUni(GetCommandLineW()));
        sb.AppendLine("exe=" + System.Reflection.Assembly.GetExecutingAssembly().Location);
        for (int i = 0; i < args.Length; i++) sb.AppendLine("arg[" + i + "]=" + args[i]);
        try
        {
            using (var s = new ManagementObjectSearcher("SELECT ParentProcessId FROM Win32_Process WHERE ProcessId=" + pid))
            foreach (ManagementObject o in s.Get())
            {
                uint ppid = (uint)o["ParentProcessId"];
                string pname = "?";
                try { pname = System.Diagnostics.Process.GetProcessById((int)ppid).ProcessName; } catch { }
                sb.AppendLine("parent=" + pname + " " + ppid);
            }
        }
        catch (Exception e) { sb.AppendLine("parent=error " + e.Message); }
        File.WriteAllText(Path.Combine(dir, pid + ".txt"), sb.ToString(), new UTF8Encoding(false));
        Thread.Sleep(Timeout.Infinite);
    }
}
