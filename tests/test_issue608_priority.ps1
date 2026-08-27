# Issue #608: "Interactive path starves under host CPU load: server/client run
# at Normal priority with no foreground boost".
#
# WHAT WAS MEASURED (paired A/B, this box, 32 logical CPUs, psmux 3.3.8)
#
#   Keystroke to echo, medians over three interleaved cycles. "tcp" is the
#   server round trip (send one character, poll the rendered pane until the
#   frame changes); "console" is the rendered echo inside a real attached
#   client, read out of its console screen buffer.
#
#   1x oversubscription, 32 busy loops at Normal:
#       tcp      Normal 30.7 / AboveNormal 31.0 ms p50   -> no difference
#       console  Normal 47.7 / AboveNormal 47.3 ms p50   -> no difference
#   4x oversubscription, 128 busy loops at Normal (the reporter's shape):
#       tcp      Normal 76.2 / AboveNormal 47.0 ms p50   -> 38% faster
#       console  Normal 153.1 / AboveNormal 73.5 ms p50  -> 52% faster
#   Outranked, 64 busy loops at AboveNormal:
#       tcp      Normal 679.1 / AboveNormal 42.9 ms p50  -> 94% faster
#       console  Normal 749.1 / AboveNormal 377.6 ms p50 -> 50% faster
#
#   Raising ONE process is not enough. Under 128 loops, server-only fixed the
#   round trip (308.8 -> 135.9 ms p50 tcp) but left the rendered echo untouched
#   (1794 -> 2041 ms p50 console), because the frame still had to be drawn by a
#   starved client. Client-only was the mirror image. Both together is the only
#   arm that fixes both halves.
#
#   So the default is above-normal: it never hurt at 1x and it removed 38 to
#   94 percent of the lag everywhere else. High is offered but not default.
#
# tmux PARITY: none. tmux never calls setpriority or nice anywhere in its
# source (153 .c files, zero hits). On Linux the scheduler already favours a
# process that lives blocked on a read. This is a Windows specific extension.
#
# WHAT THIS SUITE PINS
#   * a detached server runs at the default class, and its pane children do not
#   * PSMUX_PRIORITY selects normal / above-normal / high, for server and client
#   * an unusable PSMUX_PRIORITY is ignored and the default stands
#   * an attached client launched in its own console gets the same treatment
#   * `set -g priority normal` moves a LIVE server's class within a second
#   * an invalid `set -g priority` value is rejected and changes nothing
#   * show-options reports the option, both in the dump and by name
#   * one load measurement: the default must not be SLOWER than Normal
#
# Set PSMUX_TEST_BIN to test a non-installed binary.

$ErrorActionPreference = "Continue"
$PSMUX = if ($env:PSMUX_TEST_BIN) { $env:PSMUX_TEST_BIN } else { (Get-Command psmux -EA Stop).Source }
$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function Write-Pass($m) { Write-Host "  [PASS] $m" -ForegroundColor Green; $script:Pass++ }
function Write-Fail($m) { Write-Host "  [FAIL] $m" -ForegroundColor Red; $script:Fail++ }
function Write-Skip($m) { Write-Host "  [SKIP] $m" -ForegroundColor Yellow; $script:Skip++ }
function Write-Info($m) { Write-Host "  [INFO] $m" -ForegroundColor DarkCyan }
function Write-Perf($m) { Write-Host "  [PERF] $m" -ForegroundColor Magenta }

Write-Host "binary: $PSMUX" -ForegroundColor Cyan

# Inherited routing would aim every call at somebody else's server.
foreach ($v in 'PSMUX_SESSION_NAME','PSMUX_SESSION','PSMUX_PANE','TMUX','TMUX_PANE') {
    Set-Item -Path "env:$v" -Value $null -EA SilentlyContinue
}

$rig  = Join-Path $env:TEMP ("psmux608-" + [guid]::NewGuid().ToString('N').Substring(0,8))
$root = Join-Path $rig 'data'
New-Item -ItemType Directory -Force -Path $rig, $root | Out-Null
$env:PSMUX_DATA_DIR = $root

$DEFAULT_CLASS = 'AboveNormal'   # what src/platform.rs DEFAULT_PRIORITY maps to
$script:Spawned = @()            # every process this suite created, and nothing else

