# Issue #644: "A pane exactly 1 row tall is never repainted by the client".
#
# `split-window -t r:0.0 -l 9999` leaves the pane being split one row tall. That
# is tmux's own arithmetic: `layout_split_sizes` clamps an oversized size to
# `ss - 2` and hands the rest to the other side (tmux layout.c:1321-1325), and
# `PANE_MINIMUM` is 1 (tmux tmux.h:110), so one row is a legal pane there and is
# painted like any other.
#
# psmux agreed about the layout and then contradicted it twice:
#
#   * `resize_window_panes` rounded the pane's pseudoconsole and parser up to 2
#     rows while the slot stayed 1, so `list-panes` reported `pane_height` 2
#     with `pane_top` and `pane_bottom` both 0.
#   * The client, handed a source taller than the rect it had, took the pane for
#     an oversized preview and painted the BOTTOM of the two rows, which for a
#     program that writes to its first row is the blank one.
#
# The pane kept running and `capture-pane` kept returning live content the whole
# time, which is why this reads as a dead program rather than a sizing bug. Only
# the bytes the CLIENT emits can tell the two apart, so this script hosts a real
# attached client inside a CreatePseudoConsole (tests/conptycap.cs, the same
# approach as tests/test_issue639_wide_char_clear.ps1) and counts how many times
# the pane's own text reaches it.
#
# The sizing rule and the renderer are covered with no server at all by
# tests-rs/test_issue644_one_row_pane.rs.

$ErrorActionPreference = "Continue"

$SOCK = "i644"
$script:TestsPassed = 0
$script:TestsFailed = 0
$script:TestsSkipped = 0

function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:TestsPassed++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red;   $script:TestsFailed++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:TestsSkipped++ }
function Write-Test($m) { Write-Host "`n[$m]" -ForegroundColor Cyan }

$PSMUX = $env:PSMUX_TEST_EXE
if (-not $PSMUX) { $PSMUX = (Resolve-Path "$PSScriptRoot\..\target\release\psmux.exe" -EA SilentlyContinue).Path }
if (-not $PSMUX) { $PSMUX = (Get-Command psmux -EA SilentlyContinue).Source }
if (-not $PSMUX) { Write-Host "psmux not found"; exit 1 }

$work = Join-Path $env:TEMP "psmux_i644"
New-Item -ItemType Directory -Force -Path $work | Out-Null

$COLS = 120
$ROWS = 30

# Session names are unique per run. A killed session leaves its name files
# behind in the data root, and `new-session` then refuses that name, so reusing
# fixed names makes the SECOND run of this script fail for a reason that has
# nothing to do with #644.
$RUN = "$PID"
function Sess([string]$n) { return "i644${n}_$RUN" }
function Kill-Sess([string]$n) { & $PSMUX -L $SOCK kill-session -t $n 2>&1 | Out-Null }

# --- the ticker --------------------------------------------------------------
# Rewrites its own first row twice a second, alternating two words. Written with
# [Console]::Out.Write rather than Write-Host because NO_COLOR, which some shells
# export, strips escape sequences out of Write-Host.
$ticker = Join-Path $work "i644_ticker.ps1"
@'
$e = [char]27
$i = 0
while ($true) {
    $i++
    if ($i % 2) { $word = "AAAAAAAA" } else { $word = "BBBBBBBB" }
    [Console]::Out.Write("$e[H$e[2K$word")
    Start-Sleep -Milliseconds 500
}
'@ | Set-Content -Path $ticker -Encoding ASCII

# --- prerequisites -----------------------------------------------------------
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if (-not (Test-Path $csc)) {
    $csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) "csc.exe"
}
$capSrc = Join-Path $PSScriptRoot "conptycap.cs"
$capExe = Join-Path $work "conptycap.exe"
if ((Test-Path $capSrc) -and (Test-Path $csc) -and -not (Test-Path $capExe)) {
    & $csc -nologo -optimize "-out:$capExe" $capSrc 2>&1 | Out-Null
}

