# PR #781: synchronized output (DECSET 2026).
#
# A program that brackets a frame with ESC[?2026h ... ESC[?2026l asks the
# terminal to show the frame only once it is complete.  Before PR #781 psmux
# parsed and rendered every chunk as it arrived, so a TUI repainting in pieces
# (Claude Code, for one) showed half drawn frames and a flashing cursor.
#
# tmux (input.c case 2026, screen-write.c screen_write_start_sync and
# screen_write_stop_sync) keeps parsing but defers its redraw to the outer
# terminal until ESC[?2026l or a 1 s timer, and answers DECRQM 2026 with
# MODE_SYNC ? 1 : 2.  A psmux client renders a full snapshot of the pane, so
# psmux holds the pane's bytes instead (SyncGate in src/pane.rs).
#
# Measured on build 26200 with the frame child below (15 frames, 8 lines
# written 60 ms apart inside each update):
#   master    capture-pane torn 354 / 477, dump-state torn 355 / 477
#   PR #781   capture-pane torn   0 / 461, dump-state torn   0 / 461
#
# Part 1  no torn frame in capture-pane or in the client's dump-state
# Part 2  an update that never closes is shown after the 1 s timeout, even
#         though the pane goes silent (nothing else wakes the parser)
# Part 3  OpenConsole host (forwards queries to psmux): DECRQM 2026 answers 2
#         outside an update and 1 inside, and a query inside an open update is
#         answered at once instead of after the 1 s timeout.  The inbox host
#         answers DECRQM itself (ESC[?2026;0$y) and swallows it, so this part
#         needs OpenConsole and is skipped without it.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "pr781sync$PID"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor DarkYellow; $script:TestsSkipped++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor Gray }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
foreach ($v in 'PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') { Remove-Item "env:$v" -EA SilentlyContinue }

$savedDataDir = $env:PSMUX_DATA_DIR
$savedConpty = $env:PSMUX_CONPTY_DIR
$work = Join-Path $env:TEMP "psmux_pr781_$PID"
New-Item -ItemType Directory -Force $work | Out-Null
$env:PSMUX_DATA_DIR = Join-Path $work "data"
New-Item -ItemType Directory -Force $env:PSMUX_DATA_DIR | Out-Null
Remove-Item env:PSMUX_CONPTY_DIR -EA SilentlyContinue

function Cleanup {
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
}

function Get-Dump($session) {
    $portFile = Get-ChildItem $env:PSMUX_DATA_DIR -Recurse -Filter "*$session.port" -EA SilentlyContinue | Select-Object -First 1
    $keyFile = Get-ChildItem $env:PSMUX_DATA_DIR -Recurse -Filter "*$session.key" -EA SilentlyContinue | Select-Object -First 1
    if (-not $portFile -or -not $keyFile) { return $null }
    try {
        $tcp = [System.Net.Sockets.TcpClient]::new("127.0.0.1", [int](Get-Content $portFile.FullName -Raw).Trim())
        $tcp.NoDelay = $true; $tcp.ReceiveTimeout = 3000
        $st = $tcp.GetStream(); $w = [IO.StreamWriter]::new($st); $r = [IO.StreamReader]::new($st)
        $w.Write("AUTH $((Get-Content $keyFile.FullName -Raw).Trim())`n"); $w.Flush(); $null = $r.ReadLine()
        $w.Write("dump-state`n"); $w.Flush()
        $best = $null
        for ($j = 0; $j -lt 20; $j++) {
            $line = $r.ReadLine()
            if ($null -eq $line) { break }
            if ($line.StartsWith("{")) { $best = $line; break }
        }
        $tcp.Close()
        return $best
    } catch { return $null }
}

# Frame child: each frame clears the screen and writes 8 lines and an END
# line slowly inside one synchronized update.
$frames = Join-Path $work "frames.ps1"
@'
param([int]$Frames = 15, [int]$Lines = 8, [int]$PieceMs = 60, [int]$GapMs = 150)
$e = [char]27
$out = [Console]::OpenStandardOutput()
function W([string]$s) { $b = [Text.Encoding]::UTF8.GetBytes($s); $out.Write($b, 0, $b.Length); $out.Flush() }
for ($f = 1; $f -le $Frames; $f++) {
    W "$e[?2026h"
    W "$e[2J$e[H"
    for ($l = 1; $l -le $Lines; $l++) { W ("SYNCF{0:D3} L{1}`r`n" -f $f, $l); Start-Sleep -Milliseconds $PieceMs }
    W ("SYNCF{0:D3} END" -f $f)
    W "$e[?2026l"
    Start-Sleep -Milliseconds $GapMs
}
W "`r`nFRAMES_DONE`r`n"
Start-Sleep 30
'@ | Set-Content $frames

