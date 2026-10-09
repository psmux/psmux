# perf_metrics_common.ps1 - the pieces every perf gate needs, in one place.
#
# WHY THIS FILE EXISTS
# --------------------
# Four suites time psmux and every one of them wrote its own JSON with its own
# field names. A run recorded in one file could not be lined up against a run
# recorded in another, and none of the gate files recorded WHICH BUILD produced
# the number: two of the three gates default to the psmux on PATH, the third
# defaults to target\release, and the JSON said only "binary". Comparing a
# suspected regression against last week meant guessing.
#
# So every perf JSON now carries the same envelope:
#
#   schema        the envelope version, so a reader can tell old files apart
#   suite         the suite that wrote it
#   timestamp     round trip "o" format, local time with offset
#   binary        full path of the psmux under test
#   git_sha       short HEAD of the repository the binary was built in, found by
#                 walking up from the binary, or "installed" when the binary
#                 does not sit in a work tree (a cargo install copy does not)
#   machine       computer name
#   os / cpu / cpu_count / ram_gb
#   load          what else the machine was doing: percent of TOTAL cpu busy,
#                 sampled from \Processor(_Total)\% Processor Time, with every
#                 sample a suite took during the run summarised as n/min/p50/max
#
# WHY THE LOAD BLOCK EXISTS
# -------------------------
# The owner's machine routinely runs five or six agents building and testing at
# once, and a creation that is served by a warm spare in 17 ms on a quiet box
# takes 35 ms on a box at 60 percent, while the one creation in ten that has to
# wait out a cold pwsh start goes from 470 ms to 1780 ms. Measured here on
# 2026-09-22 with five other agents building:
#
#   quiet  (2026-09-21 18:41)  new-window  p50 25 ms  p90 110 ms  max  474 ms
#   loaded (2026-09-22 22:19)  new-window  p50 35 ms  p90 794 ms  max 1781 ms
#
# The p50 barely moves because it is the pool's own path. The tail is the
# machine. A JSON that does not say which of the two it was recorded cannot
# tell a regression from a busy afternoon, so the load goes in the envelope and
# a gate may consult it before it decides whether a tail is a failure.
#
# and the same shapes for numbers: Get-PerfStats returns n/min/p50/p90/p99/max/
# mean, and the resource helpers return working set, private bytes and CPU in
# one shape for the server and the client alike.
#
# MEMORY AND CPU
# --------------
# The server is found by its <ns>__<session>.pid anchor file, never by image
# name: a warm standby shares the image and the namespace but never the anchor,
# and the owner's machine routinely has a dozen psmux servers belonging to other
# sessions. The anchor holds "<pid>:<creationtime>" so a recycled PID is
# rejected. Clients are the processes running THIS binary with this namespace on
# their command line that are not the server.
#
# Nothing in this file kills anything. It only reads.
#
# Dot source it:  . "$PSScriptRoot\perf_metrics_common.ps1"

# 3 added the `load` block to the envelope. Everything schema 2 carried is still
# carried under the same names, so a schema 2 file and a schema 3 file line up
# field for field and perf_summary.ps1 reads both.
$script:PerfSchema = 3

# ── percentiles ───────────────────────────────────────────────────────────
# Nearest rank on the sorted sample, which is the rule the keystroke gate
# already used, so numbers recorded before and after this helper landed stay
# comparable.
function Get-PerfPercentile {
    param([object[]]$Values, [double]$P)
    $v = @($Values | Where-Object { $null -ne $_ } | ForEach-Object { [double]$_ } | Sort-Object)
    if ($v.Count -eq 0) { return $null }
    $i = [Math]::Floor(($P / 100.0) * ($v.Count - 1))
    if ($i -lt 0) { $i = 0 }
    if ($i -gt $v.Count - 1) { $i = $v.Count - 1 }
    return [double]$v[[int]$i]
}

