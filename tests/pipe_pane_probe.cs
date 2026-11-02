// Runs inside the test's owned shell process, not an extra supervisor.
// ASCII input is deliberate: ConPTY is a console, not an arbitrary-byte PTY.
// Queue/tee unit tests cover byte values that the real console may transform.
using System;
using System.IO;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

public static class PipePaneProbe
{
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr GetStdHandle(int n);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetConsoleMode(IntPtr h, out uint mode);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool SetConsoleMode(IntPtr h, uint mode);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool SetConsoleCP(uint cp);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool ReadFile(IntPtr h, byte[] b, uint n, out uint read, IntPtr ov);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool WriteFile(IntPtr h, byte[] b, uint n, out uint written, IntPtr ov);

    static void Event(string root, string value)
    {
        byte[] bytes = Encoding.ASCII.GetBytes(value + "\n");
        using (FileStream f = new FileStream(root + ".events", FileMode.Append, FileAccess.Write, FileShare.ReadWrite))
            f.Write(bytes, 0, bytes.Length);
    }
    static void Wait(string path)
    {
        DateTime deadline = DateTime.UtcNow.AddSeconds(90);
        while (!File.Exists(path)) {
            if (DateTime.UtcNow >= deadline) throw new TimeoutException(path);
            Thread.Sleep(10);
        }
    }
    static void Write(byte[] bytes, int count)
    {
        // WriteFile can complete partially. Count progress only after every byte
        // of this chunk has actually been accepted by the OS pipe/console.
        int offset = 0;
        while (offset < count) {
            byte[] part = bytes;
            if (offset != 0) { part = new byte[count - offset]; Buffer.BlockCopy(bytes, offset, part, 0, part.Length); }
            uint n;
            if (!WriteFile(GetStdHandle(-11), part, (uint)(count - offset), out n, IntPtr.Zero) || n == 0)
                throw new IOException("stdout write failed: " + Marshal.GetLastWin32Error());
            offset += (int)n;
        }
    }
    static void Produce(string root, int total, bool console)
    {
        Wait(root + ".go");
        byte[] chunk = new byte[8192];
        int sent = 0;
        while (sent < total) {
            int n = Math.Min(chunk.Length, total - sent);
            for (int i = 0; i < n; i++) chunk[i] = console ? (byte)((i % 128 == 126) ? '\r' : (i % 128 == 127) ? '\n' : 'Q') : (byte)('a' + (sent + i) % 26);
            Write(chunk, n);
            sent += n;
            Event(root, "wrote " + sent);
        }
        if (console) { byte[] tail = Encoding.ASCII.GetBytes("PIPE_TAIL_TOKEN\r\n"); Write(tail, tail.Length); }
        Event(root, "produced " + sent);
    }
    static void Receive(string root, int total)
    {
        IntPtr input = GetStdHandle(-10);
        uint mode;
        if (!GetConsoleMode(input, out mode) || !SetConsoleMode(input, (mode & ~7u) | 0x0200u) || !SetConsoleCP(65001))
            throw new IOException("could not configure raw console input");
        Event(root, "raw");
        Wait(root + ".go");
        byte[] b = new byte[8192];
        int received = 0;
        using (FileStream capture = new FileStream(root + ".bytes", FileMode.Create, FileAccess.Write, FileShare.ReadWrite)) {
            while (received < total) {
                uint n;
                if (!ReadFile(input, b, (uint)Math.Min(b.Length, total - received), out n, IntPtr.Zero) || n == 0)
                    throw new IOException("console input ended at " + received);
                capture.Write(b, 0, (int)n);
                received += (int)n;
            }
        }
        Event(root, "received-all");
        if (File.Exists(root + ".reply")) {
            byte[] reply = Encoding.ASCII.GetBytes("PANE_REPLY_TOKEN\r\n"); Write(reply, reply.Length);
        }
        Wait(root + ".finish");
    }
    static void Sink(string root, bool duplex, int total)
    {
        if (duplex) Produce(root, total, false);
        else if (File.Exists(root + ".stall")) Wait(root + ".go");
        byte[] b = new byte[8192];
        string tail = "";
        using (Stream input = Console.OpenStandardInput())
        using (FileStream capture = new FileStream(root + ".bytes", FileMode.Create, FileAccess.Write, FileShare.ReadWrite)) {
            int n;
            while ((n = input.Read(b, 0, b.Length)) != 0) {
                capture.Write(b, 0, n); capture.Flush();
                Event(root, "read " + capture.Length);
                if (duplex) {
                    tail += Encoding.ASCII.GetString(b, 0, n);
                    if (tail.Contains("PANE_REPLY_TOKEN")) break;
                    if (tail.Length > 128) tail = tail.Substring(tail.Length - 128);
                }
            }
        }
        Event(root, "eof");
    }
    public static void Run(string mode, string root, int total)
    {
        File.WriteAllText(root + ".pid", Process.GetCurrentProcess().Id.ToString());
        Event(root, "ready");
        try {
            if (mode == "receive") Receive(root, total);
            else if (mode == "produce") Produce(root, total, false);
            else if (mode == "produce-close") {
                Produce(root, total, false);
                if (!CloseHandle(GetStdHandle(-11))) throw new IOException("stdout close failed");
                Event(root, "stdout-closed");
                Wait(root + ".finish");
            }
            else if (mode == "output") { Produce(root, total, true); Wait(root + ".finish"); }
            else if (mode == "output-exit") Produce(root, total, true);
            else if (mode == "sink") Sink(root, false, 0);
            else if (mode == "duplex") Sink(root, true, total);
            else if (mode == "fail") { Wait(root + ".go"); throw new IOException("intentional child failure"); }
            else throw new ArgumentException(mode);
            Event(root, "exit 0");
        } catch (Exception e) {
            Event(root, "exit 1 " + e.GetType().Name);
            throw;
        }
    }
}