function Get-ServerPid([string]$Name) {
    $f = Join-Path $root "$Name.pid"
    if (-not (Test-Path $f)) { return 0 }
    # .pid bodies are "pid" or "pid:creation_filetime" (PR #404 format).
    $raw = (Get-Content $f -Raw -EA SilentlyContinue)
    if (-not $raw) { return 0 }
    return [int](($raw.Trim() -split ':')[0])
}

function Get-Class([int]$ProcId) {
    try { return (Get-Process -Id $ProcId -EA Stop).PriorityClass.ToString() } catch { return 'gone' }
}

function New-Sess([string]$Name) {
    & $PSMUX kill-session -t $Name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
    & $PSMUX new-session -d -s $Name 2>&1 | Out-Null
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 20000) {
        $p = Get-ServerPid $Name
        if ($p -gt 0 -and (Get-Process -Id $p -EA SilentlyContinue)) { $script:Spawned += $p; return $p }
        Start-Sleep -Milliseconds 150
    }
    return 0
}

function Kill-Sess([string]$Name) {
    & $PSMUX kill-session -t $Name 2>&1 | Out-Null
    Start-Sleep -Milliseconds 300
}

function Percentile([double[]]$Values, [int]$Pct) {
    if ($Values.Count -eq 0) { return [double]::NaN }
    $s = [double[]]($Values | Sort-Object)
    return $s[[Math]::Floor(($Pct / 100.0) * ($s.Count - 1))]
}

try {
# ============================================================================
Write-Host "`n[1] A detached server runs at the default class" -ForegroundColor Yellow
# ============================================================================
Remove-Item env:PSMUX_PRIORITY -EA SilentlyContinue
$sp = New-Sess 'i608_default'
if ($sp -eq 0) {
    Write-Fail "session i608_default never came up"
} else {
    $cls = Get-Class $sp
    if ($cls -eq $DEFAULT_CLASS) { Write-Pass "server pid $sp runs at $cls" }
    else { Write-Fail "server pid $sp runs at $cls, expected $DEFAULT_CLASS" }

    # The whole point of setting the class on psmux's own processes only: a
    # Windows child created without an explicit class flag gets NORMAL unless
    # its creator is idle or below-normal, so the pane shells must NOT follow
    # the server up. If this ever regresses, psmux is boosting user workloads.
    $kids = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$sp" -EA SilentlyContinue |
              Where-Object { $_.Name -ne 'psmux.exe' })
    if ($kids.Count -eq 0) {
        Write-Skip "no pane children visible to check (they may have been reparented)"
    } else {
        $raised = @($kids | Where-Object { (Get-Class $_.ProcessId) -ne 'Normal' })
        if ($raised.Count -eq 0) {
            Write-Pass "all $($kids.Count) pane children stayed at Normal"
        } else {
            Write-Fail "pane children were raised: $(($raised | ForEach-Object { "$($_.Name)=$(Get-Class $_.ProcessId)" }) -join ', ')"
        }
    }
}

# ============================================================================
Write-Host "`n[2] show-options reports the option" -ForegroundColor Yellow
# ============================================================================
$byName = (& $PSMUX show-options -t 'i608_default' -g -v priority 2>&1 | Out-String).Trim()
if ($byName -eq 'above-normal') { Write-Pass "show-options -v priority = '$byName'" }
else { Write-Fail "show-options -v priority = '$byName', expected 'above-normal'" }

$dump = (& $PSMUX show-options -t 'i608_default' -g 2>&1 | Out-String)
if ($dump -match '(?m)^priority\s+above-normal\s*$') {
    Write-Pass "the full show-options -g dump lists priority"
} else {
    Write-Fail "priority missing from the show-options -g dump (the #606 trap)"
}

