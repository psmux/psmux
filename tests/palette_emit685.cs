// palette_emit685.exe [seconds]
//
// Issue #685 reproduction child.  Runs as a psmux PANE child (or standalone in a
// console) and emits, in order:
//
//   ESC ] 4 ; 4 ; rgb:00/00/80 ESC \     set palette index 4 to #000080
//   MARK_A  ESC[44m "   " ESC[0m "|"     a cell painted with the legacy index 4 bg
//   MARK_B  ESC[48;5;4m "   " ESC[0m "|" the same colour through indexed SGR
//   MARK_C  ESC[38;5;14m "XXX" ESC[0m    indexed fg, index 14
//
// then idles so the pane stays alive for a capture.  With OSC 4 honoured, the
// client must paint those cells with the RGB, not with the outer terminal's
// scheme.
//
// Optional args:
//   -noosc      skip the OSC 4 entirely (control pane: must stay ESC[44m)
//   -reset      after painting, emit OSC 104;4 and repaint MARK_D
//   -resetall   after painting, emit OSC 104 (all) and repaint MARK_D
//   -sec N      idle for N seconds (default 30)
//   -hash       use the #rrggbb form instead of rgb:rr/gg/bb
//   -wide       use the rgb:rrrr/gggg/bbbb form
//   -multi      set indexes 4 and 6 in ONE OSC 4 sequence
using System;
using System.IO;
using System.Text;
using System.Threading;

class PaletteEmit685
{
    static Stream o;
    static void W(string s)
    {
        byte[] b = Encoding.ASCII.GetBytes(s);
        o.Write(b, 0, b.Length);
        o.Flush();
    }

    static void Main(string[] args)
    {
        o = Console.OpenStandardOutput();
        bool noosc = false, reset = false, resetall = false, hash = false, wide = false, multi = false;
        int sec = 30;
        for (int i = 0; i < args.Length; i++)
        {
            string a = args[i].ToLowerInvariant();
            if (a == "-noosc") noosc = true;
            else if (a == "-reset") reset = true;
            else if (a == "-resetall") resetall = true;
            else if (a == "-hash") hash = true;
            else if (a == "-wide") wide = true;
            else if (a == "-multi") multi = true;
            else if (a == "-sec" && i + 1 < args.Length) { sec = int.Parse(args[++i]); i = i; }
        }

        string esc = "\u001b";
        string st = esc + "\\";

        W(esc + "[2J" + esc + "[H");
        Thread.Sleep(150);

        if (!noosc)
        {
            string body;
            if (multi) body = "4;4;rgb:00/00/80;6;rgb:00/80/80";
            else if (hash) body = "4;4;#000080";
            else if (wide) body = "4;4;rgb:0000/0000/8080";
            else body = "4;4;rgb:00/00/80";
            W(esc + "]" + body + st);
            Thread.Sleep(150);
        }

        // Each coloured run of blanks is followed by a visible "|". The inbox
        // console host of Windows 10 (build 19045) repaints its buffer when
        // the pane is resized, and it writes the blanks at the end of a line
        // as ESC[K in the default colours, so blanks left last on the line
        // lose their background before psmux reads them.
        W("MARK_A" + esc + "[44m" + "   " + esc + "[0m|\r\n");
        W("MARK_B" + esc + "[48;5;4m" + "   " + esc + "[0m|\r\n");
        W("MARK_C" + esc + "[38;5;14m" + "XXX" + esc + "[0m\r\n");
        W("MARK_E" + esc + "[48;5;6m" + "   " + esc + "[0m|\r\n");
        o.Flush();
        Thread.Sleep(300);

        if (reset || resetall)
        {
            W(esc + "]" + (resetall ? "104" : "104;4") + st);
            Thread.Sleep(150);
            W("MARK_D" + esc + "[48;5;4m" + "   " + esc + "[0m|\r\n");
            o.Flush();
        }

        Thread.Sleep(sec * 1000);
    }
}
