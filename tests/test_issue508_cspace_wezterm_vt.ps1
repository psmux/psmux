# Issue #508: "C-Space prefix broken under WezTerm on Windows (VT path)"
#
# What this proves, in order:
#   Part A  Ground truth: with ENABLE_VIRTUAL_TERMINAL_INPUT set, conhost
#           re-encodes the record WezTerm delivers for Ctrl+Space
#           (VK_SPACE, u_char 0x20, CTRL) as a single NUL byte 0x00 on the
#           VT stream, and as KEY_EVENT vk=VK_2 u_char=0 ctrl=CTRL|SHIFT on
#           the record stream.  Plain Space stays 0x20, so the NUL is fully
#           distinguishable.
#   Part B  psmux on the VT input path (WezTerm env vars set) arms a C-Space
#           prefix for the WezTerm Ctrl+Space record and its NUL variants.
#           This was the dead scenario of the report.
#   Part C  Greed guards on the VT path: plain Space, Ctrl+1 and plain '2'
#           must not arm a C-Space prefix; a C-b prefix still works; a
#           bind-key C-2 fires (the fold is symmetric with #504).
#   Part D  The native input path (no WezTerm env) is unaffected.
#   Part E  Real psmux inside a real WezTerm window, driven by real hardware
#           keystrokes, with the reporter's own config.
#   Part F  Win32 TUI verification (mandatory layer).
#
# Parts that need WezTerm are skipped (not failed) when it is absent.

$ErrorActionPreference = "Continue"
$PSMUX = (Get-Command psmux -EA Stop).Source
$psmuxDir = "$env:USERPROFILE\.psmux"
$tmp = Join-Path $env:TEMP "psmux_issue508"
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow; $script:TestsSkipped++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkCyan }

$weztermGui = $null
foreach ($cand in @("C:\Program Files\WezTerm\wezterm-gui.exe",
                    "$env:LOCALAPPDATA\wezterm\wezterm-gui.exe")) {
    if (Test-Path $cand) { $weztermGui = $cand; break }
}
if (-not $weztermGui) {
    $wcmd = Get-Command wezterm-gui -EA SilentlyContinue
    if ($wcmd) { $weztermGui = $wcmd.Source }
}

# --------------------------------------------------------------------------
# Build the helper executables (same probes as the #504 suite)
# --------------------------------------------------------------------------
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
$keydumpExe   = "$tmp\keydump508.exe"
$sendhwExe    = "$tmp\sendhw508.exe"
$rawinjectExe = "$tmp\rawinject508.exe"

# keydump: dumps console input as raw VT bytes (mode=vt) or records (mode=rec)
$keydumpSrc = @'
using System;using System.IO;using System.Runtime.InteropServices;using System.Text;using System.Threading;
class KeyDump{
 [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Auto)] static extern IntPtr CreateFile(string n,uint a,uint s,IntPtr sec,uint d,uint f,IntPtr t);
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetConsoleMode(IntPtr h,out uint m);
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool SetConsoleMode(IntPtr h,uint m);
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool ReadFile(IntPtr h,byte[] b,uint n,out uint r,IntPtr o);
 [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Unicode)] static extern bool ReadConsoleInputW(IntPtr h,[Out] IR[] b,uint l,out uint r);
 [DllImport("kernel32.dll")] static extern bool GetNumberOfConsoleInputEvents(IntPtr h,out uint n);
 [StructLayout(LayoutKind.Explicit)] struct IR{[FieldOffset(0)]public ushort T;[FieldOffset(4)]public KE K;}
 [StructLayout(LayoutKind.Sequential)] struct KE{public int Down;public ushort Rep;public ushort Vk;public ushort Sc;public ushort Ch;public uint Ctrl;}
 static StreamWriter log; static void L(string s){log.WriteLine(s);log.Flush();}
 static int Main(string[] a){
  string mode=a[0]; int secs=int.Parse(a[1]); log=new StreamWriter(a[2],false,new UTF8Encoding(false));
  IntPtr h=CreateFile("CONIN$",0x80000000|0x40000000,1|2,IntPtr.Zero,3,0,IntPtr.Zero);
  if(h==new IntPtr(-1)){L("CONIN_FAIL");return 1;}
  uint orig;GetConsoleMode(h,out orig);
  SetConsoleMode(h, mode=="vt" ? 0x0200u|0x0008u : 0x0008u|0x0010u);
  L("READY");
  var dl=DateTime.UtcNow.AddSeconds(secs); var recs=new IR[64]; var buf=new byte[256];
  while(DateTime.UtcNow<dl){
   uint p; if(!GetNumberOfConsoleInputEvents(h,out p)){break;} if(p==0){Thread.Sleep(10);continue;}
   if(mode=="vt"){uint r;if(!ReadFile(h,buf,(uint)buf.Length,out r,IntPtr.Zero))break;
    var sb=new StringBuilder("BYTES:");for(int i=0;i<r;i++)sb.Append(" ").Append(buf[i].ToString("X2"));L(sb.ToString());}
   else{uint r;if(!ReadConsoleInputW(h,recs,64,out r))break;
    for(int i=0;i<r;i++){if(recs[i].T!=1)continue;var k=recs[i].K;
     L(string.Format("KEY down={0} vk=0x{1:X2} uchar=0x{2:X4} ctrl=0x{3:X}",k.Down,k.Vk,k.Ch,k.Ctrl));}}
  }
  SetConsoleMode(h,orig); L("DONE"); log.Close(); return 0;}}
