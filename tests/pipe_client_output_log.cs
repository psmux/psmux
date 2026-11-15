// pipe_client_output_log.exe <log> <psmux.exe> <args...>
//
// Plays the outer terminal for a psmux client on the pipe route: starts the
// client with its stdin and stdout on pipes (PSMUX_PIPE_VT=1), answers XTWINOPS
// CSI 18 t with 30 rows by 120 columns, and appends everything the client
// writes to <log> as it arrives, as raw bytes, so a test can look at what was
// drawn after a point by the log's length at that point.
//
// Compile: csc /nologo /optimize /out:pipe_client_output_log.exe pipe_client_output_log.cs
using System;
using System.Diagnostics;
using System.IO;
using System.Text;

class PipeClientOutputLog
{
    static void Main(string[] args)
    {
        string log = args[0];
        var psi = new ProcessStartInfo(args[1]) { UseShellExecute = false, RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true };
        var rest = new StringBuilder();
        for (int i = 2; i < args.Length; i++) rest.Append(i > 2 ? " " : "").Append(args[i]);
        psi.Arguments = rest.ToString();
        psi.EnvironmentVariables["PSMUX_PIPE_VT"] = "1";
        var p = Process.Start(psi);
        var stdin = p.StandardInput.BaseStream;
        var stdout = p.StandardOutput.BaseStream;
        var buf = new byte[65536];
        var ask = Encoding.ASCII.GetBytes("\x1b[18t");
        var answer = Encoding.ASCII.GetBytes("\x1b[8;30;120t");
        int matched = 0;
        using (var file = new FileStream(log, FileMode.Create, FileAccess.Write, FileShare.ReadWrite))
        {
            while (true)
            {
                int n;
                try { n = stdout.Read(buf, 0, buf.Length); } catch { break; }
                if (n <= 0) break;
                file.Write(buf, 0, n);
                file.Flush();
                for (int i = 0; i < n; i++)
                {
                    matched = buf[i] == ask[matched] ? matched + 1 : (buf[i] == ask[0] ? 1 : 0);
                    if (matched == ask.Length)
                    {
                        matched = 0;
                        try { stdin.Write(answer, 0, answer.Length); stdin.Flush(); } catch { }
                    }
                }
            }
        }
        try { p.WaitForExit(2000); } catch { }
        try { p.Kill(); } catch { }
    }
}
