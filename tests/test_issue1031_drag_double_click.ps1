# Issue #1031: with `mouse-drag-enter-copy-mode on`, a left drag from the live
# view entered copy mode, then about 300 ms in became a double click: the
# selection turned into the word under the press, was copied, and copy mode
# exited with the button still down.
#
# The client replays the press after `copy-enter` so the selection anchors at
# the press cell, and the server's click timer counted that replay as the
# second click. The replay now travels as `pane-mouse ... R`, which the click
# timer skips (tmux: copy-mode -M never sees a second button down).
#
# Layers:
#   A  real attached client, MOUSE_EVENT records injected into its console:
#      press, drag over several lines, hold the button 1.5 s, release
#   B  real double click and real triple click through the same client still
#      copy the word and the line
#   C  the control verbs straight at the server: press, copy-enter, R replay,
#      drag, wait past the click window
#
# Run:  pwsh -NoProfile -ExecutionPolicy Bypass -File tests\test_issue1031_drag_double_click.ps1
#       PSMUX_EXE selects the binary under test.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_EXE) { $env:PSMUX_EXE } else { (Get-Command psmux -EA Stop).Source }
$NS = "i1031d" + (Get-Random -Maximum 99999)
$S = "s1031"
$TMP = Join-Path $env:TEMP ("i1031_suite_" + $NS)
$DATA = Join-Path $TMP "data"
New-Item -ItemType Directory -Force -Path $DATA | Out-Null
$env:PSMUX_DATA_DIR = $DATA
$env:PSMUX_NO_WARM = "1"

