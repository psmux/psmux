# Issue #792: display-popup frame parity with tmux.
#
# Reported: an attached client drew every popup frame yellow with plain lines
# and the command string as the title. popup-border-style, popup-border-lines
# and popup-style were ignored on that path, and the server's display-popup
# arm parsed only -w -h -d/-c -E -K, so -B, -T, -b, -S and -s were glued onto
# the command (the popup ran "-T MyTitle ping ..." and pwsh reported "The term
# '-T' is not recognized").
#
# Three layers, all against one private -L namespace:
#   1. Server state: dump-state reports the resolved frame and the command
#      without any of the frame flags.
#   2. Client bytes: a real attached client inside a CreatePseudoConsole
#      (tests/conptycap.cs). The frame glyphs, the exact SGR in front of them
#      and the title are read from what the client writes to its terminal.
#   3. Visible client: a real conhost window running `attach`, read back with
#      ReadConsoleOutput (tests/console_screen_read.cs).
# The popup command prints the console size it was given, which proves the
# PTY child is sized to the frame (38x8 inside a 40x10 frame, 40x10 with -B).
#
# Safety: unique -L namespace, PSMUX_DATA_DIR honoured (a private one is made
# when unset), cleanup only with `-L <ns> kill-server` and by recorded PID.

$ErrorActionPreference = "Continue"
$ESC = [char]27
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red;   $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Test($m) { Write-Host "`n[$m]" -ForegroundColor Cyan }

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -EA SilentlyContinue).Source }
if (-not $PSMUX) { Write-Host "psmux not found"; exit 1 }

$work = Join-Path $env:TEMP "psmux_i792_$PID"
New-Item -ItemType Directory -Force -Path $work | Out-Null
if (-not $env:PSMUX_DATA_DIR) {
    $env:PSMUX_DATA_DIR = Join-Path $work "data"
    New-Item -ItemType Directory -Force -Path $env:PSMUX_DATA_DIR | Out-Null
}
# An attached client refuses to nest, and NO_COLOR would strip the SGR under test.
foreach ($v in 'PSMUX_SESSION','PSMUX_SESSION_NAME','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX','NO_COLOR') {
    Remove-Item "env:$v" -EA SilentlyContinue
}
$NS = "i792_$PID"
$started = New-Object System.Collections.Generic.List[int]

Write-Host "=== Issue #792: display-popup frame, title, -B, -b, -S, -s ===" -ForegroundColor White
Write-Host "psmux: $PSMUX  namespace: $NS  data: $env:PSMUX_DATA_DIR"

# The popup command: print the console size the child was given, then wait.
$SIZECMD = 'echo ("SIZE=" + [Console]::WindowWidth + "x" + [Console]::WindowHeight); Start-Sleep 25'

function New-TestSession([string]$Name) {
    & $PSMUX -L $NS kill-session -t $Name 2>&1 | Out-Null
    & $PSMUX -L $NS new-session -d -s $Name -x 100 -y 30 2>&1 | Out-Null
    Start-Sleep -Seconds 2
    & $PSMUX -L $NS has-session -t $Name 2>&1 | Out-Null
    return ($LASTEXITCODE -eq 0)
}
function Set-PopupOptions {
    & $PSMUX -L $NS set -g popup-border-style 'fg=#3b434d' 2>&1 | Out-Null
    & $PSMUX -L $NS set -g popup-border-lines rounded 2>&1 | Out-Null
    & $PSMUX -L $NS set -g popup-style 'bg=blue' 2>&1 | Out-Null
}
function Get-PopupState([string]$Session) {
    $raw = (& $PSMUX -L $NS dump-state -t $Session 2>&1 | Out-String)
    $h = @{}
    foreach ($m in [regex]::Matches($raw, '"(popup_[a-z_]+)":("((?:[^"\\]|\\.)*)"|[0-9]+|true|false)')) {
        $h[$m.Groups[1].Value] = if ($m.Groups[3].Success) { $m.Groups[3].Value } else { $m.Groups[2].Value }
    }
    return $h
}

