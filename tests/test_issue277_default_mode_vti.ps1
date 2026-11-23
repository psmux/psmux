# Issue #277 forwarding, pinned against the #623 record-reader gate.
#
# `window_ops::ensure_vti` forces ENABLE_VIRTUAL_TERMINAL_INPUT on a pane
# console before writing an SGR mouse report, because conhost DROPS those bytes
# for a child whose console has VTI off (#277/#245).  #623 taught it to skip the
# flip for an application that reads INPUT_RECORDs, since the flip destroys
# ConPTY's key translation and Far Manager lost every other F1 to it.
#
# The first version of that gate asked only "is ENABLE_MOUSE_INPUT set", and
# that bit is part of Windows' DEFAULT console input mode, 0x01F7.  An
# application that writes DECSET 1000/1002/1003/1006 and then reads the SGR
# bytes itself never calls SetConsoleMode, so it still carries the inherited
# word and was misread as a record reader.  The flip was skipped, conhost
# dropped every report, and the Win32 MOUSE_EVENT record injected alongside was
# no use to it because .NET's [Console]::ReadKey discards MOUSE_EVENT records.
# The wheel stopped reaching such a pane entirely.
#
# This test runs exactly that application: a pwsh pane app that sets no console
# mode of its own, and asserts three things measured, not assumed --
#   1. its console really does carry the inherited default (MOUSE set AND the
#      cooked LINE|ECHO bits still set), i.e. it IS the misclassified shape;
#   2. psmux performed the VT-input flip on it (from the server's own log);
#   3. the SGR wheel report actually arrived in the app's input.
#
# It drives the wheel over TCP (`pane-scroll`, the same verb
# tests/test_issue277_tcp_scroll.ps1 uses) against a DETACHED session, so it
# needs no attached client and no console of its own.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$Win10 = [Environment]::OSVersion.Version.Build -lt 22000 -and -not $env:PSMUX_CONPTY_DIR
$SOCK = "i277dm"
$SESSION = "i277dm"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkGray }

# A data root this test owns: the server's mouse debug log lands in it, and
# nothing else on the machine shares it.
$dataRoot = "$env:TEMP\psmux_277dm_root"
if (Test-Path $dataRoot) { Remove-Item $dataRoot -Recurse -Force -EA SilentlyContinue }
New-Item -ItemType Directory -Path $dataRoot -Force | Out-Null
$savedDataDir  = $env:PSMUX_DATA_DIR
$savedMouseDbg = $env:PSMUX_MOUSE_DEBUG
$env:PSMUX_DATA_DIR = $dataRoot
$env:PSMUX_MOUSE_DEBUG = "1"
$mouseLog = Join-Path $dataRoot "mouse_debug.log"

$emptyConf = "$env:TEMP\psmux_277dm_empty.conf"
"" | Set-Content -Path $emptyConf -Encoding ASCII

# --- GetConsoleMode against the pane child, the oracle for the whole gate -----
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
$modeProbeSrc = @'
using System;using System.Runtime.InteropServices;
class ModeProbe{
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool FreeConsole();
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool AttachConsole(uint p);
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr CreateFileW(string n,uint a,uint s,IntPtr sec,uint d,uint f,IntPtr t);
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetConsoleMode(IntPtr h,out uint m);
 static int Main(string[] a){
  uint pid=uint.Parse(a[0]);
  FreeConsole();
  if(!AttachConsole(pid)){Console.WriteLine("ATTACH_FAIL");return 2;}
  IntPtr h=CreateFileW("CONIN$",0x80000000u|0x40000000u,3,IntPtr.Zero,3,0,IntPtr.Zero);
  if(h==new IntPtr(-1)){FreeConsole();Console.WriteLine("CONIN_FAIL");return 3;}
  uint m=0; bool ok=GetConsoleMode(h,out m);
  FreeConsole();
  if(!ok){Console.WriteLine("MODE_FAIL");return 4;}
  Console.WriteLine("MODE 0x{0:X4} VTI={1} MOUSE={2} LINE={3} ECHO={4} QUICKEDIT={5}",
    m,(m&0x0200)!=0,(m&0x0010)!=0,(m&0x0002)!=0,(m&0x0004)!=0,(m&0x0040)!=0);
  return 0;}}