function Get-PerfStats {
    param([object[]]$Values, [int]$Round = 2)
    $v = @($Values | Where-Object { $null -ne $_ } | ForEach-Object { [double]$_ })
    if ($v.Count -eq 0) {
        return [ordered]@{ n = 0; min = $null; p50 = $null; p90 = $null; p99 = $null; max = $null; mean = $null }
    }
    $r = { param($x) if ($null -eq $x) { $null } else { [math]::Round([double]$x, $Round) } }
    return [ordered]@{
        n    = $v.Count
        min  = & $r (Get-PerfPercentile $v 0)
        p50  = & $r (Get-PerfPercentile $v 50)
        p90  = & $r (Get-PerfPercentile $v 90)
        p99  = & $r (Get-PerfPercentile $v 99)
        max  = & $r (Get-PerfPercentile $v 100)
        mean = & $r (($v | Measure-Object -Average).Average)
    }
}

# ── which build is this ───────────────────────────────────────────────────
# A binary inside a work tree gets that tree's short HEAD. A binary that is not
# (C:\Users\<user>\.cargo\bin\psmux.exe, an installer copy) is honestly labelled
# "installed" rather than being tagged with whatever the current directory's
# HEAD happens to be, which would be a lie in exactly the case that matters:
# comparing a fresh build against the installed one.
function Get-PerfGitSha {
    param([string]$Binary)
    if (-not $Binary) { return "unknown" }
    $dir = try { Split-Path -Parent (Resolve-Path -LiteralPath $Binary -ErrorAction Stop).Path } catch { return "unknown" }
    $probe = $dir
    for ($i = 0; $i -lt 8 -and $probe; $i++) {
        if (Test-Path (Join-Path $probe ".git")) {
            try {
                $sha = (& git -C $probe rev-parse --short HEAD 2>$null | Select-Object -First 1)
                if ($sha) { return $sha.Trim() }
            } catch { }
            return "unknown"
        }
        $parent = Split-Path -Parent $probe
        if ($parent -eq $probe) { break }
        $probe = $parent
    }
    return "installed"
}

# `psmux -V` answers with the tmux compatibility line first and its own build
# line second ("psmux 3.3.8 (<sha> <date>)"), and it is the second one that
# identifies the build, so it is preferred when present.
function Get-PerfBinaryVersion {
    param([string]$Binary)
    try {
        $lines = @(& $Binary -V 2>$null)
        $own = $lines | Where-Object { $_ -match '^psmux ' } | Select-Object -First 1
        if ($own) { return ([string]$own).Trim() }
        if ($lines.Count -gt 0) { return ([string]$lines[0]).Trim() }
    } catch { }
    return ""
}