# ---------------------------------------------------------------------------
# Layer 1: server state
# ---------------------------------------------------------------------------
Write-Test "Server state: flags parsed, frame resolved"
if (New-TestSession "st") {
    Set-PopupOptions
    & $PSMUX -L $NS display-popup -t st -w 40 -h 10 'Start-Sleep 20' 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    $s = Get-PopupState "st"
    if ($s['popup_active'] -eq 'true') { Write-Pass "popup is open" } else { Write-Fail "popup did not open: $($s | Out-String)" }
    if ($s.ContainsKey('popup_title') -and $s['popup_title'] -eq '') { Write-Pass "no -T: empty title (tmux draws none)" }
    else { Write-Fail "no -T: title should be empty, got '$($s['popup_title'])'" }
    if ($s['popup_border_lines'] -eq 'rounded') { Write-Pass "popup-border-lines rounded reaches the frame" } else { Write-Fail "border lines '$($s['popup_border_lines'])'" }
    if ($s['popup_border_style'] -eq 'fg=#3b434d') { Write-Pass "popup-border-style reaches the frame" } else { Write-Fail "border style '$($s['popup_border_style'])'" }
    if ($s['popup_style'] -eq 'bg=blue') { Write-Pass "popup-style reaches the frame" } else { Write-Fail "popup style '$($s['popup_style'])'" }

    & $PSMUX -L $NS display-popup -t st -B -T MyTitle -b double -S fg=red -s bg=green -w 40 -h 10 'Start-Sleep 20' 2>&1 | Out-Null
    Start-Sleep -Milliseconds 800
    $s = Get-PopupState "st"
    if ($s['popup_command'] -eq 'Start-Sleep 20') { Write-Pass "command carries none of the frame flags: '$($s['popup_command'])'" }
    else { Write-Fail "command polluted by flags: '$($s['popup_command'])'" }
    if ($s['popup_title'] -eq 'MyTitle') { Write-Pass "-T sets the title" } else { Write-Fail "-T title '$($s['popup_title'])'" }
    if ($s['popup_border_lines'] -eq 'none') { Write-Pass "-B wins over -b and the option" } else { Write-Fail "-B lines '$($s['popup_border_lines'])'" }
    if ($s['popup_border_style'] -eq 'fg=#3b434d,fg=red') { Write-Pass "-S applies over popup-border-style" } else { Write-Fail "-S style '$($s['popup_border_style'])'" }
    if ($s['popup_style'] -eq 'bg=blue,bg=green') { Write-Pass "-s applies over popup-style" } else { Write-Fail "-s style '$($s['popup_style'])'" }

    & $PSMUX -L $NS display-popup -t st -C 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    $s = Get-PopupState "st"
    if ($s['popup_active'] -eq 'false') { Write-Pass "-C closes the popup" } else { Write-Fail "-C left popup_active=$($s['popup_active'])" }

    $err = (& $PSMUX -L $NS display-popup -t st -b wobbly 'x' 2>&1 | Out-String).Trim()
    $rc = $LASTEXITCODE
    if ($rc -eq 1 -and $err -match 'popup-border-lines unknown value: wobbly') { Write-Pass "bad -b fails at exit 1: $err" }
    else { Write-Fail "bad -b: rc=$rc out='$err'" }
    & $PSMUX -L $NS kill-session -t st 2>&1 | Out-Null
} else { Write-Fail "session st did not start" }

# ---------------------------------------------------------------------------
# Layer 2: the bytes an attached client writes (ConPTY capture)
# ---------------------------------------------------------------------------
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$capExe = Join-Path $work "conptycap.exe"
$capSrc = Join-Path $PSScriptRoot "conptycap.cs"
if ((Test-Path $capSrc) -and (Test-Path $csc) -and -not (Test-Path $capExe)) {
    & $csc -nologo -optimize "-out:$capExe" $capSrc 2>&1 | Out-Null
}

