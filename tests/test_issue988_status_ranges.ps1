# Issue #988: status line click ranges.
#
# tmux 3.8 keeps every range type of status-format (window|N, left, right,
# session|$id, pane|%id, user|X, control|N), picks the mouse key from the
# range under the click (MouseDown1Status, MouseDown1StatusLeft,
# MouseDown1StatusRight, MouseDown1ControlN, or MouseDown1StatusDefault
# outside every range), and fills #{mouse_status_range},
# #{mouse_status_line}, #{mouse_x} and #{mouse_y} for the binding.
#
# The issue's config is loaded into an isolated namespace.  A real psmux
# CLIENT is hosted under a pseudoconsole (conptyfeed610.cs) and SGR mouse
# sequences are written into its input, the way a terminal reports a click.
# No keystroke reaches the desktop.  Every process this test starts is
# stopped by its PID.
#
# Usage: tests\test_issue988_status_ranges.ps1 [-Exe path\to\psmux.exe]

param([string]$Exe = $env:PSMUX_EXE)

$ErrorActionPreference = "Continue"
Remove-Item Env:SHELL -EA SilentlyContinue
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

if (-not $Exe) { $Exe = (Get-Command psmux -EA Stop).Source }
$PSMUX = (Resolve-Path $Exe).Path
$TESTS = $PSScriptRoot
$SOCK = "t988_" + [guid]::NewGuid().ToString("N").Substring(0, 8)
$OUT = Join-Path $env:TEMP "psmux_$SOCK"
New-Item -ItemType Directory -Force $OUT | Out-Null
$before = (& $PSMUX ls 2>&1 | Out-String).Trim()
$oldDataDir = $env:PSMUX_DATA_DIR
$env:PSMUX_DATA_DIR = Join-Path $OUT "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$feed = "$OUT\conptyfeed988.exe"
& $csc /nologo /out:$feed /platform:x64 "$TESTS\conptyfeed610.cs" 2>&1 | Out-Null
if (-not (Test-Path $feed)) { Write-Host "helper build failed" -ForegroundColor Red; exit 1 }

function P { & $PSMUX -L $SOCK @args 2>&1 | Out-String }

$conf = "$OUT\ranges.conf"
@'
set -g mouse on
set -g status-position top
set -g status-interval 1
set -g status-format[0] '#[range=window|#{window_index}]WINDOW#[norange] #[range=user|alpha]USERA#[norange] #[range=user|beta]USERB#[norange] #[range=left]LEFT#[norange] #[range=right]RIGHT#[norange] #[range=session|#{session_id}]SESSION#[norange] #[range=pane|#{pane_id}]PANE#[norange] #[range=control|3]CONTROL#[norange] NORANGE'
bind -n MouseDown1Status set -gF @hit 'MouseDown1Status range=[#{mouse_status_range}] line=[#{mouse_status_line}] x=[#{mouse_x}] y=[#{mouse_y}]'
bind -n MouseDown1StatusLeft set -gF @hit 'MouseDown1StatusLeft range=[#{mouse_status_range}] line=[#{mouse_status_line}] x=[#{mouse_x}] y=[#{mouse_y}]'
bind -n MouseDown1StatusRight set -gF @hit 'MouseDown1StatusRight range=[#{mouse_status_range}] line=[#{mouse_status_line}] x=[#{mouse_x}] y=[#{mouse_y}]'
bind -n MouseDown1StatusDefault set -gF @hit 'MouseDown1StatusDefault range=[#{mouse_status_range}] line=[#{mouse_status_line}] x=[#{mouse_x}] y=[#{mouse_y}]'
bind -n MouseDown1Control3 set -gF @hit 'MouseDown1Control3 range=[#{mouse_status_range}] line=[#{mouse_status_line}] x=[#{mouse_x}] y=[#{mouse_y}]'
bind -n MouseDown1Pane set -gF @hit 'MouseDown1Pane x=[#{mouse_x}] y=[#{mouse_y}]'
'@ | Set-Content -Path $conf -Encoding ASCII

$started = @()

Write-Host "Binary: $PSMUX" -ForegroundColor Cyan
Write-Host "Namespace: $SOCK"