# ── what else the machine was doing ───────────────────────────────────────
# One sample of \Processor(_Total)\% Processor Time, which is the percentage of
# TOTAL cpu across every core, so 60 on a 32 core box means about 19 cores busy.
#
# Get-Counter is used first because it is the number Task Manager shows. It is
# not always available: counter names are localised, the performance counter
# service can be disabled, and a remote-free machine with a corrupt registry
# answers nothing. The CIM class is the fallback, and $null is the honest answer
# when neither works - a gate must never fail because it could not read a
# counter, it must only lose the ability to explain a slow tail.
function Get-PerfMachineLoad {
    param([int]$SampleMs = 1000)
    $pct = $null
    # Per process CPU over the same second the counter samples, so a loaded
    # sample can say WHO loaded it.  Sweep 2026-10-02 failed five perf gates on
    # a box where 48 orphaned pstop.exe each burned half a core, and the gates
    # only said "load 100%".
    $cpuBefore = @{}
    $swTop = [System.Diagnostics.Stopwatch]::StartNew()
    try { foreach ($p in Get-Process -ErrorAction SilentlyContinue) { try { $cpuBefore[$p.Id] = $p.TotalProcessorTime.TotalMilliseconds } catch { } } } catch { }
    try {
        $s = Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 1 -MaxSamples 1 -ErrorAction Stop
        $pct = [math]::Round([double]($s.CounterSamples | Select-Object -First 1 -ExpandProperty CookedValue), 1)
    } catch {
        try {
            $c = Get-CimInstance Win32_PerfFormattedData_PerfOS_Processor -Filter "Name='_Total'" -ErrorAction Stop
            if ($c) { $pct = [math]::Round([double]$c.PercentProcessorTime, 1) }
        } catch { }
    }
    if ($null -ne $pct) {
        if ($pct -lt 0) { $pct = 0.0 }
        if ($pct -gt 100) { $pct = 100.0 }
    }
    $procs = 0
    $top = ""
    try {
        $after = @(Get-Process -ErrorAction SilentlyContinue)
        $procs = $after.Count
        if ($null -ne $pct -and $pct -ge 5) {
            $wallMs = [math]::Max(1.0, $swTop.Elapsed.TotalMilliseconds)
            $cores = [Environment]::ProcessorCount
            $rows = foreach ($p in $after) {
                if (-not $cpuBefore.ContainsKey($p.Id)) { continue }
                $d = 0.0
                try { $d = $p.TotalProcessorTime.TotalMilliseconds - $cpuBefore[$p.Id] } catch { continue }
                if ($d -le 0) { continue }
                [pscustomobject]@{ Id = $p.Id; Name = $p.ProcessName; Pct = $d * 100.0 / ($wallMs * $cores) }
            }
            $best = @($rows | Sort-Object Pct -Descending | Select-Object -First 5)
            if ($best.Count) {
                $parents = @{}
                try {
                    $ids = ($best | ForEach-Object { "ProcessId=$($_.Id)" }) -join ' OR '
                    foreach ($c in Get-CimInstance Win32_Process -Filter $ids -ErrorAction Stop) { $parents[[int]$c.ProcessId] = [int]$c.ParentProcessId }
                } catch { }
                $top = ($best | ForEach-Object {
                    $pp = if ($parents.ContainsKey($_.Id)) { " ppid " + $parents[$_.Id] } else { "" }
                    "{0}:{1}{2} {3:N1}% ({4:N2} cores)" -f $_.Name, $_.Id, $pp, $_.Pct, ($_.Pct * $cores / 100.0)
                }) -join ", "
            }
        }
    } catch { }
    return [ordered]@{
        at          = (Get-Date).ToString("o")
        cpu_pct     = $pct
        process_cnt = $procs
        # Empty when the box was under 5 percent; otherwise the five busiest
        # processes over the sampled second, in percent of TOTAL cpu.
        top_cpu     = $top
    }
}

# One line naming the busiest processes of the most loaded sample, for a
# gate's "machine load" line.  Empty when no sample reached 5 percent.
function Get-PerfLoadTopText {
    $worst = $script:PerfLoadSamples | Where-Object { $null -ne $_.cpu_pct } | Sort-Object { $_.cpu_pct } -Descending | Select-Object -First 1
    if (-not $worst -or -not $worst.top_cpu) { return "" }
    return ("  top cpu at the {0}% sample: {1}" -f $worst.cpu_pct, $worst.top_cpu)
}

# Samples accumulate for the life of the suite process. A gate calls
# Add-PerfLoadSample at the points it cares about (before a cell, after a cell)
# and the summary rides into the envelope automatically.
$script:PerfLoadSamples = New-Object System.Collections.Generic.List[object]

function Add-PerfLoadSample {
    param([string]$Label = "")
    $s = Get-PerfMachineLoad
    $s["label"] = $Label
    $script:PerfLoadSamples.Add($s) | Out-Null
    return $s
}

function Get-PerfLoadSummary {
    $vals = @($script:PerfLoadSamples | ForEach-Object { $_.cpu_pct } | Where-Object { $null -ne $_ })
    $st = Get-PerfStats $vals 1
    return [ordered]@{
        n       = $st.n
        min_pct = $st.min
        p50_pct = $st.p50
        max_pct = $st.max
        # .ToArray(), not @(...). Wrapping a List[object] of ordered dictionaries
        # in @() and then putting it in an [ordered] literal throws "Argument
        # types do not match" in pwsh 7.6, and the throw happens while building
        # the envelope, which would take every perf JSON with it.
        samples = $script:PerfLoadSamples.ToArray()
    }
}