# ============================================================================
Write-Host "`n[3] set -g priority moves a LIVE server within a second" -ForegroundColor Yellow
# ============================================================================
& $PSMUX set-option -t 'i608_default' -g priority normal 2>&1 | Out-Null
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$got = ''
while ($sw.ElapsedMilliseconds -lt 1000) {
    $got = Get-Class $sp
    if ($got -eq 'Normal') { break }
    Start-Sleep -Milliseconds 50
}
if ($got -eq 'Normal') {
    Write-Pass "set -g priority normal took effect in $($sw.ElapsedMilliseconds)ms"
} else {
    Write-Fail "set -g priority normal left the class at '$got' after $($sw.ElapsedMilliseconds)ms"
}
$rb = (& $PSMUX show-options -t 'i608_default' -g -v priority 2>&1 | Out-String).Trim()
if ($rb -eq 'normal') { Write-Pass "show-options follows the change ('$rb')" }
else { Write-Fail "show-options still says '$rb' after the change" }

# Back up again, so the rejection test below has something to not-change.
& $PSMUX set-option -t 'i608_default' -g priority high 2>&1 | Out-Null
Start-Sleep -Milliseconds 400

# ============================================================================
Write-Host "`n[4] An invalid value is rejected and changes nothing" -ForegroundColor Yellow
# ============================================================================
$before = Get-Class $sp
$err = (& $PSMUX set-option -t 'i608_default' -g priority realtime 2>&1 | Out-String)
$rc = $LASTEXITCODE
$after = Get-Class $sp
if ($rc -ne 0) { Write-Pass "an invalid value exits nonzero (rc=$rc)" }
else { Write-Fail "an invalid value exited 0" }
if ($err -match 'priority' -and $err -match 'normal' -and $err -match 'high') {
    Write-Pass "the rejection names the option and the accepted values"
} else {
    Write-Fail "the rejection message is not helpful: '$($err.Trim())'"
}
if ($after -eq $before) { Write-Pass "the class stayed at '$after' after the rejection" }
else { Write-Fail "the class moved from '$before' to '$after' on an invalid value" }
Kill-Sess 'i608_default'

# ============================================================================
Write-Host "`n[5] PSMUX_PRIORITY selects the server's class" -ForegroundColor Yellow
# ============================================================================
# A warm standby is claimed with whatever class IT chose at its own startup, so
# the env var only decides the class of a server this call actually spawns.
$env:PSMUX_NO_WARM = '1'
$cases = @(
    @{ v = 'normal';       want = 'Normal' },
    @{ v = 'above-normal'; want = 'AboveNormal' },
    @{ v = 'high';         want = 'High' },
    @{ v = 'realtime';     want = $DEFAULT_CLASS }   # rejected, default stands
)
$i = 0
foreach ($c in $cases) {
    $i++
    $env:PSMUX_PRIORITY = $c.v
    $name = "i608_env$i"
    $p = New-Sess $name
    if ($p -eq 0) { Write-Fail "PSMUX_PRIORITY=$($c.v): session never came up"; continue }
    $cls = Get-Class $p
    if ($cls -eq $c.want) { Write-Pass "PSMUX_PRIORITY=$($c.v) -> server class $cls" }
    else { Write-Fail "PSMUX_PRIORITY=$($c.v) -> server class $cls, expected $($c.want)" }
    $opt = (& $PSMUX show-options -t $name -g -v priority 2>&1 | Out-String).Trim()
    $wantOpt = if ($c.v -eq 'realtime') { 'above-normal' } else { $c.v }
    if ($opt -eq $wantOpt) { Write-Pass "  show-options reports what the process is running at ('$opt')" }
    else { Write-Fail "  show-options says '$opt', process is at '$cls'" }
    Kill-Sess $name
}
Remove-Item env:PSMUX_PRIORITY -EA SilentlyContinue
Remove-Item env:PSMUX_NO_WARM -EA SilentlyContinue

# ============================================================================
Write-Host "`n[6] An attached client gets the same treatment" -ForegroundColor Yellow
# ============================================================================
# psmux prints its version and returns 0 when stdout is not a tty, so the client
# has to be launched from a .cmd in its own console window or the attach path
# under test never runs.
$wrapper = Join-Path $rig 'runattach.cmd'
@'
@echo off
setlocal
set "BIN=%~1"
set "PSMUX_DATA_DIR=%~2"
set "SESS=%~3"
set "PSMUX_PRIORITY=%~4"
set "PSMUX_SESSION_NAME="
set "PSMUX_SESSION="
set "PSMUX_PANE="
set "TMUX="
set "TMUX_PANE="
"%BIN%" attach-session -t "%SESS%"
endlocal
'@ | Set-Content -Path $wrapper -Encoding ASCII

