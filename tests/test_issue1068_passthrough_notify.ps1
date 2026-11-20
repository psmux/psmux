# PR #1068: what an attached client writes to its outer terminal when a pane
# program emits desktop notifications, byte level.
#
# tmux 3.8 parity:
#   * A pane's own OSC 9 text, OSC 777;notify, OSC 99 and OSC 9001 never reach
#     the outer terminal (input.c input_exit_osc handles only OSC 9;4 and drops
#     the rest), whatever allow-passthrough says.
#   * A `DCS tmux;` wrapped notification is written raw to the outer terminal
#     with allow-passthrough on (visible pane only) or all (any pane), never
#     with off (input_dcs_dispatch, screen_write_rawstring).
#
# Before (master df7c6968): the wrapped notification never reached the client
# in any mode. With the PR as submitted: the four unwrapped OSCs reached the
# client even with the default allow-passthrough off.
#
# Hosts a real attached client inside a CreatePseudoConsole in passthrough
# mode (tests/conptycap.cs, as test_issue735_cursor_attach_bytes does).

$ErrorActionPreference = "Continue"
$ESC = [char]27
$BEL = [char]7
$SOCK = "i1068_$PID"
$env:PSMUX_RUN_NOPAUSE = "1"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red;   $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Test($m) { Write-Host "`n[$m]" -ForegroundColor Cyan }

$PSMUX = $env:PSMUX_TEST_BIN
if (-not $PSMUX) { $PSMUX = $env:PSMUX_TEST_EXE }
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -EA SilentlyContinue).Source }
if (-not $PSMUX) { Write-Host "psmux not found"; exit 1 }
Write-Host "binary: $PSMUX"

$work = Join-Path $env:TEMP "psmux_i1068_$PID"
New-Item -ItemType Directory -Force -Path $work | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $work "data"
New-Item -ItemType Directory -Force -Path $env:PSMUX_DATA_DIR | Out-Null

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$capSrc = Join-Path $PSScriptRoot "conptycap.cs"
$capExe = Join-Path $work "conptycap.exe"
if ((Test-Path $capSrc) -and (Test-Path $csc)) { & $csc -nologo -optimize "-out:$capExe" $capSrc 2>&1 | Out-Null }
if (-not (Test-Path $capExe)) { Write-Skip "csc.exe or tests/conptycap.cs unavailable"; exit 0 }
if ([Environment]::OSVersion.Version.Build -lt 22621) { Write-Skip "ConPTY passthrough needs build 22621 or later"; exit 0 }

# The pane program: each notification form, then a marker line.
$emit = Join-Path $work "emit.ps1"
@'
param([string]$Tag)
$e = [char]27; $b = [char]7; $o = [Console]::Out
foreach ($s in @(
    "$e]9;N9PLAIN-$Tag$b",
    "$e]777;notify;T;N777-$Tag$b",
    "$e]99;i=1;N99-$Tag$e\",
    "$e]9001;CmdNotFound;N9001-$Tag$b",
    "${e}Ptmux;$e$e]9;WRAPPED-$Tag$b$e\")) {
    $o.Write($s); $o.Flush(); Start-Sleep -Milliseconds 250
}
$o.Write("DONE-$Tag`r`n"); $o.Flush()
'@ | Set-Content -Path $emit -Encoding UTF8

# Attach a client in a pseudoconsole, run the emitter in window $EmitWin
# while window $ActiveWin is shown, return what the client wrote.
function Capture-Case {
    param([string]$Name, [string]$Mode, [int]$EmitWin = 0, [int]$ActiveWin = 0)
    $cf = Join-Path $work "$Name.conf"
    "set -g allow-passthrough $Mode" | Set-Content -Path $cf -Encoding ASCII
    & $PSMUX -L $SOCK -f $cf new-session -d -s $Name -x 100 -y 30 2>&1 | Out-Null
    $up = $false
    for ($i = 0; $i -lt 40; $i++) {
        & $PSMUX -L $SOCK has-session -t $Name 2>&1 | Out-Null
        if ($LASTEXITCODE -eq 0) { $up = $true; break }
        Start-Sleep -Milliseconds 250
    }
    if (-not $up) { return $null }
    & $PSMUX -L $SOCK set -g allow-passthrough $Mode 2>&1 | Out-Null
    if ($ActiveWin -ne $EmitWin) {
        & $PSMUX -L $SOCK new-window -t $Name 2>&1 | Out-Null
        & $PSMUX -L $SOCK select-window -t "${Name}:$ActiveWin" 2>&1 | Out-Null
    }
    $launch = Join-Path $work "attach_$Name.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
"$PSMUX" -L $SOCK attach -t $Name
"@ | Set-Content -Path $launch -Encoding ASCII
    $outBin = Join-Path $work "client_$Name.bin"
    Remove-Item $outBin -Force -EA SilentlyContinue
    $env:CONPTYCAP_DRAIN_MS = "20000"
    $cap = Start-Process -FilePath $capExe -ArgumentList @($outBin, "100", "30", "8", $launch) -WindowStyle Minimized -PassThru
    Start-Sleep -Milliseconds 2500
    & $PSMUX -L $SOCK send-keys -t "${Name}:$EmitWin" "& '$emit' $Name" Enter 2>&1 | Out-Null
    $done = $false
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 250
        $c = (& $PSMUX -L $SOCK capture-pane -p -t "${Name}:$EmitWin") -join "`n"
        if ($c -match "(?m)^DONE-$Name") { $done = $true; break }
    }
    Start-Sleep -Milliseconds 1000
    & $PSMUX -L $SOCK detach-client -s $Name 2>&1 | Out-Null
    if (-not $cap.WaitForExit(30000)) { try { Stop-Process -Id $cap.Id -Force } catch {} }
    & $PSMUX -L $SOCK kill-session -t $Name 2>&1 | Out-Null
    if (-not $done -or -not (Test-Path $outBin)) { return $null }
    return [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($outBin))
}

