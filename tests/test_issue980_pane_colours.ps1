# Issue #980: pane-colours (other name pane-colors) was accepted and kept as
# text, and nothing read it.  Measured on master 3b3ce8fe:
#
#   show -gw pane-colours              -> "pane-colours " (name, empty value)
#   set -gw 'pane-colours[1]' zzz      -> taken, no error
#   set -gw pane-colours 'red,blue green'; show -gw pane-colours
#                                      -> "pane-colours red,blue green", one line
#   set -w 'pane-colours[2]' '#0000ff'; show -w pane-colours -> "pane-colours"
#   set -p 'pane-colours[3]' '#ff0000' -> "pane-scoped option ... is not supported"
#   pane-colours[1] #00ff00, pane writes ESC[31m R
#                                      -> client wrote ESC[31m R (index 1)
#   the same, pane asks ESC]4;1;?      -> rgb:c5c5/0f0f/1f1f (Campbell red)
#
# tmux 3.8: options-table.c:1614-1620 (a window and pane array of colours,
# no default), :284 (pane-colors), colour.c:1293-1322 (the default palette),
# colour.c:1251-1268 (OSC 4 entry first, then the default), tty.c:2822-2960
# (indexed colours drawn through it), input.c:2948 (OSC 4 queries answered
# from it), input.c:1407 and :3446-3475 (RIS and OSC 104 drop only the OSC 4
# entries).
#
# Fully isolated: a private -L namespace and PSMUX_DATA_DIR, and every
# process this test starts is stopped by its PID.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } else { (Get-Command psmux -EA Stop).Source }
$SOCK = "i980_$PID"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

# The default namespace, before this test points PSMUX_DATA_DIR elsewhere.
$defaultBefore = (& $PSMUX ls 2>&1 | Out-String).Trim()
$savedDataDir = $env:PSMUX_DATA_DIR
$savedNoWarm = $env:PSMUX_NO_WARM
$savedNoColor = $env:NO_COLOR
$env:PSMUX_DATA_DIR = "$env:TEMP\psmux_980_root_$PID"
$env:PSMUX_NO_WARM = "1"
# NO_COLOR would strip every SGR the client writes and make the draw checks
# vacuous.
$env:NO_COLOR = $null
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null

$TESTS = Split-Path -Parent $MyInvocation.MyCommand.Path
$OUT = "$env:TEMP\psmux_980_out_$PID"
New-Item -ItemType Directory -Force $OUT | Out-Null

$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$emit = "$OUT\seq_emit980.exe"
$feed = "$OUT\conptyfeed980.exe"
& $csc /nologo /out:$emit /platform:x64 "$TESTS\seq_emit980.cs" 2>&1 | Out-Null
& $csc /nologo /out:$feed /platform:x64 "$TESTS\conptyfeed610.cs" 2>&1 | Out-Null
if (-not (Test-Path $emit) -or -not (Test-Path $feed)) {
    Write-Host "helper build failed" -ForegroundColor Red
    exit 1
}

function P { & $PSMUX -L $SOCK @args 2>&1 | Out-String }
function Lines($text) { ($text -replace "`r", "").TrimEnd("`n") }

$started = @()

# A pane child that writes `seq` (spelled as seq_emit980.cs reads it) and
# logs what psmux types back.  Returns the log once the child wrote it.
function Wait-Log($log, $seconds) {
    for ($k = 0; $k -lt ($seconds * 4); $k++) {
        if (Test-Path $log) {
            Start-Sleep -Milliseconds 200
            return (Get-Content $log -Raw)
        }
        Start-Sleep -Milliseconds 250
    }
    return $null
}

# Host the psmux CLIENT under a real pseudoconsole and read its output bytes,
# with window `$win` of the one session selected.
#
# Every pane here is a window of ONE session: a psmux server holds one
# session, so `set -g` in it reaches no pane of another `new-session`.
function Capture-Client($win, $seconds) {
    P select-window -t "base:$win" | Out-Null
    $sess = "base"
    $ctrl = "$OUT\$win.ctrl"
    $cap = "$OUT\$win.bin"
    Remove-Item $ctrl, $cap -EA SilentlyContinue
    Set-Content $ctrl "" -NoNewline
    $p = Start-Process -FilePath $feed `
        -ArgumentList "`"$ctrl`"", "`"$cap`"", "`"$PSMUX`" -L $SOCK attach-session -t $sess" `
        -PassThru -WindowStyle Hidden
    $script:started += $p.Id
    Start-Sleep -Seconds $seconds
    try { Stop-Process -Id $p.Id -Force -EA Stop } catch {}
    Start-Sleep -Milliseconds 400
    if (-not (Test-Path $cap)) { return "" }
    $fs = [IO.File]::Open($cap, 'Open', 'Read', 'ReadWrite')
    $buf = New-Object byte[] $fs.Length
    $null = $fs.Read($buf, 0, $buf.Length)
    $fs.Close()
    return [Text.Encoding]::ASCII.GetString($buf)
}