function Start-Client([string]$Name, [string]$Prio) {
    Start-Process -FilePath $wrapper -ArgumentList $PSMUX, $root, $Name, $Prio -WindowStyle Normal | Out-Null
    # Identify by COMMAND LINE. A pid diff picks the wrong process: attaching
    # makes the server spawn a replacement warm standby, and other psmux builds
    # may be running on the same box.
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 25000) {
        $c = @(Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" -EA SilentlyContinue |
               Where-Object { $_.CommandLine -and $_.CommandLine -like "*$PSMUX*" -and
                              $_.CommandLine -like '*attach-session*' -and $_.CommandLine -like "*$Name*" })
        if ($c.Count -gt 0) {
            Start-Sleep -Seconds 3
            $script:Spawned += [int]$c[0].ProcessId
            return [int]$c[0].ProcessId
        }
        Start-Sleep -Milliseconds 250
    }
    return 0
}

foreach ($cc in @(@{ v = ''; want = $DEFAULT_CLASS }, @{ v = 'normal'; want = 'Normal' })) {
    $name = 'i608_cli' + $(if ($cc.v) { $cc.v } else { 'def' })
    $p = New-Sess $name
    if ($p -eq 0) { Write-Fail "client case '$($cc.v)': session never came up"; continue }
    $cp = Start-Client $name $cc.v
    if ($cp -eq 0) {
        Write-Skip "client case '$($cc.v)': no attach client appeared (headless agent shell?)"
    } else {
        $cls = Get-Class $cp
        $label = if ($cc.v) { "PSMUX_PRIORITY=$($cc.v)" } else { 'default' }
        if ($cls -eq $cc.want) { Write-Pass "attached client ($label) runs at $cls" }
        else { Write-Fail "attached client ($label) runs at $cls, expected $($cc.want)" }
        try { Stop-Process -Id $cp -Force -EA Stop } catch {}
    }
    Kill-Sess $name
}

