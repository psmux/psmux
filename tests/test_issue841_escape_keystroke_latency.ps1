# Issue #841 follow up: a typed Escape must reach the pane as fast as a letter.
#
# The client held every bare Escape for a fixed 50 ms (EscCoalesce, added for
# #611) on every input route and ignored escape-time.  Measured before the fix
# with this suite's harness, mean ms from the terminal writing the key to the
# pane child reading its first byte, all on one clock:
#
#   route                               letter  Escape  Alt+x  Up
#   win32-input-mode (Windows Terminal)   0.9    50.7    1.6   0.7
#   classic console window (key press)    0.9    50.8     -     -
#   VT route (WezTerm/Alacritty/ssh)      1.0    60.1    1.9   1.2
#   VT route, escape-time 10              0.9    53.8    1.3   0.9
#
# After: win32 0.8, classic 0.8, VT default 59.6 (unchanged by design: the
# byte stream is ambiguous there, so it keeps a wait, now escape-time capped
# at 50 ms), VT with escape-time 10: 14.8, escape-time 0: 1.7.
#
# How a key press is told apart from an Escape the console parsed out of a
# VT byte (the #611 case, still held and still merged into Alt+Enter, see
# tests/test_issue611_shift_enter_event_modifiers.ps1): a key press is a DOWN
# record with nothing queued behind it; a parsed byte arrives as DOWN and UP in
# the same read.
#
# Layers: a real pseudoconsole host (conpty_timed_host.cs) playing Windows
# Terminal and a VT terminal in front of an attached client; a classic console
# window driven by WriteConsoleInput (injector_timed.cs) with the key held
# down 80 ms like a person; the pane child is console_record_dump.cs in VT
# input mode.  Everything uses one Windows clock, so no drift correction.

param([int]$Reps = 4, [int]$GapMs = 350, [double]$MaxExtraMs = 10)

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_BIN) { $env:PSMUX_BIN } elseif ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:TestsPassed = 0
$script:TestsFailed = 0
function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$env:PSMUX_RUN_NOPAUSE = '1'
foreach ($v in 'PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX','PSMUX_VT_INPUT') { Remove-Item "env:$v" -EA SilentlyContinue }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$work = Join-Path $env:TEMP "psmux_i841k_$PID"
New-Item -ItemType Directory -Force $work | Out-Null
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$host_ = Join-Path $work "conpty_timed_host.exe"
$dumpExe = Join-Path $work "console_record_dump.exe"
$inj = Join-Path $work "injector_timed.exe"
foreach ($pair in @(@($host_, 'conpty_timed_host.cs'), @($dumpExe, 'console_record_dump.cs'), @($inj, 'injector_timed.cs'))) {
    & $csc /nologo /optimize /out:$($pair[0]) (Join-Path $repoTests $pair[1]) 2>&1 | Out-Null
    if (-not (Test-Path $pair[0])) { Write-Host "FATAL: could not compile $($pair[1])" -ForegroundColor Red; exit 1 }
}

function W32Hex($vk, $sc, $uc, $down, $cs) {
    (("$([char]27)[$vk;$sc;$uc;$down;$cs;1_").ToCharArray() | ForEach-Object { '{0:x2}' -f [int]$_ }) -join ''
}

function Read-Stream($dump) {
    @(Get-Content $dump -EA SilentlyContinue | Where-Object { $_ -match ' DOWN ' } | ForEach-Object {
        if ($_ -match '^([\d.]+) .* ch=0x00([0-9A-F]{2})') { [pscustomobject]@{ t = [double]$Matches[1]; b = $Matches[2].ToLower() } } })
}

# Walk the expected byte groups through the pane's stream; returns kind -> latencies.
function Measure-Stream($stream, $expect, $writes) {
    $i = 0; $out = @{}; $bad = @()
    foreach ($e in $expect) {
        $label = $e[0]; $bytes = $e[1]; $ok = $true
        for ($k = 0; $k -lt $bytes.Count; $k++) { if ($i + $k -ge $stream.Count -or $stream[$i + $k].b -ne $bytes[$k]) { $ok = $false } }
        if (-not $ok) { $bad += $label; continue }
        $kind = $label -replace '\d+$', ''
        if (-not $out[$kind]) { $out[$kind] = @() }
        $out[$kind] += ($stream[$i].t - $writes[$label])
        $i += $bytes.Count
    }
    [pscustomobject]@{ lat = $out; bad = $bad }
}