# Attach a client inside a pseudoconsole, open the popup once the client has
# drawn, and return everything the client wrote.
function Capture-Popup {
    param([string]$Session, [string[]]$PopupArgs)
    if (-not (Test-Path $capExe)) { return $null }
    $launch = Join-Path $work "attach_$Session.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set PSMUX=
set NO_COLOR=
"$PSMUX" -L $NS attach -t $Session
"@ | Set-Content -Path $launch -Encoding ASCII
    $outBin = Join-Path $work "client_$Session.bin"
    Remove-Item $outBin, "$outBin.log" -Force -EA SilentlyContinue
    $env:CONPTYCAP_DRAIN_MS = "9000"
    $cap = Start-Process -FilePath $capExe -ArgumentList @($outBin, "100", "30", "0", $launch) -WindowStyle Minimized -PassThru
    $started.Add($cap.Id)
    Start-Sleep -Seconds 3
    & $PSMUX -L $NS display-popup -t $Session @PopupArgs 2>&1 | Out-Null
    $cap.WaitForExit(20000) | Out-Null
    # The capture's child (cmd running the client) loses its pseudoconsole
    # when the capture exits; stop it by its recorded PID if it lingers.
    $log = Get-Content "$outBin.log" -Raw -EA SilentlyContinue
    if ($log -match 'childPid=(\d+)') {
        $cpid = [int]$Matches[1]
        Get-CimInstance Win32_Process -Filter "ParentProcessId=$cpid" -EA SilentlyContinue | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
        Stop-Process -Id $cpid -Force -EA SilentlyContinue
    }
    if (-not (Test-Path $outBin)) { return $null }
    return [System.Text.Encoding]::UTF8.GetString([System.IO.File]::ReadAllBytes($outBin))
}
function Vis([string]$s) { if ($null -eq $s) { return "<none>" }; return ($s -replace $ESC, '<ESC>') }
# The SGR run directly in front of the first occurrence of a glyph.
function Sgr-Before([string]$Text, [string]$Glyph) {
    $m = [regex]::Match($Text, "((?:$ESC\[[0-9;:]*m)+)(?:$ESC\[[0-9;]*[HG])?" + [regex]::Escape($Glyph))
    if ($m.Success) { return $m.Groups[1].Value }
    return $null
}

Write-Test "Attached client bytes: options honoured, no title without -T"
if (-not (Test-Path $capExe)) {
    Write-Skip "csc.exe or tests/conptycap.cs unavailable, client byte capture skipped"
} elseif (New-TestSession "cap1") {
    Set-PopupOptions
    $t = Capture-Popup "cap1" @('-w', '40', '-h', '10', $SIZECMD)
    if (-not $t) { Write-Skip "no client capture" } else {
        if ($t.Contains([string][char]0x256D) -and $t.Contains([string][char]0x256F)) { Write-Pass "rounded corners drawn" } else { Write-Fail "no rounded corners in the client output" }
        $sgr = Sgr-Before $t ([string][char]0x256D)
        if ($sgr -match '38;2;59;67;77') { Write-Pass "frame colour is popup-border-style #3b434d: $(Vis $sgr)" }
        else { Write-Fail "frame SGR expected 38;2;59;67;77, got $(Vis $sgr)" }
        if ($sgr -notmatch '\[33m|;33m|\[93m') { Write-Pass "frame is not yellow" } else { Write-Fail "frame still yellow: $(Vis $sgr)" }
        if ($t -notmatch 'Start-Sleep 25' -and $t -notmatch 'echo \(') { Write-Pass "command string is not drawn as a title" } else { Write-Fail "command string appears in the client output" }
        if ($t -match 'SIZE=38x8') { Write-Pass "popup child got 38x8 inside a 40x10 frame" } else { Write-Fail "child size line not found (expected SIZE=38x8)" }
        $sgrBody = Sgr-Before $t 'SIZE='
        if ($sgrBody -match '44m|48;5;4m|48;2;0;0;128') { Write-Pass "content has popup-style bg=blue: $(Vis $sgrBody)" }
        else { Write-Fail "content SGR expected blue background, got $(Vis $sgrBody)" }
    }
    & $PSMUX -L $NS kill-session -t cap1 2>&1 | Out-Null
} else { Write-Fail "session cap1 did not start" }

