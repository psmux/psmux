// console_window_of.exe <pid> [<pid> ...]
//
// Prints "<pid> <hwnd>" for each pid, where hwnd is the top level window that
// shows that process's console: the classic conhost window, or the Windows
// Terminal window a default terminal handoff put it in (the console window of
// such a process is a hidden PseudoConsoleWindow whose root owner is the
// terminal's window). 0 means the process has no console window or could not
// be attached to.
//
// The test runner uses it to tell a console window a suite opened apart from
// one that a program the suite never started opened during the suite (the
// amesh daemon that Task Scheduler restarts, for instance), so it never closes
// the latter. Run it as its own process: AttachConsole needs this process to
// have no console of its own, which FreeConsole gives it, and doing that inside
// the runner would cost the runner its console.
using System;
using System.Runtime.InteropServices;

static class ConsoleWindowOf
{
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool FreeConsole();
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll")] static extern IntPtr GetConsoleWindow();
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr hwnd, uint flags);
    const uint GA_ROOTOWNER = 3;

    static int Main(string[] args)
    {
        var lines = new System.Text.StringBuilder();
        foreach (var a in args)
        {
            uint pid;
            if (!uint.TryParse(a, out pid)) { continue; }
            long hwnd = 0;
            FreeConsole();
            if (AttachConsole(pid))
            {
                IntPtr w = GetConsoleWindow();
                if (w != IntPtr.Zero)
                {
                    IntPtr root = GetAncestor(w, GA_ROOTOWNER);
                    hwnd = (root != IntPtr.Zero ? root : w).ToInt64();
                }
                FreeConsole();
            }
            lines.AppendLine(pid + " " + hwnd);
        }
        // No console to write to after FreeConsole: the caller redirects stdout,
        // which is a pipe or file handle and survives FreeConsole.
        var stdout = Console.OpenStandardOutput();
        var bytes = System.Text.Encoding.ASCII.GetBytes(lines.ToString());
        stdout.Write(bytes, 0, bytes.Length);
        stdout.Flush();
        return 0;
    }
}