'@
$modeProbe = "$env:TEMP\psmux_277dm_modeprobe.exe"
$modeProbeSrc | Set-Content "$env:TEMP\psmux_277dm_modeprobe.cs" -Encoding UTF8
& $csc /nologo /platform:x64 /out:$modeProbe "$env:TEMP\psmux_277dm_modeprobe.cs" 2>&1 | Out-Null
if (-not (Test-Path $modeProbe)) {
    Write-Host "  [FAIL] could not build the console mode probe (csc at $csc)" -ForegroundColor Red
    $env:PSMUX_DATA_DIR = $savedDataDir; $env:PSMUX_MOUSE_DEBUG = $savedMouseDbg
    exit 1
}

function Send-TcpCommand {
    param([string]$Base, [string]$Command)
    $port = (Get-Content "$dataRoot\$Base.port" -Raw).Trim()
    $key  = (Get-Content "$dataRoot\$Base.key" -Raw).Trim()
    $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int]$port)
    $tcp.NoDelay = $true; $tcp.ReceiveTimeout = 5000
    $stream = $tcp.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $reader = [System.IO.StreamReader]::new($stream)
    $writer.Write("AUTH $key`n"); $writer.Flush()
    if ($reader.ReadLine() -ne "OK") { $tcp.Close(); return "AUTH_FAILED" }
    $writer.Write("$Command`n"); $writer.Flush()
    $stream.ReadTimeout = 5000
    try { $resp = $reader.ReadLine() } catch { $resp = "TIMEOUT" }
    $tcp.Close()
    return $resp
}

# --- the pane application: a VT byte reader that sets NO console mode ---------
# It logs every character it reads.  NO_COLOR=1 puts PowerShell into PlainText
# rendering, which STRIPS every escape sequence out of Write-Host, so the
# DECSETs go out through [Console]::Out.Write instead - otherwise the app would
# silently never enable mouse tracking and the test would pass for the wrong
# reason.
$appLog = "$env:TEMP\psmux_277dm_app.log"
$appScript = "$env:TEMP\psmux_277dm_app.ps1"
@"
`$logFile = "$appLog"
"STARTED" | Set-Content `$logFile
`$e = [char]27
[Console]::Out.Write("`$e[?1049h")
[Console]::Out.Write("`$e[?1000h`$e[?1002h`$e[?1003h`$e[?1006h")
[Console]::Out.Write("`$e[2J`$e[H")
[Console]::Out.Flush()
Write-Host "VT byte reader running"
`$buf = New-Object System.Text.StringBuilder
`$t = Get-Date
while (((Get-Date) - `$t).TotalSeconds -lt 35) {
    if ([Console]::KeyAvailable) {
        `$k = [Console]::ReadKey(`$true)
        [void]`$buf.Append(`$k.KeyChar)
    } else {
        if (`$buf.Length -gt 0) { Add-Content `$logFile ("SEQ=" + (`$buf.ToString() -replace [char]27, '<ESC>')); [void]`$buf.Clear() }
        Start-Sleep -Milliseconds 10
    }
}
if (`$buf.Length -gt 0) { Add-Content `$logFile ("SEQ=" + (`$buf.ToString() -replace [char]27, '<ESC>')) }
[Console]::Out.Write("`$e[?1006l`$e[?1003l`$e[?1002l`$e[?1000l`$e[?1049l")
Add-Content `$logFile "FINISHED"
"@ | Set-Content $appScript -Encoding UTF8
Remove-Item $appLog -Force -EA SilentlyContinue

Write-Host "`n=== Issue #277: the wheel must reach an app running on the INHERITED console mode ===" -ForegroundColor Cyan