& $PSMUX -L $SOCK -f $conf new-session -d -s t -x 120 -y 29 2>&1 | Out-Null
Start-Sleep -Seconds 2

$ctrl = "$OUT\client.ctrl"
$cap = "$OUT\client.bin"
Set-Content $ctrl "" -NoNewline
$client = Start-Process -FilePath $feed `
    -ArgumentList "`"$ctrl`"", "`"$cap`"", "`"$PSMUX`" -L $SOCK attach-session -t t" `
    -PassThru -WindowStyle Hidden
$started += $client.Id

# Wait for the client to draw the status line.
for ($i = 0; $i -lt 40; $i++) {
    Start-Sleep -Milliseconds 250
    if ((Test-Path $cap) -and ((Get-Item $cap).Length -gt 200)) {
        $fs = [IO.File]::Open($cap, 'Open', 'Read', 'ReadWrite')
        $buf = New-Object byte[] $fs.Length; $null = $fs.Read($buf, 0, $buf.Length); $fs.Close()
        if ([Text.Encoding]::ASCII.GetString($buf).Contains("NORANGE")) { break }
    }
}
Start-Sleep -Milliseconds 800

function Hex($s) { ([Text.Encoding]::ASCII.GetBytes($s) | ForEach-Object { '{0:x2}' -f $_ }) -join ' ' }

# Click at 0 based column $x, row $y: SGR press then release (1 based).
function Click($x, $y) {
    P set -g "@hit" "none" | Out-Null
    $e = [char]27
    $press = "$e[<0;$($x + 1);$($y + 1)M"
    $release = "$e[<0;$($x + 1);$($y + 1)m"
    Add-Content $ctrl ("HEX " + (Hex $press))
    Start-Sleep -Milliseconds 120
    Add-Content $ctrl ("HEX " + (Hex $release))
    for ($k = 0; $k -lt 20; $k++) {
        Start-Sleep -Milliseconds 150
        $v = (P show -gv "@hit").Trim()
        if ($v -ne "none") { return $v }
    }
    return (P show -gv "@hit").Trim()
}

Write-Host "`n=== Group 1: the issue's table (tmux 3.8 output) ===" -ForegroundColor Cyan
$expect = [ordered]@{
    "WINDOW"  = @(1,  "MouseDown1Status range=[window] line=[0] x=[1] y=[0]")
    "USERA"   = @(8,  "MouseDown1Status range=[alpha] line=[0] x=[8] y=[0]")
    "USERB"   = @(14, "MouseDown1Status range=[beta] line=[0] x=[14] y=[0]")
    "LEFT"    = @(20, "MouseDown1StatusLeft range=[left] line=[0] x=[20] y=[0]")
    "RIGHT"   = @(25, "MouseDown1StatusRight range=[right] line=[0] x=[25] y=[0]")
    "SESSION" = @(31, "MouseDown1Status range=[session] line=[0] x=[31] y=[0]")
    "PANE"    = @(39, "MouseDown1Status range=[pane] line=[0] x=[39] y=[0]")
    "CONTROL" = @(44, "MouseDown1Control3 range=[control] line=[0] x=[44] y=[0]")
    "NORANGE" = @(52, "MouseDown1StatusDefault range=[] line=[0] x=[52] y=[0]")
}
foreach ($word in $expect.Keys) {
    $x = $expect[$word][0]; $want = $expect[$word][1]
    $got = Click $x 0
    if ($got -eq $want) { Write-Pass "$word -> $got" }
    else { Write-Fail "$word -> got [$got] want [$want]" }
}

Write-Host "`n=== Group 2: a pane click reports mouse_x / mouse_y ===" -ForegroundColor Cyan
# Column 20, row 10 (1 based) of the terminal; the status line is row 1, so
# the pane cell is x=19, y=8.
$got = Click 19 9
$want = "MouseDown1Pane x=[19] y=[8]"
if ($got -eq $want) { Write-Pass "pane click -> $got" } else { Write-Fail "pane click -> got [$got] want [$want]" }

