// fake_outer_terminal.exe <log> <seconds> <answer|silent> <psmux.exe> <args...>
//
// Plays the outer terminal of a psmux client on the pipe route: starts the
// client with its stdin and stdout on pipes (PSMUX_PIPE_VT=1), answers the
// XTWINOPS size query CSI 18 t with 30 rows by 120 columns, and for each
// OSC 4 query `ESC ] 4 ; N ; ?` the client writes, appends it to <log> (ESC
// shown as \e, BEL as \a) and, with `answer`, replies
// `ESC ] 4 ; N ; rgb:NN/NN/NN ESC \`, NN being N in two hex digits, so each
// index has its own colour. Stops the client after <seconds>.
//
// Compile: csc /nologo /optimize /out:fake_outer_terminal.exe fake_outer_terminal.cs
using System;
using System.Diagnostics;
using System.IO;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;

class FakeOuterTerminal
{
    static void Main(string[] args)
    {
        string log = args[0];
        int seconds = int.Parse(args[1]);
        bool answer = args[2] == "answer";
        var psi = new ProcessStartInfo(args[3])
        {
            UseShellExecute = false,
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };
        var rest = new StringBuilder();
        for (int i = 4; i < args.Length; i++) rest.Append(i > 4 ? " " : "").Append(args[i]);
        psi.Arguments = rest.ToString();
        psi.EnvironmentVariables["PSMUX_PIPE_VT"] = "1";
        File.WriteAllText(log, "");
        var p = Process.Start(psi);
        var stdin = p.StandardInput.BaseStream;
        var stdout = p.StandardOutput.BaseStream;
        var reader = new Thread(() =>
        {
            var buf = new byte[65536];
            string tail = "";
            while (true)
            {
                int n;
                try { n = stdout.Read(buf, 0, buf.Length); } catch { break; }
                if (n <= 0) break;
                string s = tail + Encoding.UTF8.GetString(buf, 0, n);
                int done = 0;
                foreach (Match m in Regex.Matches(s, "\x1b\\[18t|\x1b\\]4;(\\d+);\\?(\x07|\x1b\\\\)"))
                {
                    done = m.Index + m.Length;
                    string reply;
                    if (m.Value == "\x1b[18t")
                    {
                        reply = "\x1b[8;30;120t";
                    }
                    else
                    {
                        File.AppendAllText(log, m.Value.Replace("\x1b", "\\e").Replace("\x07", "\\a") + "\n");
                        if (!answer) continue;
                        int idx = int.Parse(m.Groups[1].Value);
                        string hx = idx.ToString("x2");
                        reply = "\x1b]4;" + idx + ";rgb:" + hx + "/" + hx + "/" + hx + "\x1b\\";
                    }
                    var b = Encoding.ASCII.GetBytes(reply);
                    try { stdin.Write(b, 0, b.Length); stdin.Flush(); } catch { }
                }
                // Keep what may be the start of a query split across reads.
                string left = s.Substring(done);
                tail = left.Length > 32 ? left.Substring(left.Length - 32) : left;
            }
        });
        reader.IsBackground = true;
        reader.Start();
        Thread.Sleep(seconds * 1000);
        try { p.Kill(); } catch { }
    }
}