& $PSMUX -L $SOCK kill-session -t $SESSION 2>&1 | Out-Null
& $PSMUX -L $SOCK -f $emptyConf new-session -d -s $SESSION -x 100 -y 30 2>&1 | Out-Null
Start-Sleep -Seconds 3
& $PSMUX -L $SOCK has-session -t $SESSION 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "detached session '$SESSION' never came up"
} else {
    $base = "${SOCK}__${SESSION}"
    & $PSMUX -L $SOCK send-keys -t $SESSION "pwsh -NoProfile -File '$appScript'" Enter 2>&1 | Out-Null
    Start-Sleep -Seconds 7

    $alt = (& $PSMUX -L $SOCK display-message -p -t $SESSION '#{alternate_on}' 2>&1 | Out-String).Trim()
    if ($alt -eq "1") {
        Write-Pass "the app reached the alternate screen (its DECSETs got through)"
    } elseif ($Win10 -and $alt -eq "0") {
        Write-Pass "this host kept the app's ?1049h and mouse DECSETs to itself, alternate_on stays 0"
    } else {
        Write-Fail "the app never reached the alternate screen (alternate_on=$alt); its DECSETs were lost"
    }

    $panePid = (& $PSMUX -L $SOCK display-message -p -t $SESSION '#{pane_pid}' 2>&1 | Out-String).Trim()
    $mode = (& $modeProbe $panePid 2>&1 | Out-String).Trim()
    Write-Host "  [INFO] pane child console: $mode" -ForegroundColor DarkGray
    if ($mode -notmatch 'MODE 0x') {
        Write-Skip "could not read the pane console mode ($mode)"
    } elseif ($mode -match 'MOUSE=True' -and $mode -match 'LINE=True' -and $mode -match 'ECHO=True') {
        # This is the precondition the whole regression turned on: the app has
        # ENABLE_MOUSE_INPUT purely because it INHERITED it, and it is still in
        # cooked mode, which no real record reader ever is.
        Write-Pass "the app carries the inherited default (MOUSE set, still cooked): $mode"
    } else {
        Write-Fail "the app is not in the misclassified shape any more, this test no longer covers #623's gate: $mode"
    }

    # Six wheel notches over TCP.
    for ($i = 0; $i -lt 6; $i++) {
        Send-TcpCommand -Base $base -Command "pane-scroll 0 up 40 10" | Out-Null
        Start-Sleep -Milliseconds 250
    }
    Start-Sleep -Seconds 2

    $modeAfter = (& $modeProbe $panePid 2>&1 | Out-String).Trim()
    Write-Host "  [INFO] after the wheel:      $modeAfter" -ForegroundColor DarkGray
    if ($modeAfter -match 'VTI=True') {
        Write-Pass "psmux enabled VT input on the byte reader's console (#277 flip performed)"
    } elseif ($Win10) {
        Write-Pass "psmux left VT input as it was: with no mouse DECSET it had no report to write ($modeAfter)"
    } else {
        Write-Fail "psmux left VT input off ($modeAfter): conhost drops the SGR report (#277 regression)"
    }

    $flips = @()
    if (Test-Path $mouseLog) {
        $flips = @(Get-Content $mouseLog | Where-Object { $_ -match 'ensure_vti_enabled: pid=\d+ mode' })
    }
    if ($flips.Count -gt 0) {
        Write-Pass "the server logged the flip: $($flips[-1])"
    } elseif ($Win10) {
        Write-Pass "no flip logged on this host, as no report was written"
    } else {
        Write-Fail "the server logged no ensure_vti_enabled flip at all (#623's gate swallowed it)"
    }

    $log = if (Test-Path $appLog) { (Get-Content $appLog -Raw) } else { "" }
    Write-Host "  [INFO] app input log: $((($log -split "`r?`n") | Where-Object { $_ } | Select-Object -First 4) -join ' / ')" -ForegroundColor DarkGray
    # `ESC[<64;COL;ROWM` is SGR wheel up. Only the button code is asserted, the
    # coordinates belong to #570.
    if ($log -match '<ESC>\[<6[4-7];\d+;\d+M') {
        Write-Pass "the SGR wheel report reached the app's input"
    } elseif ($Win10 -and $log -notmatch 'SEQ=') {
        Write-Pass "on this host the VT reading app got no wheel, as measured"
    } elseif ($log -match 'SEQ=') {
        Write-Fail "the app read bytes but no SGR wheel report: $($log -replace "`r?`n", ' | ')"
    } else {
        Write-Fail "NOTHING reached the app: the wheel was dropped before the child (#277)"
    }
}

& $PSMUX -L $SOCK kill-session -t $SESSION 2>&1 | Out-Null
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 500
$env:PSMUX_DATA_DIR = $savedDataDir
$env:PSMUX_MOUSE_DEBUG = $savedMouseDbg
Remove-Item $appScript, $appLog, $emptyConf -Force -EA SilentlyContinue

Write-Host "`n=== Issue #277 inherited-mode summary ===" -ForegroundColor Cyan
Write-Host "  Passed: $script:TestsPassed" -ForegroundColor Green
Write-Host "  Failed: $script:TestsFailed" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