# Any SGR mouse report at 0 based column $x, row $y: button code $b,
# `M` press or `m` release.
function Sgr($b, $x, $y, $kind) { "$([char]27)[<$b;$($x + 1);$($y + 1)$kind" }
function Feed([string[]]$seqs) { Add-Content $ctrl ("HEX " + (($seqs | ForEach-Object { Hex $_ }) -join ' ')) }

# Wait for option $opt to leave "none"; "none" if it never does.
function Wait-Opt($opt, $tries = 14) {
    for ($k = 0; $k -lt $tries; $k++) {
        Start-Sleep -Milliseconds 150
        $v = (P show -gv $opt).Trim()
        if ($v -ne "none") { return $v }
    }
    return "none"
}
function Check($label, $got, $want) {
    if ($got -eq $want) { Write-Pass "$label -> [$got]" } else { Write-Fail "$label -> got [$got] want [$want]" }
}
function Redraw { Start-Sleep -Milliseconds 1500 }

Write-Host "`n=== Group 3: the {mouse} target follows the range ===" -ForegroundColor Cyan
P new-window -d | Out-Null
P select-window -t t:0 | Out-Null
P split-window -t t:0 | Out-Null
$panes = @((P list-panes -t t:0 -F '#{pane_id}') -split "`r?`n" | Where-Object { $_ })
$pa = $panes[0]; $pb = $panes[1]
P set -g status-format[0] "#[range=window|1]W1#[norange] #[range=pane|$pa]PA#[norange] #[range=session|`$99]S99#[norange] #[range=window|7]W7#[norange] END" | Out-Null
P bind -n MouseDown1Status 'select-window -t = ; set -gF @hit "sel=#{window_index}"' | Out-Null
Redraw
$got = Click 0 0
Check "window range: select-window -t = picks window 1" $got "sel=1"
P select-window -t t:0 | Out-Null
P select-pane -t $pb | Out-Null
P bind -n MouseDown1Status 'select-pane -t = ; set -gF @hit "pane=#{pane_id}"' | Out-Null
Redraw
$got = Click 4 0
Check "pane range: select-pane -t = picks $pa" $got "pane=$pa"
$got = Click 8 0
Check "range=session|`$99 (no such session) gives no key" $got "none"
$got = Click 10 0
Check "range=window|7 (no such window) gives no key" $got "none"
# The built in MouseDown1Status (tmux's switch-client -t =) on a pane range.
P bind -n MouseDown1Status 'switch-client -t =' | Out-Null
P select-pane -t $pb | Out-Null
Redraw
Click 4 0 | Out-Null
Start-Sleep -Milliseconds 600
Check "built in MouseDown1Status on a pane range selects it" (P display -p '#{pane_id}').Trim() $pa

Write-Host "`n=== Group 4: psmux's own status layout keeps every click and wheel ===" -ForegroundColor Cyan
foreach ($k in "MouseDown1StatusLeft", "MouseDown1StatusRight", "MouseDown1StatusDefault", "MouseDown1Control3") {
    P unbind -n $k | Out-Null
}
P set -gu status-format | Out-Null
P set -g status-left "[LEFT] " | Out-Null
P set -g status-right "RIGHTSIDE" | Out-Null
P bind -n MouseDown1Status 'set -gF @hit "S range=[#{mouse_status_range}] x=[#{mouse_x}]"' | Out-Null
Redraw
Check "status-left, MouseDown1StatusLeft unbound: MouseDown1Status runs" (Click 1 0) "S range=[left] x=[1]"
Check "status-right: MouseDown1Status runs" (Click 115 0) "S range=[right] x=[115]"
Check "empty middle: MouseDown1Status runs" (Click 60 0) "S range=[] x=[60]"
P bind -n MouseDown1StatusLeft 'set -gF @hit "L range=[#{mouse_status_range}]"' | Out-Null
Redraw
Check "status-left with MouseDown1StatusLeft bound: that key" (Click 1 0) "L range=[left]"
P unbind -n MouseDown1StatusLeft | Out-Null
P bind -n MouseDown1Status 'switch-client -t =' | Out-Null
P select-window -t t:0 | Out-Null
Redraw
Feed @((Sgr 65 60 0 'M'))
Start-Sleep -Milliseconds 900
Check "wheel down on empty status space: next-window as before" (P display -p '#{window_index}').Trim() "1"
Feed @((Sgr 64 1 0 'M'))
Start-Sleep -Milliseconds 900
Check "wheel up on status-left: previous-window as before" (P display -p '#{window_index}').Trim() "0"