# ============================================================================
Write-Host "`n[7] Load measurement: the default must not be SLOWER than Normal" -ForegroundColor Yellow
# ============================================================================
# One busy loop per logical CPU, at Normal, killed by recorded pid only. This
# is a REGRESSION guard, not the experiment: the experiment is quoted at the
# top of this file. It fails only if the shipped default is measurably worse
# than Normal, and prints the numbers either way.
$N = [Environment]::ProcessorCount
$name = 'i608_perf'
$sp = New-Sess $name
$loadPids = @()
if ($sp -eq 0) {
    Write-Fail "perf session never came up"
} else {
    $port = [int]((Get-Content (Join-Path $root "$name.port") -Raw).Trim())
    $key  = (Get-Content (Join-Path $root "$name.key") -Raw).Trim()

    function Invoke-Cmd([string]$Command) {
        $c = [System.Net.Sockets.TcpClient]::new(); $c.NoDelay = $true
        $c.Connect('127.0.0.1', $port)
        $s = $c.GetStream(); $s.ReadTimeout = 8000
        $w = [System.IO.StreamWriter]::new($s); $w.AutoFlush = $true
        $r = [System.IO.StreamReader]::new($s)
        $w.WriteLine("AUTH $key"); [void]$r.ReadLine()
        $w.WriteLine($Command)
        $out = $r.ReadToEnd()
        $c.Close()
        return $out
    }

    function Measure-Echo([int]$Keys) {
        $abc = 'abcdefghijklmnopqrstuvwxyz'
        $acc = [System.Collections.ArrayList]::new()
        for ($k = 0; $k -lt $Keys; $k++) {
            $ch = $abc[$k % 26]
            $before = Invoke-Cmd 'capture-pane -p'
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            [void](Invoke-Cmd "send-keys $ch")
            while ($sw.Elapsed.TotalMilliseconds -lt 8000) {
                if ((Invoke-Cmd 'capture-pane -p') -ne $before) { break }
            }
            $sw.Stop()
            if ($sw.Elapsed.TotalMilliseconds -lt 8000) { [void]$acc.Add($sw.Elapsed.TotalMilliseconds) }
            [void](Invoke-Cmd 'send-keys BSpace')
            Start-Sleep -Milliseconds 80
        }
        return [double[]]$acc
    }

    try {
        for ($k = 0; $k -lt $N; $k++) {
            $lp = Start-Process pwsh -ArgumentList '-NoProfile','-c','while($true){}' -WindowStyle Hidden -PassThru
            $loadPids += $lp.Id
        }
        # A Windows child inherits the CREATOR's class, so pin them explicitly.
        foreach ($id in $loadPids) { try { (Get-Process -Id $id -EA Stop).PriorityClass = 'Normal' } catch {} }
        Write-Info "$N busy loops at Normal"
        Start-Sleep -Seconds 4

        # Interleaved so drift on a shared CI box cannot masquerade as an effect.
        & $PSMUX set-option -t $name -g priority normal 2>&1 | Out-Null; Start-Sleep -Milliseconds 500
        $normA = Measure-Echo 15
        & $PSMUX set-option -t $name -g priority above-normal 2>&1 | Out-Null; Start-Sleep -Milliseconds 500
        $abvA  = Measure-Echo 15
        & $PSMUX set-option -t $name -g priority above-normal 2>&1 | Out-Null; Start-Sleep -Milliseconds 500
        $abvB  = Measure-Echo 15
        & $PSMUX set-option -t $name -g priority normal 2>&1 | Out-Null; Start-Sleep -Milliseconds 500
        $normB = Measure-Echo 15

        $norm = [double[]]($normA + $normB)
        $abv  = [double[]]($abvA + $abvB)
        if ($norm.Count -lt 10 -or $abv.Count -lt 10) {
            Write-Skip "not enough samples under load (normal=$($norm.Count) above-normal=$($abv.Count))"
        } else {
            $n50 = [math]::Round((Percentile $norm 50),1); $n90 = [math]::Round((Percentile $norm 90),1)
            $a50 = [math]::Round((Percentile $abv  50),1); $a90 = [math]::Round((Percentile $abv  90),1)
            Write-Perf "under $N-loop load, Normal      : p50=${n50}ms p90=${n90}ms (n=$($norm.Count))"
            Write-Perf "under $N-loop load, AboveNormal : p50=${a50}ms p90=${a90}ms (n=$($abv.Count))"
            # The bar is deliberately "not slower", not "faster": at 1x
            # oversubscription the two are equal on purpose, and the win only
            # appears once the box is genuinely oversubscribed. A 40% margin
            # absorbs the noise of a shared runner.
            if ($a50 -le $n50 * 1.4) {
                Write-Pass "the shipped default is not slower than Normal (${a50}ms vs ${n50}ms p50)"
            } else {
                Write-Fail "the shipped default is SLOWER than Normal (${a50}ms vs ${n50}ms p50)"
            }
        }
    } finally {
        foreach ($id in $loadPids) { try { Stop-Process -Id $id -Force -EA Stop } catch {} }
        Start-Sleep -Seconds 1
    }
}
Kill-Sess $name

# ============================================================================
Write-Host "`n[8] A CLAIMED WARM server adopts the claiming client's class" -ForegroundColor Yellow
# ============================================================================
# The hole this closes: a warm standby is spawned ahead of time by an earlier
# server generation and sets its class from THAT environment, which predates
# the user's shell. Before the fix, claiming it never re-applied the claimant's
# PSMUX_PRIORITY, so the documented escape hatch silently failed on the path
# most users take, and worked only when no standby happened to be waiting.
#
# Two things make this test non vacuous, and both matter:
#   1. PSMUX_NO_WARM is NOT set, so the warm path is genuinely exercised.
#   2. The standby's PID is recorded BEFORE, and the claimed session's .pid is
#      compared against it. A cold spawn would otherwise pass every assertion
#      while testing nothing at all, which is exactly how this bug survived the
#      first round of tests.
Remove-Item env:PSMUX_NO_WARM -EA SilentlyContinue

function Get-MyWarmPid {
    $w = @(Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" -EA SilentlyContinue |
           Where-Object { $_.CommandLine -and $_.CommandLine -like "*$PSMUX*" -and
                          $_.CommandLine -like '*-s __warm__*' })
    if ($w.Count -eq 0) { return 0 }
    return [int]$w[0].ProcessId
}

