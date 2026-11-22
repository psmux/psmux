# Issue #597 (follow up): psmux never answered the XTVERSION round trip.
#
# Claude Code asks the terminal `ESC [ > 0 q` three times at startup and logs
# "no XTVERSION reply" when nothing comes back, then falls back to a
# conservative terminal profile.  Measured in a pane of a detached psmux session
# with tests/query_probe_child.cs, before the fix:
#
#   XTVERSION   ESC [ > 0 q      -> 0 bytes
#   DA1         ESC [ c          -> ESC [ ? 61 ; 6 ; 7 ; 21 ; 22 ; 23 ; 24 ; 28 ; 32 ; 42 c
#   DA2         ESC [ > c        -> ESC [ > 0 ; 10 ; 1 c
#   DSR-CPR     ESC [ 6 n        -> ESC [ 1 ; 1 R
#   DECRQM 2026 ESC [ ? 2026 $ p -> ESC [ ? 2026 ; 0 $ y
#
# Everything except XTVERSION is answered by the ConPTY host itself, which also
# swallows those queries: a PSMUX_PANE_RAW=1 capture of the same run contains
# ONLY `1b 5b 3e 30 71` and `1b 5b 3e 71`, never the DA/DSR/DECRQM bytes.  So
# XTVERSION is both the one query psmux can see and the one nobody answers.
#
# tmux parity (tmux next-3.8, input.c):
#   line  343  { 'q', ">", INPUT_CSI_XDA }
#   line 1884  case INPUT_CSI_XDA: n = input_get(ictx, 0, 0, 0);
#   line 1887  if (n == 0) input_reply(ictx, 1, "\033P>|tmux %s\033\\", getversion());
#
# The reply goes in as console input (WriteConsoleInput via send_vt_response),
# not down the PTY writer: the ConPTY is created with
# PSEUDOCONSOLE_WIN32_INPUT_MODE, where a raw VT response wedges the Win32 input
# parser (issue #313).
#
# Layers: byte-exact E2E in a detached pane, the negative control (a non-zero
#         parameter stays unanswered), and the queries ConPTY owns are still
#         answered so the fix did not shadow them.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "i597xtv"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }

$probe = "$env:TEMP\psmux_i597_query_probe.exe"
Remove-Item $probe -Force -EA SilentlyContinue
& $csc /nologo /optimize /out:$probe (Join-Path $repoTests "query_probe_child.cs") 2>&1 | Out-Null
if (-not (Test-Path $probe)) { Write-Host "FATAL: could not compile query_probe_child.cs" -ForegroundColor Red; exit 1 }

# psmux refuses to start a client from inside a pane, and the runner may be one.
foreach ($v in 'PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') {
    Remove-Item "env:$v" -EA SilentlyContinue
}

$log = "$env:TEMP\psmux_i597_query_probe.txt"

# The Windows 10 inbox conhost (build 19045) hands psmux a query only when it
# next draws something, so there the probe writes a letter after each query
# (see query_probe_child.cs).
$paint = if ([Environment]::OSVersion.Version.Build -lt 22000) { " paint" } else { "" }

function Invoke-Probe($sessionName) {
    Remove-Item $log -Force -EA SilentlyContinue
    & $PSMUX -L $NS new-session -d -s $sessionName -x 100 -y 30 "$probe $log 900$paint" 2>&1 | Out-Null
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline -and -not (Test-Path $log)) { Start-Sleep -Milliseconds 300 }
    if (-not (Test-Path $log)) { return $null }
    # The probe writes the file in one shot at the end, but give the write a
    # moment to land before reading it.
    Start-Sleep -Milliseconds 300
    Get-Content $log -Raw
}

function Get-ProbeLine($text, $name) {
    foreach ($line in ($text -split "`r?`n")) {
        if ($line.StartsWith("$name ")) { return $line }
    }
    return ""
}

& $PSMUX -L $NS kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

Write-Host "`n[Part 1] XTVERSION round trip inside a detached pane" -ForegroundColor Yellow
$out = Invoke-Probe "xtv"
if (-not $out) {
    Write-Host "FATAL: the probe never wrote its log" -ForegroundColor Red
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    exit 1
}
Write-Host ($out.TrimEnd())

