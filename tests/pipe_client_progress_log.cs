// pipe_client_progress_log.exe <log> <psmux.exe> <args...>
//
// Plays the outer terminal for a psmux client on the pipe route: starts the
// client with its stdin and stdout on pipes (PSMUX_PIPE_VT=1), answers XTWINOPS
// CSI 18 t with 30 rows by 120 columns, and appends each OSC 9 ; 4 the client
// writes to <log> as soon as it is seen, one a line, ESC as \e and BEL as \a.
//
// Compile: csc /nologo /optimize /out:pipe_client_progress_log.exe pipe_client_progress_log.cs
using System;
using System.Diagnostics;
using System.IO;
using System.Text;
using System.Text.RegularExpressions;

class PipeClientProgressLog
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
        var pending = "";
        File.WriteAllText(log, "");
        while (true)
        {
            int n;
            try { n = stdout.Read(buf, 0, buf.Length); } catch { break; }
            if (n <= 0) break;
            string s = pending + Encoding.UTF8.GetString(buf, 0, n);
            int done = 0;
            foreach (Match m in Regex.Matches(s, "\x1b\\[18t|\x1b\\]9;4[^\x07\x1b]*(\x07|\x1b\\\\)"))
            {
                done = m.Index + m.Length;
                if (m.Value == "\x1b[18t")
                {
                    var b = Encoding.ASCII.GetBytes("\x1b[8;30;120t");
                    try { stdin.Write(b, 0, b.Length); stdin.Flush(); } catch { }
                }
                else
                {
                    File.AppendAllText(log, m.Value.Replace("\x1b", "\\e").Replace("\x07", "\\a") + "\n");
                }
            }
            // Keep a short tail so a sequence split across reads is still seen.
            string tail = s.Substring(done);
            pending = tail.Length > 32 ? tail.Substring(tail.Length - 32) : tail;
        }
        try { p.Kill(); } catch { }
    }
}