function Invoke-PtyRoute([string]$route, [string]$conf) {
    $ns = "i841k$PID" + (Get-Random -Maximum 999)
    $lines = @('S 2500'); $expect = @()
    for ($r = 0; $r -lt $Reps; $r++) {
        $c = [char](97 + $r); $ch = '{0:x2}' -f [int]$c; $vk = [int][char]::ToUpper($c)
        if ($route -eq 'vt') {
            $keys = @(@("chr$r", @($ch), @($ch)), @("esc$r", @('1b'), @('1b')), @("altx$r", @('1b78'), @('1b','78')), @("up$r", @('1b5b41'), @('1b','5b','41')))
        } else {
            $keys = @(
                @("chr$r", @((W32Hex $vk 30 ([int]$c) 1 0), 'S80', (W32Hex $vk 30 ([int]$c) 0 0)), @($ch)),
                @("esc$r", @((W32Hex 27 1 27 1 0), 'S80', (W32Hex 27 1 27 0 0)), @('1b')),
                @("altx$r", @((W32Hex 18 56 0 1 2), (W32Hex 88 45 120 1 2), 'S80', (W32Hex 88 45 120 0 2), (W32Hex 18 56 0 0 0)), @('1b','78')),
                @("up$r", @((W32Hex 38 72 0 1 256), 'S80', (W32Hex 38 72 0 0 256)), @('1b','5b','41')))
        }
        foreach ($k in $keys) {
            $expect += ,@($k[0], $k[2]); $first = $true
            foreach ($h in $k[1]) {
                if ($h -match '^S(\d+)$') { $lines += "S $($Matches[1])"; continue }
                $lines += "H $(if ($first) { $k[0] } else { '_' }) $h"; $first = $false
            }
            $lines += "S $GapMs"
        }
    }
    $lines += 'S 800'
    $script = Join-Path $work "script_$route.txt"; $lines | Set-Content $script
    $dump = Join-Path $work "dump_$route.txt"; $hlog = Join-Path $work "hostlog_$route.txt"
    $cfg = Join-Path $work "psmux_$route.conf"; [IO.File]::WriteAllText($cfg, "set -g status off`n$conf`n")
    $secs = [int]((($lines | Where-Object { $_ -match '^S' } | ForEach-Object { [int]($_ -split ' ')[1] } | Measure-Object -Sum).Sum) / 1000) + 3
    if ($route -eq 'vt') { $env:PSMUX_VT_INPUT = '1' }
    $h = Start-Process $host_ -ArgumentList $hlog, $script, (Join-Path $work "out_$route.bin"), '--', "`"$PSMUX`" -L $ns -f `"$cfg`" new-session -s m `"$dumpExe $dump $secs vt`"" -PassThru -WindowStyle Minimized
    Remove-Item env:PSMUX_VT_INPUT -EA SilentlyContinue
    if (-not $h.WaitForExit(($secs + 30) * 1000)) { Stop-Process -Id $h.Id -Force -EA SilentlyContinue }
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    $writes = @{}
    Get-Content $hlog -EA SilentlyContinue | Where-Object { $_ -notmatch 'CHILD' } | ForEach-Object { $x = $_ -split ' ', 2; if ($x[1] -ne '_') { $writes[$x[1]] = [double]$x[0] } }
    Measure-Stream (Read-Stream $dump) $expect $writes
}

