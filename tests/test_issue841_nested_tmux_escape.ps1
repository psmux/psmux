# Issue #841: nested tmux in psmux, Escape arrives late.
#
# Reporter: wezterm -> psmux -> wsl (arch) -> tmux -> nvim.  Escape reaches nvim
# late whatever psmux escape-time (500, 10) or tmux escape-time (10) says, while
# wezterm -> wsl -> tmux -> nvim and wezterm -> psmux -> wsl -> nvim are fine.
#
# Root cause.  tmux 3.8 asks its outer terminal `CSI 18 t CSI 14 t` whenever
# its tty has no pixel size (tty.c:146-150), which a WSL pty never has, sets
# TTY_WINSIZEQUERY, and clears it only when the `CSI 4 ; h ; w t` answer to
# 14 t arrives (tty-keys.c:732).  While it is set every lone Escape is held
# for 500 ms whatever escape-time is (tty-keys.c:980-987, commit 418ddada).
# The inbox console host answers `CSI 18 t` itself but forwards `CSI 14 t`
# (PSMUX_PANE_RAW capture: `ESC[14t` present, `ESC[18t` absent), and psmux
# answered nothing, so the question stayed open for the life of the tmux
# client.  tmux answers it from input_csi_dispatch_winops (input.c:2162).
#
# Measured before the fix (tmux 3.8 built from the local tmux tree in WSL
# Ubuntu, escape-time 10, timestamped WriteConsoleInput into the attached
# client):  psmux -> wsl -> tmux 3.8 -> raw reader: every Escape 555 to 571 ms
# late; with nvim as the reader 597 to 619 ms.  After: 60 to 125 ms once the
# five second startup window (unanswerable OSC 10/11, see below) has passed.
#
# Part 1 needs nothing but psmux.  Part 2 needs WSL with python3 and a tmux of
# at least 3.8 (set PSMUX_841_TMUX to its path inside WSL when it is not the
# distribution's tmux); without one it is reported as SKIP.

param([int]$Reps = 6, [int]$GapMs = 600, [int]$ThresholdMs = 250)

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "i841esc" + (Get-Random -Maximum 99999)
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }

$env:PSMUX_RUN_NOPAUSE = '1'
foreach ($v in 'PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "env:$v" -EA SilentlyContinue }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$work = Join-Path $env:TEMP "psmux_i841_$PID"
New-Item -ItemType Directory -Force $work | Out-Null
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }

function Read-ProbeLine($text, $name) {
    foreach ($line in ($text -split "`r?`n")) { if ($line.StartsWith("$name ")) { return $line } }
    return ""
}
function Get-ProbeReply($text, $name) {
    $line = Read-ProbeLine $text $name
    if ($line -notmatch 'hex=\[([^\]]*)\]') { return $null }
    if ($Matches[1] -eq '') { return '' }
    return -join ($Matches[1] -split ' ' | ForEach-Object { [char][byte]("0x$_") })
}
function Show($s) { if ($null -eq $s) { return '<no line>' }; $s.Replace([string][char]27, 'ESC') }