# The one question a gate asks of the load: was this machine quiet enough that a
# slow tail can only have been the product? Defaults to 25 percent of total cpu,
# which on the owner's 32 core box is eight cores busy: comfortably above an
# idle desktop and comfortably below one sibling cargo build.
#
# $null (the counter could not be read) is treated as NOT quiet. A gate that
# cannot see the machine must not pretend the machine was idle.
function Test-PerfMachineQuiet {
    param([double]$MaxPct = 25.0)
    $vals = @($script:PerfLoadSamples | ForEach-Object { $_.cpu_pct } | Where-Object { $null -ne $_ })
    if ($vals.Count -eq 0) { return $false }
    if ($vals.Count -ne $script:PerfLoadSamples.Count) { return $false }
    return ((Get-PerfPercentile $vals 50) -le $MaxPct)
}

# ── the envelope every perf JSON carries ──────────────────────────────────
function Get-PerfEnvelope {
    param([string]$Suite, [string]$Binary)
    $cpu = ""
    try { $cpu = (Get-CimInstance Win32_Processor -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Name) } catch { }
    $ram = 0
    try { $ram = [math]::Round((Get-CimInstance Win32_ComputerSystem -ErrorAction SilentlyContinue).TotalPhysicalMemory / 1GB, 1) } catch { }
    # A suite that never called Add-PerfLoadSample still gets one reading, taken
    # here, so no perf JSON can be written without saying what the machine was
    # doing. It costs one second, once, at the end of a run.
    if ($script:PerfLoadSamples.Count -eq 0) { Add-PerfLoadSample "at_write" | Out-Null }
    return [ordered]@{
        schema    = $script:PerfSchema
        suite     = $Suite
        timestamp = (Get-Date).ToString("o")
        binary    = $Binary
        git_sha   = (Get-PerfGitSha $Binary)
        version   = (Get-PerfBinaryVersion $Binary)
        machine   = $env:COMPUTERNAME
        os        = [System.Environment]::OSVersion.VersionString
        cpu       = ($cpu -replace '\s+$', '')
        cpu_count = [Environment]::ProcessorCount
        ram_gb    = $ram
        load      = (Get-PerfLoadSummary)
    }
}

function Get-PerfMetricsDir {
    param([string]$MetricsDir = "")
    # PSMUX_METRICS_DIR sends an A/B or bisect run (old builds, forced options)
    # somewhere else, so it never lands in the trend tests\perf_summary.ps1 reads.
    if (-not $MetricsDir -and $env:PSMUX_METRICS_DIR) { $MetricsDir = $env:PSMUX_METRICS_DIR }
    if (-not $MetricsDir) { $MetricsDir = Join-Path $env:USERPROFILE ".psmux-test-data\metrics" }
    if (-not (Test-Path $MetricsDir)) { New-Item -ItemType Directory -Force -Path $MetricsDir | Out-Null }
    return $MetricsDir
}

# Envelope + payload, written OUTSIDE the repo. Returns the path, or $null if
# the write failed; a perf suite must never die because a disk was full.
function Write-PerfMetrics {
    param(
        [string]$Suite,
        [string]$Binary,
        [System.Collections.IDictionary]$Data,
        [string]$FileStem = "",
        [string]$MetricsDir = "",
        [string]$Stamp = "",
        [int]$Depth = 8
    )
    try {
        $dir = Get-PerfMetricsDir $MetricsDir
        if (-not $FileStem) { $FileStem = $Suite }
        if (-not $Stamp) { $Stamp = Get-Date -Format "yyyyMMdd-HHmmss" }
        $payload = Get-PerfEnvelope $Suite $Binary
        foreach ($k in $Data.Keys) { $payload[$k] = $Data[$k] }
        $path = Join-Path $dir "$FileStem-$Stamp.json"
        ($payload | ConvertTo-Json -Depth $Depth) | Set-Content -LiteralPath $path -Encoding UTF8
        return $path
    } catch {
        Write-Host "[WARN] could not write the metrics JSON: $_" -ForegroundColor Yellow
        return $null
    }
}