function Invoke-ClassicRoute {
    $ns = "i841c$PID" + (Get-Random -Maximum 999)
    $dump = Join-Path $work "dump_classic.txt"
    $cfg = Join-Path $work "psmux_classic.conf"; [IO.File]::WriteAllText($cfg, "set -g status off`n")
    $keys = @()
    for ($r = 0; $r -lt $Reps; $r++) { foreach ($k in @([string][char](97 + $r), 'ESC')) { $keys += @("D:$k", 'S80', "U:$k", "S$GapMs") } }
    $secs = [int]($Reps * 2 * ($GapMs + 80) / 1000) + 8
    $client = Start-Process -FilePath $PSMUX -ArgumentList '-L', $ns, '-f', "`"$cfg`"", 'new-session', '-s', 'm', "`"$dumpExe $dump $secs vt`"" -PassThru
    for ($i = 0; $i -lt 60 -and -not (Test-Path $dump); $i++) { Start-Sleep -Milliseconds 250 }
    Start-Sleep -Milliseconds 1000
    $ij = Join-Path $work "inject_classic.txt"
    & $inj $client.Id $ij @keys
    Start-Sleep -Milliseconds 800
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    if (-not $client.HasExited) { Stop-Process -Id $client.Id -Force -EA SilentlyContinue }
    $sent = @(Get-Content $ij -EA SilentlyContinue | ForEach-Object { $x = $_ -split ' ', 2; [pscustomobject]@{ t = [double]$x[0]; k = $x[1] } })
    $expect = @(); $writes = @{}; $n = 0
    foreach ($s in $sent) {
        $label = if ($s.k -eq 'ESC') { "esc$n" } else { "chr$n" }
        $bytes = @(if ($s.k -eq 'ESC') { '1b' } else { '{0:x2}' -f [int][char]$s.k })
        $expect += ,@($label, $bytes); $writes[$label] = $s.t; $n++
    }
    Measure-Stream (Read-Stream $dump) $expect $writes
}

function Mean($a) { if (-not $a) { return [double]::NaN }; ($a | Measure-Object -Average).Average }

function Assert-Route($name, $res, [switch]$EscMayWait, [double]$EscCeiling = 0) {
    if ($res.bad.Count -gt 0) { Write-Fail "${name}: keys arrived mangled or missing: $($res.bad -join ', ')"; return }
    $chr = Mean $res.lat['chr']; $esc = Mean $res.lat['esc']
    $line = "{0}: letter {1:F1} ms, Escape {2:F1} ms" -f $name, $chr, $esc
    if ($res.lat['altx']) { $line += (", Alt+x {0:F1} ms, Up {1:F1} ms" -f (Mean $res.lat['altx']), (Mean $res.lat['up'])) }
    $limit = if ($EscCeiling -gt 0) { $EscCeiling } else { $chr + $MaxExtraMs }
    if ($esc -le $limit) { Write-Pass "${line} (Escape within $([math]::Round($limit, 1)) ms)" }
    else { Write-Fail "${line} (Escape over $([math]::Round($limit, 1)) ms: the bare Escape is being held)" }
    foreach ($kind in 'altx', 'up') {
        if ($res.lat[$kind] -and (Mean $res.lat[$kind]) -gt $chr + $MaxExtraMs) { Write-Fail ("{0}: {1} took {2:F1} ms" -f $name, $kind, (Mean $res.lat[$kind])) }
    }
}

Write-Host "`n[Part 1] win32-input-mode route (what Windows Terminal writes)" -ForegroundColor Yellow
Assert-Route "win32-input-mode" (Invoke-PtyRoute 'w32' '')

Write-Host "`n[Part 2] classic console window, key held 80 ms like a person" -ForegroundColor Yellow
Assert-Route "classic console" (Invoke-ClassicRoute)

Write-Host "`n[Part 3] VT route follows escape-time (tmux: a lone ESC byte waits escape-time)" -ForegroundColor Yellow
Assert-Route "VT route, escape-time 10" (Invoke-PtyRoute 'vt' 'set -g escape-time 10') -EscCeiling 30
Assert-Route "VT route, escape-time 0" (Invoke-PtyRoute 'vt' 'set -g escape-time 0')

# The default is tmux 3.5's 10 ms (it was tmux 3.4's 500, capped at a 50 ms
# hold, so 60.1 ms measured on the VT route before the default changed).
Write-Host "`n[Part 4] VT route at the default escape-time" -ForegroundColor Yellow
Assert-Route "VT route, default escape-time" (Invoke-PtyRoute 'vt' '') -EscCeiling 30