function Sgr-After($stream, $marker) {
    $i = $stream.IndexOf($marker)
    if ($i -lt 0) { return $null }
    $m = [regex]::Match($stream.Substring($i + $marker.Length), '^\x1b\[([0-9;:]*)m')
    if ($m.Success) { return $m.Groups[1].Value }
    return "(no SGR)"
}

function Replies($log) {
    if ($null -eq $log) { return ,@() }
    return ,@([regex]::Matches($log, 'ESC\]4;1;(rgb:[0-9a-f/]+)') | ForEach-Object { $_.Groups[1].Value })
}

Write-Host "Binary: $PSMUX" -ForegroundColor Cyan

P new-session -d -s base -x 80 -y 24 | Out-Null
Start-Sleep -Seconds 2

Write-Host "`n=== Group 0: a server that never sets pane-colours is as before ===" -ForegroundColor Cyan
$log0 = "$OUT\q0.log"
P new-window -d -t base -n q0 "$emit $log0 \e]4\s1\s?\a 2500 10" | Out-Null
$r0 = Replies (Wait-Log $log0 20)
if ($r0.Count -eq 1 -and $r0[0] -ne "rgb:0000/ffff/0000") {
    Write-Pass "OSC 4 ; 1 ; ? with no pane-colours reports the terminal's colour ($($r0[0]))"
} else {
    Write-Fail "OSC 4 ; 1 ; ? with no pane-colours gave '$($r0 -join ',')'"
}
P new-window -d -t base -n d0 "$emit $OUT\d0.log MARK_A\e[31mR\e[0m 200 30" | Out-Null
Start-Sleep -Seconds 3
$s0 = Capture-Client "d0" 5
$a0 = Sgr-After $s0 "MARK_A"
if ($a0 -eq "31") { Write-Pass "ESC[31m with no pane-colours still leaves the client as ESC[31m" }
else { Write-Fail "ESC[31m with no pane-colours left the client as ESC[${a0}m" }
P kill-window -t base:d0 | Out-Null
P kill-window -t base:q0 | Out-Null

Write-Host "`n=== Group 1: the option is a colour array ===" -ForegroundColor Cyan
$o = Lines (P show -gw pane-colours)
if ($o -eq "pane-colours") { Write-Pass "show -gw pane-colours with nothing set prints the bare name" }
else { Write-Fail "show -gw pane-colours with nothing set printed '$o'" }

foreach ($bad in @("zzz", "9")) {
    $o = Lines (P set -gw "pane-colours[1]" $bad)
    if ($o -match "bad colour: $bad") { Write-Pass "set -gw pane-colours[1] $bad is refused: $o" }
    else { Write-Fail "set -gw pane-colours[1] $bad gave '$o', expected bad colour: $bad" }
}
$o = Lines (P show -gw pane-colours)
if ($o -eq "pane-colours") { Write-Pass "a refused colour stored nothing" }
else { Write-Fail "after the refused sets show -gw printed '$o'" }

P set -gw pane-colours "red,blue green" | Out-Null
$o = Lines (P show -gw pane-colours)
$want = "pane-colours[0] red`npane-colours[1] blue`npane-colours[2] green"
if ($o -eq $want) { Write-Pass "set -gw pane-colours 'red,blue green' is three elements" }
else { Write-Fail "show -gw pane-colours printed '$o'" }
$o = Lines (P display -p "#{pane-colours}")
if ($o -eq "red blue green") { Write-Pass "#{pane-colours} is the array joined with spaces" }
else { Write-Fail "#{pane-colours} gave '$o'" }
$o = Lines (P show -gw pane-colors)
if ($o -eq $want) { Write-Pass "pane-colors reads the same array, named pane-colours" }
else { Write-Fail "show -gw pane-colors printed '$o'" }