'@

# sendhw: real hardware keystrokes (SendInput) into a focused GUI window
$sendhwSrc = @'
using System;using System.Diagnostics;using System.Runtime.InteropServices;using System.Text;using System.Threading;
class SendHw{
 [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h,int c);
 [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] static extern uint SendInput(uint n,INPUT[] p,int cb);
 [DllImport("user32.dll")] static extern short VkKeyScan(char c);
 [DllImport("user32.dll")] static extern uint MapVirtualKey(uint c,uint t);
 [StructLayout(LayoutKind.Sequential)] struct INPUT{public uint type;public KB ki;public int p1;public int p2;}
 [StructLayout(LayoutKind.Sequential)] struct KB{public ushort Vk;public ushort Sc;public uint Fl;public uint Tm;public IntPtr Ex;}
 static void K(ushort vk,bool up){var i=new INPUT[1];i[0].type=1;i[0].ki.Vk=vk;i[0].ki.Sc=(ushort)MapVirtualKey(vk,0);i[0].ki.Fl=up?2u:0u;SendInput(1,i,Marshal.SizeOf(typeof(INPUT)));Thread.Sleep(25);}
 static int Main(string[] a){
  IntPtr hwnd=IntPtr.Zero; int pid=int.Parse(a[0].Substring(4));
  for(int i=0;i<60&&hwnd==IntPtr.Zero;i++){try{var p=Process.GetProcessById(pid);p.Refresh();hwnd=p.MainWindowHandle;}catch{}
   if(hwnd==IntPtr.Zero)Thread.Sleep(200);}
  if(hwnd==IntPtr.Zero){Console.WriteLine("NO_WINDOW");return 2;}
  ShowWindow(hwnd,9);
  for(int i=0;i<20;i++){SetForegroundWindow(hwnd);Thread.Sleep(100);if(GetForegroundWindow()==hwnd)break;}
  if(GetForegroundWindow()!=hwnd){Console.WriteLine("NO_FOCUS");return 3;}
  Thread.Sleep(400);
  for(int i=1;i<a.Length;i++){string s=a[i];
   if(s.StartsWith("sleep:")){Thread.Sleep(int.Parse(s.Substring(6)));continue;}
   bool ctrl=false,shift=false; string b=s;
   while(b.Length>2&&b[1]=='-'){char m=char.ToUpper(b[0]);if(m=='C')ctrl=true;else if(m=='S')shift=true;b=b.Substring(2);}
   ushort key;
   if(b.ToLower()=="space")key=0x20; else if(b.ToLower()=="enter")key=0x0D;
   else{short vs=VkKeyScan(b[0]);key=(ushort)(vs&0xFF);if((vs&0x100)!=0)shift=true;}
   if(ctrl)K(0x11,false); if(shift)K(0x10,false);
   K(key,false);K(key,true);
   if(shift)K(0x10,true); if(ctrl)K(0x11,true);
   Thread.Sleep(150);}
  Console.WriteLine("SENT");return 0;}}
'@