# ── Part 5: prefix keys still work with the short escape-time ──────────────
# A bottom pane is active; prefix Up must select the top one, prefix Down
# written as `ESC`, then `[B` 4 ms later (a sequence split across two reads,
# inside escape-time) must come back down, and prefix Alt+Up must resize.
function W32Rec($vk, $sc, $uc, $down, $cs) { "H k $(W32Hex $vk $sc $uc $down $cs)" }
function Invoke-PrefixCheck([string]$route) {
    $ns = "i841p$PID" + (Get-Random -Maximum 999)
    $q = "`"$PSMUX`" -L $ns display -p -t m #{pane_index}:#{pane_height}"
    if ($route -eq 'vt') {
        $prefix = @('H k 02'); $up = @('H k 1b5b41'); $down = @('H k 1b', 'S 4', 'H k 5b42'); $altUp = @('H k 1b5b313b3341')
    } else {
        $prefix = @((W32Rec 17 29 0 1 8), (W32Rec 66 48 2 1 8), (W32Rec 66 48 2 0 8), (W32Rec 17 29 0 0 0))
        $up = @((W32Rec 38 72 0 1 256), (W32Rec 38 72 0 0 256))
        $down = @((W32Rec 40 80 0 1 256), (W32Rec 40 80 0 0 256))
        $altUp = @((W32Rec 18 56 0 1 2), (W32Rec 38 72 0 1 258), (W32Rec 38 72 0 0 258), (W32Rec 18 56 0 0 0))
    }
    $lines = @('S 3000', "X split `"$PSMUX`" -L $ns split-window -v -t m", 'S 800', "X start $q") +
        $prefix + $up + @('S 700', "X up $q") + $prefix + $down + @('S 700', "X down $q") +
        $prefix + $altUp + @('S 700', "X altup $q", 'S 300')
    $script = Join-Path $work "prefix_$route.txt"; $lines | Set-Content $script
    $hlog = Join-Path $work "prefixlog_$route.txt"
    $cfg = Join-Path $work "prefix_$route.conf"; [IO.File]::WriteAllText($cfg, "set -g status off`n")
    if ($route -eq 'vt') { $env:PSMUX_VT_INPUT = '1' }
    $h = Start-Process $host_ -ArgumentList $hlog, $script, (Join-Path $work "prefix_$route.bin"), '--', "`"$PSMUX`" -L $ns -f `"$cfg`" new-session -s m" -PassThru -WindowStyle Minimized
    Remove-Item env:PSMUX_VT_INPUT -EA SilentlyContinue
    if (-not $h.WaitForExit(40000)) { Stop-Process -Id $h.Id -Force -EA SilentlyContinue }
    & $PSMUX -L $ns kill-server 2>&1 | Out-Null
    $r = @{}
    Get-Content $hlog -EA SilentlyContinue | ForEach-Object { if ($_ -match '^\S+ (\w+) => (\d+):(\d+)') { $r[$Matches[1]] = @([int]$Matches[2], [int]$Matches[3]) } }
    $r
}
foreach ($route in 'w32', 'vt') {
    Write-Host "`n[Part 5] prefix keys on the $route route" -ForegroundColor Yellow
    $r = Invoke-PrefixCheck $route
    if (-not $r['start'] -or -not $r['altup']) { Write-Fail "$route prefix check: no pane readings ($(($r.Keys) -join ','))"; continue }
    if ($r['start'][0] -eq 1 -and $r['up'][0] -eq 0) { Write-Pass "$route prefix Up selected the pane above (1 -> 0)" }
    else { Write-Fail "$route prefix Up: pane $($r['start'][0]) -> $($r['up'][0])" }
    if ($r['down'][0] -eq 1) { Write-Pass "$route prefix Down came back (0 -> 1)$(if ($route -eq 'vt') { ', written as ESC then [B 4 ms apart' })" }
    else { Write-Fail "$route prefix Down: pane $($r['up'][0]) -> $($r['down'][0])" }
    if ($r['altup'][1] -ne $r['down'][1]) { Write-Pass "$route prefix Alt+Up resized the pane (height $($r['down'][1]) -> $($r['altup'][1]))" }
    else { Write-Fail "$route prefix Alt+Up left the height at $($r['altup'][1])" }
}

if (-not $env:PSMUX_841_KEEP) { Remove-Item $work -Recurse -Force -EA SilentlyContinue }
Write-Host "`n=== Issue #841 keystroke latency results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