function Classify([string]$text) {
    $m = [regex]::Matches($text, 'SYNCF(\d{3}) (L\d|END)')
    if ($m.Count -eq 0) { return "none" }
    $last = ($m | ForEach-Object { $_.Groups[1].Value } | Sort-Object | Select-Object -Last 1)
    $parts = @($m | Where-Object { $_.Groups[1].Value -eq $last } | ForEach-Object { $_.Groups[2].Value } | Sort-Object -Unique)
    if ($parts.Count -eq 9) { return "full" }
    return "torn:F$last has $($parts.Count)/9"
}

Cleanup

# ── Part 1 ────────────────────────────────────────────────────────────────────
Write-Host "`n[Part 1] Frames inside ESC[?2026h ... ESC[?2026l are never seen half drawn" -ForegroundColor Yellow
& $PSMUX -L $NS new-session -d -s tear -x 100 -y 30 2>&1 | Out-Null
Start-Sleep -Milliseconds 2500
& $PSMUX -L $NS send-keys -t tear "pwsh -NoProfile -File '$frames'" Enter
$cap = @{ full = 0; torn = 0; none = 0 }; $dmp = @{ full = 0; torn = 0; none = 0 }
$examples = @(); $started = $false
$sw = [Diagnostics.Stopwatch]::StartNew()
while ($sw.Elapsed.TotalSeconds -lt 60) {
    $c = (& $PSMUX -L $NS capture-pane -p -t tear) -join "`n"
    if ($c -match 'FRAMES_DONE') { break }
    $k = Classify $c
    if (-not $started) { if ($k -eq "none") { Start-Sleep -Milliseconds 20; continue }; $started = $true }
    if ($k -like "torn*") { $cap.torn++; if ($examples.Count -lt 3) { $examples += "capture-pane $k" } } else { $cap[$k]++ }
    $d = Get-Dump "tear"
    if ($d) {
        $kd = Classify $d
        if ($kd -like "torn*") { $dmp.torn++; if ($examples.Count -lt 6) { $examples += "dump-state $kd" } } else { $dmp[$kd]++ }
    }
}
$capTotal = $cap.full + $cap.torn + $cap.none
$dmpTotal = $dmp.full + $dmp.torn + $dmp.none
Write-Info "capture-pane: $($cap.full) full, $($cap.torn) torn, $($cap.none) blank of $capTotal"
Write-Info "dump-state:   $($dmp.full) full, $($dmp.torn) torn, $($dmp.none) blank of $dmpTotal"
if ($examples.Count) { Write-Info ("examples: " + ($examples -join "; ")) }
if ($capTotal -lt 50) {
    Write-Fail "too few capture-pane samples ($capTotal) to judge; did the frame child run?"
} elseif ($cap.torn -eq 0 -and $cap.none -eq 0) {
    Write-Pass "capture-pane never showed a half drawn frame ($capTotal samples)"
} else {
    Write-Fail "capture-pane showed $($cap.torn) torn and $($cap.none) blank frames of $capTotal"
}
if ($dmpTotal -lt 50) {
    Write-Fail "too few dump-state samples ($dmpTotal) to judge"
} elseif ($dmp.torn -eq 0 -and $dmp.none -eq 0) {
    Write-Pass "the client's dump-state never carried a half drawn frame ($dmpTotal samples)"
} else {
    Write-Fail "dump-state carried $($dmp.torn) torn and $($dmp.none) blank frames of $dmpTotal"
}
if ($cap.full -ge 5) { Write-Pass "complete frames kept arriving ($($cap.full) full samples)" }
else { Write-Fail "only $($cap.full) complete frames were seen" }

# ── Part 2 ────────────────────────────────────────────────────────────────────
Write-Host "`n[Part 2] An update that never closes is shown after the 1 s timeout" -ForegroundColor Yellow
$never = Join-Path $work "never.ps1"
@'
$e = [char]27
$out = [Console]::OpenStandardOutput()
function W([string]$s) { $b = [Text.Encoding]::UTF8.GetBytes($s); $out.Write($b, 0, $b.Length); $out.Flush() }
W "BEFOREMARK`r`n"
Start-Sleep -Milliseconds 700
W "$e[?2026h$e[2J$e[HNEVERCLOSED"
Start-Sleep 30
'@ | Set-Content $never
& $PSMUX -L $NS new-session -d -s never -x 100 -y 30 "pwsh -NoProfile -File $never" 2>&1 | Out-Null
$tBefore = $null; $tNever = $null; $bothGoneEarly = $false
$sw = [Diagnostics.Stopwatch]::StartNew()
while ($sw.Elapsed.TotalSeconds -lt 30) {
    $c = (& $PSMUX -L $NS capture-pane -p -t never) -join "`n"
    if ($null -eq $tBefore -and $c -match 'BEFOREMARK') { $tBefore = $sw.ElapsedMilliseconds }
    if ($c -match 'NEVERCLOSED') { $tNever = $sw.ElapsedMilliseconds; break }
    Start-Sleep -Milliseconds 20
}
if ($null -eq $tBefore -or $null -eq $tNever) {
    Write-Fail "markers not seen (BEFOREMARK at $tBefore ms, NEVERCLOSED at $tNever ms)"
} else {
    # The child writes the update 700 ms after BEFOREMARK, so NEVERCLOSED is
    # due about 1700 ms after it: 700 ms plus the 1 s hold.  Without a hold it
    # shows about 700 ms after it.
    $gap = $tNever - $tBefore
    Write-Info "BEFOREMARK at $tBefore ms, NEVERCLOSED at $tNever ms, gap $gap ms (no hold: ~700, held: ~1700)"
    if ($gap -ge 1400) { Write-Pass "the unterminated update was held (gap $gap ms)" }
    else { Write-Fail "the unterminated update was shown at once (gap $gap ms), nothing held it" }
    if ($gap -le 3500) { Write-Pass "the 1 s timeout released it although the pane went silent (gap $gap ms)" }
    else { Write-Fail "the update stayed held far past the 1 s timeout (gap $gap ms)" }
}