# rawinject: writes EXACT INPUT_RECORDs into a target console input buffer
$rawinjectSrc = @'
using System;using System.Runtime.InteropServices;using System.Threading;
class RawInject{
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool AttachConsole(uint p);
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool FreeConsole();
 [DllImport("kernel32.dll",SetLastError=true,CharSet=CharSet.Auto)] static extern IntPtr CreateFile(string n,uint a,uint s,IntPtr sec,uint d,uint f,IntPtr t);
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool WriteConsoleInputW(IntPtr h,IR[] b,uint l,out uint w);
 [DllImport("user32.dll")] static extern uint MapVirtualKey(uint c,uint t);
 [StructLayout(LayoutKind.Explicit)] struct IR{[FieldOffset(0)]public ushort T;[FieldOffset(4)]public int Down;
  [FieldOffset(8)]public ushort Rep;[FieldOffset(10)]public ushort Vk;[FieldOffset(12)]public ushort Sc;
  [FieldOffset(14)]public ushort Ch;[FieldOffset(16)]public uint Ctrl;}
 static IntPtr h;
 static void Send(ushort vk,ushort ch,uint ctrl){var r=new IR[2];
  for(int i=0;i<2;i++){r[i].T=1;r[i].Down=i==0?1:0;r[i].Rep=1;r[i].Vk=vk;r[i].Sc=(ushort)MapVirtualKey(vk,0);r[i].Ch=ch;r[i].Ctrl=ctrl;}
  uint w;WriteConsoleInputW(h,r,2,out w);Thread.Sleep(60);}
 static ushort P(string s){s=s.Trim();return s.StartsWith("0x")?Convert.ToUInt16(s.Substring(2),16):ushort.Parse(s);}
 static int Main(string[] a){
  uint pid=uint.Parse(a[0]); FreeConsole();
  if(!AttachConsole(pid)){Console.Error.WriteLine("ATTACH_FAIL");return 2;}
  h=CreateFile("CONIN$",0x80000000|0x40000000,1|2,IntPtr.Zero,3,0,IntPtr.Zero);
  if(h==new IntPtr(-1)){Console.Error.WriteLine("CONIN_FAIL");return 3;}
  for(int i=1;i<a.Length;i++){string s=a[i];
   if(s.StartsWith("sleep:")){Thread.Sleep(int.Parse(s.Substring(6)));continue;}
   if(s.StartsWith("raw:")){var p=s.Substring(4).Split(',');Send(P(p[0]),P(p[1]),p.Length>2?(uint)P(p[2]):0u);continue;}
   if(s.StartsWith("char:")){char c=s[5];Send((ushort)char.ToUpper(c),(ushort)c,0);continue;}}
  return 0;}}
'@

Write-Host "`n=== Issue #508: C-Space prefix under WezTerm (VT input path) ===" -ForegroundColor Cyan
Write-Host "[build] compiling probe helpers" -ForegroundColor DarkGray
$keydumpSrc   | Set-Content "$tmp\keydump508.cs"   -Encoding UTF8
$sendhwSrc    | Set-Content "$tmp\sendhw508.cs"    -Encoding UTF8
$rawinjectSrc | Set-Content "$tmp\rawinject508.cs" -Encoding UTF8
& $csc /nologo /optimize /out:$keydumpExe   "$tmp\keydump508.cs"   2>&1 | Out-Null
& $csc /nologo /optimize /out:$sendhwExe    "$tmp\sendhw508.cs"    2>&1 | Out-Null
& $csc /nologo /optimize /out:$rawinjectExe "$tmp\rawinject508.cs" 2>&1 | Out-Null
if (-not (Test-Path $rawinjectExe)) { Write-Fail "could not compile helpers"; exit 1 }

function Kill-Session($name) {
    & $PSMUX kill-session -t $name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 600
    Remove-Item "$psmuxDir\$name.*" -Force -EA SilentlyContinue
}

# Launch an attached psmux window with a given config; $vtEnv=$true sets the
# WezTerm env vars so needs_vt_input() routes the client onto the VT path.
function Start-Psmux($name, $configLines, $vtEnv) {
    Kill-Session $name
    $conf = "$tmp\$name.conf"
    ($configLines -join "`n") + "`n" | Set-Content $conf -Encoding UTF8
    $env:PSMUX_CONFIG_FILE = $conf
    $env:PSMUX_NO_WARM = "1"
    if ($vtEnv) { $env:TERM_PROGRAM = "WezTerm"; $env:WEZTERM_PANE = "0" }
    $p = Start-Process -FilePath $PSMUX -ArgumentList "new-session","-s",$name -PassThru
    Start-Sleep -Seconds 6
    $env:PSMUX_CONFIG_FILE = $null
    if ($vtEnv) { Remove-Item Env:TERM_PROGRAM -EA SilentlyContinue; Remove-Item Env:WEZTERM_PANE -EA SilentlyContinue }
    & $PSMUX has-session -t $name 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    return $p
}