Write-Host "`n=== Group 5: with status-format, tmux's keys exactly ===" -ForegroundColor Cyan
P set -g status-format[0] '#[range=window|0]WIN#[norange] #[range=left]LEFT#[norange] #[range=right]RIGHT#[norange] NORANGE' | Out-Null
P select-window -t t:0 | Out-Null
P bind -n MouseDown1Status 'set -gF @hit "S"' | Out-Null
Redraw
Check "NORANGE with only MouseDown1Status bound: no key (tmux StatusDefault)" (Click 17 0) "none"
Feed @((Sgr 65 17 0 'M'))
Start-Sleep -Milliseconds 900
Check "wheel down on NORANGE, WheelDownStatusDefault unbound: nothing" (P display -p '#{window_index}').Trim() "0"
P bind -n WheelDownStatusDefault 'set -gF @hit "WD range=[#{mouse_status_range}]"' | Out-Null
Redraw
P set -g "@hit" "none" | Out-Null
Feed @((Sgr 65 17 0 'M'))
Check "WheelDownStatusDefault bound: runs" (Wait-Opt "@hit") "WD range=[]"
Feed @((Sgr 65 1 0 'M'))
Start-Sleep -Milliseconds 900
Check "wheel down on the window range: WheelDownStatus (next-window)" (P display -p '#{window_index}').Trim() "1"
P select-window -t t:0 | Out-Null

Write-Host "`n=== Group 6: other buttons, release and multi clicks ===" -ForegroundColor Cyan
P bind -n MouseDown3StatusRight 'set -gF @hit "R3 range=[#{mouse_status_range}]"' | Out-Null
P bind -n MouseUp1StatusLeft 'set -gF @up "U1 range=[#{mouse_status_range}]"' | Out-Null
P bind -n DoubleClick1StatusLeft 'set -gF @dbl "D1 range=[#{mouse_status_range}]"' | Out-Null
P bind -n M-MouseDown1StatusRight 'set -gF @hit "M-R1"' | Out-Null
Redraw
P set -g "@hit" "none" | Out-Null
Feed @((Sgr 2 11 0 'M'), (Sgr 2 11 0 'm'))
Check "right press on RIGHT: MouseDown3StatusRight" (Wait-Opt "@hit") "R3 range=[right]"
P set -g "@up" "none" | Out-Null
Feed @((Sgr 0 5 0 'M'), (Sgr 0 5 0 'm'))
Check "release on LEFT: MouseUp1StatusLeft" (Wait-Opt "@up") "U1 range=[left]"
P bind -n SecondClick1StatusLeft 'set -gF @sec "S1 range=[#{mouse_status_range}]"' | Out-Null
Redraw
P set -g "@dbl" "none" | Out-Null
P set -g "@sec" "none" | Out-Null
Feed @((Sgr 0 5 0 'M'), (Sgr 0 5 0 'm'), (Sgr 0 5 0 'M'), (Sgr 0 5 0 'm'))
Check "second press on LEFT: SecondClick1StatusLeft" (Wait-Opt "@sec") "S1 range=[left]"
Check "double click on LEFT: DoubleClick1StatusLeft" (Wait-Opt "@dbl") "D1 range=[left]"
P set -g "@hit" "none" | Out-Null
Feed @((Sgr 8 11 0 'M'), (Sgr 8 11 0 'm'))
Check "Alt press on RIGHT: M-MouseDown1StatusRight" (Wait-Opt "@hit") "M-R1"

# Clean up only this namespace.
foreach ($id in $started) { try { Stop-Process -Id $id -Force -EA Stop } catch {} }
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
$env:PSMUX_DATA_DIR = $oldDataDir
$after = (& $PSMUX ls 2>&1 | Out-String).Trim()
if ($before -ne $after) { Write-Host "WARNING default namespace changed: [$before] -> [$after]" -ForegroundColor Yellow }

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