$version = ((& $PSMUX -V) | Select-Object -First 1) -replace '^tmux\s+', ''
$version = $version.Trim()
$expectedText = "<ESC>P>|tmux $version<ESC>\"
# ESC P > | t m u x   <version> ESC backslash
$expectedHex = (@(0x1b, 0x50, 0x3e, 0x7c) + ([System.Text.Encoding]::ASCII.GetBytes("tmux $version")) + @(0x1b, 0x5c) |
    ForEach-Object { $_.ToString('x2') }) -join ' '

$line = Get-ProbeLine $out "XTVERSION"
if ($line -match 'got=(\d+) bytes') {
    $n = [int]$Matches[1]
    if ($n -eq 0) {
        Write-Fail "XTVERSION got 0 bytes: psmux did not answer ESC[>0q"
    } else {
        Write-Pass "XTVERSION got $n bytes back"
    }
} else {
    Write-Fail "could not read the XTVERSION result line: $line"
}
if ($line -match 'hex=\[([^\]]*)\]') {
    if ($Matches[1] -eq $expectedHex) {
        Write-Pass "XTVERSION reply is byte exact: [$expectedHex]"
    } else {
        Write-Fail "XTVERSION reply bytes [$($Matches[1])] != expected [$expectedHex]"
    }
}
if ($line -match 'text=\[([^\]]*)\]') {
    if ($Matches[1] -eq $expectedText) {
        Write-Pass "XTVERSION reply reads $expectedText"
    } else {
        Write-Fail "XTVERSION reply reads [$($Matches[1])], expected [$expectedText]"
    }
}

Write-Host "`n[Part 2] the parameterless form CSI > q is answered too" -ForegroundColor Yellow
$line = Get-ProbeLine $out "XTVERSION_NOPARAM"
if ($line -match 'hex=\[([^\]]*)\]') {
    if ($Matches[1] -eq $expectedHex) {
        Write-Pass "ESC[>q is answered with the same reply (xterm defaults Ps to 0)"
    } else {
        Write-Fail "ESC[>q reply bytes [$($Matches[1])] != expected [$expectedHex]"
    }
} else {
    Write-Fail "could not read the XTVERSION_NOPARAM result line: $line"
}

Write-Host "`n[Part 3] the queries ConPTY owns still come back" -ForegroundColor Yellow
foreach ($pair in @(
    @("DA1",         '^\x1b\[\?\d+(;\d+)*c$'),
    @("DA2",         '^\x1b\[>\d+(;\d+)*c$'),
    @("DSR_CPR",     '^\x1b\[\d+;\d+R$'),
    # Issue #597 follow up: psmux answers `CSI 5 n` itself now.  The anchored
    # match is the point: on this host exactly one `ESC[0n` must come back, so
    # a second reply from psmux on top of the host's would fail here.
    @("DSR_STATUS",  '^\x1b\[0n$'),
    @("DECRQM_2026", '^\x1b\[\?2026;\d+\$y$'))) {
    $name = $pair[0]
    $line = Get-ProbeLine $out $name
    if ($line -match 'hex=\[([^\]]*)\]') {
        $bytes = if ($Matches[1] -eq '') { @() } else { $Matches[1] -split ' ' | ForEach-Object { [byte]("0x$_") } }
        $s = -join ($bytes | ForEach-Object { [char]$_ })
        if ($s -match $pair[1]) {
            Write-Pass "$name still answered, and psmux did not add a second reply"
        } else {
            Write-Fail "$name answer changed shape: [$($Matches[1])]"
        }
    } else {
        Write-Fail "could not read the $name result line: $line"
    }
}

Write-Host "`n[Part 4] negative control: a non-zero parameter stays unanswered" -ForegroundColor Yellow
# `CSI > 1 q` is a cursor-style request, not a version request. xterm and tmux
# both ignore it, so psmux must not answer it either.
$line = Get-ProbeLine $out "XTVERSION_PARAM1"
if ($line -match 'got=(\d+) bytes') {
    if ([int]$Matches[1] -eq 0) {
        Write-Pass "ESC[>1q drew no reply (cursor-style request, not a version request)"
    } else {
        Write-Fail "ESC[>1q was answered with $($Matches[1]) bytes: $line"
    }
} else {
    Write-Fail "could not read the XTVERSION_PARAM1 result line: $line"
}

& $PSMUX -L $NS kill-server 2>&1 | Out-Null
Remove-Item $log -Force -EA SilentlyContinue

Write-Host "`n=== Issue #597 XTVERSION results: $($script:TestsPassed) passed, $($script:TestsFailed) failed ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
