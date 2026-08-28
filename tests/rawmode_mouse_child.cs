// Raw-mode mouse child: models a full screen TUI running inside a psmux pane that
// registers for the mouse and later spawns a node child which enters raw mode
// (issue #613).
//
// libuv's uv_tty_set_mode writes ENABLE_WINDOW_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT
// (0x0208) over the WHOLE console input mode word and does not restore the previous
// mode when the process exits.  The console outlives the child, so from that moment
// the pane's console has no ENABLE_MOUSE_INPUT even though the pane's own application
// still wants the wheel.  This helper reproduces exactly that, with a real node child.
//
// Build: csc /nologo /out:rawmode_mouse_child.exe rawmode_mouse_child.cs
// Usage: rawmode_mouse_child.exe [alt=0|1] [decset=0|1] [conmouse=0|1] [log=PATH]
//        defaults: alt=1 decset=0 conmouse=1
//
// Commands typed into the pane (send-keys), one byte each:
//   'N'  spawn `node -e "process.stdin.setRawMode(true)"` with inherited stdio,
//        wait for it to exit cleanly, then log the console input mode before and after
//   'L'  spawn a LONG lived node child in raw mode and leave it running
//   'K'  kill that long lived child the way Ctrl+C does, with no libuv restore
//   'R'  the raw stripper fallback (SetConsoleMode 0x0208 in-process), so the scenario
//        is still exercised on a machine with no node on PATH
//   'M'  log the current console input mode
//   Ctrl+Z (0x1A) quit
using System;
using System.Diagnostics;
using System.IO;
using System.Text;
using System.Runtime.InteropServices;