Cleanup
Start-Sleep -Milliseconds 500

# ── Part 3 ────────────────────────────────────────────────────────────────────
Write-Host "`n[Part 3] DECRQM 2026 and queries inside an open update (OpenConsole host)" -ForegroundColor Yellow
$hostDir = $env:PSMUX_TEST_CONPTY_DIR
if (-not $hostDir) { $hostDir = "$env:TEMP\occ-conpty\nugetstable" }
if (-not ((Test-Path (Join-Path $hostDir "conpty.dll")) -and (Test-Path (Join-Path $hostDir "OpenConsole.exe")))) {
    Write-Skip "no conpty.dll + OpenConsole.exe at $hostDir (run tests/test_issue597_da_replies_openconsole.ps1 once to fetch it)"
} else {
    $csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
    if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
    $probe = Join-Path $work "sync_output_probe.exe"
    & $csc /nologo /optimize /out:$probe (Join-Path $repoTests "sync_output_probe.cs") 2>&1 | Out-Null
    if (-not (Test-Path $probe)) {
        Write-Fail "could not compile tests/sync_output_probe.cs"
    } else {
        $env:PSMUX_CONPTY_DIR = $hostDir
        $log = Join-Path $work "probe.txt"
        & $PSMUX -L $NS new-session -d -s probe -x 100 -y 30 "$probe $log 3000" 2>&1 | Out-Null
        $deadline = (Get-Date).AddSeconds(60)
        while ((Get-Date) -lt $deadline -and -not (Test-Path $log)) { Start-Sleep -Milliseconds 300 }
        Start-Sleep -Milliseconds 300
        Remove-Item env:PSMUX_CONPTY_DIR -EA SilentlyContinue
        if (-not (Test-Path $log)) {
            Write-Fail "the probe never wrote its log"
        } else {
            $text = Get-Content $log -Raw
            Write-Host ($text.TrimEnd())
            function Line($name) { foreach ($l in ($text -split "`r?`n")) { if ($l.StartsWith("$name ")) { return $l } }; return "" }
            $l = Line "DECRQM_2026_OUTSIDE"
            if ($l -match [regex]::Escape('text=[<ESC>[?2026;2$y]')) { Write-Pass "DECRQM 2026 outside an update answers ESC[?2026;2`$y (reset, tmux)" }
            else { Write-Fail "DECRQM 2026 outside an update: $l" }
            $l = Line "DECRQM_2026_IN_SYNC"
            if ($l -match [regex]::Escape('text=[<ESC>[?2026;1$y]')) { Write-Pass "DECRQM 2026 inside an update answers ESC[?2026;1`$y (set, tmux)" }
            else { Write-Fail "DECRQM 2026 inside an update: $l" }
            foreach ($q in "DA1_IN_SYNC", "DECRQM_2026_IN_SYNC", "CPR_IN_SYNC") {
                $l = Line $q
                if ($l -match 'ms=(-?\d+)') {
                    $ms = [int]$Matches[1]
                    if ($ms -ge 0 -and $ms -lt 500) { Write-Pass "$q answered in $ms ms, not held for the 1 s timeout" }
                    else { Write-Fail "$q answered in $ms ms (held by the sync gate?)" }
                } else { Write-Fail "could not read $q" }
            }
        }
    }
}

Cleanup
if ($null -ne $savedDataDir) { $env:PSMUX_DATA_DIR = $savedDataDir } else { Remove-Item env:PSMUX_DATA_DIR -EA SilentlyContinue }
if ($null -ne $savedConpty) { $env:PSMUX_CONPTY_DIR = $savedConpty }
Remove-Item $work -Recurse -Force -EA SilentlyContinue

Write-Host "`n=== PR #781 synchronized output: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:TestsSkipped) skipped ===" -ForegroundColor Cyan
exit $script:TestsFailed
