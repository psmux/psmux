using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

// Keystroke to PIXELS, the same way for every terminal.
//
// The other keystroke probes in this tree inject into a console input buffer
// and watch a console screen buffer, which is upstream of a terminal's own
// renderer, so they cannot compare psmux with Windows Terminal or WezTerm.
// This one treats every host alike: the key goes in through SendInput, the
// way a keyboard's key does, to whichever window is in the foreground, and
// the answer is read off the SCREEN, from the desktop DWM composed.
//
//   1. find the target window (--pid: the console window of a process, for a
//      conhost hosted shell or psmux client; --title: a top level window
//      whose caption contains the text, for WT and WezTerm)
//   2. bring it to the foreground and REFUSE TO TYPE unless
//      GetForegroundWindow() is exactly that window, checked again before
//      every single key, so a key can never land in some other window
//   3. type one warm up key, diff the whole client area to find the row the
//      text appears on, and watch only a strip around that row from then on
//      (a strip copies in well under a millisecond; the whole window does not)
//   4. per key: copy the strip, stamp QPC, SendInput the key, copy the strip
//      in a tight loop until any pixel differs, stamp QPC
//   5. Escape at the end, which makes PSReadLine drop the typed line
//
// The shell in the target should have a STEADY cursor (DECSCUSR 2), or a
// blink that lands inside a wait reads as a fast key.
//
// Output lines: "HWND <hex> <title>", "STRIP x y w h", "GRAB_MS <one strip
// copy, the probe's resolution>", "LAT <i> <ms>",
// "MISS <i>", "SUMMARY n=.. p50=.. p90=.. p99=.. max=..", or a line starting
// "FOCUS" when the window could not be brought to the foreground or lost it,
// in which case nothing more is typed and the exit code is 3.
class PixelKeyLat {
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr h);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool AttachThreadInput(uint a, uint b, bool attach);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder sb, int n);
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] static extern IntPtr GetDC(IntPtr h);
    [DllImport("user32.dll")] static extern int ReleaseDC(IntPtr h, IntPtr dc);
    [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleDC(IntPtr dc);
    [DllImport("gdi32.dll")] static extern IntPtr SelectObject(IntPtr dc, IntPtr o);
    [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr o);
    [DllImport("gdi32.dll")] static extern bool DeleteDC(IntPtr dc);
    [DllImport("gdi32.dll")] static extern bool BitBlt(IntPtr d, int x, int y, int w, int h, IntPtr s, int sx, int sy, uint rop);
    [DllImport("gdi32.dll")] static extern IntPtr CreateDIBSection(IntPtr dc, ref BITMAPINFO bi, uint usage, out IntPtr bits, IntPtr sec, uint off);
    [DllImport("gdi32.dll")] static extern bool GdiFlush();
    [DllImport("kernel32.dll")] static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll")] static extern bool FreeConsole();
    [DllImport("kernel32.dll")] static extern IntPtr GetConsoleWindow();
    [DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint n, INPUT[] inputs, int size);
    [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
    [DllImport("winmm.dll")] static extern uint timeBeginPeriod(uint p);
    [DllImport("msvcrt.dll", CallingConvention = CallingConvention.Cdecl)] static extern int memcmp(IntPtr a, IntPtr b, UIntPtr n);
    [DllImport("msvcrt.dll", CallingConvention = CallingConvention.Cdecl)] static extern IntPtr memcpy(IntPtr d, IntPtr s, UIntPtr n);

    [StructLayout(LayoutKind.Sequential)] struct RECT { public int l, t, r, b; }
    [StructLayout(LayoutKind.Sequential)] struct POINT { public int x, y; }
    [StructLayout(LayoutKind.Sequential)] struct BITMAPINFOHEADER {
        public uint size; public int w, h; public ushort planes, bits; public uint comp, sizeImage; public int xppm, yppm; public uint used, important;
    }
    [StructLayout(LayoutKind.Sequential)] struct BITMAPINFO { public BITMAPINFOHEADER h; public uint c0; }
    [StructLayout(LayoutKind.Sequential)] struct KEYBDINPUT { public ushort vk, scan; public uint flags, time; public IntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT { public int dx, dy; public uint data, flags, time; public IntPtr extra; }
    [StructLayout(LayoutKind.Explicit)] struct INPUTU { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public INPUTU u; }

    const uint KEYEVENTF_KEYUP = 2, KEYEVENTF_UNICODE = 4;
    static StreamWriter Out;
    static void Say(string s) { Out.WriteLine(s); Out.Flush(); Console.WriteLine(s); }

    // A capture surface: a DIB section the screen is BitBlt'ed into.
    class Cap {
        public IntPtr dc, bmp, bits; public int x, y, w, h; public int bytes;
        public Cap(IntPtr screen, int x, int y, int w, int h) {
            this.x = x; this.y = y; this.w = w; this.h = h;
            var bi = new BITMAPINFO();
            bi.h.size = (uint)Marshal.SizeOf(typeof(BITMAPINFOHEADER)); bi.h.w = w; bi.h.h = -h; bi.h.planes = 1; bi.h.bits = 32;
            dc = CreateCompatibleDC(screen);
            bmp = CreateDIBSection(screen, ref bi, 0, out bits, IntPtr.Zero, 0);
            SelectObject(dc, bmp);
            bytes = w * h * 4;
        }
        public void Grab(IntPtr screen) { BitBlt(dc, 0, 0, w, h, screen, x, y, 0x00CC0020); GdiFlush(); }
        public void Free() { DeleteObject(bmp); DeleteDC(dc); }
    }

    static void Key(char c, ushort vk) {
        var a = new INPUT[2];
        a[0].type = 1; a[1].type = 1;
        if (vk != 0) { a[0].u.ki.vk = vk; a[1].u.ki.vk = vk; a[1].u.ki.flags = KEYEVENTF_KEYUP; }
        else {
            a[0].u.ki.scan = c; a[0].u.ki.flags = KEYEVENTF_UNICODE;
            a[1].u.ki.scan = c; a[1].u.ki.flags = KEYEVENTF_UNICODE | KEYEVENTF_KEYUP;
        }
        SendInput(2, a, Marshal.SizeOf(typeof(INPUT)));
    }

    static bool Focus(IntPtr h) {
        if (IsIconic(h)) ShowWindow(h, 9);
        for (int i = 0; i < 10; i++) {
            if (GetForegroundWindow() == h) return true;
            uint dummy;
            uint fg = GetWindowThreadProcessId(GetForegroundWindow(), out dummy);
            uint me = GetCurrentThreadId();
            AttachThreadInput(me, fg, true);
            BringWindowToTop(h);
            SetForegroundWindow(h);
            AttachThreadInput(me, fg, false);
            Thread.Sleep(100);
        }
        return GetForegroundWindow() == h;
    }

    static int Main(string[] a) {
        string title = null, outPath = "pixel_keylat.txt"; uint pid = 0; int n = 30, gap = 150, timeoutMs = 1000;
        for (int i = 0; i + 1 < a.Length; i += 2) {
            switch (a[i]) {
                case "--pid": pid = uint.Parse(a[i + 1]); break;
                case "--title": title = a[i + 1]; break;
                case "--n": n = int.Parse(a[i + 1]); break;
                case "--gap": gap = int.Parse(a[i + 1]); break;
                case "--timeout": timeoutMs = int.Parse(a[i + 1]); break;
                case "--out": outPath = a[i + 1]; break;
            }
        }
        Out = new StreamWriter(outPath, false, new UTF8Encoding(false));
        SetProcessDPIAware();
        timeBeginPeriod(1);

        IntPtr hwnd = IntPtr.Zero;
        if (pid != 0) {
            FreeConsole();
            if (AttachConsole(pid)) { hwnd = GetConsoleWindow(); FreeConsole(); }
        } else if (title != null) {
            EnumWindows((h, l) => {
                if (!IsWindowVisible(h)) return true;
                var sb = new StringBuilder(512); GetWindowText(h, sb, 512);
                if (sb.ToString().Contains(title)) { hwnd = h; return false; }
                return true;
            }, IntPtr.Zero);
        }
        if (hwnd == IntPtr.Zero) { Say("NOWINDOW"); Out.Close(); return 2; }
        var tsb = new StringBuilder(512); GetWindowText(hwnd, tsb, 512);
        Say(string.Format("HWND {0:X} {1}", hwnd.ToInt64(), tsb));

        if (!Focus(hwnd)) { Say("FOCUS could not bring the window to the foreground; nothing typed"); Out.Close(); return 3; }
        Thread.Sleep(300);

        RECT rc; GetClientRect(hwnd, out rc);
        var p0 = new POINT(); ClientToScreen(hwnd, ref p0);
        int cw = rc.r - rc.l, ch = rc.b - rc.t;
        IntPtr screen = GetDC(IntPtr.Zero);
        var full = new Cap(screen, p0.x, p0.y, cw, ch);
        var full2 = new Cap(screen, p0.x, p0.y, cw, ch);
        full.Grab(screen);

        // warm up key: find the text row
        if (GetForegroundWindow() != hwnd) { Say("FOCUS lost before the warm up key; nothing typed"); Out.Close(); return 3; }
        Key('x', 0);
        int minY = -1, maxY = -1;
        var sw = Stopwatch.StartNew();
        while (sw.ElapsedMilliseconds < 3000 && minY < 0) {
            full2.Grab(screen);
            unsafe {
                uint* A = (uint*)full.bits; uint* B = (uint*)full2.bits;
                for (int y = 0; y < ch; y++) {
                    for (int x = 0; x < cw; x++) {
                        if (A[y * cw + x] != B[y * cw + x]) { if (minY < 0) minY = y; maxY = y; break; }
                    }
                }
            }
            if (minY < 0) Thread.Sleep(2);
        }
        if (minY < 0) { Say("NOCHANGE the warm up key never changed the window"); Out.Close(); return 4; }
        int pad = Math.Max(4, (maxY - minY));
        int sy0 = Math.Max(0, minY - pad), sy1 = Math.Min(ch, maxY + pad + 1);
        var strip = new Cap(screen, p0.x, p0.y + sy0, cw, sy1 - sy0);
        var strip2 = new Cap(screen, p0.x, p0.y + sy0, cw, sy1 - sy0);
        Say(string.Format("STRIP {0} {1} {2} {3}", strip.x, strip.y, strip.w, strip.h));
        Thread.Sleep(gap);

        // The probe's own resolution: how long one strip copy takes.
        {
            var g = Stopwatch.StartNew();
            for (int k = 0; k < 50; k++) strip2.Grab(screen);
            Say(string.Format(System.Globalization.CultureInfo.InvariantCulture, "GRAB_MS {0:F3}", g.Elapsed.TotalMilliseconds / 50.0));
        }
        var lat = new List<double>();
        string chars = "abcdefghijklmnopqrstuvwxyz";
        int rc2 = 0;
        for (int i = 0; i < n; i++) {
            if (GetForegroundWindow() != hwnd) { Say("FOCUS lost at key " + i + "; stopped typing"); rc2 = 3; break; }
            strip.Grab(screen);
            long t0 = Stopwatch.GetTimestamp();
            Key(chars[i % chars.Length], 0);
            double ms = -1;
            while (true) {
                strip2.Grab(screen);
                long t = Stopwatch.GetTimestamp();
                if (memcmp(strip.bits, strip2.bits, (UIntPtr)strip.bytes) != 0) { ms = (t - t0) * 1000.0 / Stopwatch.Frequency; break; }
                if ((t - t0) * 1000.0 / Stopwatch.Frequency > timeoutMs) break;
            }
            if (ms >= 0) { lat.Add(ms); Say(string.Format(System.Globalization.CultureInfo.InvariantCulture, "LAT {0} {1:F3}", i, ms)); }
            else Say("MISS " + i);
            Thread.Sleep(gap);
        }
        if (GetForegroundWindow() == hwnd) Key((char)0, 0x1B);
        ReleaseDC(IntPtr.Zero, screen);
        lat.Sort();
        if (lat.Count > 0) {
            Func<double, double> P = q => lat[(int)Math.Floor(q * (lat.Count - 1))];
            Say(string.Format(System.Globalization.CultureInfo.InvariantCulture, "SUMMARY n={0} min={1:F2} p50={2:F2} p90={3:F2} p99={4:F2} max={5:F2}", lat.Count, lat[0], P(0.5), P(0.9), P(0.99), lat[lat.Count - 1]));
        }
        Out.Close();
        return rc2;
    }
}