# Hosts `attach` under a real pseudoconsole for $DrainMs and returns every byte
# the client wrote. conptycap exits hard, so the client it started is orphaned:
# it is ended here by the pid conptycap logged, never by name.
function Capture-Client {
    param([string]$Session, [int]$DrainMs = 7000)
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
"$PSMUX" -L $SOCK attach -t $Session
"@ | Set-Content -Path $launch -Encoding ASCII
    $outBin = Join-Path $work "client_$Session.bin"
    Remove-Item $outBin -Force -EA SilentlyContinue
    $env:CONPTYCAP_DRAIN_MS = "$DrainMs"
    Start-Process -FilePath $capExe -ArgumentList @($outBin,"$COLS","$ROWS","8",$launch) -Wait -WindowStyle Minimized
    $log = "$outBin.log"
    if (Test-Path $log) {
        $m = Select-String -Path $log -Pattern 'childPid=(\d+)' | Select-Object -First 1
        if ($m) {
            $cpid = [int]$m.Matches[0].Groups[1].Value
            Get-CimInstance Win32_Process -Filter "ParentProcessId=$cpid" -EA SilentlyContinue |
                ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
            Stop-Process -Id $cpid -Force -EA SilentlyContinue
        }
    }
    if (Test-Path $outBin) { return $outBin }
    return $null
}

# How many times the ticker's word CHANGED in the client's byte stream. Zero is
# the bug: the pane is alive, the client paints nothing.
function Count-Transitions([string]$bin) {
    $txt = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($bin))
    $seq = [regex]::Matches($txt, 'AAAAAAAA|BBBBBBBB') | ForEach-Object { $_.Value.Substring(0,1) }
    $n = 0
    for ($i = 1; $i -lt $seq.Count; $i++) { if ($seq[$i] -ne $seq[$i-1]) { $n++ } }
    return $n
}

# Start a detached session and wait until the server really has it. A server
# that is still coming up answers "no server running", which would turn every
# assertion that follows into a failure about the wrong thing.
function Start-Sess {
    param([string]$Session, [string[]]$Command, [int]$TimeoutMs = 15000)
    for ($attempt = 0; $attempt -lt 3; $attempt++) {
        & $PSMUX -L $SOCK new-session -d -s $Session -x $COLS -y $ROWS @Command 2>&1 | Out-Null
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
            & $PSMUX -L $SOCK has-session -t $Session 2>&1 | Out-Null
            if ($LASTEXITCODE -eq 0) { return $true }
            Start-Sleep -Milliseconds 250
        }
        Start-Sleep -Milliseconds 600
    }
    return $false
}

# Watch the pane server side until its content actually CHANGES, and return the
# two different values. Returns $null if it never changed, which means the pane
# is not ticking and there is nothing for a client to paint.
function Get-TickerChange {
    param([string]$Session, [int]$TimeoutMs = 6000)
    $first = $null
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        $now = (& $PSMUX -L $SOCK capture-pane -p -t "${Session}:0.0" 2>&1 | Out-String).Trim()
        if ($now -match 'AAAAAAAA|BBBBBBBB') {
            if ($first -eq $null) { $first = $now }
            elseif ($now -ne $first) { return @($first, $now) }
        }
        Start-Sleep -Milliseconds 250
    }
    return $null
}