Write-Test "Attached client bytes: -T title"
if ((Test-Path $capExe) -and (New-TestSession "cap2")) {
    Set-PopupOptions
    $t = Capture-Popup "cap2" @('-w', '40', '-h', '10', '-T', 'MyTitle', $SIZECMD)
    if (-not $t) { Write-Skip "no client capture" } else {
        $rx = [string][char]0x256D + '(?:' + "$ESC" + '\[[0-9;]*m)*' + [string][char]0x2500 + '(?:' + "$ESC" + '\[[0-9;]*m)*MyTitle'
        if ($t -match $rx) { Write-Pass "title 'MyTitle' starts two cells in on the top border" } else { Write-Fail "top border with MyTitle not found" }
        if ($t -notmatch '-T') { Write-Pass "-T did not reach the command" } else { Write-Fail "'-T' appears in the client output" }
    }
    & $PSMUX -L $NS kill-session -t cap2 2>&1 | Out-Null
}

Write-Test "Attached client bytes: -B gives the child the full area"
if ((Test-Path $capExe) -and (New-TestSession "cap3")) {
    Set-PopupOptions
    $t = Capture-Popup "cap3" @('-B', '-w', '40', '-h', '10', $SIZECMD)
    if (-not $t) { Write-Skip "no client capture" } else {
        if ($t -match 'SIZE=40x10') { Write-Pass "popup child got the full 40x10" } else { Write-Fail "expected SIZE=40x10 with -B" }
        if (-not $t.Contains([string][char]0x256D)) { Write-Pass "no frame drawn" } else { Write-Fail "a frame was drawn with -B" }
        if ($t -notmatch "'-B'|-B:") { Write-Pass "-B did not reach the command" } else { Write-Fail "-B leaked into the command" }
    }
    & $PSMUX -L $NS kill-session -t cap3 2>&1 | Out-Null
}

Write-Test "Attached client bytes: -b -S -s win over the options"
if ((Test-Path $capExe) -and (New-TestSession "cap4")) {
    Set-PopupOptions
    $t = Capture-Popup "cap4" @('-b', 'double', '-S', 'fg=red', '-s', 'bg=green', '-w', '40', '-h', '10', $SIZECMD)
    if (-not $t) { Write-Skip "no client capture" } else {
        if ($t.Contains([string][char]0x2554) -and $t.Contains([string][char]0x255D)) { Write-Pass "-b double draws double lines" } else { Write-Fail "no double line corners" }
        $sgr = Sgr-Before $t ([string][char]0x2554)
        if ($sgr -match '\[31m|;31m|38;5;1m') { Write-Pass "-S fg=red colours the frame: $(Vis $sgr)" } else { Write-Fail "frame SGR expected red, got $(Vis $sgr)" }
        $sgrBody = Sgr-Before $t 'SIZE='
        if ($sgrBody -match '42m|48;5;2m') { Write-Pass "-s bg=green colours the content: $(Vis $sgrBody)" } else { Write-Fail "content SGR expected green background, got $(Vis $sgrBody)" }
    }
    & $PSMUX -L $NS kill-session -t cap4 2>&1 | Out-Null
}

