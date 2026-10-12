# Issue #597 (follow up): psmux answers DA1, DA2, DSR and DECRQM itself, on a
# console host that forwards them instead of answering them.
#
# The inbox Windows console host answers those four and never forwards them, so
# the default path cannot exercise this at all: a PSMUX_PANE_RAW=1 capture of a
# pane on 26200 contains no `ESC[c` whatsoever.  The only host on hand that does
# forward them is OpenConsole, which psmux can be pointed at with
# PSMUX_CONPTY_DIR (an opt in, nothing is bundled).  So this suite downloads the
# MIT licensed `Microsoft.Windows.Console.ConPTY` package from nuget.org when it
# is not already unpacked, and skips cleanly when there is no network.
#
# Measured before the fix, with that host:
#
#   DA1         ESC [ c          -> 0 bytes
#   DA2         ESC [ > c        -> 0 bytes
#   DECRQM 2026 ESC [ ? 2026 $ p -> 0 bytes
#   DECRQM 1006 ESC [ ? 1006 $ p -> 0 bytes
#   prompt visible at 3777 / 3747 / 3799 ms  (629 / 676 / 627 ms on the inbox host)
#
# The three seconds are not a psmux slowdown: OpenConsole 1.24 opens the pane by
# writing `ESC[1t ESC[c` toward psmux and parks the child inside its console
# connect until the DA1 answer arrives.  psmux still does not answer `ESC[1t`
# (tmux's input_csi_dispatch_winops `case 1:` only breaks), so a sub second
# prompt here is what proves DA1 was the sequence being waited on.
#
# tmux parity, input.c: DA1 1581-1595, DA2 1597-1608, DSR 1722-1737,
# DECRQM private 1650-1721, DECRQM ANSI 1637-1649.
#
# Layers: byte exact E2E through tests/query_probe_child.cs in a detached pane,
#         plus the launch timing the same opt in used to cost.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$NS = "i597da"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($msg) { Write-Host "  [PASS] $msg" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red; $script:TestsFailed++ }
function Write-Skip($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Info($msg) { Write-Host "  [INFO] $msg" -ForegroundColor DarkGray }

# psmux refuses to start a client from inside a pane, and the runner may be one.
foreach ($v in 'PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE','PSMUX') {
    Remove-Item "env:$v" -EA SilentlyContinue
}

# ── find or fetch a console host that forwards the queries ────────────────────

$hostDir = $env:PSMUX_TEST_CONPTY_DIR
if (-not $hostDir -or -not (Test-Path (Join-Path $hostDir "conpty.dll"))) {
    $hostDir = "$env:TEMP\occ-conpty\nugetstable"
}
if (-not ((Test-Path (Join-Path $hostDir "conpty.dll")) -and (Test-Path (Join-Path $hostDir "OpenConsole.exe")))) {
    Write-Info "no unpacked console host at $hostDir, trying nuget.org"
    $ver = "1.24.260710001"
    $url = "https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/$ver/microsoft.windows.console.conpty.$ver.nupkg"
    $dl = "$env:TEMP\psmux_i597_conpty_dl"
    try {
        New-Item -ItemType Directory -Force $dl | Out-Null
        New-Item -ItemType Directory -Force $hostDir | Out-Null
        Invoke-WebRequest -Uri $url -OutFile "$dl\pkg.zip" -UseBasicParsing -TimeoutSec 60
        Expand-Archive -Path "$dl\pkg.zip" -DestinationPath "$dl\x" -Force
        Copy-Item "$dl\x\runtimes\win-x64\native\conpty.dll" $hostDir -Force
        Copy-Item "$dl\x\build\native\runtimes\x64\OpenConsole.exe" $hostDir -Force
    } catch {
        Write-Skip "could not fetch Microsoft.Windows.Console.ConPTY ($($_.Exception.Message.Split("`n")[0]))"
        Write-Host "`n=== Issue #597 DA replies on OpenConsole: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:TestsSkipped) skipped ===" -ForegroundColor Cyan
        exit 0
    }
}
if (-not ((Test-Path (Join-Path $hostDir "conpty.dll")) -and (Test-Path (Join-Path $hostDir "OpenConsole.exe")))) {
    Write-Skip "no conpty.dll + OpenConsole.exe pair available, nothing to test"
    Write-Host "`n=== Issue #597 DA replies on OpenConsole: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:TestsSkipped) skipped ===" -ForegroundColor Cyan
    exit 0
}
Write-Info "console host: $hostDir (conpty.dll $((Get-Item (Join-Path $hostDir 'conpty.dll')).VersionInfo.FileVersion))"
$env:PSMUX_CONPTY_DIR = $hostDir

# ── compile the probe ─────────────────────────────────────────────────────────

$repoTests = Split-Path -Parent $MyInvocation.MyCommand.Path
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) { $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe" }
$probe = "$env:TEMP\psmux_i597_da_probe.exe"
Remove-Item $probe -Force -EA SilentlyContinue
& $csc /nologo /optimize /out:$probe (Join-Path $repoTests "query_probe_child.cs") 2>&1 | Out-Null
if (-not (Test-Path $probe)) {
    Write-Skip "could not compile query_probe_child.cs"
    Write-Host "`n=== Issue #597 DA replies on OpenConsole: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:TestsSkipped) skipped ===" -ForegroundColor Cyan
    exit 0
}

$log = "$env:TEMP\psmux_i597_da_probe.txt"
& $PSMUX -L $NS kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# ── part 1: the pane's console host is really OpenConsole ────────────────────

Write-Host "`n[Part 1] the opt in really loaded the other console host" -ForegroundColor Yellow
& $PSMUX -L $NS new-session -d -s da_host -x 100 -y 30 2>&1 | Out-Null
Start-Sleep -Milliseconds 1500
$srv = @(Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" -EA SilentlyContinue |
    Where-Object { $_.ExecutablePath -eq $PSMUX })
$occ = @()
foreach ($s in $srv) {
    $occ += @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($s.ProcessId)" -EA SilentlyContinue |
        Where-Object { $_.Name -eq 'OpenConsole.exe' })
}
if ($occ.Count -gt 0) {
    Write-Pass "the pane is hosted by OpenConsole.exe, not the inbox conhost"
} else {
    Write-Fail "no OpenConsole.exe child of a psmux server: PSMUX_CONPTY_DIR did not take"
}
& $PSMUX -L $NS kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# ── part 2: the four answers, byte exact ─────────────────────────────────────