# ── finding the processes of one session ──────────────────────────────────
function Get-PerfDataDir {
    if ($env:PSMUX_DATA_DIR) { return $env:PSMUX_DATA_DIR.TrimEnd('\', '/') }
    return "$env:USERPROFILE\.psmux"
}

# The server for one session, from its <ns>__<session>.pid anchor. Never by
# image name: a warm standby shares the image and the namespace but never this
# file.
function Get-PerfServerPid {
    param([string]$Ns, [string]$Session, [string]$DataDir = "")
    if (-not $DataDir) { $DataDir = Get-PerfDataDir }
    $f = Join-Path $DataDir "${Ns}__$Session.pid"
    if (-not (Test-Path $f)) { return 0 }
    $txt = ""
    try { $txt = (Get-Content -LiteralPath $f -Raw).Trim() } catch { return 0 }
    $first = ($txt -split ':')[0]
    $out = 0
    if (-not [int]::TryParse($first, [ref]$out)) { return 0 }
    if (-not (Get-Process -Id $out -ErrorAction SilentlyContinue)) { return 0 }
    return $out
}

# Every process running THIS binary with this namespace on its command line.
# ExecutablePath is matched exactly, so a differently built psmux belonging to
# another agent or to the user's own session is never sampled.
function Get-PerfNamespacePids {
    param([string]$Binary, [string]$Ns)
    $name = [IO.Path]::GetFileName($Binary)
    $out = @()
    try {
        $out = @(Get-CimInstance Win32_Process -Filter "Name='$name'" -ErrorAction SilentlyContinue |
            Where-Object { $_.ExecutablePath -eq $Binary -and $_.CommandLine -and $_.CommandLine -match ("(?i)-L\s+" + [regex]::Escape($Ns) + "(\s|$)") } |
            Select-Object -ExpandProperty ProcessId)
    } catch { }
    return @($out | ForEach-Object { [int]$_ })
}

# The attached client of one session: this binary, this namespace, not the
# server. There is normally exactly one; the first is taken when a suite has
# attached more than once.
function Get-PerfClientPid {
    param([string]$Binary, [string]$Ns, [int]$ServerPid = 0)
    foreach ($p in (Get-PerfNamespacePids -Binary $Binary -Ns $Ns)) {
        if ($p -ne $ServerPid) { return $p }
    }
    return 0
}

# ── one sample of one process ─────────────────────────────────────────────
function Get-PerfProcSample {
    param([int]$ProcId, [string]$Role)
    if ($ProcId -le 0) { return $null }
    $p = Get-Process -Id $ProcId -ErrorAction SilentlyContinue
    if (-not $p) { return $null }
    $cpu = 0.0
    try { $cpu = $p.TotalProcessorTime.TotalMilliseconds } catch { }
    return [pscustomobject]@{
        role       = $Role
        pid        = $ProcId
        name       = $p.ProcessName
        ws_mb      = [math]::Round($p.WorkingSet64 / 1MB, 2)
        private_mb = [math]::Round($p.PrivateMemorySize64 / 1MB, 2)
        cpu_ms     = [math]::Round($cpu, 1)
        threads    = $p.Threads.Count
    }
}

# $Roles is role name -> pid. Returns role -> sample, missing roles dropped.
function Get-PerfResourceSnapshot {
    param([System.Collections.IDictionary]$Roles)
    $o = [ordered]@{}
    foreach ($r in @($Roles.Keys)) {
        $s = Get-PerfProcSample ([int]$Roles[$r]) $r
        if ($s) { $o[$r] = $s }
    }
    return $o
}

# CPU consumed between two snapshots of the same roles. $Div turns raw ms into
# the reported unit and $Mul scales it: (elapsed ms, 100) gives percent of one
# core, (key count, 100) gives ms of CPU per 100 keystrokes. A role whose PID
# changed between the two snapshots is dropped: that is not a delta.
function Get-PerfCpuDelta {
    param($A, $B, [double]$Div, [double]$Mul = 1.0)
    $o = [ordered]@{}
    if (-not $A -or -not $B -or $Div -le 0) { return $o }
    foreach ($r in @($A.Keys)) {
        if (-not $B.Contains($r)) { continue }
        if ($A[$r].pid -ne $B[$r].pid) { continue }
        $d = [double]($B[$r].cpu_ms - $A[$r].cpu_ms)
        if ($d -lt 0) { $d = 0 }
        $o[$r] = [math]::Round(($d / $Div) * $Mul, 2)
    }
    return $o
}

# Sit still for $Seconds with nothing driving the session and report each role's
# CPU as a percentage of ONE core. This is the busy polling detector: a 1 ms
# sleep loop that Windows rounds to a 15.6 ms timer tick is invisible in every
# latency number and obvious here. Resolution is one scheduler tick (15.6 ms)
# over the window, ie 0.52 percent of a core for a 3 s window.
function Measure-PerfIdleCpu {
    param([System.Collections.IDictionary]$Roles, [int]$Seconds = 3)
    $a = Get-PerfResourceSnapshot $Roles
    $sw = [Diagnostics.Stopwatch]::StartNew()
    Start-Sleep -Milliseconds ($Seconds * 1000)
    $sw.Stop()
    $b = Get-PerfResourceSnapshot $Roles
    return [ordered]@{
        window_ms      = [math]::Round($sw.Elapsed.TotalMilliseconds, 0)
        pct_of_one_core = (Get-PerfCpuDelta $a $b ([double]$sw.Elapsed.TotalMilliseconds) 100.0)
    }
}

# Working set / private bytes of a snapshot, flattened for the JSON, plus the
# server+client totals that are the number the owner actually reads.
function Get-PerfMemorySummary {
    param($Snapshot)
    $o = [ordered]@{}
    $wsTotal = 0.0; $privTotal = 0.0
    foreach ($r in @($Snapshot.Keys)) {
        $o[$r] = [ordered]@{
            pid        = $Snapshot[$r].pid
            ws_mb      = $Snapshot[$r].ws_mb
            private_mb = $Snapshot[$r].private_mb
            cpu_ms     = $Snapshot[$r].cpu_ms
            threads    = $Snapshot[$r].threads
        }
        if ($r -in @("server", "client")) {
            $wsTotal += [double]$Snapshot[$r].ws_mb
            $privTotal += [double]$Snapshot[$r].private_mb
        }
    }
    $o["psmux_total_ws_mb"] = [math]::Round($wsTotal, 2)
    $o["psmux_total_private_mb"] = [math]::Round($privTotal, 2)
    return $o
}

function Format-PerfResourceLine {
    param($Snapshot, [string]$Label)
    $parts = @()
    foreach ($r in @($Snapshot.Keys)) {
        $parts += ("{0} ws {1}MB priv {2}MB cpu {3}ms" -f $r, $Snapshot[$r].ws_mb, $Snapshot[$r].private_mb, $Snapshot[$r].cpu_ms)
    }
    if ($parts.Count -eq 0) { return "$Label : no process found" }
    return ("{0} : {1}" -f $Label, ($parts -join "  |  "))
}

# ── splitting a shell keystroke into the shell's part and psmux's part ──
#
# With pwsh in the pane, keystroke to screen is two terms in series: the time
# the pane's own shell and conhost take to put the echo on the pseudoconsole
# output pipe, and every psmux hop around it (client, socket, server loop, pty
# write, and after the read: parse, frame, socket, client draw). Only the second
# is psmux's. The first is a PSReadLine redraw that conhost serialises in two
# frames with a timer wait between them, and how long that wait lasts depends
# on the machine's timer state, not on psmux.
#
# That is why a floor measured in ANOTHER pseudoconsole host is not a like for
# like baseline on its own. Measured 2026-09-30 on one machine, same minutes:
# the standalone host (tests/conpty_echolat.cs) read 11.1ms while pwsh in a
# psmux pane took 15.0ms from pty write to the read carrying the character; on
# every earlier day both read about 15.7. Neither psmux build moved it: an A/B
# of 39ac085 against 99bfd9e, three interleaved pairs, agreed to 0.1ms, and a
# later run caught the pane itself at 11.8. Subtracting a floor that sits in a
# different timer state from the pane reported 4 to 5ms of "psmux overhead"
# that psmux never spent.
#
# So the shell cell runs with PSMUX_PTY_TRACE on and splits every keystroke
# with the server's own QPC stamps (src/pty_trace.rs): `w` is psmux finishing
# the pty write of the key, and the first `r` after it whose visible text holds
# the character is the pane's echo coming back. shell = r - w, and
# psmux = (appear - inject) - shell, per keystroke, on one clock (QPC is system
# wide, and keylat logs its QPC base).
#
# Returns $null when the trace or the keylat log is unusable, so the caller
# reports that instead of passing on missing data.
function Get-PerfShellSplit {
    param([string]$KeylatOut, [string]$TraceBase)
    if (-not (Test-Path $KeylatOut)) { return $null }
    $inv = [Globalization.CultureInfo]::InvariantCulture
    $klog = Get-Content $KeylatOut
    $base = $null; $freq = $null
    $trials = @()
    foreach ($l in $klog) {
        $m = [regex]::Match($l, '^QPCBASE (\d+) freq (\d+)')
        if ($m.Success) { $base = [int64]$m.Groups[1].Value; $freq = [double]$m.Groups[2].Value; continue }
        $m = [regex]::Match($l, '^TRIALT (\d+) char=(\S) t0_ms=([0-9.]+) appear_ms=([0-9.]+)')
        if ($m.Success) {
            $trials += [pscustomobject]@{
                ch = $m.Groups[2].Value
                t0 = [double]::Parse($m.Groups[3].Value, $inv)
                ap = [double]::Parse($m.Groups[4].Value, $inv)
            }
        }
    }
    if ($null -eq $base -or $trials.Count -eq 0) { return $null }

    # The server is the one process that writes keys into a pane; the client's
    # file and any warm standby's carry no `w`.
    $dir = Split-Path -Parent $TraceBase
    $leaf = Split-Path -Leaf $TraceBase
    $events = $null
    foreach ($f in @(Get-ChildItem -Path $dir -Filter "$leaf.*" -ErrorAction SilentlyContinue)) {
        $ev = New-Object System.Collections.Generic.List[object]
        $hasW = $false
        foreach ($l in (Get-Content $f.FullName)) {
            $p = $l.Split(' ', 5)
            if ($p.Count -lt 5) { continue }
            $st = $p[1]
            if ($st -cne 'w' -and $st -cne 'r') { continue }
            if ($st -ceq 'w') { $hasW = $true }
            $ev.Add([pscustomobject]@{ t = [int64]$p[0]; s = $st; x = $p[4] })
        }
        if ($hasW) { $events = $ev; break }
    }
    if ($null -eq $events) { return $null }

    $shell = New-Object System.Collections.Generic.List[double]
    $own = New-Object System.Collections.Generic.List[double]
    $cursor = 0
    foreach ($tr in $trials) {
        $t0q = $base + [int64]($tr.t0 * $freq / 1000.0)
        $apq = $base + [int64]($tr.ap * $freq / 1000.0)
        $wi = -1
        for ($i = $cursor; $i -lt $events.Count; $i++) {
            $e = $events[$i]
            if ($e.t -gt $apq) { break }
            if ($e.s -ceq 'w' -and $e.t -ge $t0q -and $e.x -ceq $tr.ch) { $wi = $i; break }
        }
        if ($wi -lt 0) { continue }
        $cursor = $wi + 1
        for ($j = $wi + 1; $j -lt $events.Count; $j++) {
            $e = $events[$j]
            if ($e.t -gt $apq) { break }
            if ($e.s -cne 'r') { continue }
            # Visible text only: drop CSI and OSC sequences, then any other
            # escaped byte, so a colour code can never fake a match.
            $vis = [regex]::Replace($e.x, '<E>\[[0-9;?<=>]*[ -/]*[@-~]', '')
            $vis = [regex]::Replace($vis, '<E>\].*?(<07>|<E>\\)', '')
            $vis = [regex]::Replace($vis, '<(E|CR|LF|[0-9A-F]{2})>', '')
            if ($vis.Contains($tr.ch)) {
                $s = ($e.t - $events[$wi].t) * 1000.0 / $freq
                $shell.Add($s)
                $own.Add(($tr.ap - $tr.t0) - $s)
                break
            }
        }
    }
    if ($shell.Count -eq 0) { return $null }
    return [pscustomobject]@{ Trials = $trials.Count; ShellMs = $shell.ToArray(); PsmuxMs = $own.ToArray() }
}