function Get-Panes([string]$Session) {
    $out = & $PSMUX -L $SOCK list-panes -t $Session `
        -F "#{pane_index}|#{pane_height}|#{pane_width}|#{pane_top}|#{pane_bottom}|#{pane_left}|#{pane_right}" 2>&1 | Out-String
    $rows = @()
    foreach ($ln in ($out -split "`r?`n")) {
        if ($ln -match '^(\d+)\|(\d+)\|(\d+)\|(\d+)\|(\d+)\|(\d+)\|(\d+)$') {
            $rows += [pscustomobject]@{
                Index = [int]$matches[1]; Height = [int]$matches[2]; Width = [int]$matches[3]
                Top = [int]$matches[4]; Bottom = [int]$matches[5]
                Left = [int]$matches[6]; Right = [int]$matches[7]
            }
        }
    }
    return $rows
}

# pane_height must equal the rows the pane actually occupies. #644 broke exactly
# this: height 2 with top and bottom both 0.
function Check-Geometry([string]$Label, [string]$Session, [int]$ExpectHeight0) {
    # The split is applied and the panes resized asynchronously, so poll rather
    # than sampling once: a single early read sees one pane, or none at all.
    $panes = @()
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 8000) {
        $panes = Get-Panes $Session
        if ($panes.Count -ge 2) { break }
        Start-Sleep -Milliseconds 300
    }
    if ($panes.Count -lt 2) { Write-Fail "$Label : expected 2 panes, saw $($panes.Count)"; return }
    $bad = @()
    foreach ($p in $panes) {
        $rows = $p.Bottom - $p.Top + 1
        $cols = $p.Right - $p.Left + 1
        if ($p.Height -ne $rows) { $bad += "pane $($p.Index) height=$($p.Height) but rows $($p.Top)..$($p.Bottom) = $rows" }
        if ($p.Width  -ne $cols) { $bad += "pane $($p.Index) width=$($p.Width) but cols $($p.Left)..$($p.Right) = $cols" }
    }
    if ($bad.Count -gt 0) {
        Write-Fail "$Label : geometry disagrees with itself"
        $bad | ForEach-Object { Write-Host "        $_" -ForegroundColor DarkYellow }
        return
    }
    if ($ExpectHeight0 -gt 0 -and $panes[0].Height -ne $ExpectHeight0) {
        Write-Fail "$Label : pane 0 should be $ExpectHeight0 rows, got $($panes[0].Height)"
        return
    }
    Write-Pass "$Label : pane 0 is $($panes[0].Height) row(s), every pane_height matches its own top/bottom"
}

# === RUN =====================================================================
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

# --- 1. the reported case, and its two controls ------------------------------
# -l 9999 leaves 1 row, -l 27 leaves 2, -l 26 leaves more. The 2 row case is the
# reporter's own control: it always worked, and must keep working.
$cases = @(
    @{ L = 9999; Rows = 1 },
    @{ L = 27;   Rows = 2 },
    @{ L = 26;   Rows = 3 }   # tmux 3.4 on 120x30: 30 - 26 - 1 border = 3
)
$counts = @{}
foreach ($c in $cases) {
    $l = $c.L
    Write-Test "#644 split -l $l : the pane the split leaves behind keeps repainting"
    $sess = Sess "_$l"
    Kill-Sess $sess
    Start-Sleep -Milliseconds 400
    if (-not (Start-Sess $sess @("powershell -NoProfile -ExecutionPolicy Bypass -File $ticker"))) {
        Write-Fail "-l $l : session did not start"; continue
    }
    Start-Sleep -Milliseconds 1200

    & $PSMUX -L $SOCK split-window -t "${sess}:0.0" -l $l cmd.exe 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Fail "-l $l : split failed"; Kill-Sess $sess; continue }
    Start-Sleep -Milliseconds 800

    Check-Geometry "-l $l" $sess $c.Rows

    # MANDATORY GATE. The bug is "the pane changes and the client does not
    # paint it", so the pane has to be seen changing first. If the ticker is
    # stalled (a loaded box can starve it) then a client with nothing to paint
    # is CORRECT, and counting its updates would report a failure about the
    # wrong thing.
    $ticking = Get-TickerChange $sess
    if (-not $ticking) {
        Write-Skip "-l $l : the ticker is not changing server side, nothing to measure"
        Kill-Sess $sess
        continue
    }
    Write-Pass "-l $l : capture-pane shows the pane running ($($ticking[0]) then $($ticking[1]))"

    if (-not (Test-Path $capExe)) {
        Write-Skip "-l $l : csc.exe or tests/conptycap.cs unavailable, client byte capture skipped"
        Kill-Sess $sess
        continue
    }
    $bin = Capture-Client -Session $sess
    Kill-Sess $sess
    if (-not $bin) { Write-Skip "-l $l : no client bytes captured"; continue }

    $tr = Count-Transitions $bin
    $counts[$l] = $tr
    # Seven seconds of a twice-a-second ticker is about 13 changes. Anything
    # above a handful proves the pane is being painted; zero is the bug.
    if ($tr -ge 5) {
        Write-Pass "-l $l : $tr content updates reached the client"
    } else {
        Write-Fail "-l $l : only $tr content updates reached the client (this is #644)"
    }
}

# The 1 row pane must not be meaningfully slower to paint than the 2 row one.
Write-Test "#644 one row keeps up with two rows"
if ($counts.ContainsKey(9999) -and $counts.ContainsKey(27)) {
    $one = $counts[9999]; $two = $counts[27]
    if ($one -ge [math]::Floor($two * 0.7)) {
        Write-Pass "1 row got $one updates against $two for 2 rows"
    } else {
        Write-Fail "1 row got $one updates against $two for 2 rows"
    }
} else {
    Write-Skip "not both cases measured"
}

# --- 2. every other route to a one cell pane ---------------------------------
# The reporter believed `-l 1` and `resize-pane -y 1` could not produce one, and
# they can: the layout gave a 1 cell slot in every one of these and only
# pane_height disagreed.
Write-Test "#644 geometry stays consistent however a one cell slot is reached"
$edge = @(
    @{ Name = "l1";     Args = @("-l","1");      Vertical = $true  },
    @{ Name = "l29";    Args = @("-l","29");     Vertical = $true  },
    @{ Name = "lbig";   Args = @("-l","9999");   Vertical = $true  },
    @{ Name = "hlbig";  Args = @("-h","-l","9999"); Vertical = $false }
)
foreach ($e in $edge) {
    $sess = Sess "e_$($e.Name)"
    Kill-Sess $sess
    Start-Sleep -Milliseconds 300
    if (-not (Start-Sess $sess @("cmd.exe"))) { Write-Fail "split $($e.Name) : session did not start"; continue }
    Start-Sleep -Milliseconds 600
    & $PSMUX -L $SOCK split-window -t "${sess}:0.0" @($e.Args) cmd.exe 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
    Check-Geometry "split $($e.Name)" $sess 0
    Kill-Sess $sess
}

# resize-pane down to a single cell, both axes.
foreach ($axis in @("-y","-x")) {
    $sess = Sess "r$($axis.Substring(1))"
    Kill-Sess $sess
    Start-Sleep -Milliseconds 300
    if (-not (Start-Sess $sess @("cmd.exe"))) { Write-Fail "resize-pane $axis 1 : session did not start"; continue }
    Start-Sleep -Milliseconds 600
    $dir = if ($axis -eq "-y") { "-v" } else { "-h" }
    & $PSMUX -L $SOCK split-window -t "${sess}:0.0" $dir cmd.exe 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
    & $PSMUX -L $SOCK resize-pane -t "${sess}:0.0" $axis 1 2>&1 | Out-Null
    Start-Sleep -Milliseconds 700
    Check-Geometry "resize-pane $axis 1" $sess 0
    Kill-Sess $sess
}

# --- 3. capture-pane agrees with the pane's own size -------------------------
Write-Test "#644 capture-pane of a one row pane returns exactly one row"
$sess = Sess "cap"
Kill-Sess $sess
Start-Sleep -Milliseconds 300
$capStarted = Start-Sess $sess @("powershell -NoProfile -ExecutionPolicy Bypass -File $ticker")
Start-Sleep -Milliseconds 1200
& $PSMUX -L $SOCK split-window -t "${sess}:0.0" -l 9999 cmd.exe 2>&1 | Out-Null
Start-Sleep -Milliseconds 900
$panes = @()
$sw = [System.Diagnostics.Stopwatch]::StartNew()
while ($sw.ElapsedMilliseconds -lt 8000) {
    $panes = Get-Panes $sess
    if ($panes.Count -ge 2) { break }
    Start-Sleep -Milliseconds 300
}
$cap = (& $PSMUX -L $SOCK capture-pane -p -t "${sess}:0.0" 2>&1 | Out-String)
$lines = @($cap -split "`r?`n" | Where-Object { $_ -ne "" })
if (-not $capStarted -or $panes.Count -lt 2) {
    Write-Skip "capture-pane check: the session did not come up, nothing to compare"
} elseif ($panes[0].Height -eq 1 -and $lines.Count -eq 1 -and $lines[0] -match 'AAAAAAAA|BBBBBBBB') {
    Write-Pass "one row pane, one captured line: [$($lines[0])]"
} else {
    Write-Fail "pane height $($panes[0].Height), captured $($lines.Count) line(s): $($lines -join ' / ')"
}
Kill-Sess $sess

# --- 4. a click still lands in the one row pane ------------------------------
# The reporter noted clicks were delivered even while nothing painted, so the
# fix must not have cost that. Driven through an attached client that accepts
# input (tests/docker_conpty_attach_host.cs is a generic Win32 ConPTY host).
Write-Test "#644 a click at row 1 selects the one row pane"
$hostSrc = Join-Path $PSScriptRoot "docker_conpty_attach_host.cs"
$hostExe = Join-Path $work "attachhost644.exe"
if ((Test-Path $hostSrc) -and (Test-Path $csc) -and -not (Test-Path $hostExe)) {
    & $csc -nologo -optimize "-out:$hostExe" $hostSrc 2>&1 | Out-Null
}
if (-not (Test-Path $hostExe)) {
    Write-Skip "csc.exe or tests/docker_conpty_attach_host.cs unavailable, click check skipped"
} else {
    $sess = Sess "m"
    Kill-Sess $sess
    Start-Sleep -Milliseconds 300
    $mouseStarted = Start-Sess $sess @("cmd.exe")
    Start-Sleep -Milliseconds 600
    & $PSMUX -L $SOCK set-option -g mouse on 2>&1 | Out-Null
    & $PSMUX -L $SOCK split-window -t "${sess}:0.0" -l 9999 cmd.exe 2>&1 | Out-Null
    Start-Sleep -Milliseconds 900

    $ctrl = Join-Path $work "i644m.ctrl"
    $out  = Join-Path $work "i644m.bin"
    $log  = Join-Path $work "i644m.log"
    Remove-Item $ctrl,$out,$log -Force -EA SilentlyContinue
    New-Item -ItemType File -Path $ctrl | Out-Null
    $launch = Join-Path $work "i644m_attach.cmd"
@"
@echo off
set PSMUX_SESSION=
set PSMUX_SESSION_NAME=
set PSMUX_PANE=
set TMUX=
set TMUX_PANE=
set NO_COLOR=
"$PSMUX" -L $SOCK attach -t $sess
"@ | Set-Content -Path $launch -Encoding ASCII

    $hp = Start-Process -FilePath $hostExe -ArgumentList @($ctrl,$out,$log,"$COLS","$ROWS",$launch) -PassThru -WindowStyle Minimized

    # Wait for the attach to settle. A client that is still coming up has not
    # registered for mouse input yet, and a click sent into that window is
    # simply dropped, which would read as the fix having cost mouse delivery.
    function Active-Pane {
        $v = (& $PSMUX -L $SOCK display-message -p -t $sess "#{pane_index}" 2>&1 | Out-String).Trim()
        if ($v -match '^\d+$') { return $v }
        return $null
    }
    $before = $null
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 12000) {
        $before = Active-Pane
        if ($before -ne $null -and (Test-Path $out) -and (Get-Item $out).Length -gt 0) { break }
        Start-Sleep -Milliseconds 400
    }
    Start-Sleep -Milliseconds 800

    # SGR left press and release at column 10, row 1 (1 based), inside pane 0.
    function To-Hex([string]$s) { ($s.ToCharArray() | ForEach-Object { '{0:x2}' -f [int]$_ }) -join '' }
    $after = $null
    for ($try = 0; $try -lt 3; $try++) {
        Add-Content -Path $ctrl -Value ("HEX " + (To-Hex ([char]27 + "[<0;10;1M")))
        Start-Sleep -Milliseconds 400
        Add-Content -Path $ctrl -Value ("HEX " + (To-Hex ([char]27 + "[<0;10;1m")))
        Start-Sleep -Seconds 2
        $after = Active-Pane
        if ($after -eq "0") { break }
    }
    Add-Content -Path $ctrl -Value "QUIT"
    Start-Sleep -Seconds 2

    if (Test-Path $log) {
        $m = Select-String -Path $log -Pattern 'childPid=(\d+)' | Select-Object -First 1
        if ($m) {
            $cpid = [int]$m.Matches[0].Groups[1].Value
            Get-CimInstance Win32_Process -Filter "ParentProcessId=$cpid" -EA SilentlyContinue |
                ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
            Stop-Process -Id $cpid -Force -EA SilentlyContinue
        }
    }
    if ($hp -and -not $hp.HasExited) { Stop-Process -Id $hp.Id -Force -EA SilentlyContinue }
    & $PSMUX -L $SOCK set-option -g mouse off 2>&1 | Out-Null
    Kill-Sess $sess

    if (-not $mouseStarted) {
        Write-Fail "the click check could not start its session"
    } elseif ($before -eq "1" -and $after -eq "0") {
        Write-Pass "the click moved the active pane from 1 to 0, the one row pane"
    } elseif ($after -eq "0") {
        Write-Pass "the one row pane is active after the click (was '$before')"
    } elseif ([Environment]::OSVersion.Version.Build -lt 22000) {
        # The click is an SGR report written into the client's pseudo console,
        # and the inbox one of Windows 10 (19045) has no ConPTY mouse (#457).
        Write-Skip "Windows 10 pseudo console hands the client no click (active '$before' then '$after')"
    } else {
        Write-Fail "the click did not reach the one row pane (active '$before' then '$after')"
    }
}

# === SUMMARY =================================================================
& $PSMUX -L $SOCK kill-server 2>&1 | Out-Null
Start-Sleep -Milliseconds 400

Write-Host ""
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host " #644 one row pane repaint" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host "  Passed:  $script:TestsPassed" -ForegroundColor Green
Write-Host "  Failed:  $script:TestsFailed" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Gray" })
Write-Host "  Skipped: $script:TestsSkipped" -ForegroundColor Yellow
if ($script:TestsFailed -gt 0) { exit 1 }
exit 0