Write-Host "`n[Part 2] DA1, DA2, DSR and DECRQM answered by psmux" -ForegroundColor Yellow
Remove-Item $log -Force -EA SilentlyContinue
& $PSMUX -L $NS new-session -d -s da_probe -x 100 -y 30 "$probe $log 900" 2>&1 | Out-Null
$deadline = (Get-Date).AddSeconds(90)
while ((Get-Date) -lt $deadline -and -not (Test-Path $log)) { Start-Sleep -Milliseconds 300 }
if (-not (Test-Path $log)) {
    Write-Fail "the probe never wrote its log"
    & $PSMUX -L $NS kill-server 2>&1 | Out-Null
    Write-Host "`n=== Issue #597 DA replies on OpenConsole: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:TestsSkipped) skipped ===" -ForegroundColor Cyan
    exit 1
}
Start-Sleep -Milliseconds 300
$out = Get-Content $log -Raw
Write-Host ($out.TrimEnd())

function Get-ProbeHex($text, $name) {
    foreach ($line in ($text -split "`r?`n")) {
        if ($line.StartsWith("$name ") -and $line -match 'hex=\[([^\]]*)\]') { return $Matches[1] }
    }
    return $null
}

# tmux input.c: 1589 "\033[?1;2c", 1602 "\033[>84;0;0c",
#               1650-1721 "\033[?%d;%d$y".
# 2026 is reported 2 (reset) like tmux since PR #781 gave psmux synchronized
# output (tmux input.c, case 2026: MODE_SYNC ? 1 : 2).  The inbox console host
# still answers 0 itself; only a host that forwards the query reaches psmux.
# 1006 is SGR mouse reporting, which the probe has not enabled, so tmux's
# value semantics make it 2 (reset).
foreach ($case in @(
    @{ n = "DA1";         want = "1b 5b 3f 31 3b 32 63";                   why = "ESC[?1;2c, tmux input.c:1589" },
    @{ n = "DA2";         want = "1b 5b 3e 38 34 3b 30 3b 30 63";          why = "ESC[>84;0;0c, tmux input.c:1602" },
    @{ n = "DECRQM_2026"; want = "1b 5b 3f 32 30 32 36 3b 32 24 79";       why = "ESC[?2026;2`$y, reset (PR #781)" },
    @{ n = "DECRQM_1006"; want = "1b 5b 3f 31 30 30 36 3b 32 24 79";       why = "ESC[?1006;2`$y, reset" },
    @{ n = "DSR_STATUS";  want = "1b 5b 30 6e";                            why = "ESC[0n, tmux input.c:1727" }
)) {
    $got = Get-ProbeHex $out $case.n
    if ($null -eq $got) {
        Write-Fail "could not read the $($case.n) result line"
    } elseif ($got -eq $case.want) {
        Write-Pass "$($case.n) answered byte exact: $($case.why)"
    } elseif ($got -eq "") {
        Write-Fail "$($case.n) got 0 bytes back: the query went unanswered"
    } else {
        Write-Fail "$($case.n) answered [$got], expected [$($case.want)]"
    }
}

# DSR-CPR keeps belonging to the ESC[6n responder, which reports the real
# cursor.  It must still come back exactly once, not twice.
$got = Get-ProbeHex $out "DSR_CPR"
$cprText = ""
if (-not [string]::IsNullOrEmpty($got)) {
    $cprText = -join ($got -split ' ' | ForEach-Object { [char][byte]("0x$_") })
}
if ($cprText -match '^\x1b\[\d+;\d+R$') {
    Write-Pass "DSR-CPR still answered exactly once by the ESC[6n responder: [$got]"
} else {
    Write-Fail "DSR-CPR answer changed shape: [$got]"
}