class RawModeMouseChild {
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool GetConsoleMode(IntPtr h, out uint mode);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool SetConsoleMode(IntPtr h, uint mode);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool ReadFile(IntPtr h, byte[] buf, uint n, out uint read, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool WriteFile(IntPtr h, byte[] buf, uint n, out uint written, IntPtr ov);

    const int STD_INPUT_HANDLE = -10;
    const int STD_OUTPUT_HANDLE = -11;
    const uint ENABLE_PROCESSED_INPUT = 0x0001;
    const uint ENABLE_LINE_INPUT = 0x0002;
    const uint ENABLE_ECHO_INPUT = 0x0004;
    const uint ENABLE_MOUSE_INPUT = 0x0010;
    const uint ENABLE_WINDOW_INPUT = 0x0008;
    const uint ENABLE_QUICK_EDIT_MODE = 0x0040;
    const uint ENABLE_EXTENDED_FLAGS = 0x0080;
    const uint ENABLE_VIRTUAL_TERMINAL_INPUT = 0x0200;
    const uint ENABLE_VIRTUAL_TERMINAL_PROCESSING = 0x0004;
    // Exactly what libuv writes for raw mode, measured on node v24.
    const uint LIBUV_RAW_MODE = ENABLE_WINDOW_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT;

    static string log;
    static IntPtr hIn;

    static void Emit(IntPtr hOut, string s) {
        byte[] b = Encoding.ASCII.GetBytes(s);
        uint w;
        WriteFile(hOut, b, (uint)b.Length, out w, IntPtr.Zero);
    }

    static uint Mode() {
        uint m;
        return GetConsoleMode(hIn, out m) ? m : 0xFFFFFFFF;
    }

    static void LogMode(string tag) {
        uint m = Mode();
        File.AppendAllText(log, string.Format("MODE {0} 0x{1:X4} mouse={2}\n",
            tag, m, (m & ENABLE_MOUSE_INPUT) != 0));
    }

    static bool Flag(string[] args, string name, bool dflt) {
        foreach (string a in args) {
            if (a.StartsWith(name + "=", StringComparison.OrdinalIgnoreCase))
                return a.Substring(name.Length + 1).Trim() == "1";
        }
        return dflt;
    }

    static void SpawnNodeRawMode() {
        LogMode("before-node");
        bool ran = false;
        try {
            var psi = new ProcessStartInfo("node", "-e \"process.stdin.setRawMode(true)\"");
            psi.UseShellExecute = false;   // inherit this console, so node touches OUR CONIN$
            var p = Process.Start(psi);
            if (p != null) { p.WaitForExit(8000); ran = true; }
        } catch (Exception e) {
            File.AppendAllText(log, "NODE_SPAWN_FAILED " + e.Message + "\n");
        }
        File.AppendAllText(log, "NODE_RAN " + ran + "\n");
        LogMode("after-node");
    }

    static Process longChild;

    static void SpawnLongNodeRawMode() {
        LogMode("before-longnode");
        try {
            var psi = new ProcessStartInfo("node",
                "-e \"process.stdin.setRawMode(true); setInterval(function(){}, 1000)\"");
            psi.UseShellExecute = false;
            longChild = Process.Start(psi);
            File.AppendAllText(log, "LONGNODE_PID " + (longChild == null ? -1 : longChild.Id) + "\n");
        } catch (Exception e) {
            File.AppendAllText(log, "LONGNODE_SPAWN_FAILED " + e.Message + "\n");
        }
        System.Threading.Thread.Sleep(2500);
        LogMode("longnode-running");
    }

    static void KillLongNode() {
        // Ctrl+C parity: the child dies without running libuv's tty reset, so the
        // console keeps whatever raw mode word libuv last wrote.
        try {
            if (longChild != null && !longChild.HasExited) { longChild.Kill(); longChild.WaitForExit(5000); }
            File.AppendAllText(log, "LONGNODE_KILLED\n");
        } catch (Exception e) {
            File.AppendAllText(log, "LONGNODE_KILL_FAILED " + e.Message + "\n");
        }
        System.Threading.Thread.Sleep(500);
        LogMode("after-longnode-killed");
    }

    static void StripLikeLibuv() {
        LogMode("before-strip");
        SetConsoleMode(hIn, LIBUV_RAW_MODE);
        File.AppendAllText(log, "STRIPPED\n");
        LogMode("after-strip");
    }

    static int Main(string[] args) {
        bool alt      = Flag(args, "alt", true);
        bool decset   = Flag(args, "decset", false);
        bool conmouse = Flag(args, "conmouse", true);

        log = null;
        foreach (string a in args) {
            if (a.StartsWith("log=", StringComparison.OrdinalIgnoreCase)) log = a.Substring(4);
        }
        if (string.IsNullOrWhiteSpace(log)) log = Environment.GetEnvironmentVariable("PSMUX_RAWMODE_LOG");
        if (string.IsNullOrWhiteSpace(log)) log = Path.Combine(Environment.GetEnvironmentVariable("TEMP"), "psmux_rawmode_child.txt");

        File.WriteAllText(log, string.Format("RAWMODE START alt={0} decset={1} conmouse={2}\n", alt, decset, conmouse));

        hIn = GetStdHandle(STD_INPUT_HANDLE);
        IntPtr hOut = GetStdHandle(STD_OUTPUT_HANDLE);

        uint outMode;
        if (GetConsoleMode(hOut, out outMode))
            SetConsoleMode(hOut, outMode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);

        if (alt) Emit(hOut, "\x1b[?1049h\x1b[H\x1b[2J");
        if (decset) Emit(hOut, "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h");
        Emit(hOut, "\x1b[HRAWMODE_READY\r\n");

        uint mode;
        if (GetConsoleMode(hIn, out mode)) {
            uint newMode = mode & ~(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT | ENABLE_QUICK_EDIT_MODE);
            newMode |= ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_EXTENDED_FLAGS;
            if (conmouse) newMode |= ENABLE_MOUSE_INPUT; else newMode &= ~ENABLE_MOUSE_INPUT;
            SetConsoleMode(hIn, newMode);
            File.AppendAllText(log, string.Format("stdin mode {0:X} -> {1:X}\n", mode, newMode));
        } else {
            File.AppendAllText(log, "GetConsoleMode(stdin) failed\n");
        }
        LogMode("start");

        byte[] buf = new byte[512];
        while (true) {
            uint read;
            if (!ReadFile(hIn, buf, (uint)buf.Length, out read, IntPtr.Zero)) {
                System.Threading.Thread.Sleep(20);
                continue;
            }
            if (read == 0) { System.Threading.Thread.Sleep(10); continue; }

            var hex = new StringBuilder();
            var txt = new StringBuilder();
            for (int i = 0; i < read; i++) {
                hex.AppendFormat("{0:X2} ", buf[i]);
                byte b = buf[i];
                if (b == 0x1b) txt.Append("<ESC>");
                else if (b >= 0x20 && b < 0x7f) txt.Append((char)b);
                else txt.AppendFormat("<{0:X2}>", b);
            }
            File.AppendAllText(log, string.Format("RECV {0}  |  {1}\n", txt.ToString(), hex.ToString().Trim()));

            for (int i = 0; i < read; i++) {
                byte b = buf[i];
                if (b == 0x1a) { // Ctrl+Z quits
                    if (decset) Emit(hOut, "\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l");
                    if (alt) Emit(hOut, "\x1b[?1049l");
                    File.AppendAllText(log, "RAWMODE END\n");
                    return 0;
                }
                if (b == (byte)'N') SpawnNodeRawMode();
                else if (b == (byte)'L') SpawnLongNodeRawMode();
                else if (b == (byte)'K') KillLongNode();
                else if (b == (byte)'R') StripLikeLibuv();
                else if (b == (byte)'M') LogMode("probe");
            }
        }
    }
}