P set -w "pane-colours[2]" "#0000FF" | Out-Null
$o = Lines (P show -w pane-colours)
if ($o -eq "pane-colours[2] #0000ff") { Write-Pass "set -w writes the window's own table: $o" }
else { Write-Fail "show -w pane-colours printed '$o'" }

$o = Lines (P set -p "pane-colours[3]" "#ff0000")
if ($o -eq "") { Write-Pass "set -p pane-colours[3] is taken" } else { Write-Fail "set -p pane-colours[3] gave '$o'" }
$o = Lines (P show -p pane-colours)
if ($o -eq "pane-colours[3] #ff0000") { Write-Pass "show -p prints the pane's own table: $o" }
else { Write-Fail "show -p pane-colours printed '$o'" }
$o = Lines (P show -pA pane-colours)
if ($o -eq "pane-colours[3] #ff0000") { Write-Pass "show -pA prints only the pane's table, nothing merged" }
else { Write-Fail "show -pA pane-colours printed '$o'" }
P set -pu pane-colours | Out-Null
$o = Lines (P show -pA pane-colours)
if ($o -eq "pane-colours[2]* #0000ff") { Write-Pass "after set -pu the pane inherits its window's table whole: $o" }
else { Write-Fail "after set -pu show -pA printed '$o'" }
P set -wu pane-colours | Out-Null
$o = Lines (P show -pA pane-colours)
if ($o -eq "pane-colours[0]* red`npane-colours[1]* blue`npane-colours[2]* green") {
    Write-Pass "after set -wu the pane inherits the global table"
} else { Write-Fail "after set -wu show -pA printed '$o'" }
P set -gu pane-colours | Out-Null
$o = Lines (P show -gw pane-colours)
if ($o -eq "pane-colours") { Write-Pass "set -gu empties the global array" } else { Write-Fail "after set -gu show -gw printed '$o'" }

Write-Host "`n=== Group 2: indexed colours are drawn through it ===" -ForegroundColor Cyan
P set -gw "pane-colours[1]" "#00ff00" | Out-Null
P set -gw "pane-colours[2]" "blue" | Out-Null
P new-window -d -t base -n d1 "$emit $OUT\d1.log MARK_A\e[31mR\e[0mMARK_B\e[32mG\e[0mMARK_C\e[38\s5\s1mI\e[0m 200 30" | Out-Null
Start-Sleep -Seconds 3
$s1 = Capture-Client "d1" 5
$a = Sgr-After $s1 "MARK_A"
if ($a -eq "38;2;0;255;0") { Write-Pass "ESC[31m left the client as ESC[38;2;0;255;0m (master: ESC[31m)" }
else { Write-Fail "ESC[31m left the client as ESC[${a}m, expected ESC[38;2;0;255;0m" }
$b = Sgr-After $s1 "MARK_B"
if ($b -eq "34") { Write-Pass "ESC[32m with pane-colours[2] blue left the client as ESC[34m, as tmux" }
else { Write-Fail "ESC[32m with pane-colours[2] blue left the client as ESC[${b}m" }
$c = Sgr-After $s1 "MARK_C"
if ($c -eq "38;2;0;255;0") { Write-Pass "ESC[38;5;1m goes through the palette too" }
else { Write-Fail "ESC[38;5;1m left the client as ESC[${c}m" }
P kill-window -t base:d1 | Out-Null

P new-window -d -t base -n d2 "$emit $OUT\d2.log \e]4\s1\srgb:11/22/33\aMARK_A\e[31mR\e[0m 200 30" | Out-Null
Start-Sleep -Seconds 3
$s2 = Capture-Client "d2" 5
$a = Sgr-After $s2 "MARK_A"
if ($a -eq "38;2;17;34;51") { Write-Pass "the program's own OSC 4 entry wins over pane-colours" }
else { Write-Fail "with OSC 4 1 set, ESC[31m left the client as ESC[${a}m, expected ESC[38;2;17;34;51m" }
P kill-window -t base:d2 | Out-Null