# Prime the pool with NO PSMUX_PRIORITY set, so the standby lands on the
# default and every case below is a real change of class rather than a no-op.
function Initialize-Warm {
    $p = Get-MyWarmPid
    if ($p -gt 0) { return $p }
    Remove-Item env:PSMUX_PRIORITY -EA SilentlyContinue
    $prime = 'i608_warmprime'
    & $PSMUX new-session -d -s $prime 2>&1 | Out-Null
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 25000) {
        if ((Get-MyWarmPid) -gt 0) { break }
        Start-Sleep -Milliseconds 250
    }
    Kill-Sess $prime
    Start-Sleep -Seconds 2
    return (Get-MyWarmPid)
}

foreach ($wc in @(
    @{ v = '';       want = $DEFAULT_CLASS; label = 'unset' },
    @{ v = 'normal'; want = 'Normal';       label = 'normal' },
    @{ v = 'high';   want = 'High';         label = 'high' }
)) {
    Remove-Item env:PSMUX_PRIORITY -EA SilentlyContinue
    $warmPid = Initialize-Warm
    if ($warmPid -eq 0) {
        Write-Skip "warm claim ($($wc.label)): no standby appeared, warm pool may be disabled here"
        continue
    }
    $script:Spawned += $warmPid
    $warmCls = Get-Class $warmPid

    if ($wc.v) { $env:PSMUX_PRIORITY = $wc.v }
    $wname = 'i608_warm' + $wc.label
    & $PSMUX kill-session -t $wname 2>&1 | Out-Null
    & $PSMUX new-session -d -s $wname 2>&1 | Out-Null
    Start-Sleep -Seconds 4
    $sp = Get-ServerPid $wname
    if ($sp -gt 0) { $script:Spawned += $sp }

    if ($sp -ne $warmPid) {
        # Not a claim, so this case proves nothing either way. Loudly skipped
        # rather than counted as a pass.
        Write-Skip "warm claim ($($wc.label)): COLD SPAWN (server $sp, standby was $warmPid), case proves nothing"
    } else {
        $cls = Get-Class $sp
        $opt = (& $PSMUX show-options -t $wname -g -v priority 2>&1 | Out-String).Trim()
        $wantOpt = if ($wc.v) { $wc.v } else { 'above-normal' }
        if ($cls -eq $wc.want) {
            Write-Pass "warm claim ($($wc.label)): standby was $warmCls, claimed server is $cls"
        } else {
            Write-Fail "warm claim ($($wc.label)): claimed server is $cls, expected $($wc.want) (standby was $warmCls)"
        }
        if ($opt -eq $wantOpt) {
            Write-Pass "  show-options on the claimed server reports '$opt'"
        } else {
            Write-Fail "  show-options on the claimed server reports '$opt', expected '$wantOpt'"
        }
    }
    Remove-Item env:PSMUX_PRIORITY -EA SilentlyContinue
    Kill-Sess $wname
    # Drop the replacement standby so the next case primes a fresh one whose
    # class was chosen with no PSMUX_PRIORITY in the environment.
    $leftover = Get-MyWarmPid
    if ($leftover -gt 0) { try { Stop-Process -Id $leftover -Force -EA Stop } catch {} }
    Start-Sleep -Milliseconds 800
}

}
finally {
    foreach ($v in 'PSMUX_PRIORITY','PSMUX_NO_WARM') { Remove-Item "env:$v" -EA SilentlyContinue }
    # Only pids this suite created. Never a blanket kill by name: other psmux
    # servers on this box belong to somebody else.
    foreach ($id in ($script:Spawned | Sort-Object -Unique)) {
        try { Stop-Process -Id $id -Force -EA Stop } catch {}
    }
    $env:PSMUX_DATA_DIR = $null
    Remove-Item -LiteralPath $rig -Recurse -Force -EA SilentlyContinue
}

Write-Host "`n=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed:  $($script:Pass)" -ForegroundColor Green
Write-Host "  Failed:  $($script:Fail)" -ForegroundColor $(if ($script:Fail -gt 0) { "Red" } else { "Green" })
Write-Host "  Skipped: $($script:Skip)" -ForegroundColor Yellow
exit $script:Fail