# ── Part 1: CSI 14 t is answered, once, with tmux's numbers ──────────────────
Write-Host "`n[Part 1] XTWINOPS size reports inside a detached pane" -ForegroundColor Yellow
$probe = Join-Path $work "query_probe.exe"
& $csc /nologo /optimize /out:$probe (Join-Path $repoTests "query_probe_child.cs") 2>&1 | Out-Null
if (-not (Test-Path $probe)) {
    Write-Fail "could not compile query_probe_child.cs"
} else {
    $log = Join-Path $work "probe.txt"
    & $PSMUX -L $NS new-session -d -s p1 -x 100 -y 30 "$probe $log 600" 2>&1 | Out-Null
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline -and -not (Test-Path $log)) { Start-Sleep -Milliseconds 300 }
    Start-Sleep -Milliseconds 300
    $out = if (Test-Path $log) { Get-Content $log -Raw } else { '' }
    & $PSMUX -L $NS kill-session -t p1 2>&1 | Out-Null

    $r18 = Get-ProbeReply $out 'XTWINOPS_18'
    $r14 = Get-ProbeReply $out 'XTWINOPS_14'
    $r16 = Get-ProbeReply $out 'XTWINOPS_16'
    if ($r18 -match "^\x1b\[8;(\d+);(\d+)t$") {
        $rows = [int]$Matches[1]; $cols = [int]$Matches[2]
        Write-Pass "CSI 18 t still answered exactly once by the console host: $(Show $r18)"
        $want14 = "$([char]27)[4;$($rows * 20);$($cols * 10)t"
        if ($r14 -eq $want14) {
            Write-Pass "CSI 14 t answered exactly once, $(Show $r14) (the pane's 10x20 sixel cell: rows*20, cols*10)"
        } else {
            Write-Fail "CSI 14 t reply was [$(Show $r14)], expected [$(Show $want14)]  (empty = the #841 bug: tmux 3.8 holds every Escape 500 ms)"
        }
    } else {
        Write-Fail "could not read the pane size from the CSI 18 t reply: [$(Show $r18)]"
    }
    if ($r16 -eq "$([char]27)[6;20;10t") { Write-Pass "CSI 16 t answered with the pane's sixel cell, $(Show $r16)" }
    else { Write-Fail "CSI 16 t reply was [$(Show $r16)], expected [ESC[6;20;10t]" }
}

# ── Part 2: Escape latency through a real tmux 3.8 nested in a psmux pane ────
Write-Host "`n[Part 2] Escape latency: attached psmux client -> wsl -> tmux 3.8 -> raw reader" -ForegroundColor Yellow
$tmux = if ($env:PSMUX_841_TMUX) { $env:PSMUX_841_TMUX } else { 'tmux' }
$wslOk = $false
if (Get-Command wsl.exe -EA SilentlyContinue) {
    $ver = (wsl.exe -e sh -c "command -v python3 >/dev/null && $tmux -V 2>/dev/null" 2>$null | Out-String).Trim()
    if ($ver -match 'tmux (?:next-)?(\d+)\.(\d+)') {
        $major = [int]$Matches[1]; $minor = [int]$Matches[2]
        if ($major -gt 3 -or ($major -eq 3 -and $minor -ge 8)) { $wslOk = $true }
        else { Write-Skip "WSL tmux is '$ver'; the 500 ms hold needs tmux 3.8 or newer (set PSMUX_841_TMUX)" }
    } else { Write-Skip "no WSL distribution with python3 and tmux found" }
} else { Write-Skip "wsl.exe not available" }

