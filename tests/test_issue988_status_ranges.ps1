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

Write-Host "`n=== Group 3: the {mouse} target follows the range ===" -ForegroundColor Cyan
P new-window -d | Out-Null
P bind -n MouseDown1Status 'set -gF @hit "#{session_name}:#{window_index}"' | Out-Null
P set -g status-format[0] '#[range=window|1]W1#[norange] #[range=window|0]W0#[norange]' | Out-Null
Start-Sleep -Milliseconds 1500
P select-window -t t:0 | Out-Null
Start-Sleep -Milliseconds 500
P bind -n MouseDown1Status 'select-window -t = ; set -gF @hit "sel=#{window_index}"' | Out-Null
$got = Click 0 0
if ($got -eq "sel=1") { Write-Pass "window range: select-window -t = picked window 1" }
else { Write-Fail "window range: got [$got] want [sel=1]" }

# Clean up only this namespace.
foreach ($id in $started) { try { Stop-Process -Id $id -Force -EA Stop } catch {} }
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
$env:PSMUX_DATA_DIR = $oldDataDir
$after = (& $PSMUX ls 2>&1 | Out-String).Trim()
if ($before -ne $after) { Write-Host "WARNING default namespace changed: [$before] -> [$after]" -ForegroundColor Yellow }

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed"
if ($script:TestsFailed -gt 0) { exit 1 } else { exit 0 }