# ---------------------------------------------------------------------------
# Layer 3: a visible attached client, read back from its console
# ---------------------------------------------------------------------------
Write-Test "Visible attached client (conhost window, ReadConsoleOutput)"
$srExe = Join-Path $work "console_screen_read.exe"
$srSrc = Join-Path $PSScriptRoot "console_screen_read.cs"
if ((Test-Path $srSrc) -and (Test-Path $csc) -and -not (Test-Path $srExe)) {
    & $csc -nologo -optimize "-out:$srExe" $srSrc 2>&1 | Out-Null
}
function Read-ClientScreen([int]$ClientPid) {
    $o = Join-Path $work "screen_$ClientPid.txt"
    Remove-Item $o -Force -EA SilentlyContinue
    & $srExe $ClientPid $o | Out-Null
    if (-not (Test-Path $o)) { return $null }
    $lines = Get-Content $o -Encoding UTF8
    return @{
        Text = @($lines | Where-Object { $_.StartsWith('T|') } | ForEach-Object { $_.Substring(2) })
        Attr = @($lines | Where-Object { $_.StartsWith('A|') } | ForEach-Object { $_.Substring(2) })
    }
}
if (-not (Test-Path $srExe)) {
    Write-Skip "csc.exe or tests/console_screen_read.cs unavailable"
} elseif (New-TestSession "vis") {
    Set-PopupOptions
    $con = Start-Process conhost.exe -ArgumentList "`"$PSMUX`" -L $NS attach -t vis" -PassThru
    $started.Add($con.Id)
    Start-Sleep -Seconds 3
    $client = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($con.Id)" | Where-Object { $_.Name -like 'psmux*' -or $_.Name -like 'tmux*' -or $_.Name -like 'pmux*' } | Select-Object -First 1
    if (-not $client) {
        Write-Skip "no attached client under conhost $($con.Id)"
    } else {
        $started.Add([int]$client.ProcessId)
        & $PSMUX -L $NS display-popup -t vis -T VisTitle -w 40 -h 10 $SIZECMD 2>&1 | Out-Null
        Start-Sleep -Seconds 3
        $scr = Read-ClientScreen ([int]$client.ProcessId)
        if (-not $scr) { Write-Skip "screen read failed" } else {
            $top = $scr.Text | Where-Object { $_.Contains([string][char]0x256D) } | Select-Object -First 1
            if ($top -and $top.Contains([string][char]0x256D + [string][char]0x2500 + 'VisTitle')) { Write-Pass "visible frame: '$($top.Trim())'" }
            else { Write-Fail "visible top border not as expected: '$top'" }
            $body = $scr.Text | Where-Object { $_ -match 'SIZE=' } | Select-Object -First 1
            if ($body -match [string][char]0x2502 + 'SIZE=38x8') { Write-Pass "visible content inside the frame: '$($body.Trim())'" } else { Write-Fail "visible content row: '$body'" }
            $ri = [array]::IndexOf($scr.Text, $body)
            if ($ri -ge 0) {
                $col = $scr.Text[$ri].IndexOf('SIZE=')
                $cells = $scr.Attr[$ri].Trim().Split(' ')
                $attr = [Convert]::ToInt32($cells[$col], 16)
                if ((($attr -shr 4) -band 0xF) -eq 1) { Write-Pass "visible content background is blue (attr $($cells[$col]))" }
                else { Write-Fail "visible content attr $($cells[$col]), expected background blue" }
                $battr = [Convert]::ToInt32($cells[$col - 1], 16)
                if (($battr -band 0xF) -notin 6, 14) { Write-Pass "visible frame is not yellow (attr $($cells[$col - 1]))" }
                else { Write-Fail "visible frame is yellow (attr $($cells[$col - 1]))" }
            }
        }
        & $PSMUX -L $NS display-popup -t vis -B -w 40 -h 10 $SIZECMD 2>&1 | Out-Null
        Start-Sleep -Seconds 3
        $scr = Read-ClientScreen ([int]$client.ProcessId)
        if ($scr) {
            $body = $scr.Text | Where-Object { $_ -match 'SIZE=' } | Select-Object -First 1
            if ($body -match 'SIZE=40x10' -and $body -notmatch [string][char]0x2502) { Write-Pass "visible -B popup has no frame and a 40x10 child: '$($body.Trim())'" }
            else { Write-Fail "visible -B row: '$body'" }
        }
    }
    & $PSMUX -L $NS kill-session -t vis 2>&1 | Out-Null
}

# ---------------------------------------------------------------------------
# Cleanup: this namespace only, and the PIDs this suite started.
# ---------------------------------------------------------------------------
& $PSMUX -L $NS kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 800
foreach ($p in $started) { Stop-Process -Id $p -Force -EA SilentlyContinue }
Start-Sleep -Milliseconds 500
$left = @($started | Where-Object { Get-Process -Id $_ -EA SilentlyContinue })
if ($left.Count -eq 0) { Write-Pass "no window or client this suite opened is left" } else { Write-Fail "left running: $($left -join ', ')" }

Write-Host "`nPassed: $script:TestsPassed  Failed: $script:TestsFailed  Skipped: $script:TestsSkipped"
exit $script:TestsFailed