if ($wslOk) {
    $inj = Join-Path $work "injector_timed.exe"
    & $csc /nologo /optimize /out:$inj (Join-Path $repoTests "injector_timed.cs") 2>&1 | Out-Null
    $reader = Join-Path $work "rawread.py"
    @'
import os, sys, time, tty, termios, select
out = sys.argv[1]
old = termios.tcgetattr(0)
tty.setraw(0)
f = open(out, "w", buffering=1)
f.write("START\n")
try:
    while True:
        r, _, _ = select.select([0], [], [], 60)
        if not r:
            break
        b = os.read(0, 1024)
        f.write("%.3f %s\n" % (time.time() * 1000, b.hex(" ")))
        if b"q" in b:
            break
finally:
    termios.tcsetattr(0, termios.TCSADRAIN, old)
    f.close()
'@ | Set-Content -Path $reader -Encoding ascii
    $tconf = Join-Path $work "tmux.conf"
    "set -g escape-time 10`nset -g status off`n" | Set-Content -Path $tconf -Encoding ascii -NoNewline
    $wslPath = { param($p) (wsl.exe -e wslpath -a ($p -replace '\\','/') 2>$null | Out-String).Trim() }
    $wReader = & $wslPath $reader; $wConf = & $wslPath $tconf
    $wLog = "/tmp/psmux_i841_$PID.log"
    $tsock = "i841t$PID"
    wsl.exe -e sh -c "rm -f $wLog" 2>$null
    $cmd = "wsl.exe -e $tmux -L $tsock -f $wConf new-session 'python3 $wReader $wLog'"
    $client = Start-Process -FilePath $PSMUX -ArgumentList '-L', $NS, 'new-session', '-s', 'p2', "`"$cmd`"" -PassThru
    $started = $false
    for ($i = 0; $i -lt 60; $i++) {
        Start-Sleep -Milliseconds 500
        if ((wsl.exe -e sh -c "cat $wLog 2>/dev/null" | Out-String) -match 'START') { $started = $true; break }
    }
    if (-not $started) {
        Write-Fail "the reader never started inside nested tmux"
    } else {
        # tmux also asks OSC 10/11 at startup, which the console host swallows
        # before psmux can see it, and holds Escape until its five second start
        # timer gives up (tty.c:311-322).  That window is outside psmux's reach,
        # so the steady state after it is what is measured.
        Start-Sleep -Seconds 6
        $keys = @('a', "S$GapMs")
        for ($r = 0; $r -lt $Reps; $r++) { $keys += @('ESC', "S$GapMs", [string][char](98 + $r), "S$GapMs") }
        $keys += 'q'
        $injLog = Join-Path $work "inject.txt"
        & $inj $client.Id $injLog @keys
        Start-Sleep -Milliseconds 1500
        $arrRaw = wsl.exe -e sh -c "cat $wLog"
        $sent = @(Get-Content $injLog | ForEach-Object { $x = $_ -split ' ', 2; [pscustomobject]@{ t = [double]$x[0]; k = $x[1] } })
        $got = @($arrRaw | Where-Object { $_ -notmatch 'START' } | ForEach-Object {
            $x = $_ -split ' ', 2; $t = [double]$x[0]
            foreach ($b in ($x[1] -split ' ')) { [pscustomobject]@{ t = $t; b = $b } } })
        $seq = ($got | ForEach-Object b) -join ' '
        Write-Host "    bytes as tmux delivered them: $seq"
        $n = [Math]::Min($sent.Count, $got.Count)
        $lat = @(); for ($i = 0; $i -lt $n; $i++) { $lat += ($got[$i].t - $sent[$i].t) }
        # The WSL clock runs at its own rate and occasionally steps, so each
        # Escape is judged against the keys either side of it, not against an
        # absolute clock; a pair whose neighbours disagree by 150 ms straddles
        # a step and is dropped.
        $d = @()
        for ($i = 1; $i -lt $n - 1; $i++) {
            if ($sent[$i].k -eq 'ESC' -and $got[$i].b -eq '1b' -and [Math]::Abs($lat[$i + 1] - $lat[$i - 1]) -lt 150) {
                $d += ($lat[$i] - ($lat[$i - 1] + $lat[$i + 1]) / 2)
            }
        }
        $expectSeq = (@('61') + (0..($Reps - 1) | ForEach-Object { '1b'; '{0:x2}' -f (98 + $_) }) + @('71')) -join ' '
        if ($seq -eq $expectSeq) { Write-Pass "every Escape arrived alone, nothing glued to it or lost" }
        else { Write-Fail "byte stream was [$seq], expected [$expectSeq]" }
        if ($d.Count -lt 2) {
            Write-Fail "too few clean Escape samples ($($d.Count)) to judge latency"
        } else {
            $mean = ($d | Measure-Object -Average).Average
            $list = ($d | ForEach-Object { '{0:F0}' -f $_ }) -join ', '
            if ($mean -lt $ThresholdMs) {
                Write-Pass ("Escape held {0:F0} ms on average past an ordinary key (samples {1}), under {2} ms" -f $mean, $list, $ThresholdMs)
            } else {
                Write-Fail ("Escape held {0:F0} ms on average past an ordinary key (samples {1}); the #841 bug holds every Escape about 500 ms" -f $mean, $list)
            }
        }
    }
    wsl.exe -e $tmux -L $tsock kill-server 2>$null | Out-Null
    wsl.exe -e sh -c "rm -f $wLog" 2>$null
    & $PSMUX -L $NS kill-session -t p2 2>&1 | Out-Null
    Start-Sleep -Milliseconds 500
    if (-not $client.HasExited) { Stop-Process -Id $client.Id -Force -EA SilentlyContinue }
}

& $PSMUX -L $NS kill-server 2>&1 | Out-Null
Remove-Item $work -Recurse -Force -EA SilentlyContinue

Write-Host "`n=== Issue #841 results: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:TestsSkipped) skipped ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