function Count($Text, $Seq) { return ([regex]::Matches($Text, [regex]::Escape($Seq))).Count }

function Check-Unwrapped($t, $Name) {
    $leaked = @()
    foreach ($s in @("$ESC]9;N9PLAIN-$Name", "$ESC]777;notify;T;N777-$Name", "$ESC]99;i=1;N99-$Name", "$ESC]9001;CmdNotFound;N9001-$Name")) {
        if ((Count $t $s) -gt 0) { $leaked += $s.Replace("$ESC", "ESC") }
    }
    if ($leaked.Count -eq 0) { Write-Pass "no unwrapped notification OSC reached the outer terminal" }
    else { Write-Fail ("unwrapped OSC forwarded: " + ($leaked -join ", ")) }
}

$wrapped = { param($n) "$ESC]9;WRAPPED-$n$BEL" }

Write-Test "allow-passthrough off (default): nothing reaches the outer terminal"
$t = Capture-Case "off" "off"
if ($null -eq $t) { Write-Fail "no capture" } else {
    Check-Unwrapped $t "off"
    if ((Count $t (& $wrapped "off")) -eq 0) { Write-Pass "wrapped notification dropped" } else { Write-Fail "wrapped notification forwarded with off" }
}

Write-Test "allow-passthrough on, visible pane: only the wrapped notification, once"
$t = Capture-Case "on" "on"
if ($null -eq $t) { Write-Fail "no capture" } else {
    Check-Unwrapped $t "on"
    $n = Count $t (& $wrapped "on")
    if ($n -eq 1) { Write-Pass "wrapped OSC 9 written raw exactly once" } else { Write-Fail "wrapped OSC 9 written $n times" }
}

Write-Test "allow-passthrough on, hidden window: not forwarded (tmux: visible panes only)"
$t = Capture-Case "onhid" "on" 0 1
if ($null -eq $t) { Write-Fail "no capture" } else {
    $n = Count $t (& $wrapped "onhid")
    if ($n -eq 0) { Write-Pass "hidden pane's passthrough dropped" } else { Write-Fail "hidden pane's passthrough written $n times" }
}

Write-Test "allow-passthrough all, hidden window: forwarded once"
$t = Capture-Case "allhid" "all" 0 1
if ($null -eq $t) { Write-Fail "no capture" } else {
    Check-Unwrapped $t "allhid"
    $n = Count $t (& $wrapped "allhid")
    if ($n -eq 1) { Write-Pass "hidden pane's passthrough written once with all" } else { Write-Fail "hidden pane's passthrough written $n times" }
}

& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
Remove-Item $work -Recurse -Force -EA SilentlyContinue

Write-Host ""
Write-Host "RESULT: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:TestsSkipped) skipped"
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