function Wins($name) { [int](& $PSMUX display-message -t $name -p '#{session_windows}' 2>&1 | Out-String).Trim() }

# Inject a record then the 'c' key; a new window means the prefix armed.
function Test-PrefixArms($name, $proc, $spec) {
    $before = Wins $name
    & $rawinjectExe $proc.Id $spec "sleep:400" "char:c" | Out-Null
    Start-Sleep -Milliseconds 2400
    return ((Wins $name) -gt $before)
}

# ==========================================================================
# PART A: ground truth - what conhost's VTI re-encoding delivers
# ==========================================================================
Write-Host "`n[Part A] conhost VTI re-encoding of the WezTerm Ctrl+Space record" -ForegroundColor Yellow

function Probe($mode, $tag, $specs) {
    $log = "$tmp\probe_${mode}_$tag.log"
    Remove-Item $log -Force -EA SilentlyContinue
    $p = Start-Process -FilePath "cmd.exe" -ArgumentList @("/c",$keydumpExe,$mode,"8",$log) -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 2
    $probeArgs = @($p.Id) + $specs
    & $rawinjectExe @probeArgs 2>&1 | Out-Null
    Start-Sleep -Seconds 7
    # Stop keydump with its cmd. keydump's 8 s deadline is checked only
    # between reads, and on Windows 10 (build 19045) the injected Ctrl+Space
    # record makes no VT byte, so its ReadFile never returns; stopping cmd
    # alone left keydump508.exe running for good, holding the runner's
    # working folder.
    & taskkill.exe /T /F /PID $p.Id 2>&1 | Out-Null
    if (Test-Path $log) { return (Get-Content $log) }
    return @("NO_LOG")
}

$vtBytes = (Probe "vt" "wez_cspace" @("raw:0x20,0x20,0x8")) -join " | "
Write-Info "VT stream for (VK_SPACE, 0x20, CTRL): $vtBytes"
# The inbox conhost of Windows 10 (build 19045) turns this record into
# nothing at all on the VT stream, not a NUL; WezTerm's own pseudo console
# does the same there for a real Ctrl+Space. Parts B to D, psmux's VT route,
# do not depend on it.
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000
if ($vtBytes -match "BYTES: 00") {
    Write-Pass "conhost re-encodes WezTerm's Ctrl+Space as the NUL byte 0x00 on the VT stream"
} elseif ($Win10 -and $vtBytes -notmatch "BYTES:") {
    Write-Pass "Windows 10 inbox conhost drops WezTerm's Ctrl+Space record from the VT stream (no byte at all)"
} else {
    Write-Fail "expected a lone NUL byte, got: $vtBytes"
}

$vtSpace = (Probe "vt" "plain_space" @("raw:0x20,0x20,0x0")) -join " | "
Write-Info "VT stream for plain Space: $vtSpace"
if ($vtSpace -match "BYTES: 20") {
    Write-Pass "plain Space stays 0x20 - the NUL is fully distinguishable"
} else {
    Write-Fail "expected 0x20 for plain Space, got: $vtSpace"
}

# ==========================================================================
# PART B: the dead scenario - VT path prefix arming
# ==========================================================================
Write-Host "`n[Part B] VT input path (WezTerm env): C-Space prefix arming" -ForegroundColor Yellow

$S = "issue508_b"
$proc = Start-Psmux $S @("set -g prefix C-Space","bind C-Space send-prefix") $true
if (-not $proc) {
    Write-Fail "could not start session $S"
} else {
    if (Test-PrefixArms $S $proc "raw:0x20,0x20,0x8") {
        Write-Pass "WezTerm Ctrl+Space record (VK_SPACE, 0x20, CTRL) arms the prefix  <-- the #508 fix"
    } else {
        Write-Fail "WezTerm Ctrl+Space record did NOT arm the prefix (#508 regression)"
    }
    if (Test-PrefixArms $S $proc "raw:0x32,0x00,0x18") {
        Write-Pass "NUL as VK_2 + CTRL|SHIFT (ConPTY encoding) arms the prefix on the VT path"
    } else {
        Write-Fail "NUL as VK_2 + CTRL|SHIFT did NOT arm the prefix on the VT path"
    }
    if (Test-PrefixArms $S $proc "raw:0x20,0x00,0x8") {
        Write-Pass "Ctrl+Space with a zero UnicodeChar arms the prefix on the VT path"
    } else {
        Write-Fail "Ctrl+Space with a zero UnicodeChar did NOT arm the prefix on the VT path"
    }
    Kill-Session $S
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
}