$script:Pass = 0; $script:Fail = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan
Write-Host ("version: " + (& $PSMUX -V)) -ForegroundColor Cyan

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$dragInj = Join-Path $TMP "drag.exe"
$multiInj = Join-Path $TMP "multiclick.exe"
& $csc /nologo /optimize /out:$dragInj (Join-Path $PSScriptRoot "mouse_drag_hold_injector.cs") 2>&1 | Out-Null
$multiSrc = Join-Path $TMP "multiclick.cs"
@'
using System;
using System.Runtime.InteropServices;
using System.Threading;
// multiclick.exe <pid> <x> <y> <count> <gapMs>: count left clicks at one cell
class MultiClick
{
    [DllImport("kernel32.dll")] static extern bool FreeConsole();
    [DllImport("kernel32.dll")] static extern bool AttachConsole(uint pid);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern IntPtr CreateFileW(string n, uint a, uint s, IntPtr sec, uint d, uint f, IntPtr t);
    [DllImport("kernel32.dll")] static extern bool WriteConsoleInput(IntPtr h, INPUT_RECORD[] b, uint l, out uint w);
    [StructLayout(LayoutKind.Sequential)] struct COORD { public short X; public short Y; }
    [StructLayout(LayoutKind.Sequential)] struct MER { public COORD P; public uint B; public uint C; public uint F; }
    [StructLayout(LayoutKind.Explicit, Size = 20)] struct INPUT_RECORD { [FieldOffset(0)] public ushort T; [FieldOffset(4)] public MER M; }
    static INPUT_RECORD R(short x, short y, uint b, uint f) { var r = new INPUT_RECORD(); r.T = 2; r.M.P.X = x; r.M.P.Y = y; r.M.B = b; r.M.F = f; return r; }
    static int Main(string[] a)
    {
        uint pid = uint.Parse(a[0]); short x = short.Parse(a[1]); short y = short.Parse(a[2]);
        int n = int.Parse(a[3]); int gap = int.Parse(a[4]);
        FreeConsole(); if (!AttachConsole(pid)) return 2;
        IntPtr h = CreateFileW("CONIN$", 0xC0000000, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
        uint w;
        for (int i = 0; i < n; i++)
        {
            WriteConsoleInput(h, new[] { R(x, y, 1, i == 0 ? 0u : 2u) }, 1, out w);
            Thread.Sleep(30);
            WriteConsoleInput(h, new[] { R(x, y, 0, 0) }, 1, out w);
            Thread.Sleep(gap);
        }
        return 0;
    }
}
'@ | Set-Content -Path $multiSrc -Encoding ASCII
& $csc /nologo /optimize /out:$multiInj $multiSrc 2>&1 | Out-Null
foreach ($x in @($dragInj, $multiInj)) { if (-not (Test-Path $x)) { Write-Host "FATAL: could not compile $x" -ForegroundColor Red; exit 1 } }

$conf = Join-Path $TMP "i1031.conf"
"set -g mouse on`nset -g mouse-drag-enter-copy-mode on`nset -g history-limit 5000`n" | Set-Content $conf -Encoding ASCII
$launch = Join-Path $TMP "launch.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set PSMUX_NO_WARM=1
set PSMUX_DATA_DIR=$DATA
"$PSMUX" -L $NS -f "$conf" new-session -s $S -x 120 -y 30
"@ | Set-Content $launch -Encoding ASCII

function Fmt([string]$f) { (& $PSMUX -L $NS display-message -t $S -p $f 2>&1 | Out-String).Trim() }
function Buffers { (& $PSMUX -L $NS list-buffers -F '#{buffer_name}' 2>&1 | Out-String).Trim() }
function TopBuf { (& $PSMUX -L $NS show-buffer 2>&1 | Out-String) }
$script:Launcher = $null

function Start-Client {
    $script:Launcher = Start-Process -FilePath $launch -PassThru
    for ($i = 0; $i -lt 80; $i++) { & $PSMUX -L $NS has-session -t $S 2>$null; if ($LASTEXITCODE -eq 0) { break }; Start-Sleep -Milliseconds 250 }
    Start-Sleep -Seconds 3
    $cli = Get-CimInstance Win32_Process -Filter "Name='$([IO.Path]::GetFileName($PSMUX))'" |
        Where-Object { $_.CommandLine -match "-L $NS " -and $_.CommandLine -match "new-session" } | Select-Object -First 1
    if (-not $cli) { return $null }
    & $PSMUX -L $NS send-keys -t $S 'cls; 1..40 | % { "line $_ some words to select across" }' Enter
    Start-Sleep -Seconds 2
    return [int]$cli.ProcessId
}
function Stop-Client {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    if ($script:Launcher -and -not $script:Launcher.HasExited) { Stop-Process -Id $script:Launcher.Id -Force -EA SilentlyContinue }
}
function Reset-State {
    & $PSMUX -L $NS send-keys -t $S -X cancel 2>&1 | Out-Null
    foreach ($b in ((Buffers) -split "`r?`n")) { if ($b) { & $PSMUX -L $NS delete-buffer -b $b 2>&1 | Out-Null } }
    Start-Sleep -Milliseconds 700
}

try {
    $cliPid = Start-Client
    if (-not $cliPid) { Write-Fail "attached client did not start"; throw "no client" }
    Write-Info "client pid $cliPid, namespace $NS"

    # ---------------- A: drag from the live view ----------------
    Write-Host "`n[A] drag from the live view, button held 1.5 s" -ForegroundColor Cyan
    Reset-State
    $inj = Start-Process -FilePath $dragInj -ArgumentList $cliPid,10,8,30,14,6,30,1500,0 -PassThru -WindowStyle Hidden
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $samples = @()
    while (-not $inj.HasExited -and $sw.ElapsedMilliseconds -lt 8000) {
        $samples += [pscustomobject]@{ T = $sw.ElapsedMilliseconds; S = (Fmt '#{pane_in_mode}|#{selection_start_x},#{selection_start_y}-#{selection_end_x},#{selection_end_y}'); B = (Buffers) }
        Start-Sleep -Milliseconds 80
    }
    $held = @($samples | Where-Object { $_.T -ge 700 -and $_.T -le 1500 })
    foreach ($s in $samples) { Write-Info ("t={0}ms mode|sel={1} buffers=[{2}]" -f $s.T, $s.S, ($s.B -replace "`r?`n", ",")) }
    if ($held.Count -gt 0 -and @($held | Where-Object { $_.S -notlike '1|*' }).Count -eq 0) { Write-Pass "copy mode stays on while the button is held (700..1500 ms)" }
    else { Write-Fail "copy mode left while the button was still held" }
    if (@($held | Where-Object { $_.S -like '*|10,8-*' }).Count -eq $held.Count -and $held.Count -gt 0) { Write-Pass "selection keeps its anchor at the press cell (10,8)" }
    else { Write-Fail "selection anchor moved off the press cell during the hold" }
    if (@($samples | Where-Object { $_.T -le 1500 -and $_.B }).Count -eq 0) { Write-Pass "nothing copied while the button is down" }
    else { Write-Fail "a buffer was created during the drag" }
    Start-Sleep -Milliseconds 900
    $buf = TopBuf
    $lines = @($buf -split "`r?`n" | Where-Object { $_ -match 'line \d+' })
    if ($lines.Count -ge 5) { Write-Pass "release copies the dragged rows ($($lines.Count) lines)" }
    else { Write-Fail "release copy is not the dragged rows: [$buf]" }
    if ((Fmt '#{pane_in_mode}') -eq '0') { Write-Pass "copy mode exits after the release" } else { Write-Fail "still in copy mode after the release" }

    # ---------------- B: real double and triple clicks ----------------
    Write-Host "`n[B] real double and triple clicks still work" -ForegroundColor Cyan
    Reset-State
    $inj = Start-Process -FilePath $multiInj -ArgumentList $cliPid,12,10,2,80 -PassThru -WindowStyle Hidden
    $inj.WaitForExit(); Start-Sleep -Milliseconds 1200
    $buf = (TopBuf).Trim()
    if ($buf -eq 'some') { Write-Pass "double click copies the word: '$buf'" } else { Write-Fail "double click copied '$buf', expected 'some'" }
    Reset-State
    $inj = Start-Process -FilePath $multiInj -ArgumentList $cliPid,12,10,3,80 -PassThru -WindowStyle Hidden
    $inj.WaitForExit(); Start-Sleep -Milliseconds 1200
    $buf = (TopBuf).Trim()
    if ($buf -match '^line \d+ some words to select across$') { Write-Pass "triple click copies the line: '$buf'" } else { Write-Fail "triple click copied '$buf', expected a whole line" }

    # ---------------- C: control verbs at the server ----------------
    Write-Host "`n[C] pane-mouse verbs: press, copy-enter, R replay, drag" -ForegroundColor Cyan
    Reset-State
    $pid1 = (Fmt '#{pane_id}').TrimStart('%')
    & $PSMUX -L $NS pane-mouse $pid1 0 4 5 M 2>&1 | Out-Null
    & $PSMUX -L $NS copy-mode -t $S 2>&1 | Out-Null
    & $PSMUX -L $NS pane-mouse $pid1 0 4 5 R 2>&1 | Out-Null
    & $PSMUX -L $NS pane-mouse $pid1 32 20 8 M 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900
    $st = Fmt '#{pane_in_mode}|#{selection_start_x},#{selection_start_y}'
    Write-Info "900 ms after the drag: $st, buffers=[$(Buffers)]"
    if ($st -eq '1|4,5') { Write-Pass "R replay keeps the drag selection past the click window" } else { Write-Fail "drag selection changed past the click window: $st" }
    if (-not (Buffers)) { Write-Pass "R replay copies nothing before the release" } else { Write-Fail "something was copied before the release" }
    & $PSMUX -L $NS pane-mouse $pid1 0 20 8 m 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $buf = TopBuf
    if (@($buf -split "`r?`n" | Where-Object { $_ -match 'line \d+' }).Count -ge 3) { Write-Pass "release copies the dragged rows" } else { Write-Fail "release copied [$buf]" }
}
finally {
    Stop-Client
    Remove-Item -Recurse -Force $TMP -EA SilentlyContinue
}

Write-Host "`nResults: $($script:Pass) passed, $($script:Fail) failed" -ForegroundColor $(if ($script:Fail) { 'Red' } else { 'Green' })
exit $script:Fail