# XTVERSION is the reader thread's, and must be untouched by any of this.
$ver = ((& $PSMUX -V) | Select-Object -First 1) -replace '^tmux\s+', ''
$wantXtv = (@(0x1b, 0x50, 0x3e, 0x7c) + ([System.Text.Encoding]::ASCII.GetBytes("tmux $($ver.Trim())")) + @(0x1b, 0x5c) |
    ForEach-Object { $_.ToString('x2') }) -join ' '
$got = Get-ProbeHex $out "XTVERSION"
if ($got -eq $wantXtv) {
    Write-Pass "XTVERSION is still answered, and still byte exact"
} else {
    Write-Fail "XTVERSION reply changed: [$got] != [$wantXtv]"
}

# Negative control: a non-zero DA parameter is a different request and stays
# unanswered on this host too.
$got = Get-ProbeHex $out "XTVERSION_PARAM1"
if ($got -eq "") {
    Write-Pass "ESC[>1q still drew no reply, so the new scanner did not widen the net"
} else {
    Write-Fail "ESC[>1q was answered with [$got]"
}

& $PSMUX -L $NS kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# ── part 3: the three second stall is gone ───────────────────────────────────

Write-Host "`n[Part 3] a pane on this host reaches a prompt as fast as one on the inbox host" -ForegroundColor Yellow
# The bar is RELATIVE to the inbox conhost on the same box, not a fixed number.
# A Windows 10 box with only Windows PowerShell 5.1 takes 2.2 to 2.6 s to a
# prompt under either host (reporter's measurement on #597), so a fixed 1.5 s
# bar failed there without any stall. What DA1 unanswered costs is a flat
# 3 s hold on top of whatever the shell takes, so: the supplied host must reach
# a prompt within 1500 ms of the inbox host's median, and never 3 s late. The
# prompt regex accepts pwsh, Windows PowerShell and cmd, and SHELL is cleared
# so a bash runner (Git Bash exports it) does not open bash panes instead.
$savedShell = $env:SHELL
Remove-Item env:SHELL -EA SilentlyContinue
function Measure-PromptMs([string]$Tag) {
    $out = @()
    for ($i = 1; $i -le 3; $i++) {
        & $PSMUX -L $NS kill-server 2>&1 | Out-Null
        Start-Sleep -Milliseconds 600
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $PSMUX -L $NS new-session -d -s "da_$Tag$i" -x 100 -y 30 2>&1 | Out-Null
        $ms = -1
        $dl = (Get-Date).AddSeconds(20)
        while ((Get-Date) -lt $dl) {
            $cap = (& $PSMUX -L $NS capture-pane -p -t "da_$Tag$i" 2>&1) -join "`n"
            if ($cap -match '(PS [A-Za-z]:\\[^\r\n]*>|^[A-Za-z]:\\[^\r\n]*>)') { $ms = $sw.ElapsedMilliseconds; break }
            Start-Sleep -Milliseconds 100
        }
        $sw.Stop()
        $out += $ms
        & $PSMUX -L $NS kill-server 2>&1 | Out-Null
        Start-Sleep -Milliseconds 300
    }
    return $out
}
$times = Measure-PromptMs 't'
Write-Info "supplied host, prompt visible at: $($times -join ', ') ms"
Remove-Item env:PSMUX_CONPTY_DIR -EA SilentlyContinue
$inbox = Measure-PromptMs 'i'
$env:PSMUX_CONPTY_DIR = $hostDir
Write-Info "inbox conhost, prompt visible at: $($inbox -join ', ') ms"
if ($savedShell) { $env:SHELL = $savedShell }
$inboxOk = @($inbox | Where-Object { $_ -ge 0 })
$inboxMedian = if ($inboxOk.Count -gt 0) { ($inboxOk | Sort-Object)[[int][Math]::Floor(($inboxOk.Count - 1) / 2)] } else { -1 }
$limit = if ($inboxMedian -ge 0) { $inboxMedian + 1500 } else { 3000 }
$bad = @($times | Where-Object { $_ -lt 0 -or $_ -ge $limit -or $_ -ge 3000 })
if ($bad.Count -eq 0) {
    Write-Pass "no DA1 hold: supplied host $($times -join ', ') ms against inbox median $inboxMedian ms (limit $limit ms)"
} else {
    Write-Fail "a pane still stalled: supplied host $($times -join ', ') ms, inbox median $inboxMedian ms, limit $limit ms (unanswered DA1 costs about 3000 ms)"
}

& $PSMUX -L $NS kill-server 2>&1 | Out-Null
Remove-Item $log -Force -EA SilentlyContinue
Remove-Item env:PSMUX_CONPTY_DIR -EA SilentlyContinue

Write-Host "`n=== Issue #597 DA replies on OpenConsole: $($script:TestsPassed) passed, $($script:TestsFailed) failed, $($script:TestsSkipped) skipped ===" -ForegroundColor Cyan
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