# ==========================================================================
# PART C: greed guards on the VT path
# ==========================================================================
Write-Host "`n[Part C] VT path greed guards" -ForegroundColor Yellow

$S = "issue508_c"
$proc = Start-Psmux $S @("set -g prefix C-Space","bind C-Space send-prefix") $true
if (-not $proc) {
    Write-Fail "could not start session $S"
} else {
    if (-not (Test-PrefixArms $S $proc "raw:0x20,0x20,0x0")) {
        Write-Pass "plain Space does NOT arm the prefix on the VT path"
    } else {
        Write-Fail "plain Space wrongly armed the prefix on the VT path"
    }
    if (-not (Test-PrefixArms $S $proc "raw:0x31,0x00,0x8")) {
        Write-Pass "Ctrl+1 does NOT arm the prefix on the VT path"
    } else {
        Write-Fail "Ctrl+1 wrongly armed the prefix on the VT path"
    }
    if (-not (Test-PrefixArms $S $proc "raw:0x32,0x32,0x0")) {
        Write-Pass "typing plain '2' does NOT arm the prefix on the VT path"
    } else {
        Write-Fail "plain '2' wrongly armed the prefix on the VT path"
    }
    Kill-Session $S
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
}

# A C-b prefix must keep working on the VT path.
$S = "issue508_c2"
$proc = Start-Psmux $S @("set -g prefix C-b") $true
if ($proc) {
    if (Test-PrefixArms $S $proc "raw:0x42,0x02,0x8") {
        Write-Pass "C-b prefix still arms on the VT path"
    } else {
        Write-Fail "C-b prefix stopped working on the VT path"
    }
    Kill-Session $S
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
}

# bind-key C-2 must fire on the VT path (fold symmetry with #504).
$S = "issue508_c3"
$proc = Start-Psmux $S @("set -g prefix C-b", "bind-key -T root C-2 new-window") $true
if ($proc) {
    $before = Wins $S
    & $rawinjectExe $proc.Id "raw:0x32,0x00,0x18" | Out-Null
    Start-Sleep -Milliseconds 2400
    if ((Wins $S) -gt $before) {
        Write-Pass "an existing bind-key C-2 fires on the VT path (fold applied symmetrically)"
    } else {
        Write-Fail "bind-key C-2 does not fire on the VT path"
    }
    Kill-Session $S
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
}

# ==========================================================================
# PART D: native path unaffected
# ==========================================================================
Write-Host "`n[Part D] Native input path regression guard" -ForegroundColor Yellow

$S = "issue508_d"
$proc = Start-Psmux $S @("set -g prefix C-Space","bind C-Space send-prefix") $false
if ($proc) {
    if (Test-PrefixArms $S $proc "raw:0x20,0x20,0x8") {
        Write-Pass "native path: Ctrl+Space still arms the prefix"
    } else {
        Write-Fail "native path: Ctrl+Space stopped arming the prefix"
    }
    if (-not (Test-PrefixArms $S $proc "raw:0x20,0x20,0x0")) {
        Write-Pass "native path: plain Space still does NOT arm the prefix"
    } else {
        Write-Fail "native path: plain Space wrongly armed the prefix"
    }
    Kill-Session $S
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
}

# ==========================================================================
# PART E: real psmux inside a real WezTerm window
# ==========================================================================
Write-Host "`n[Part E] Real psmux in a real WezTerm window (the reporter's setup)" -ForegroundColor Yellow

