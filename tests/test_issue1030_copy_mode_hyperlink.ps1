# Issue #1030: OSC 8 hyperlinks were lost on every row psmux drew while the
# pane was in copy mode, so links scrolled back into view were not clickable.
#
# This runs a REAL attached client inside a pseudo console this script creates
# and records every byte the client's terminal emits. ConPTY keeps hyperlinks
# as cell attributes and re-emits them as OSC 8 on its output, so the recording
# shows exactly which drawn text carried a link:
#
#   1. normal mode: the linked line is drawn wrapped in OSC 8 (control)
#   2. copy mode, page up until the linked line is back in view: the linked
#      text must again be wrapped in OSC 8. Before the fix the text came back
#      with no OSC 8 at all.
#
# The pane's own ConPTY only delivers OSC 8 to psmux in passthrough mode. On a
# host where it strips them, the link never reaches psmux and the test SKIPs
# (that case is covered by tests-rs/test_issue1030_copy_mode_hyperlink.rs).
#
# Own -L namespace and PSMUX_DATA_DIR; cleans up with kill-server on that
# namespace only. Set $env:PSMUX_EXE to test a specific build.
param([string]$Exe = $env:PSMUX_EXE)
$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Remove-Item Env:\NO_COLOR -EA SilentlyContinue
if (-not $Exe) { $Exe = (Get-Command psmux -EA Stop).Source }
$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow }

$ns = "t1030" + (Get-Random -Maximum 999999)
$work = Join-Path ([IO.Path]::GetTempPath()) ("psmux_1030_" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force $work | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $work "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
$uri = "https://example.com/i1030"
$linkText = "CLICKME1030"

Write-Host "=== Issue #1030: OSC 8 links in copy mode ($Exe) ===" -ForegroundColor Cyan

# ── pseudo console recorder ───────────────────────────────────────────────
$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
if (-not (Test-Path $csc)) { $csc = "$env:WINDIR\Microsoft.NET\Framework64\v4.0.30319\csc.exe" }
if (-not (Test-Path $csc)) { Write-Skip "no csc.exe to build the pseudo console recorder"; exit 0 }
$src = Join-Path $work "ptyrec.cs"
@'
using System; using System.Diagnostics; using System.IO; using System.Runtime.InteropServices; using System.Text; using System.Threading;
class PtyRec {
  [StructLayout(LayoutKind.Sequential)] struct COORD { public short X, Y; }
  [StructLayout(LayoutKind.Sequential)] struct STARTUPINFO { public int cb; public string a, b, c; public int x, y, xs, ys, xc, yc, fa, fl; public short sw, cb2; public IntPtr r2, i, o, e; }
  [StructLayout(LayoutKind.Sequential)] struct STARTUPINFOEX { public STARTUPINFO S; public IntPtr L; }
  [StructLayout(LayoutKind.Sequential)] struct PI { public IntPtr hp, ht; public int pid, tid; }
  [DllImport("kernel32.dll")] static extern bool CreatePipe(out IntPtr r, out IntPtr w, IntPtr a, int n);
  [DllImport("kernel32.dll")] static extern int CreatePseudoConsole(COORD s, IntPtr i, IntPtr o, uint f, out IntPtr pc);
  [DllImport("kernel32.dll")] static extern bool InitializeProcThreadAttributeList(IntPtr l, int c, int f, ref IntPtr s);
  [DllImport("kernel32.dll")] static extern bool UpdateProcThreadAttribute(IntPtr l, uint f, IntPtr a, IntPtr v, IntPtr cb, IntPtr p, IntPtr r);
  [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool CreateProcess(string a, string c, IntPtr pp, IntPtr pt, bool inh, uint f, IntPtr env, string d, ref STARTUPINFOEX si, out PI pi);
  [DllImport("kernel32.dll")] static extern bool ReadFile(IntPtr h, byte[] b, int n, out int r, IntPtr o);
  [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr h, uint c);
  static long written; static object lk = new object();
  static void Main(string[] a) {
    string outPath = a[0]; string[] script = File.ReadAllLines(a[1]); string cmd = File.ReadAllText(a[2]).Trim();
    string marks = outPath + ".marks"; File.WriteAllText(marks, "");
    IntPtr ir, iw, or_, ow; CreatePipe(out ir, out iw, IntPtr.Zero, 0); CreatePipe(out or_, out ow, IntPtr.Zero, 0);
    COORD sz; sz.X = 100; sz.Y = 30; IntPtr pc;
    if (CreatePseudoConsole(sz, ir, ow, 0, out pc) != 0) { File.AppendAllText(marks, "ERR pty\n"); return; }
    IntPtr n = IntPtr.Zero; InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref n);
    IntPtr l = Marshal.AllocHGlobal(n); InitializeProcThreadAttributeList(l, 1, 0, ref n);
    UpdateProcThreadAttribute(l, 0, (IntPtr)0x00020016, pc, (IntPtr)IntPtr.Size, IntPtr.Zero, IntPtr.Zero);
    STARTUPINFOEX si = new STARTUPINFOEX(); si.S.cb = Marshal.SizeOf(typeof(STARTUPINFOEX)); si.L = l; PI pi;
    if (!CreateProcess(null, cmd, IntPtr.Zero, IntPtr.Zero, false, 0x00080000, IntPtr.Zero, null, ref si, out pi)) { File.AppendAllText(marks, "ERR spawn\n"); return; }
    var f = new FileStream(outPath, FileMode.Create, FileAccess.Write, FileShare.Read);
    var t = new Thread(() => { byte[] b = new byte[65536]; int k; try { while (ReadFile(or_, b, b.Length, out k, IntPtr.Zero) && k > 0) lock (lk) { f.Write(b, 0, k); f.Flush(); written += k; } } catch { } });
    t.IsBackground = true; t.Start();
    foreach (string raw in script) {
      string s = raw.Trim(); if (s.Length == 0) continue;
      string op = s.Substring(0, 1), rest = s.Length > 2 ? s.Substring(2) : "";
      if (op == "S") Thread.Sleep(int.Parse(rest));
      else if (op == "M") { long o; lock (lk) o = written; File.AppendAllText(marks, rest + " " + o + "\n"); }
      else if (op == "X") { int sp = rest.IndexOf(' '); var p = new ProcessStartInfo(rest.Substring(0, sp), rest.Substring(sp + 1)); p.UseShellExecute = false; p.CreateNoWindow = true; var q = Process.Start(p); q.WaitForExit(15000); }
    }
    Thread.Sleep(300); lock (lk) f.Flush(); File.AppendAllText(marks, "END " + written + "\n");
    TerminateProcess(pi.hp, 0); Environment.Exit(0);
  }
}
'@ | Set-Content $src -Encoding ASCII
$rec = Join-Path $work "ptyrec.exe"
& $csc /nologo /out:"$rec" "$src" 2>&1 | Out-Null
if (-not (Test-Path $rec)) { Write-Skip "could not build the pseudo console recorder"; exit 0 }

# ── pane payloads ─────────────────────────────────────────────────────────
$emitLink = Join-Path $work "emit_link.ps1"
@"
`$s = [char]27 + ']8;;$uri' + [char]27 + '\' + '$linkText' + [char]27 + ']8;;' + [char]27 + '\' + [char]10
`$b = [Text.Encoding]::UTF8.GetBytes(`$s); `$o = [Console]::OpenStandardOutput(); `$o.Write(`$b,0,`$b.Length); `$o.Flush()
"@ | Set-Content $emitLink -Encoding UTF8
$emitLines = Join-Path $work "emit_lines.ps1"
'1..100 | ForEach-Object { "filler line $_" }' | Set-Content $emitLines -Encoding UTF8
$conf = Join-Path $work "test.conf"
"set -g mouse off" | Set-Content $conf -Encoding ASCII

function Cleanup {
    & $Exe -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    Remove-Item $work -Recurse -Force -EA SilentlyContinue
}

& $Exe -L $ns -f $conf new-session -d -s s -x 100 -y 29 2>&1 | Out-Null
Start-Sleep -Seconds 3
& $Exe -L $ns has-session -t s 2>$null
if ($LASTEXITCODE -ne 0) { Write-Fail "session create failed"; Cleanup; exit 1 }

$script = Join-Path $work "script.txt"
$lines = @(
    "S 3000", "M attached",
    "X $Exe -L $ns send-keys -t s `"& '$emitLink'`" Enter",
    "S 2500", "M link_shown",
    "X $Exe -L $ns send-keys -t s `"& '$emitLines'`" Enter",
    "S 2500", "M lines_done",
    "X $Exe -L $ns copy-mode -t s",
    "S 800", "M copy_entered"
)
for ($i = 0; $i -lt 5; $i++) { $lines += "X $Exe -L $ns send-keys -X -t s page-up"; $lines += "S 500" }
$lines += "S 1000"; $lines += "M paged_up"
$lines | Set-Content $script -Encoding ASCII

$out = Join-Path $work "client.bin"
$cmdFile = Join-Path $work "cmd.txt"
"`"$Exe`" -L $ns attach -t s" | Set-Content $cmdFile -Encoding ASCII
$p = Start-Process -FilePath $rec -ArgumentList @("`"$out`"", "`"$script`"", "`"$cmdFile`"") -PassThru -WindowStyle Hidden
if (-not $p.WaitForExit(90000)) { Stop-Process -Id $p.Id -Force -EA SilentlyContinue }
$marks = @{}
if (Test-Path "$out.marks") {
    Get-Content "$out.marks" | ForEach-Object { $a = $_ -split ' '; if ($a.Count -eq 2) { $marks[$a[0]] = [int64]$a[1] } }
}
if (-not (Test-Path $out) -or -not $marks.ContainsKey("paged_up")) {
    Write-Fail "the recorder produced no client output"; Cleanup; exit 1
}
$bytes = [IO.File]::ReadAllBytes($out)
function Seg($a, $b) { [Text.Encoding]::UTF8.GetString($bytes, [int]$a, [int]($b - $a)) }
$normal = Seg $marks["attached"] $marks["link_shown"]
$paged = Seg $marks["copy_entered"] $marks["paged_up"]
$oscUri = [regex]::Escape("`e]8;") + "[^;`e]*;" + [regex]::Escape($uri) + "(`e\\|`a)"
# ConPTY may put a line break or cursor move between the OSC 8 open and the
# text it wraps; the link is still open across them.
$linkedText = $oscUri + "(?:\r|\n|`e\[[0-9;?]*[A-Za-z])*" + [regex]::Escape($linkText)
function Show($s) { $i = $s.IndexOf($linkText); if ($i -lt 0) { return "(text absent)" }; $st = [Math]::Max(0, $i - 50); ($s.Substring($st, [Math]::Min($s.Length - $st, 90)) -replace "`e", "<ESC>") }

Write-Host "  normal mode bytes around the link : $(Show $normal)"
Write-Host "  copy mode bytes around the link   : $(Show $paged)"

if (-not $normal.Contains($linkText)) {
    Write-Fail "the linked text never reached the client in normal mode"
} elseif (-not [regex]::IsMatch($normal, $oscUri)) {
    Write-Skip "no OSC 8 even in normal mode: the pane's ConPTY strips OSC 8 on this host (not passthrough)."
    Write-Skip "The copy mode path is covered by tests-rs/test_issue1030_copy_mode_hyperlink.rs."
} else {
    Write-Pass "normal mode: the client drew the link wrapped in OSC 8"
    if (-not $paged.Contains($linkText)) {
        Write-Fail "copy mode page up never brought the linked line back into view"
    } elseif ([regex]::IsMatch($paged, $linkedText)) {
        Write-Pass "copy mode after page up: the scrolled back link is drawn wrapped in OSC 8"
    } else {
        Write-Fail "copy mode after page up: '$linkText' drawn WITHOUT its OSC 8 hyperlink (#1030)"
    }
}

Cleanup
Write-Host "`n=== Results: Passed=$($script:Pass) Failed=$($script:Fail) ===" -ForegroundColor Cyan
exit $script:Fail