Write-Host "`n=== Group 3: OSC 4 queries report it ===" -ForegroundColor Cyan
$log1 = "$OUT\q1.log"
P new-window -d -t base -n q1 "$emit $log1 \e]4\s1\s?\a\p\e]4\s1\srgb:11/22/33\a\p\e]4\s1\s?\a\p\e]104\s1\a\p\e]4\s1\s?\a 2500 10" | Out-Null
$r1 = Replies (Wait-Log $log1 30)
if ($r1.Count -ge 1 -and $r1[0] -eq "rgb:0000/ffff/0000") { Write-Pass "OSC 4 ; 1 ; ? reports pane-colours[1] (master: the terminal's red)" }
else { Write-Fail "first OSC 4 ; 1 ; ? gave '$($r1 -join ',')'" }
if ($r1.Count -ge 2 -and $r1[1] -eq "rgb:1111/2222/3333") { Write-Pass "after OSC 4 ; 1 ; rgb:11/22/33 it reports the program's entry" }
else { Write-Fail "second OSC 4 ; 1 ; ? gave '$($r1 -join ',')'" }
if ($r1.Count -ge 3 -and $r1[2] -eq "rgb:0000/ffff/0000") { Write-Pass "OSC 104 ; 1 drops only the program's entry" }
else { Write-Fail "third OSC 4 ; 1 ; ? gave '$($r1 -join ',')'" }
P kill-window -t base:q1 | Out-Null

Write-Host "`n=== Group 4: the first table that holds it wins whole ===" -ForegroundColor Cyan
# A window table set while the pane runs (the child pauses three seconds).
$log2 = "$OUT\q2.log"
P new-window -d -t base -n q2 "$emit $log2 \p\p\p\e]4\s1\s?\a 2500 10" | Out-Null
P set -w -t base:q2 "pane-colours[1]" "#0000ff" | Out-Null
$r2 = Replies (Wait-Log $log2 30)
if ($r2.Count -eq 1 -and $r2[0] -eq "rgb:0000/0000/ffff") { Write-Pass "a window's own table wins over the global one" }
else { Write-Fail "with set -w [1] #0000ff the query gave '$($r2 -join ',')'" }

# A pane table set while the pane runs.
$log3 = "$OUT\q3.log"
$pid3 = Lines (P new-window -d -P -F "#{pane_id}" -t base -n q3 "$emit $log3 \p\p\p\e]4\s1\s?\a 2500 10")
P set -w -t base:q3 "pane-colours[1]" "#0000ff" | Out-Null
P set -p -t $pid3 "pane-colours[5]" "#ff0000" | Out-Null
$r3 = Replies (Wait-Log $log3 30)
# The pane's table holds only [5]; [1] is not taken from the window's.
if ($r3.Count -eq 1 -and $r3[0] -ne "rgb:0000/0000/ffff" -and $r3[0] -ne "rgb:0000/ffff/0000") {
    Write-Pass "a pane's own table wins whole: [1] is not filled from the window ($($r3[0]))"
} else { Write-Fail "with set -p [5] only, [1] was reported as '$($r3 -join ',')'" }

# A pane moved out of a window with its own table takes the new window's.
$log4 = "$OUT\q4.log"
P new-window -d -t base -n q4 | Out-Null
P set -w -t base:q4 "pane-colours[1]" "#0000ff" | Out-Null
$pid4 = Lines (P split-window -d -P -F "#{pane_id}" -t base:q4 "$emit $log4 \p\p\p\p\e]4\s1\s?\a 2500 10")
Start-Sleep -Milliseconds 500
P break-pane -d -s $pid4 | Out-Null
$r4 = Replies (Wait-Log $log4 30)
if ($r4.Count -eq 1 -and $r4[0] -eq "rgb:0000/ffff/0000") { Write-Pass "break-pane hands the pane the global table" }
else { Write-Fail "after break-pane the query gave '$($r4 -join ',')'" }

& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
foreach ($id in $started) { try { Stop-Process -Id $id -Force -EA Stop } catch {} }
$env:PSMUX_DATA_DIR = $savedDataDir
$env:PSMUX_NO_WARM = $savedNoWarm
$env:NO_COLOR = $savedNoColor
$defaultAfter = (& $PSMUX ls 2>&1 | Out-String).Trim()
if ($defaultAfter -ne $defaultBefore) {
    Write-Host "  [WARN] the default namespace changed: '$defaultBefore' -> '$defaultAfter'" -ForegroundColor Yellow
}

Write-Host "`nResults: $($script:TestsPassed) passed, $($script:TestsFailed) failed"
exit $script:TestsFailed