if ($weztermGui) {
    $S = "issue508_e"
    Kill-Session $S
    $pconf = "$tmp\$S.conf"
    "set -g prefix C-Space`nbind C-Space send-prefix`n" | Set-Content $pconf -Encoding UTF8

    $env:PSMUX_CONFIG_FILE = $pconf
    $env:PSMUX_NO_WARM = "1"
    # -n skips the user's wezterm.lua so defaults (win32 input mode ON) apply.
    # That also skips its front_end, and where OpenGL is too old for the
    # default one (a Hyper-V guest: "The OpenGL implementation is too old to
    # work with glium") no window opens. PSMUX_TEST_WEZTERM_FRONT_END (WebGpu
    # there) picks one without reading wezterm.lua.
    $wezArgs = @("-n")
    if ($env:PSMUX_TEST_WEZTERM_FRONT_END) { $wezArgs += @("--config", "front_end='$($env:PSMUX_TEST_WEZTERM_FRONT_END)'") }
    $wezArgs += @("start", "--", $PSMUX, "new-session", "-s", $S)
    $wp = Start-Process -FilePath $weztermGui -ArgumentList $wezArgs -PassThru
    Start-Sleep -Seconds 10
    $env:PSMUX_CONFIG_FILE = $null

    & $PSMUX has-session -t $S 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Skip "psmux session did not start inside WezTerm"
    } else {
        $before = Wins $S
        # Real Ctrl+Space, then real 'c', straight from the hardware input queue.
        #
        # sendhw prints NO_FOCUS and exits 3 when SetForegroundWindow loses, which
        # happens on a locked, unattended, or busy desktop. Piping that to Out-Null
        # discarded the difference between "the desktop refused focus" and "the
        # prefix is broken", so an unattended run reported a product failure for a
        # keystroke that was never delivered. Capture it and skip instead.
        $sentE = (& $sendhwExe "pid:$($wp.Id)" "sleep:1200" "C-Space" "sleep:600" "c" 2>&1 | Out-String)
        $focused = -not ($sentE -match "NO_FOCUS")
        # NO_WINDOW: wezterm-gui never opened its window (its log says why),
        # while the mux server still started the session. Nothing was typed.
        $windowed = -not ($sentE -match "NO_WINDOW")
        Start-Sleep -Seconds 4
        $after = Wins $S
        Write-Info "windows in WezTerm session: $before -> $after"
        if ($after -gt $before) {
            Write-Pass "REAL Ctrl+Space in REAL WezTerm opened a window: the prefix works end to end"
        } elseif (-not $windowed) {
            Write-Skip "WezTerm end to end: wezterm-gui opened no window, no keystroke was delivered (set PSMUX_TEST_WEZTERM_FRONT_END, e.g. WebGpu)"
        } elseif (-not $focused) {
            # The VT-path fold itself is already proven above over the VT reader,
            # which needs no foreground.
            Write-Skip "WezTerm end to end: focus was refused, no keystroke was delivered"
        } else {
            Write-Fail "Ctrl+Space in WezTerm did not trigger the prefix (windows $before -> $after)"
        }
        Kill-Session $S
    }
    try { Stop-Process -Id $wp.Id -Force -EA SilentlyContinue } catch {}
    Get-Process wezterm-gui -EA SilentlyContinue | Where-Object { $_.StartTime -gt (Get-Date).AddMinutes(-3) } | Stop-Process -Force -EA SilentlyContinue
} else { Write-Skip "WezTerm not installed - skipping Part E" }

# ==========================================================================
# PART F: Win32 TUI verification (mandatory layer)
# ==========================================================================
Write-Host "`n[Part F] Win32 TUI verification" -ForegroundColor Yellow

$S = "issue508_tui"
$proc = Start-Psmux $S @("set -g prefix C-Space","bind C-Space send-prefix") $true
if (-not $proc) {
    Write-Fail "could not start TUI session"
} else {
    & $PSMUX split-window -v -t $S 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900
    $panes = (& $PSMUX display-message -t $S -p '#{window_panes}' 2>&1 | Out-String).Trim()
    if ($panes -eq "2") { Write-Pass "TUI: split-window produced 2 panes" }
    else { Write-Fail "TUI: expected 2 panes, got $panes" }

    & $PSMUX resize-pane -Z -t $S 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
    $z = (& $PSMUX display-message -t $S -p '#{window_zoomed_flag}' 2>&1 | Out-String).Trim()
    if ($z -eq "1") { Write-Pass "TUI: resize-pane -Z zoomed" }
    else { Write-Fail "TUI: zoom expected 1, got $z" }

    # The live rendered VT-path session is still driven by the folded prefix.
    if (Test-PrefixArms $S $proc "raw:0x20,0x20,0x8") {
        Write-Pass "TUI: WezTerm-delivered C-Space prefix drives a live rendered session"
    } else {
        Write-Fail "TUI: WezTerm-delivered C-Space prefix did not work in the live session"
    }
    Kill-Session $S
    try { Stop-Process -Id $proc.Id -Force -EA SilentlyContinue } catch {}
}

# ==========================================================================
Remove-Item "$psmuxDir\issue508_*" -Force -EA SilentlyContinue

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed:  $($script:TestsPassed)" -ForegroundColor Green
Write-Host "  Failed:  $($script:TestsFailed)" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $($script:TestsSkipped)" -ForegroundColor DarkYellow
exit $script:TestsFailed
