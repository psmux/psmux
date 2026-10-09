# test_perf_vs_terminals.ps1 - psmux measured AGAINST the real terminal
# emulators installed on this machine, not against numbers quoted from docs.
#
# WHY THIS SUITE EXISTS
# ---------------------
# tests/test_perf_vs_wt.ps1 prints Windows Terminal figures that were read out
# of WT's source code. Nothing in it ever launched WT. This suite launches
# every terminal that is actually installed, runs the SAME shell inside each of
# them, and measures the four things the user feels:
#
#   1. launch to prompt        how long from "open a terminal" to a live shell
#   2. keystroke to screen     how long from a key going in to the echo showing,
#                              against the ConPTY floor measured in the same run
#   3. memory and CPU          what the psmux server, the psmux client and the
#                              host terminal cost while that shell sits there
#   4. creation latency        new-session, new-window, split-window -v and -h
#
# Hosts: bare pwsh in its own console, Windows Terminal, WezTerm, Alacritty,
# psmux attached in its own console, and psmux inside Windows Terminal (which
# is how most psmux users actually run it). A host that is not installed is
# reported SKIP and costs nothing.
#
# HOW TO READ THE NUMBERS
# -----------------------
# LAUNCH TO PROMPT. Every cell runs the identical shell:
#     pwsh -NoLogo -NoProfile -NoExit -File <marker.ps1> <markerfile>
# marker.ps1 writes [Diagnostics.Stopwatch]::GetTimestamp() and its own $PID to
# the marker file. QPC is system wide, so the suite subtracts its own
# GetTimestamp() taken immediately before Start-Process and gets a sub
# millisecond, host neutral "the shell is up" instant with no polling tax and
# no screen scraping. The marker is written just BEFORE pwsh paints its first
# prompt, so these numbers exclude the host's own first paint. They are a shell
# readiness measure, identical in every cell, which is what makes the cells
# comparable. The marker also hands back the shell's PID, which is how the
# keystroke and resource sections find the console to work on.
#
# The GUI hosted cells (WT, WezTerm, Alacritty, psmux in WT) go through one
# extra `cmd /c wrapper.cmd` hop, about 15 ms, so the wrapper can pin the
# child's environment: a `wt -w <name>` window is spawned by the Windows Terminal
# monarch process and would otherwise inherit ITS environment, not this
# suite's, which would send a psmux pane at the user's real data dir. The hop
# is present in all four GUI cells and absent in the two directly launched
# cells, so GUI to GUI comparisons are exact and the two direct cells are, if
# anything, flattered by about 15 ms.
#
# Windows Terminal is normally already running, so a `wt` window opens inside an
# existing process while WezTerm and Alacritty cold start a whole GUI process.
# wt_was_running in the metrics JSON records which case was measured. The bench
# window is named, so after the first repetition each Windows Terminal cell is a
# new TAB in that window, which is both the common user action and the only
# shape that can be reliably closed again.
#
# The launch cells are INTERLEAVED: repetition 0 of every host, then repetition
# 1 of every host, and so on. This machine drifts by a factor of two inside a
# minute, so running five repetitions of Windows Terminal and only then five of
# psmux would compare two different machines. Round robin puts every cell in
# the same weather, which is what makes psmux minus WT a number worth having.
#
# KEYSTROKE TO SCREEN is measured by tests/keylat.cs at the PTY level: a key
# record is written into the target console's input buffer and the same process
# watches that console's screen buffer for the echo, both timestamped from one
# QueryPerformanceCounter, so there is no cross process clock skew.
#
# THE HOST ROWS AND THE PSMUX ROWS ARE TAKEN AT DIFFERENT PROBE POINTS, and that
# is the whole explanation for "1 ms versus 18 ms". For Windows Terminal, WezTerm
# and Alacritty the console being watched is the pane conhost's screen buffer,
# which is UPSTREAM of the pseudoconsole pipe, and it also contains none of the
# terminal's own GPU paint (another 8 to 16 ms of frame time in each of them).
# For the psmux cells the console being watched is the psmux CLIENT's own
# console, DOWNSTREAM of that pipe, so the psmux number contains the whole psmux
# pipeline AND the pipe itself.
#
# The pipe is the expensive part and it is not psmux's. Traced by perfE on
# 2026-09-10: PSReadLine paints a keystroke in two console writes, conhost's
# pseudoconsole serializer emits the first as a 6 byte ESC[?25l chunk after about
# 0.6 ms, and then withholds the chunk carrying the character for a further
# 14.7 ms. The character sits in the pane conhost's screen buffer the whole time.
# Every ConPTY consumer on Windows pays this, Windows Terminal included; the host
# cells here simply do not measure the part of the path where it happens. At the
# SAME probe point as the host cells, psmux measures 0.55 ms against the 1.03 ms
# measured here for Windows Terminal.
#
# So the psmux rows are judged against the floor, not against the host rows and
# not against a number invented in a brief: tests/conpty_echolat.cs hosts a
# pseudoconsole with the same shell and the same key record and no psmux at all,
# it runs in the SAME run on the SAME machine. T3a and T3c used to assert how far
# above its median and p99 psmux sits (0.74 ms median, 1.57 ms p99 at the time);
# they now judge psmux's own hops split out per keystroke (1.7 to 1.8 ms median
# on 2026-10-01), and the floor is held against the pane shell's echo time (T3d).
#
# MEMORY AND CPU ride along with the keystroke section, because that is the only
# section that holds one cell alive long enough to watch it. For every cell the
# suite finds the processes that make that cell work and samples each of them
# twice, at prompt ready and again after the keystroke run:
#
#   server        the psmux server for this cell's session, identified by the
#                 <ns>__<session>.pid anchor file, not by image name, so a warm
#                 standby or another agent's psmux is never mistaken for it
#   client        the psmux CLI client attached to that session
#   shell         the pwsh being typed into, identical in every cell
#   host          the terminal emulator hosting the cell, found by walking the
#                 shell's parent chain to the first GUI process. For a Windows
#                 Terminal TAB that process is the user's existing WT, shared
#                 with their own windows, so host_shared is recorded true and
#                 its working set is NOT this cell's cost. Its CPU delta still
#                 is, because the other windows are idle.
#   console_host  the conhost or OpenConsole that owns the cell's console
#
#   memory   WorkingSet64 (what is resident) and PrivateMemorySize64 (what is
#            not shared with any other process) in MB.
#   cpu      TotalProcessorTime delta across the keystroke run, normalised to
#            ms of CPU per 100 keystrokes, so cells with different key counts
#            stay comparable.
#   idle cpu TotalProcessorTime delta over a quiet window (-IdleSeconds, 3 s by
#            default) with nothing typed, as a percentage of ONE core. This is
#            the busy polling detector: a 1 ms poll loop shows up here as
#            percent of a core burnt for nothing, and it is invisible in every
#            latency number in this file.
#
# CREATION LATENCY is measured against a detached psmux server over one
# persistent control connection: a dump-state round trip costs about 0.15 ms,
# so prompt detection is effectively continuous instead of the 16 ms per poll
# that a fresh `capture-pane` client costs. Each figure includes the roughly
# 16 to 20 ms Windows needs to start the psmux.exe CLI client, because that is
# what a user scripting psmux actually pays. These cells use the DEFAULT pane
# shell so the warm pane pool is in play, which is why they are bimodal: a
# claimed spare shell lands near 60 ms and a cold shell start near 500 ms. The
# suite flags that explicitly instead of burying it in a mean.
#
# Each measured window is killed again the moment its sample is taken. That is
# not tidiness, it is the measurement: every window left behind grows the
# dump-state frame the control connection has to read and re-scan on every
# push, and the 2026-09-10 run that left twenty windows standing reported
# 9 to 14 SECOND "creations" that were really the harness reading its own
# backlog. Killing the window keeps the frame at one or two windows, which is
# the shape a user's session actually has.
#
# THRESHOLDS, and where they come from
# ------------------------------------
# Set from the measured data on the reference machine (Ryzen AI MAX+ 395,
# Windows 11 26200, PowerShell 7) on 2026-09-10 while sibling build and
# benchmark agents were saturating the CPU. These are LOADED numbers; a quiet
# machine is faster. Medians and n>=5 per cell are used for that reason.
#
#   T0  every cell that was NOT skipped produced data. A run that prints a
#       pretty summary full of dashes is worse than a failing run, because it
#       looks like a pass. This is checked as a threshold so an empty section
#       fails loudly.
#   T1  psmux attached launch to prompt, minus bare pwsh, <= 350 ms, as an
#       ABSOLUTE DELTA and no longer as a 1.5x ratio. A ratio moves when bare
#       pwsh moves, and bare pwsh ranges from 350 to 600 ms here depending on
#       what else the machine is doing, so the same build scored 1.7x and 2.5x
#       on two runs with its own cost unchanged. The delta is what psmux owns:
#       the client, the server, the ConPTY and the warm claim. Measured 175 to
#       213 ms on master after the double shell fix, against 440 to 530 ms for
#       the bug that fix removed, so 350 ms still catches that regression.
#       T1 is the cold server cell, T1b the steady state one, same budget.
#   T2  psmux inside WT must not add more than 300 ms over plain WT, again in
#       the steady state. The owner proposed 300 ms for one number, but the
#       data shows two populations: with a server already running psmux adds
#       the client, a warm claim and a ConPTY, while the first psmux window of
#       the day additionally pays a cold server spawn (about 215 ms measured
#       separately in docs/performance.md). So 300 ms is applied to the steady
#       state and the cold case gets its own budget, T2b at 700 ms.
#   T3  psmux keystroke to screen. T3a: psmux's own hops per keystroke, median
#       <= 2.5 ms, T3c: their p99 <= 6 ms, both split out of end to end with
#       the server's pty trace in the same run (since 2026-10-01; before that
#       they were end to end minus the standalone ConPTY floor, which broke when
#       the floor moved with the machine's timer state, see section 2). T3d:
#       the pane shell's own echo time under psmux at most 7 ms over the floor.
#       T3b keeps an ABSOLUTE p99 ceiling of 25 ms so the total a user waits is
#       still bounded. A cell whose trace does not split is reported only.
#       The old absolute 10 ms median could not pass with a shell in the pane
#       and no psmux change could make it: 15.8 of those 18 ms are conhost's
#       pseudoconsole serializer. The budgets are about three times the measured
#       overhead, which is loose enough for the floor's own spread and tight
#       enough that a regression costing one 15.6 ms timer tick cannot hide.
#   T4  new-window and split-window p90 < 300 ms, first session < 1000 ms.
#   T5  zero leftover windows, tabs, shells or servers. Not a warning: a
#       benchmark that litters the desktop is a benchmark nobody will run.
#   T6  memory for ONE pane: psmux server working set <= 60 MB and psmux
#       client working set <= 60 MB. Measured here at 14 to 25 MB each, so the
#       limit is deliberately 2x the measurement: it is a regression alarm for
#       a leak or an unbounded scrollback, not a tuning target to shave. A
#       whole session of server plus client under 120 MB is also the number
#       that matters against "a multiplexer is heavy" as an argument.
#   T7  psmux idle CPU <= 3 percent of one core, server and client summed, over
#       the SETTLED quiet window with nothing typed. Two windows are measured per
#       cell: the first, half a second after the last keystroke, is the adaptive
#       poll ramping down and is reported but never judged; the second, taken
#       after a further settle, is the one T7 reads. tmux on Unix is ~0 percent
#       idle; a Windows port that polls is the known failure mode (and psmux
#       really did ship 1 ms sleeps that Windows rounded to 15.6 ms per timer
#       tick), so this is the one number that catches a regression no latency
#       test can see. Resolution: TotalProcessorTime moves in 15.6 ms ticks, so
#       one tick in a 3 s window is 0.52 percent of a core; the gate is 4 ticks.
#   T8  keystroke CPU cost <= 3000 ms of CPU per 100 keystrokes, server and
#       client summed, ie 20 ms of CPU per key. Measured 1060 ms per 100 keys on
#       a quiet machine and up to 1680 ms loaded, and the cause is FRAME COUNT,
#       not spinning: because conhost emits the cursor hide chunk and the text
#       chunk about 15 ms apart, psmux builds and pushes TWO frames per keystroke
#       at a shell prompt, the second superseding the first, at roughly 4 to
#       5.7 ms of work per frame. A pane that writes one chunk per key costs one
#       frame and about half the CPU. The follow up, which wants its own change
#       and its own sweep, is to defer a cursor-visibility-only frame by a short
#       grace so the text frame absorbs it. The client's CONSOLE HOST costs more
#       than either psmux process, 5100 to 5500 ms per 100 keys, and is reported
#       in the table but never judged: it is a legacy console window repainting
#       per write batch, and inside Windows Terminal that work is someone else's
#       GPU.
#
# CLOSING WHAT IT OPENS
# ---------------------
# Every window, tab, shell and psmux client is closed by PID the moment its
# measurement is captured, before the next cell starts, and the rule is
# ATTRIBUTION: a process is only ever killed when this run can show it started
# it. Three mechanisms, in order of strength:
#
#   1. PIDs this suite spawned itself, and the shell PID its marker reported.
#   2. Any cmd, pwsh or psmux process carrying this run's scratch directory on
#      its command line. That path contains this suite's own PID, so nothing
#      outside this run can match. This is what closes a `wt` tab whose launch
#      timed out: the tab is hosted by the wrapper cmd.exe, the tab lives
#      INSIDE the Windows Terminal process, so there is no marker to name the
#      shell and no new GUI process to reap either.
#   3. Terminal and console host processes attributed to a cell: a GUI process
#      of the cell's own host name that did not exist before the cell started,
#      or a conhost/OpenConsole whose parent is one of this run's processes or
#      the GUI process this cell opened. A console host is never killed while a
#      real client is still attached to it, because then it is somebody's live
#      window; it is reaped once its clients are gone.
#
# What T5 counts as a leftover is therefore what a user would have to close:
# a window, a tab, a shell, a psmux server. A console host that has outlived
# its client has no window of its own, so it is reaped and listed rather than
# failing the run. One that still holds a real client is a leftover and does
# fail it.
#
# What it will NOT touch, ever: this script's own process ancestry (pwsh <-
# claude.exe <- pwsh <- WindowsTerminal and the console host that WT owns for
# it), every terminal process that existed before the run, and any psmux that
# is not running THIS binary in one of THIS run's two -L namespaces.
#
# THE BUG THIS REPLACES, because it is worth knowing about. The 11:11 and 11:59
# runs on 2026-09-10 failed T5 with "leftover" WindowsTerminal and OpenConsole
# PIDs the suite had never opened, and they killed them. Two faults compounded:
# the baseline snapshot was built with `foreach ($n in $GuiNames)` at script
# scope, where $n IS the [int]$N parameter, so every iteration threw a
# conversion error on stderr and the baseline stayed EMPTY; and the audit then
# counted every process of those names that was not in the (empty) baseline as
# something the suite had opened, and killed it. The result was a benchmark that
# ended by killing the user's Windows Terminal, which is where the agent running
# it lived, which is why those runs also have empty result sections. Attribution
# plus the baseline assertion below is why neither half can happen now.
#
# The run ends with an audit that counts what it failed to close and fails T5.
#
# Exit code is the number of failed thresholds. A metrics JSON with every
# sample lands in %USERPROFILE%\.psmux-test-data\metrics\ (never in the repo),
# and it is written after EVERY section, with "complete": false until the run
# finishes, so a run that is killed half way still leaves its data and says so.
#
# Isolation: an isolated PSMUX_DATA_DIR, two per run -L namespaces, and only
# PIDs this suite started are ever killed.
#
#   pwsh -NoProfile -File tests\test_perf_vs_terminals.ps1
#   pwsh -NoProfile -File tests\test_perf_vs_terminals.ps1 -Quick
#   pwsh -NoProfile -File tests\test_perf_vs_terminals.ps1 -Binary C:\scratch\pmux.exe

param(
    [int]$N = 5,            # launch-to-prompt repetitions per host cell
    [int]$Keys = 40,        # keystrokes per latency cell (40 is the floor)
    [int]$Creates = 5,      # sequential creations per kind
    # The binary under test. Default order: -Binary, then PSMUX_TEST_BIN, then
    # this checkout's release build, then the psmux installed on PATH. A copy
    # renamed for an A/B MUST be called pmux.exe, which psmux recognises as one
    # of its own server images (PSMUX_SERVER_IMAGE_NAMES is set from it below).
    [Alias("Psmux")][string]$Binary = "",
    [int]$IdleSeconds = 3,  # quiet window for the idle CPU measurement
    # At or under this percentage of TOTAL cpu the machine counts as quiet and
    # the four launch DIFFERENCE thresholds (T1, T1b, T2, T2b) are hard
    # failures; above it they are recorded warnings. Every other threshold is
    # hard in all cases. See the comment on Check.
    [double]$QuietLoadPct = 25.0,
    [switch]$Quick,         # smaller n, for a smoke run
    [switch]$SkipLaunch,
    [switch]$SkipKeys,
    [switch]$SkipCreate
)

try { [Console]::OutputEncoding = [Text.Encoding]::UTF8 } catch {}
$ErrorActionPreference = "Continue"

if ($Quick) { $N = 3; $Creates = 3 }
if ($Keys -lt 40) { $Keys = 40 }
if ($IdleSeconds -lt 1) { $IdleSeconds = 1 }

# ───────────────────────────────────────────────────────── setup ──
$RepoRoot = Split-Path -Parent $PSScriptRoot
$Psmux = $Binary
if (-not $Psmux) { $Psmux = $env:PSMUX_EXE }
if (-not $Psmux) { $Psmux = $env:PSMUX_TEST_BIN }
if (-not $Psmux) {
    $cand = Join-Path $RepoRoot "target\release\psmux.exe"
    if (Test-Path $cand) { $Psmux = $cand }
}
if (-not $Psmux) {
    $cmd = Get-Command psmux.exe -ErrorAction SilentlyContinue
    if ($cmd) { $Psmux = $cmd.Source }
}
if (-not $Psmux -or -not (Test-Path $Psmux)) {
    Write-Host "[SKIP] no psmux binary found (pass -Binary, set PSMUX_TEST_BIN, or cargo build --release)" -ForegroundColor Yellow
    exit 0
}
$Psmux = (Resolve-Path $Psmux).Path
$PsmuxImage = [IO.Path]::GetFileName($Psmux)
$PsmuxProcName = [IO.Path]::GetFileNameWithoutExtension($Psmux)

# The pane command is passed through cmd wrappers and through psmux's own
# command joiner, so keep every path in it free of spaces where we can.
$RunId  = "pvt$PID"
# A SECOND namespace for the cold psmux cells, so they can be torn down after
# every repetition without taking the warm cells' server with them. The two ids
# deliberately share no prefix: "-L pvt7" is a substring of "-L pvt77", and the
# process sweeps match on that string.
$ColdId = "cld$PID"
$tmp = [IO.Path]::GetTempPath()
$RunDir = Join-Path $tmp "psmux_perfterm_$PID"
if ($RunDir -match '\s') {
    $alt = "C:\psmux_perfterm_$PID"
    try { New-Item -ItemType Directory -Force -Path $alt -ErrorAction Stop | Out-Null; $RunDir = $alt } catch {}
}
Remove-Item -Recurse -Force $RunDir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path (Join-Path $RunDir "data") | Out-Null

# PSMUX_METRICS_DIR: same override as tests\perf_metrics_common.ps1 (not yet dot sourced here)
$MetricsDir = if ($env:PSMUX_METRICS_DIR) { $env:PSMUX_METRICS_DIR } else { Join-Path $env:USERPROFILE ".psmux-test-data\metrics" }
New-Item -ItemType Directory -Force -Path $MetricsDir | Out-Null
$MetricsFile = Join-Path $MetricsDir ("perf_vs_terminals-{0}.json" -f (Get-Date -Format "yyyyMMdd-HHmmss"))

# Isolate the data root and teach psmux that this image name is a server image
# (the suite is normally pointed at a scratch copy named pmux.exe).
$SavedEnv = @{}
foreach ($k in @("PSMUX_DATA_DIR","PSMUX_SERVER_IMAGE_NAMES","PSMUX_SESSION_NAME","PSMUX_SESSION")) {
    $SavedEnv[$k] = [Environment]::GetEnvironmentVariable($k)
}
$env:PSMUX_DATA_DIR = Join-Path $RunDir "data"
$prevImages = $SavedEnv["PSMUX_SERVER_IMAGE_NAMES"]
$env:PSMUX_SERVER_IMAGE_NAMES = if ($prevImages) { "$prevImages,$PsmuxImage" } else { $PsmuxImage }
Remove-Item Env:\PSMUX_SESSION_NAME -ErrorAction SilentlyContinue
Remove-Item Env:\PSMUX_SESSION -ErrorAction SilentlyContinue

$Freq = [Diagnostics.Stopwatch]::Frequency
$script:Fails = 0
$script:Thresholds = [System.Collections.ArrayList]::new()
$script:Launch   = [ordered]@{}
$script:Key      = [ordered]@{}
$script:Create   = [ordered]@{}
$script:Resource = [ordered]@{}                            # per cell memory and CPU
$script:KeySplit = [ordered]@{}                            # per psmux cell: shell echo vs psmux hops, from the traced pass
$script:Floor    = $null                                   # ConPTY floor, measured in this run
$script:Started  = [System.Collections.ArrayList]::new()   # every PID we spawned
# PID -> creation time of the process we spawned under that PID. A PID on its
# own is not an identity on Windows: it is recycled within seconds on a busy
# machine, and 2026-09-16 a cell's freed pwsh PID was reissued to the cmd.exe
# wrapper of the agent harness that was running this very suite (cmd.exe is one
# of the images this suite launches, so the image guard passed). Its console
# host was then attributed to the suite and reaped, taking the agent's bash and
# node with it. Every attribution and every kill of a recorded PID now also has
# to match the creation time recorded when the PID was spawned.
$script:StartedAt = @{}
$script:Opened   = [System.Collections.ArrayList]::new()   # GUI/console PIDs attributed to us
$script:Reaped   = [System.Collections.ArrayList]::new()   # console hosts Windows had not reaped yet
$script:Complete = $false
$script:Notes    = [System.Collections.ArrayList]::new()
# Set once the launch section has its numbers: true when a REFERENCE cell's own
# p90 is more than $RefSpreadMax times its own median, which says this run's
# baseline was being stalled and no threshold on a difference from it can be
# trusted. See the comment on Check.
$script:RefUnstable = $false
$script:RefSpread   = [ordered]@{}

function Say  { param($m) Write-Host $m }
function Info { param($m) Write-Host "[INFO] $m" -ForegroundColor Cyan }
function Warn { param($m) Write-Host "[WARN] $m" -ForegroundColor Yellow; [void]$script:Notes.Add($m) }
function Skip { param($m) Write-Host "[SKIP] $m" -ForegroundColor Yellow }
$script:Clock = [Diagnostics.Stopwatch]::StartNew()
function Head { param($m) Write-Host ""; Write-Host ("=" * 78) -ForegroundColor DarkCyan; Write-Host ("  $m   [+{0:F0}s]" -f $script:Clock.Elapsed.TotalSeconds) -ForegroundColor White; Write-Host ("=" * 78) -ForegroundColor DarkCyan }

# -LoadSensitive marks a threshold that is a DIFFERENCE BETWEEN TWO TIMINGS,
# which is the one shape that cannot survive a busy machine: both arms stretch
# under load and the difference between two stretched numbers stretches with
# them. Measured on 2026-09-22 against the installed 0bcc421 with nothing
# rebuilt between the two runs:
#
#   quiet    bare 434 ms   psmux_attached 661 ms   T1 = 227 ms   PASS
#   loaded   bare 428 ms   psmux_attached 783 ms   T1 = 355 ms   FAIL (limit 350)
#
# The bare arm's own p90 in the loaded run was 736 ms against a 428 ms median,
# so the machine was stalling individual launches, and the interleaving that
# normally cancels drift cannot cancel a stall that lands in one arm.
#
# Such a threshold stays a hard failure when the run's own load samples say the
# machine was quiet, and becomes a recorded warning when they do not. Every
# threshold that is NOT a difference of two timings stays hard in all cases:
# T3b and T3c are hard (T3a, a median, follows the load policy), T4 is a p90 on
# psmux alone, T6, T7 and T8 are memory and CPU, and T5 counts processes. Those
# are the ones that still catch a regression on a loaded box.
#
# The verdict is recorded in the JSON as `pass`, `soft` and `machine_quiet`, so
# a warning is never invisible.
function Check {
    param([string]$Name, [double]$Value, [double]$Limit, [string]$Unit, [string]$Why = "", [switch]$LoadSensitive)
    $ok = $Value -le $Limit
    $quiet = $true
    try { $quiet = Test-PerfMachineQuiet $QuietLoadPct } catch { }
    # A CPU counter sampled once per section is not enough on its own. The run
    # of 2026-09-22 22:45 failed T1 and T2 while its six load samples read a p50
    # of 6.5 percent: the machine was not steadily busy, it was STALLING
    # individual launches, and a one second sample taken between sections sees
    # none of that. What does see it is the reference arm's own spread, measured
    # in the very same interleaved run:
    #
    #   quiet run   bare pwsh median 434 ms   p90 477 ms   p90/median 1.10
    #   this run    bare pwsh median 428 ms   p90 736 ms   p90/median 1.72
    #
    # A reference that unstable cannot support a threshold on a DIFFERENCE from
    # it, whatever the CPU counter says, so $script:RefUnstable counts as not
    # quiet as well. It costs nothing: the numbers are already measured.
    if ($script:RefUnstable) { $quiet = $false }
    $soft = ((-not $ok) -and $LoadSensitive -and (-not $quiet))
    if ($ok) {
        Write-Host ("[PASS] {0}: {1:F1}{3} <= {2:F1}{3}" -f $Name, $Value, $Limit, $Unit) -ForegroundColor Green
    } elseif ($soft) {
        $lp = 0; try { $lp = (Get-PerfLoadSummary).p50_pct } catch { }
        $m = ("{0}: {1:F1}{3} > {2:F1}{3}, but the machine was at {4}% of total cpu, over the {5}% quiet mark, and a difference of two timings stretches with the machine" -f $Name, $Value, $Limit, $Unit, $lp, $QuietLoadPct)
        try { $tt = Get-PerfLoadTopText; if ($tt) { $m += ";" + $tt } } catch { }
        Write-Host "[WARN] $m" -ForegroundColor Yellow
        [void]$script:Notes.Add($m)
    } else {
        Write-Host ("[FAIL] {0}: {1:F1}{3} > {2:F1}{3}"  -f $Name, $Value, $Limit, $Unit) -ForegroundColor Red; $script:Fails++
    }
    if ($Why) { Write-Host "       $Why" -ForegroundColor DarkGray }
    [void]$script:Thresholds.Add([pscustomobject]@{ name=$Name; value=[math]::Round($Value,2); limit=$Limit; unit=$Unit; pass=$ok; soft=$soft; machine_quiet=$quiet; note=$Why })
}

function Percentile {
    param([double[]]$Sorted, [double]$P)
    $i = ($Sorted.Count - 1) * $P
    $lo = [int][math]::Floor($i); $hi = [int][math]::Ceiling($i)
    if ($lo -eq $hi) { return $Sorted[$lo] }
    return $Sorted[$lo] + ($Sorted[$hi] - $Sorted[$lo]) * ($i - $lo)
}

function Stat {
    param([double[]]$s)
    if (-not $s -or $s.Count -eq 0) { return $null }
    $x = @($s | Sort-Object)
    return [pscustomobject]@{
        n       = $x.Count
        min     = [math]::Round($x[0], 2)
        median  = [math]::Round((Percentile $x 0.50), 2)
        p90     = [math]::Round((Percentile $x 0.90), 2)
        p99     = [math]::Round((Percentile $x 0.99), 2)
        max     = [math]::Round($x[-1], 2)
        mean    = [math]::Round((($x | Measure-Object -Average).Average), 2)
        samples = @($s | ForEach-Object { [math]::Round($_, 2) })
    }
}

# ───────────────────────────────────── process identity helpers ──
function Get-ProcInfo {
    param([int]$Id)
    if ($Id -le 4) { return $null }
    return Get-CimInstance Win32_Process -Filter "ProcessId=$Id" -ErrorAction SilentlyContinue
}

# ONE CIM query for every process of the given image names, instead of one query
# per PID. This matters to the measurements, not just to the clock: this machine
# has two dozen conhost processes, and a per PID query costs about 150 ms, so the
# old per cell teardown spent seconds walking them while the next cell was about
# to be timed. The CIM objects carry ProcessId, ParentProcessId, CreationDate and
# CommandLine, so no second lookup is needed either.
function Get-ProcInfosByName {
    param([string[]]$Images)
    $parts = @()
    foreach ($img in $Images) {
        if (-not $img) { continue }
        $nm = if ($img -like "*.exe") { $img } else { "$img.exe" }
        $parts += "Name='$nm'"
    }
    if ($parts.Count -eq 0) { return @() }
    return @(Get-CimInstance Win32_Process -Filter ($parts -join " OR ") -ErrorAction SilentlyContinue)
}

function Get-CimStart {
    param($Ci)
    $t = [datetime]::MinValue
    try { $t = [datetime]$Ci.CreationDate } catch {}
    return $t
}

# The parent chain of a PID, with creation time validation: a parent that is
# YOUNGER than its child means that parent PID has been recycled and the chain
# beyond it is a fiction, so the walk stops there. Without this check a recycled
# PID can make an unrelated process look like an ancestor, and killing by
# ancestry would then hit anything at all.
function Get-Chain {
    param([int]$Start, [int]$Max = 12)
    $out = @()
    $cur = $Start
    $childTime = [datetime]::MaxValue
    for ($i = 0; $i -lt $Max -and $cur -gt 4; $i++) {
        $ci = Get-ProcInfo $cur
        if (-not $ci) { break }
        $t = [datetime]::MinValue
        try { $t = [datetime]$ci.CreationDate } catch {}
        if ($t -gt $childTime) { break }
        $out += [int]$ci.ProcessId
        $childTime = $t
        $cur = [int]$ci.ParentProcessId
    }
    return $out
}

$GuiNames     = @("WindowsTerminal","wezterm-gui","alacritty")
$ConsoleNames = @("conhost","OpenConsole")
$AllHostNames = $GuiNames + $ConsoleNames

# The PID set of every terminal process that existed BEFORE this run, plus this
# script's own ancestry and the console host that owns it. Nothing in here is
# ever killed, whatever else happens.
#
# The loop variable is $hn and NOT $n on purpose. PowerShell variable names are
# case insensitive, so at script scope $n IS the [int]$N parameter, and
# assigning a host name to it throws "Cannot convert value WindowsTerminal to
# type System.Int32" on stderr. That is exactly what happened before: this
# baseline stayed EMPTY, every terminal on the machine then looked like one this
# run had opened, and the cleanup audit killed the user's Windows Terminal, the
# tab that Claude Code itself was running in included. Do not rename it back.
$BaselineHosts = @{}
foreach ($hn in $AllHostNames) { $BaselineHosts[$hn] = @(Get-Process -Name $hn -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id) }
$script:Protected = [System.Collections.Generic.HashSet[int]]::new()
$SelfChain = Get-Chain $PID
foreach ($id in $SelfChain) { [void]$script:Protected.Add($id) }
foreach ($hn in $ConsoleNames) {
    foreach ($p in @(Get-Process -Name $hn -ErrorAction SilentlyContinue)) {
        $ci = Get-ProcInfo $p.Id
        if ($ci -and $script:Protected.Contains([int]$ci.ParentProcessId)) { [void]$script:Protected.Add($p.Id) }
    }
}
foreach ($hn in $AllHostNames) { foreach ($id in $BaselineHosts[$hn]) { [void]$script:Protected.Add($id) } }

# Belt and braces for the bug described above: if a terminal is running but the
# baseline for it is empty, the protection is not in place and the run must not
# continue.
foreach ($hn in $AllHostNames) {
    $live = @(Get-Process -Name $hn -ErrorAction SilentlyContinue).Count
    if ($live -gt 0 -and @($BaselineHosts[$hn]).Count -eq 0) {
        Write-Host "[FAIL] baseline for $hn is empty while $live of them are running; refusing to run, the cleanup would not be able to tell them apart" -ForegroundColor Red
        exit 1
    }
}

function Kill-Pid {
    param([int]$Id)
    if ($Id -le 4) { return }
    if ($script:Protected.Contains($Id)) { return }   # own ancestry or a window we did not open
    try { Stop-Process -Id $Id -Force -ErrorAction SilentlyContinue } catch {}
}

# Record a PID this suite spawned together with the creation time of the
# process that holds it right now, so a later recycled PID is never mistaken
# for it. Called immediately after Start-Process or after the marker file names
# the cell's shell.
function Register-Started {
    param([int]$Id)
    if ($Id -le 4) { return }
    [void]$script:Started.Add($Id)
    $ci = Get-ProcInfo $Id
    if ($ci) { $script:StartedAt[$Id] = Get-CimStart $ci }
}

# True when the process holding $Id today is the one this suite recorded under
# that PID: same PID AND a creation time within a second of the recorded one.
# A PID this suite never recorded is never "ours". A recorded PID whose
# creation time could not be read at registration is trusted only while the
# process is still younger than the run, which keeps the pre-fix behaviour for
# that rare case without widening it.
function Is-OurProcess {
    param([int]$Id)
    if ($Id -le 4) { return $false }
    if (-not $script:StartedAt.ContainsKey($Id)) { return $false }
    $ci = Get-ProcInfo $Id
    if (-not $ci) { return $false }
    $now = Get-CimStart $ci
    $then = $script:StartedAt[$Id]
    if ($then -eq [datetime]::MinValue) { return ($now -ge $script:RunStartedAt.AddSeconds(-2)) }
    if ($now -eq [datetime]::MinValue) { return $false }
    return ([math]::Abs(($now - $then).TotalSeconds) -le 1.0)
}
$script:RunStartedAt = Get-Date

# True when the console host recorded in $Rec (a Find-CellHosts record with a
# `started` creation time) is still the same process, not a later one that was
# handed the same PID. A record without a readable creation time falls back to
# the pre-fix behaviour for that record only.
function Is-SameHost {
    param($Rec)
    $ci = Get-ProcInfo ([int]$Rec.pid)
    if (-not $ci) { return $false }
    $then = [datetime]::MinValue
    try { if ($null -ne $Rec.started) { $then = [datetime]$Rec.started } } catch {}
    if ($then -eq [datetime]::MinValue) { return $true }
    $now = Get-CimStart $ci
    if ($now -eq [datetime]::MinValue) { return $false }
    return ([math]::Abs(($now - $then).TotalSeconds) -le 1.0)
}

function Count-Children {
    param([int]$Id)
    return @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$Id" -ErrorAction SilentlyContinue).Count
}

function Snapshot-Hosts {
    $h = @{}
    foreach ($hn in $AllHostNames) { $h[$hn] = @() }
    foreach ($ci in (Get-ProcInfosByName $AllHostNames)) {
        $nm = [IO.Path]::GetFileNameWithoutExtension($ci.Name)
        if ($h.ContainsKey($nm)) { $h[$nm] += [int]$ci.ProcessId }
    }
    return $h
}

# Terminal and console host processes that appeared since $Snap AND can be
# attributed to this cell. See mechanism 3 in the header.
function Find-CellHosts {
    param($Snap, [datetime]$Since, [string]$Gui, [int[]]$Ours)
    $found = @()
    $all = Get-ProcInfosByName $AllHostNames
    $byPid = @{}
    foreach ($ci in $all) { $byPid[[int]$ci.ProcessId] = $ci }
    foreach ($ci in $all) {
        $who = [int]$ci.ProcessId
        $hn = [IO.Path]::GetFileNameWithoutExtension($ci.Name)
        if ($Snap[$hn] -contains $who) { continue }
        if ($script:Protected.Contains($who)) { continue }
        $st = Get-CimStart $ci
        if ($st -gt [datetime]::MinValue -and $st -lt $Since.AddSeconds(-2)) { continue }
        $ppid = [int]$ci.ParentProcessId
        $mine = $false
        if ($Gui -and $hn -eq $Gui) { $mine = $true }
        elseif ($ConsoleNames -contains $hn) {
            # A console host is attributed through its parent, which is the
            # process that owns the console. The parent's PID must be one of
            # ours AND its image must be something this suite actually launches.
            # The image check is not belt and braces, it is load bearing: PIDs
            # are recycled within seconds on a busy machine, and without it a
            # bash.exe started by the agent that was running the benchmark
            # inherited a just freed PID from a cell, had its console host blamed
            # on the suite, and failed T5 on three otherwise clean runs
            # (2026-09-10, "children still attached: bash.exe").
            # The image check alone was not enough either: on 2026-09-16 the
            # freed PID went to the agent harness's own cmd.exe wrapper, which
            # IS one of these images. So the parent must also be the very
            # process this suite spawned under that PID (creation time match,
            # Is-OurProcess), not merely a process of the right image at the
            # right PID.
            $ourImages = @("pwsh.exe","cmd.exe",$PsmuxImage)
            $pp = if ($byPid.ContainsKey($ppid)) { $byPid[$ppid] } else { Get-ProcInfo $ppid }
            if (($Ours -contains $ppid) -and $pp -and ($ourImages -contains $pp.Name) -and (Is-OurProcess $ppid)) { $mine = $true }
            elseif ($pp -and $Gui -and $pp.Name -like "$Gui*") { $mine = $true }
            elseif ($pp -and ($Ours -contains [int]$pp.ParentProcessId) -and ($ourImages -contains $pp.Name) -and (Is-OurProcess ([int]$pp.ParentProcessId))) { $mine = $true }
        }
        if ($mine) {
            # stamp the host with its creation time so the audit at the end can
            # tell this host from a later process that inherited its PID
            $rec = [pscustomobject]@{ pid = $who; name = $hn; ppid = $ppid; started = $st }
            $found += $rec
            [void]$script:Opened.Add($rec)
        }
    }
    return $found
}

# A GUI process this suite started is killed outright. A console host is NOT,
# while it still has a child: that makes it somebody's live window. It is
# reaped only once its client is gone and it is empty, which is the state a
# leaked tab host is in.
function Close-CellHosts {
    param($Hosts)
    if (-not $Hosts) { return }
    foreach ($h in $Hosts) { if ($GuiNames -contains $h.name) { Kill-Pid $h.pid } }
    Start-Sleep -Milliseconds 250
    foreach ($h in $Hosts) {
        if ($ConsoleNames -notcontains $h.name) { continue }
        if (-not (Get-Process -Id $h.pid -ErrorAction SilentlyContinue)) { continue }
        if ((Count-Children $h.pid) -eq 0) { Kill-Pid $h.pid }
    }
}

# The terminal emulator that owns a process, found by walking its parent chain
# to the first GUI process. The process to ask about is the one that OWNS THE
# CONSOLE the user is looking at: the pwsh for a host cell, but the psmux
# CLIENT for a psmux cell, because a psmux pane's shell lives inside the
# server's ConPTY and has no terminal above it at all.
# This works for a Windows Terminal TAB, where the hosting WT process is the
# user's existing one and no new GUI process exists to attribute; host_shared
# then says the working set is not ours to claim.
function Get-GuiHost {
    param([int]$AnchorPid)
    $out = [ordered]@{ gui = 0; gui_name = ""; gui_shared = $false }
    if ($AnchorPid -le 0) { return $out }
    foreach ($id in (Get-Chain $AnchorPid)) {
        $ci = Get-ProcInfo $id
        if (-not $ci) { continue }
        $nm = [IO.Path]::GetFileNameWithoutExtension($ci.Name)
        # STOP when the walk leaves the cell and enters the suite's own ancestry
        # through an ordinary process. A cell launched with Start-Process is a
        # child of the suite, so the chain continues up into whatever terminal the
        # suite itself was started from, and without this guard a bare pwsh
        # console cell reported the benchmark author's own Windows Terminal as its
        # host: 478 MB of working set and somebody else's CPU charged to a cell
        # that is really hosted by a conhost. Measured 2026-09-10.
        # A Windows Terminal TAB is the case this must NOT break: there the chain
        # is client <- wrapper cmd <- OpenConsole <- WindowsTerminal, it never
        # passes through the suite, and that WT is genuinely the cell's host even
        # though it is also the suite's own. Hence the test is on ordinary
        # processes only, never on a terminal or a console host.
        if (($SelfChain -contains $id) -and ($AllHostNames -notcontains $nm)) { break }
        if ($GuiNames -contains $nm) {
            $out.gui = $id; $out.gui_name = $nm
            $out.gui_shared = [bool]($BaselineHosts[$nm] -contains $id)
            break
        }
    }
    return $out
}

# The conhost or OpenConsole that owns a console. It is a CHILD of the console
# owner, not an ancestor, so the parent chain never finds it: look for one
# parented to any of the candidate PIDs (the console owner itself, and its own
# parent, which is the wrapper cmd in the GUI cells).
function Get-ConsoleHostFor {
    param([int[]]$Candidates)
    $cands = @()
    foreach ($id in $Candidates) {
        if ($id -le 0) { continue }
        $cands += $id
        $ci = Get-ProcInfo $id
        if ($ci) { $cands += [int]$ci.ParentProcessId }
    }
    $ourImages = @("pwsh.exe","cmd.exe",$PsmuxImage)
    foreach ($ci in (Get-ProcInfosByName $ConsoleNames)) {
        $who = [int]$ci.ProcessId
        if ($script:Protected.Contains($who)) { continue }
        $ppid = [int]$ci.ParentProcessId
        if ($cands -notcontains $ppid) { continue }
        # same PID recycling guard as Find-CellHosts: the owner has to be one of
        # the images this suite launches, or an unrelated console host can be
        # sampled as if it were the cell's
        $pp = Get-ProcInfo $ppid
        if (-not $pp -or ($ourImages -notcontains $pp.Name)) { continue }
        return [pscustomobject]@{ pid = $who; name = [IO.Path]::GetFileNameWithoutExtension($ci.Name) }
    }
    return $null
}

# psmux processes running THIS binary and carrying THIS run's namespace. An
# installed psmux elsewhere, another agent's scratch copy, or the main
# checkout's test runner never matches. Get-Process narrows to the image first:
# a bare Win32_Process query walks every process on the machine and costs
# seconds on a loaded box, which would show up inside the very measurements
# this suite takes.
function Get-OurPsmuxProcs {
    param([switch]$ClientsOnly, [switch]$ServersOnly, [string]$Ns = $RunId)
    $out = @()
    foreach ($ci in (Get-ProcInfosByName @($PsmuxImage))) {
        if (-not $ci.CommandLine) { continue }
        # Contains, not -like: the binary path is a literal, and -like would
        # read a bracket in it as a wildcard class.
        if (-not $ci.CommandLine.Contains($Psmux)) { continue }
        if ($ci.CommandLine -notmatch [regex]::Escape("-L $Ns")) { continue }
        $isServer = $ci.CommandLine -match '\sserver\s'
        if ($ClientsOnly -and $isServer) { continue }
        if ($ServersOnly -and -not $isServer) { continue }
        $out += $ci
    }
    return $out
}

# The server for one session, from its <ns>__<session>.pid anchor. The file
# holds "<pid>:<creationtime>", so a recycled PID is rejected instead of being
# sampled as if it were the server. A warm standby shares the image and the
# namespace but never this file, which is why the anchor is used and not the
# command line.
function Get-SessionServerPid {
    param([string]$Session, [string]$Ns = $RunId)
    $f = Join-Path $env:PSMUX_DATA_DIR "${Ns}__$Session.pid"
    if (-not (Test-Path $f)) { return 0 }
    $txt = ""
    try { $txt = (Get-Content $f -Raw).Trim() } catch { return 0 }
    $parts = $txt -split ':'
    $pid0 = 0
    if (-not [int]::TryParse($parts[0], [ref]$pid0)) { return 0 }
    if (-not (Get-Process -Id $pid0 -ErrorAction SilentlyContinue)) { return 0 }
    return $pid0
}

function Stop-OurPsmux {
    param([string]$Ns = $RunId)
    & $Psmux -L $Ns kill-server 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    foreach ($p in (Get-OurPsmuxProcs -Ns $Ns)) { Kill-Pid $p.ProcessId }
    Start-Sleep -Milliseconds 200
}

# Kill anything still carrying this run's scratch directory on its command line:
# the wrapper cmd.exe that hosts a Windows Terminal tab, a pwsh whose marker
# never arrived, a psmux client. The path contains this suite's own PID, so
# nothing outside this run can match. See mechanism 2 in the header.
function Stop-RunDirProcesses {
    foreach ($ci in (Get-ProcInfosByName @("cmd","pwsh",$PsmuxImage))) {
        $who = [int]$ci.ProcessId
        if ($who -eq $PID) { continue }
        if ($script:Protected.Contains($who)) { continue }
        if ($ci.CommandLine -and $ci.CommandLine.Contains($RunDir)) { Kill-Pid $who }
    }
}

# One cell's window, tab, shell and server, closed by PID the moment its
# measurement is captured and before the next cell starts.
function Close-Cell {
    param($Hosts, [int]$LauncherPid = 0, [int]$ShellPid = 0, [string]$Ns = "")
    if ($Ns) { Stop-OurPsmux -Ns $Ns }
    if ($ShellPid) { Kill-Pid $ShellPid }
    Start-Sleep -Milliseconds 400
    Stop-RunDirProcesses
    Start-Sleep -Milliseconds 300
    if ($LauncherPid -and (Get-Process -Id $LauncherPid -ErrorAction SilentlyContinue)) { Kill-Pid $LauncherPid }
    Close-CellHosts $Hosts
    Start-Sleep -Milliseconds 300
}

# ─────────────────────────────────────── memory and CPU sampling ──
function Sample-Proc {
    param([int]$Id, [string]$Role)
    if ($Id -le 0) { return $null }
    $p = Get-Process -Id $Id -ErrorAction SilentlyContinue
    if (-not $p) { return $null }
    $cpu = 0.0
    try { $cpu = $p.TotalProcessorTime.TotalMilliseconds } catch {}
    return [pscustomobject]@{
        role    = $Role
        pid     = $Id
        name    = $p.ProcessName
        ws_mb   = [math]::Round($p.WorkingSet64 / 1MB, 2)
        priv_mb = [math]::Round($p.PrivateMemorySize64 / 1MB, 2)
        cpu_ms  = [math]::Round($cpu, 1)
    }
}

function Sample-Roles {
    param([hashtable]$Roles)
    $o = [ordered]@{}
    foreach ($r in @("server","client","shell","host","console_host")) {
        if (-not $Roles.ContainsKey($r)) { continue }
        $s = Sample-Proc ([int]$Roles[$r]) $r
        if ($s) { $o[$r] = $s }
    }
    return $o
}

# CPU consumed between two samples of the same role set, scaled: $Scale is the
# divisor that turns raw ms into the reported unit (keystrokes/100 for the
# typing cost, idle seconds for the idle percentage).
function Cpu-Delta {
    param($A, $B, [double]$Div, [double]$Mul = 1.0)
    $o = [ordered]@{}
    if (-not $A -or -not $B) { return $o }
    foreach ($r in $A.Keys) {
        if (-not $B.Contains($r)) { continue }
        if ($A[$r].pid -ne $B[$r].pid) { continue }   # process was replaced: not a delta
        $d = ($B[$r].cpu_ms - $A[$r].cpu_ms)
        if ($d -lt 0) { $d = 0 }
        $o[$r] = [math]::Round(($d / $Div) * $Mul, 2)
    }
    return $o
}

function Sum-Roles {
    param($Map, [string[]]$Roles)
    $t = 0.0
    if (-not $Map) { return $t }
    foreach ($r in $Roles) { if ($Map.Contains($r)) { $t += [double]$Map[$r] } }
    return $t
}

# ────────────────────────────────────────────── host discovery ──
$WT = Join-Path $env:LOCALAPPDATA "Microsoft\WindowsApps\wt.exe"
if (-not (Test-Path $WT)) { $c = Get-Command wt.exe -ErrorAction SilentlyContinue; $WT = if ($c) { $c.Source } else { $null } }
$WEZ = "C:\Program Files\WezTerm\wezterm.exe"
if (-not (Test-Path $WEZ)) { $c = Get-Command wezterm.exe -ErrorAction SilentlyContinue; $WEZ = if ($c) { $c.Source } else { $null } }
$ALAC = "C:\Program Files\Alacritty\alacritty.exe"
if (-not (Test-Path $ALAC)) { $c = Get-Command alacritty.exe -ErrorAction SilentlyContinue; $ALAC = if ($c) { $c.Source } else { $null } }
if (-not (Get-Command pwsh -ErrorAction SilentlyContinue)) { Write-Host "[SKIP] pwsh not on PATH; this suite needs PowerShell 7"; exit 0 }
$WtWasRunning = [bool](Get-Process WindowsTerminal -ErrorAction SilentlyContinue)

# Every wt window this suite opens is named, so the bench window is always
# identifiable and never mixed up with one the user opened.
$WtWindow = "psmuxbench$PID"

# A terminal that is installed but cannot be STARTED. Sweeps 2026-10-07_01-38-07
# and 2026-10-07_10-59-31 both lost this whole suite to its 900 s timeout: the
# Windows Terminal app alias (WindowsApps\wt.exe) refused every launch with
# "The file cannot be accessed by the system", and each refused launch still
# waited out the 30 s marker, then retried and waited again. Three WT cells,
# two attempts, five rounds is 900 s, so the suite never reached the keystroke
# or creation sections and its JSON stayed "complete": false. The cause was the
# machine, not psmux: a staged WT update (1.24.12741) cannot register while the
# older WT hosting the developer's own session is still running (package
# activation returns 0x80073D02, ERROR_PACKAGES_IN_USE), and the alias still
# points at the old package. Restarting Windows Terminal applies the update.
#
# So the first refused Start-Process marks the HOST down: every later cell on
# that host is skipped at once, the reason goes into the JSON as hosts_down,
# and T9 fails loudly, because a comparison against Windows Terminal that
# silently has no Windows Terminal in it is worse than a failure. Everything
# that does not need the broken host is still measured.
$script:HostDown = [ordered]@{}
function Set-HostDown {
    param([string]$Gui, [string]$Why)
    if (-not $Gui -or $script:HostDown.Contains($Gui)) { return }
    $hint = ""
    if ($Why -match 'cannot be accessed by the system') {
        $hint = " (an app execution alias that cannot be opened usually means a staged package update is waiting for the running copy to exit: restart that terminal)"
    }
    $script:HostDown[$Gui] = "$Why$hint"
    Warn ("{0} could not be started, so every cell it hosts is skipped for the rest of the run: {1}{2}" -f $Gui, $Why, $hint)
}

# The sha is the one of the tree the MEASURED binary was built in, or
# "installed" for a cargo install copy, never the checkout this script happens
# to sit in: pointed at the installed psmux, the old form stamped the run with
# whatever HEAD the worktree was on, which is a lie in exactly the case that
# matters, comparing a fresh build against the installed one.
. "$PSScriptRoot\perf_metrics_common.ps1"
$GitSha = Get-PerfGitSha $Psmux
$CpuName = ""
try { $CpuName = (Get-CimInstance Win32_Processor -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Name) } catch {}
$RamGb = 0
try { $RamGb = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1) } catch {}

Head "psmux versus the terminals installed on this machine"
Info "binary        $Psmux"
Info "data dir      $env:PSMUX_DATA_DIR"
Info "namespaces    -L $RunId (warm cells and sections 2 and 3), -L $ColdId (cold launch cells)"
Info "hosts         wt=$([bool]$WT)  wezterm=$([bool]$WEZ)  alacritty=$([bool]$ALAC)"
Info "wt already running (a new window opens inside an existing process): $WtWasRunning"
Info ("n per launch cell = {0}, keystrokes per latency cell = {1}, creations per kind = {2}, idle window = {3}s" -f $N, $Keys, $Creates, $IdleSeconds)
Info ("protected from every kill: own ancestry {0} plus every terminal already open" -f ($SelfChain -join "<-"))

# ──────────────────────────────────────────────── build keylat ──
$csc = "C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
$KeyLat = Join-Path $RunDir "keylat.exe"
$KeyLatSrc = Join-Path $PSScriptRoot "keylat.cs"
if ((Test-Path $csc) -and (Test-Path $KeyLatSrc)) { & $csc /nologo /optimize "/out:$KeyLat" $KeyLatSrc 2>&1 | Out-Null }
if (-not (Test-Path $KeyLat)) { Warn "keylat.exe could not be compiled; the keystroke, memory and CPU sections will be skipped"; $KeyLat = $null }

# The ConPTY floor probe: a pseudoconsole host with NO psmux in it at all, which
# is what makes the keystroke threshold judge psmux instead of judging Windows.
# Same compiler and same staleness check as tests/test_keystroke_latency_gate.ps1.
$EchoLat = Join-Path $RunDir "conpty_echolat.exe"
$EchoLatSrc = Join-Path $PSScriptRoot "conpty_echolat.cs"
if ((Test-Path $csc) -and (Test-Path $EchoLatSrc)) { & $csc /nologo /optimize "/out:$EchoLat" $EchoLatSrc 2>&1 | Out-Null }
if (-not (Test-Path $EchoLat)) { Warn "conpty_echolat.exe could not be compiled; the keystroke threshold falls back to its absolute ceiling"; $EchoLat = $null }

# The host neutral "the shell is up" beacon.
$Marker = Join-Path $RunDir "marker.ps1"
@'
param([string]$Out)
[IO.File]::WriteAllText($Out, "$([Diagnostics.Stopwatch]::GetTimestamp()) $PID")
'@ | Set-Content -Path $Marker -Encoding UTF8

$ShellFlags = @("-NoLogo","-NoProfile","-NoExit","-File")
# argv for Start-Process / psmux: pwsh is left bare so that no path with a
# space has to survive a cmd wrapper and psmux's command joiner.
function Shell-Argv { param([string]$mf) return @("pwsh") + $ShellFlags + @($Marker, $mf) }
function Shell-CmdLine { param([string]$mf) return "pwsh " + ($ShellFlags -join ' ') + " `"$Marker`" `"$mf`"" }

function Wait-Marker {
    param([string]$File, [int]$TimeoutMs = 30000)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        if (Test-Path $File) {
            try {
                $t = [IO.File]::ReadAllText($File)
                if ($t -match '^(\d+)\s+(\d+)') { return @{ Ticks = [long]$Matches[1]; ShellPid = [int]$Matches[2] } }
            } catch {}
        }
        Start-Sleep -Milliseconds 2
    }
    return $null
}

function New-Wrapper {
    # $Trace: a PSMUX_PTY_TRACE base for a psmux cell hosted by a terminal. The
    # tab is created by the terminal process, which does not pass the caller's
    # environment through, so the variable has to be set here, inside the tab.
    param([string]$Tag, [string]$Line, [string]$Trace = "")
    $f = Join-Path $RunDir "wrap_$Tag.cmd"
    $body = "@echo off`r`n" +
            "set PSMUX_DATA_DIR=$env:PSMUX_DATA_DIR`r`n" +
            "set PSMUX_SERVER_IMAGE_NAMES=$env:PSMUX_SERVER_IMAGE_NAMES`r`n" +
            "set PSMUX_SESSION_NAME=`r`n" +
            $(if ($Trace) { "set PSMUX_PTY_TRACE=$Trace`r`n" } else { "" }) +
            "$Line`r`n"
    Set-Content -Path $f -Value $body -Encoding ASCII
    return $f
}

# ──────────────────────────────────────────── metrics, written early ──
# The JSON is rewritten after every section with complete=false, so a run that
# is killed half way through still leaves everything it had measured, and says
# which sections were asked for. A file full of empty sections used to be
# indistinguishable from a run that measured nothing.
function Save-Metrics {
    param($Rows = @(), $Deltas = [ordered]@{})
    # One load reading per save, and this function is called once after every
    # section, so the samples map onto the sections without any bookkeeping.
    # A reading costs one second; there are six saves in a full run.
    try { Add-PerfLoadSample ("section {0}" -f $script:Thresholds.Count) | Out-Null } catch { }
    $payload = [ordered]@{
        # This suite keeps its own schema rather than the shared envelope's flat
        # `schema = 2`: it is rewritten after every section (complete=false) and
        # carries one block per HOST, which the shared shape has no room for.
        # It already records everything the envelope does - binary, version,
        # git_sha, machine, cpu, ram - so `envelope_schema` and `suite` are
        # stated here to say so explicitly, and a reader that keys off either
        # name finds this file. See tests\perf_metrics_common.ps1.
        schema          = "psmux.perf_vs_terminals.v2"
        envelope_schema = 2
        suite           = "test_perf_vs_terminals"
        complete    = $script:Complete
        timestamp   = (Get-Date).ToString("o")
        binary      = $Psmux
        version     = ((& $Psmux -V 2>&1) | Select-Object -Last 1)
        git_sha     = $GitSha
        machine     = [ordered]@{
            os        = [Environment]::OSVersion.VersionString
            cpu       = $CpuName
            cpu_count = [Environment]::ProcessorCount
            ram_gb    = $RamGb
            note      = "may have been measured while other builds or benchmarks were running; medians and n>=5 are used for that reason"
        }
        # That note used to be the only statement about what else the machine
        # was doing, and it says "may have been", which answers nothing. This
        # is the measurement: \Processor(_Total)\% Processor Time sampled by
        # the shared helper, same shape and same field names as every other
        # perf JSON, so a loaded run can be told from a quiet one rather than
        # guessed at. Save-Metrics is called after every section, and the
        # samples accumulate, so the block grows as the run goes on.
        load           = (Get-PerfLoadSummary)
        # The reference arms' own p90 over their own median, and the verdict it
        # produced. This is the other half of "was this run trustworthy", and
        # unlike the CPU counter it is measured in the same interleaved window
        # as the timings it judges.
        ref_spread     = $script:RefSpread
        ref_unstable   = $script:RefUnstable
        hosts_present  = [ordered]@{ windows_terminal = [bool]$WT; wezterm = [bool]$WEZ; alacritty = [bool]$ALAC }
        hosts_down     = $script:HostDown
        wt_was_running = $WtWasRunning
        params         = [ordered]@{
            n = $N; keys = $Keys; creates = $Creates; idle_seconds = $IdleSeconds
            skip_launch = [bool]$SkipLaunch; skip_keys = [bool]$SkipKeys; skip_create = [bool]$SkipCreate
            quick = [bool]$Quick
        }
        launch_to_prompt    = $script:Launch
        keystroke_to_screen = $script:Key
        conpty_floor        = $script:Floor
        keystroke_split     = $script:KeySplit
        resources           = $script:Resource
        creation_latency    = $script:Create
        summary_table  = $Rows
        psmux_overhead = $Deltas
        thresholds     = $script:Thresholds
        failed         = $script:Fails
        warnings       = @($script:Notes)
    }
    try { $payload | ConvertTo-Json -Depth 9 | Set-Content -Path $MetricsFile -Encoding UTF8 } catch { Write-Host "[WARN] could not write the metrics JSON: $_" -ForegroundColor Yellow }
    return $payload
}
$null = Save-Metrics

# ══════════════════════════════════════════════════════════════════
#  SECTION 1  LAUNCH TO PROMPT
# ══════════════════════════════════════════════════════════════════
# The cells are INTERLEAVED: repetition 0 of every host, then repetition 1 of
# every host, and so on. Running five repetitions of Windows Terminal back to
# back and only then five of psmux makes the comparison hostage to whatever the
# machine was doing during each block, and this machine drifts by a factor of
# two within a minute. Round robin puts every cell in the same weather.
#
# Two namespaces are in play. The COLD psmux cells live in their own namespace
# that is torn down after every repetition, so each of their repetitions pays a
# real server spawn. The WARM cells live in the namespace with the anchor
# session, so a psmux server and a warm standby are always there, which is the
# steady state a user meets after the first window of the day. Separate
# namespaces are what let the two be interleaved instead of run in blocks.
#
# A repetition whose marker never arrives is RETRIED once. A single missed
# marker used to cost the whole cell its sample, and a cell with no samples is
# a dash in the summary table; one retry turns a transient miss back into data.
function Invoke-LaunchRep {
    param([hashtable]$Cell, [int]$Rep, [int]$Attempt = 0)
    $name = $Cell.Name
    $mf = Join-Path $RunDir ("m_{0}_{1}_{2}.txt" -f $name, $Rep, $Attempt)
    Remove-Item $mf -Force -ErrorAction SilentlyContinue
    $spec = & $Cell.Build ("{0}x{1}" -f $Rep, $Attempt) $mf
    $gui = $Cell.Gui
    $snap = Snapshot-Hosts
    $since = Get-Date

    if ($gui -and $script:HostDown.Contains($gui)) { return -1.0 }
    $t0 = [Diagnostics.Stopwatch]::GetTimestamp()
    try {
        $p = Start-Process -FilePath $spec.Exe -ArgumentList $spec.Argv -PassThru -ErrorAction Stop
    } catch {
        Set-HostDown $gui ($_.Exception.Message -replace '\s+', ' ')
        if (-not $gui) { Warn ("    {0,-22} rep {1}: could not start: {2}" -f $name, $Rep, $_.Exception.Message) }
        return -1.0
    }
    Register-Started $p.Id
    $m = Wait-Marker $mf 30000
    $ms = -1.0
    $shellPid = 0
    if ($m) {
        $ms = ($m.Ticks - $t0) * 1000.0 / $Freq
        $shellPid = $m.ShellPid
        Register-Started $shellPid
        Write-Host ("    {0,-22} rep {1}: {2,8:F1} ms" -f $name, $Rep, $ms) -ForegroundColor DarkGray
    } else { Warn ("    {0,-22} rep {1}: no marker within 30 s" -f $name, $Rep) }

    Start-Sleep -Milliseconds 700
    $hosts = Find-CellHosts -Snap $snap -Since $since -Gui $gui -Ours @($p.Id, $shellPid)
    $ns = if ($Cell.Cold) { $ColdId } else { "" }
    Close-Cell -Hosts $hosts -LauncherPid $p.Id -ShellPid $shellPid -Ns $ns
    if ($ms -lt 0 -and $Attempt -lt 1) {
        Warn ("    {0,-22} rep {1}: retrying once" -f $name, $Rep)
        return (Invoke-LaunchRep -Cell $Cell -Rep $Rep -Attempt ($Attempt + 1))
    }
    return $ms
}

$script:LaunchCells = @()
if (-not $SkipLaunch) {
    Head "1. LAUNCH TO PROMPT  (identical shell in every cell: pwsh -NoProfile, cells interleaved)"
    Stop-OurPsmux -Ns $RunId
    Stop-OurPsmux -Ns $ColdId

    $cells = [System.Collections.ArrayList]::new()
    [void]$cells.Add(@{ Name="bare_pwsh"; Gui=""; Cold=$false; Build = { param($i,$mf) @{ Exe = "pwsh"; Argv = ((Shell-Argv $mf) | Select-Object -Skip 1) } } })
    if ($WT) {
        [void]$cells.Add(@{ Name="wt_pwsh"; Gui="WindowsTerminal"; Cold=$false; Build = {
            param($i,$mf); $w = New-Wrapper "wt_$i" (Shell-CmdLine $mf); @{ Exe = $WT; Argv = @("-w",$WtWindow,"cmd","/c",$w) } } })
    } else { Skip "Windows Terminal not installed" }
    if ($WEZ) {
        [void]$cells.Add(@{ Name="wezterm_pwsh"; Gui="wezterm-gui"; Cold=$false; Build = {
            param($i,$mf); $w = New-Wrapper "wez_$i" (Shell-CmdLine $mf); @{ Exe = $WEZ; Argv = @("start","--","cmd","/c",$w) } } })
    } else { Skip "WezTerm not installed" }
    if ($ALAC) {
        [void]$cells.Add(@{ Name="alacritty_pwsh"; Gui="alacritty"; Cold=$false; Build = {
            param($i,$mf); $w = New-Wrapper "alac_$i" (Shell-CmdLine $mf); @{ Exe = $ALAC; Argv = @("-e","cmd","/c",$w) } } })
    } else { Skip "Alacritty not installed" }
    [void]$cells.Add(@{ Name="psmux_attached"; Gui=""; Cold=$true; Build = {
        param($i,$mf); @{ Exe = $Psmux; Argv = (@("-L",$ColdId,"new-session","-s","la$i") + (Shell-Argv $mf)) } } })
    if ($WT) {
        [void]$cells.Add(@{ Name="psmux_in_wt"; Gui="WindowsTerminal"; Cold=$true; Build = {
            param($i,$mf); $w = New-Wrapper "pwt_$i" ("`"$Psmux`" -L $ColdId new-session -s pw$i " + (Shell-CmdLine $mf)); @{ Exe = $WT; Argv = @("-w",$WtWindow,"cmd","/c",$w) } } })
    }
    [void]$cells.Add(@{ Name="psmux_attached_warm"; Gui=""; Cold=$false; Build = {
        param($i,$mf); @{ Exe = $Psmux; Argv = (@("-L",$RunId,"new-session","-s","lw$i") + (Shell-Argv $mf)) } } })
    if ($WT) {
        [void]$cells.Add(@{ Name="psmux_in_wt_warm"; Gui="WindowsTerminal"; Cold=$false; Build = {
            param($i,$mf); $w = New-Wrapper "pww_$i" ("`"$Psmux`" -L $RunId new-session -s pww$i " + (Shell-CmdLine $mf)); @{ Exe = $WT; Argv = @("-w",$WtWindow,"cmd","/c",$w) } } })
    }
    $script:LaunchCells = @($cells | ForEach-Object { $_.Name })

    # the anchor keeps a psmux server and a warm standby alive for the warm cells
    & $Psmux -L $RunId new-session -d -s anchor 2>&1 | Out-Null
    Start-Sleep -Seconds 3

    $samples = @{}
    foreach ($c in $cells) { $samples[$c.Name] = @() }
    for ($rep = 0; $rep -lt $N; $rep++) {
        Say ("  round $rep of $N")
        foreach ($c in $cells) {
            $ms = Invoke-LaunchRep -Cell $c -Rep $rep
            if ($ms -ge 0) { $samples[$c.Name] += $ms }
        }
    }
    foreach ($c in $cells) {
        $st = Stat $samples[$c.Name]
        if ($st) {
            $script:Launch[$c.Name] = $st
            Write-Host ("  {0,-22} n={1}  min={2,7:F1}  median={3,7:F1}  p90={4,7:F1}  max={5,7:F1} ms" -f $c.Name, $st.n, $st.min, $st.median, $st.p90, $st.max) -ForegroundColor Green
        } else { Warn "  $($c.Name) produced no samples" }
    }
    Stop-OurPsmux -Ns $ColdId
    Stop-OurPsmux -Ns $RunId

    Head "1. LAUNCH THRESHOLDS"
    $bare = $script:Launch["bare_pwsh"]

    # Was the baseline itself steady during this run? Measured on the reference
    # cells only, because those are the ones the difference thresholds subtract.
    # 1.35 sits above the 1.10 a quiet run produced and below the 1.72 a run
    # with stalled launches produced, both measured on this machine on
    # 2026-09-22 with the same binary.
    $RefSpreadMax = 1.35
    foreach ($refName in @("bare_pwsh", "wt_pwsh")) {
        $r = $script:Launch[$refName]
        if (-not $r -or -not $r.median -or $r.median -le 0) { continue }
        $sp = [math]::Round($r.p90 / $r.median, 3)
        $script:RefSpread[$refName] = $sp
        if ($sp -gt $RefSpreadMax) {
            $script:RefUnstable = $true
            Warn ("{0} p90 {1:F0} ms is {2}x its own median {3:F0} ms, over {4}x: this run's baseline was being stalled, so the launch DIFFERENCE thresholds are reported as warnings rather than failures" -f $refName, $r.p90, $sp, $r.median, $RefSpreadMax)
        } else {
            Info ("{0} spread p90/median {1}x, within {2}x: the baseline was steady enough to subtract from" -f $refName, $sp, $RefSpreadMax)
        }
    }
    # T1 is an ABSOLUTE DELTA, not a ratio. A ratio of medians moves when bare
    # pwsh moves, and bare pwsh on this machine ranges from 350 to 600 ms
    # depending on what else is running, so the same psmux build scored 1.7x on
    # one run and 2.5x on another with its own cost unchanged. What psmux owns is
    # the difference: the client, the server, the ConPTY and the claim. Measured
    # 175 to 213 ms on master after the double shell fix; the bug that fix removed
    # showed 440 to 530 ms here, so 350 ms still catches it with room to spare and
    # without failing on a slow shell start that psmux did not cause.
    if ($bare -and $script:Launch["psmux_attached"]) {
        $pc = $script:Launch["psmux_attached"]
        Check -LoadSensitive "T1 psmux attached launch minus bare pwsh (cold server)" ($pc.median - $bare.median) 350 "ms" `
            ("psmux_attached median {0:F0} ms minus bare pwsh {1:F0} ms; this cell pays a server spawn on every repetition" -f $pc.median, $bare.median)
    } else { Warn "T1 not evaluated (a cell is missing)" }
    # The steady state case the old ratio was judged on keeps its own line, same
    # budget: this is the launch a user meets all day, with a server already up.
    if ($bare -and $script:Launch["psmux_attached_warm"]) {
        $pa = $script:Launch["psmux_attached_warm"]
        Check -LoadSensitive "T1b psmux attached launch minus bare pwsh (warm server)" ($pa.median - $bare.median) 350 "ms" `
            ("psmux_attached_warm median {0:F0} ms minus bare pwsh {1:F0} ms" -f $pa.median, $bare.median)
    } else { Warn "T1b not evaluated (a cell is missing)" }
    if ($script:Launch["wt_pwsh"] -and $script:Launch["psmux_in_wt_warm"]) {
        $pw = $script:Launch["psmux_in_wt_warm"]; $wtc = $script:Launch["wt_pwsh"]
        Check -LoadSensitive "T2 psmux in WT over plain WT (warm)" ($pw.median - $wtc.median) 300 "ms" `
            ("psmux_in_wt_warm {0:F0} ms minus wt_pwsh {1:F0} ms" -f $pw.median, $wtc.median)
    } else { Warn "T2 not evaluated (a cell is missing)" }
    if ($script:Launch["wt_pwsh"] -and $script:Launch["psmux_in_wt"]) {
        Check -LoadSensitive "T2b psmux in WT over plain WT (cold server)" ($script:Launch["psmux_in_wt"].median - $script:Launch["wt_pwsh"].median) 700 "ms" `
            "the first psmux window of the day also pays a cold server spawn"
    }
    $null = Save-Metrics
}

# ══════════════════════════════════════════════════════════════════
#  SECTION 2  KEYSTROKE TO SCREEN, MEMORY AND CPU
# ══════════════════════════════════════════════════════════════════
function Invoke-KeyLat {
    param([int]$TargetPid, [string]$Label, [int]$Count)
    $out = Join-Path $RunDir "kl_$Label.txt"
    Remove-Item $out -Force -ErrorAction SilentlyContinue
    & $KeyLat --pid $TargetPid --label $Label --out $out --mode single --n $Count --warmup 5 --gap 120 2>&1 | Out-Null
    if (-not (Test-Path $out)) { Warn "  $Label : keylat produced no output"; return $false }
    $txt = Get-Content $out -Raw
    $m = [regex]::Match($txt, 'SUMMARY \S+ n=(\d+) min=([\d.]+) p25=([\d.]+) median=([\d.]+) mean=([\d.]+) p90=([\d.]+) p99=([\d.]+) max=([\d.]+)')
    if (-not $m.Success) { Warn ("  $Label : no SUMMARY -> " + (($txt.Trim() -split "`n")[0])); return $false }
    $raw = [regex]::Match($txt, 'RAW \S+ ([\d.,]+)')
    $samples = if ($raw.Success) { @($raw.Groups[1].Value -split ',' | Where-Object { $_ } | ForEach-Object { [double]$_ }) } else { @() }
    $st = [pscustomobject]@{
        n = [int]$m.Groups[1].Value
        min = [double]$m.Groups[2].Value
        median = [double]$m.Groups[4].Value
        mean = [double]$m.Groups[5].Value
        p90 = [double]$m.Groups[6].Value
        p99 = [double]$m.Groups[7].Value
        max = [double]$m.Groups[8].Value
        samples = $samples
    }
    $script:Key[$Label] = $st
    Write-Host ("  {0,-22} n={1,-4} min={2,6:F2}  median={3,6:F2}  p90={4,6:F2}  p99={5,6:F2}  max={6,6:F2} ms" -f `
        $Label, $st.n, $st.min, $st.median, $st.p90, $st.p99, $st.max) -ForegroundColor Green
    return $true
}

# One keystroke cell: bring the shell up, find every process that makes the
# cell work, sample memory and CPU, type into it, sample again, then sit still
# for the idle window and sample a third time. Everything it opened is closed
# before it returns.
function Measure-KeyCell {
    param([string]$Cell, [string]$Exe, [string[]]$Argv, [string]$MarkerFile,
          [string]$GuiProcName = "", [switch]$IsPsmux, [string]$Session = "", [int]$Attempt = 0,
          [string]$TraceBase = "", [switch]$SplitPass)
    # $SplitPass: the SECOND, traced pass over a psmux cell. The first pass is
    # untraced and owns the end to end number and every memory and CPU figure,
    # so those stay measurements of the shipped path: tracing writes and flushes
    # a line per hop in the server and the client, and no number that T3b or T6
    # to T8 judge should ever include that. This pass only types the same keys
    # down the same path and splits them; it samples nothing.
    $kLabel = if ($SplitPass) { "${Cell}_trace" } else { $Cell }
    $snap = Snapshot-Hosts
    $since = Get-Date
    Remove-Item $MarkerFile -Force -ErrorAction SilentlyContinue
    # A psmux cell runs with the keystroke trace on (see section 2's thresholds
    # and Get-PerfShellSplit): set only around this launch, so the client
    # inherits it and a server the client spawns inherits it from the client.
    # A terminal hosted cell also gets it from its wrapper (New-Wrapper -Trace).
    # Every attempt starts from an empty trace so a retried cell is never split
    # against a previous attempt's server.
    if ($TraceBase) {
        Get-ChildItem -Path (Split-Path -Parent $TraceBase) -Filter ((Split-Path -Leaf $TraceBase) + ".*") -ErrorAction SilentlyContinue |
            Remove-Item -Force -ErrorAction SilentlyContinue
    }
    if ($GuiProcName -and $script:HostDown.Contains($GuiProcName)) {
        Skip "  $Cell : $GuiProcName could not be started earlier in this run (see hosts_down)"
        return $false
    }
    $p = $null
    try {
        if ($TraceBase) { $env:PSMUX_PTY_TRACE = $TraceBase }
        $p = Start-Process -FilePath $Exe -ArgumentList $Argv -PassThru -ErrorAction Stop
    } catch {
        Set-HostDown $GuiProcName ($_.Exception.Message -replace '\s+', ' ')
        if (-not $GuiProcName) { Warn "  $Cell : could not start: $($_.Exception.Message)" }
        return $false
    } finally {
        Remove-Item Env:\PSMUX_PTY_TRACE -ErrorAction SilentlyContinue
    }
    Register-Started $p.Id
    $m = Wait-Marker $MarkerFile 30000
    $ok = $false
    $shellPid = 0
    if (-not $m) { Warn "  $Cell : the shell never came up" }
    else {
        $shellPid = $m.ShellPid
        Register-Started $shellPid
        Start-Sleep -Seconds 3    # let the shell finish painting its first prompt

        # who makes this cell work
        $roles = @{ shell = $shellPid }
        $target = $shellPid
        $anchor = $shellPid
        if ($IsPsmux) {
            # psmux is measured at the CLIENT console: the whole pipeline, TCP
            # hop and pushed frame included. The client is also the process the
            # terminal actually hosts, so it is the anchor for host lookup.
            #
            # Both of these are POLLED, not sampled once, and the reason is the
            # warm pane pool: new-session claims a standby server whose shell is
            # already booted, so the marker can be written before the client has
            # finished attaching and before the claim has renamed the registry
            # files from __warm__ to this session. On a loaded machine that race
            # is wide: measured 2026-09-10, a single look 3 s after the marker
            # found neither the client nor the <ns>__<session>.pid anchor, and the
            # whole psmux cell was lost for the run.
            $clientPid = 0
            $serverPid = 0
            $poll = [Diagnostics.Stopwatch]::StartNew()
            while ($poll.ElapsedMilliseconds -lt 10000 -and ($clientPid -eq 0 -or $serverPid -eq 0)) {
                if ($clientPid -eq 0) {
                    $c = @(Get-OurPsmuxProcs -ClientsOnly | Sort-Object CreationDate)
                    if ($c.Count -gt 0) { $clientPid = [int]$c[-1].ProcessId }
                }
                if ($serverPid -eq 0) { $serverPid = Get-SessionServerPid -Session $Session }
                if ($clientPid -eq 0 -or $serverPid -eq 0) { Start-Sleep -Milliseconds 250 }
            }
            if ($clientPid -gt 0) { $roles["client"] = $clientPid; $target = $clientPid; $anchor = $clientPid }
            else {
                Warn ("  {0} : no psmux client found after {1:F0} s. pmux processes seen: {2}" -f $Cell, ($poll.Elapsed.TotalSeconds), (@(Get-ProcInfosByName @($PsmuxImage) | ForEach-Object { "$($_.ProcessId):$($_.CommandLine)" }) -join " | "))
                $target = 0
            }
            if ($serverPid -gt 0) { $roles["server"] = $serverPid }
            else {
                Warn ("  {0} : no server pid anchor for session {1}; data dir holds: {2}" -f $Cell, $Session, (@(Get-ChildItem $env:PSMUX_DATA_DIR -File -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name) -join ","))
            }
        }
        $sh = Get-GuiHost $anchor
        if ($sh.gui -gt 0) { $roles["host"] = $sh.gui }
        $ch = Get-ConsoleHostFor @($anchor, $shellPid)
        if ($ch) { $roles["console_host"] = $ch.pid }

        $atPrompt = Sample-Roles $roles
        if ($target -gt 0) {
            # keylat detaches from its own console and attaches to the target's.
            # That call comes back ERROR_ACCESS_DENIED now and then on a loaded
            # machine, and a second try a moment later works, so the probe is
            # retried here rather than throwing away the cell and its memory and
            # CPU samples with it.
            for ($ka = 0; $ka -lt 3 -and -not $ok; $ka++) {
                if ($ka -gt 0) { Start-Sleep -Milliseconds 800 }
                $ok = Invoke-KeyLat $target $kLabel $Keys
            }
        }
        $afterKeys = Sample-Roles $roles

        # Split every keystroke of a psmux cell into the pane shell's own echo
        # time and psmux's hops, from the server's trace of this very run.
        if ($ok -and $SplitPass -and $TraceBase -and $script:Key[$kLabel]) {
            $srvHow = "unknown"
            if ($roles.ContainsKey("server")) {
                $sci = Get-ProcInfo ([int]$roles["server"])
                if ($sci -and $sci.CommandLine) { $srvHow = $(if ($sci.CommandLine -match '__warm__') { "warm claim" } else { "cold spawn" }) }
            }
            $sp = Get-PerfShellSplit -KeylatOut (Join-Path $RunDir "kl_$kLabel.txt") -TraceBase $TraceBase
            $st = $script:Key[$kLabel]
            $script:Key.Remove($kLabel)
            $cov = if ($sp -and $st.n -gt 0) { $sp.PsmuxMs.Count / [double]$st.n } else { 0 }
            if ($sp -and $cov -ge 0.8) {
                $hs = [double[]]($sp.PsmuxMs | Sort-Object)
                $ss = [double[]]($sp.ShellMs | Sort-Object)
                $script:KeySplit[$Cell] = ([pscustomobject]@{
                    n = $hs.Count; server = $srvHow
                    traced_median = $st.median; traced_p99 = $st.p99
                    psmux_median = (Percentile $hs 0.5); psmux_p90 = (Percentile $hs 0.9); psmux_p99 = (Percentile $hs 0.99)
                    shell_median = (Percentile $ss 0.5); shell_p99 = (Percentile $ss 0.99)
                })
                Write-Host ("      split ({0}, n={1}): pane shell echo median {2:F2} ms | psmux hops median {3:F2}  p90 {4:F2}  p99 {5:F2} ms" -f `
                    $srvHow, $hs.Count, $script:KeySplit[$Cell].shell_median, $script:KeySplit[$Cell].psmux_median, $script:KeySplit[$Cell].psmux_p90, $script:KeySplit[$Cell].psmux_p99) -ForegroundColor Green
            } else {
                Warn ("  {0} : the keystroke trace split {1} of {2} keys (server: {3}); this cell's floor subtraction is NOT judged, its end to end number is reported only" -f `
                    $Cell, $(if ($sp) { $sp.PsmuxMs.Count } else { 0 }), $st.n, $srvHow)
            }
        }

        # TWO idle windows, because psmux's client polls adaptively (1 ms while
        # typing, then 5 ms, then 50 ms) and so does ConPTY. A window opened half
        # a second after the last keystroke measures the ramp DOWN, not idle, and
        # would blame steady state for work that is really the tail of the typing
        # burst. So: one window immediately after (reported as the settling
        # figure, information only) and one after a further settle, which is the
        # steady state number T7 is judged on. Nothing is typed or resized in
        # either; anything burning CPU here is polling.
        # The SETTLED window is longer than the ramp down one on purpose, and the
        # reason is resolution, not patience. TotalProcessorTime advances in 15.6 ms
        # scheduler ticks, so in a 3 s window one tick is 0.52 percent of a core and
        # a two process sum can only land on 0, 0.52, 1.04, 1.56, 2.08 and so on.
        # T7's gate (3 percent since 2026-09-11) once sat at 2, between two of those steps: the same
        # binary measured 2.08 and 3.12 ten minutes apart, ie two ticks each then
        # three ticks each. An 8 s window puts a tick at 0.195 percent, so 2 percent
        # is ten ticks and the verdict stops depending on a single scheduling
        # accident. The ramp down window stays short because it is reported, not
        # judged, and its job is only to show the adaptive poll coming down.
        if (-not $SplitPass) {
        $settle = 4
        $settledSeconds = [math]::Max($IdleSeconds, 8)
        Start-Sleep -Milliseconds 500
        $idleA = Sample-Roles $roles
        Start-Sleep -Seconds $IdleSeconds
        $idleB = Sample-Roles $roles
        Start-Sleep -Seconds $settle
        $idleC = Sample-Roles $roles
        Start-Sleep -Seconds $settledSeconds
        $idleD = Sample-Roles $roles

        # The memory a psmux session really holds, taken here because the warm
        # pool has long finished filling by now: the server's whole tree (pane
        # shell, the pool's spare shells, their console hosts, the standby
        # server and its shells) plus the attached client. server+client alone
        # was ~25 MB against ~660 MB for the tree at warm-pool-size 3
        # (2026-10-09), so a head to head on the narrow number flattered psmux.
        # The narrow number is kept beside it (psmux_ws_mb_server_client) so the
        # trend stays comparable with runs recorded before this.
        $sessTree = $null
        if ($IsPsmux -and $roles.ContainsKey("server")) {
            $sessTree = Get-PerfTreeMemory ([int]$roles["server"])
            if ($sessTree.processes -gt 0 -and $roles.ContainsKey("client") -and ($sessTree.pids -notcontains [int]$roles["client"])) {
                $cs = Get-PerfProcSample ([int]$roles["client"]) "client"
                if ($cs) {
                    $sessTree.processes++
                    $sessTree.pids += [int]$roles["client"]
                    $sessTree.ws_mb = [math]::Round($sessTree.ws_mb + [double]$cs.ws_mb, 1)
                    $sessTree.private_mb = [math]::Round($sessTree.private_mb + [double]$cs.private_mb, 1)
                    if (-not $sessTree.by_name.Contains("psmux")) { $sessTree.by_name["psmux"] = 0 }
                    $sessTree.by_name["psmux"]++
                }
            }
            if ($sessTree.processes -eq 0) { $sessTree = $null }
        }

        $keyCpu   = Cpu-Delta $atPrompt $afterKeys ([double]$Keys) 100.0
        $idleEarly = Cpu-Delta $idleA $idleB ([double]($IdleSeconds * 1000)) 100.0
        $idleCpu  = Cpu-Delta $idleC $idleD ([double]($settledSeconds * 1000)) 100.0
        # CPU per 100 keys is strongly machine state dependent: the SAME binary
        # measured 1445 and 2734 ms ten minutes apart, and the pane shell's own CPU
        # doubled with it (859 to 1406 ms), which is the tell that the machine got
        # more expensive rather than psmux getting slower. This ratio divides
        # psmux's cost by the shell's cost in the same cell, which cancels most of
        # that factor: 1.68 then 1.94 for those same two runs. It is REPORTED and
        # never judged; it is here so the next calibration of T8 has a conditioned
        # number to work from instead of an absolute one that moves with the box.
        $shellKeyCpu = Sum-Roles $keyCpu @("shell")
        $psmuxKeyCpu = Sum-Roles $keyCpu @("server","client")
        $keyCpuRatio = if ($shellKeyCpu -gt 0) { [math]::Round($psmuxKeyCpu / $shellKeyCpu, 2) } else { $null }
        $script:Resource[$Cell] = [ordered]@{
            host_shared                  = [bool]$sh.gui_shared
            host_name                    = $sh.gui_name
            keys                         = $Keys
            idle_seconds                 = $IdleSeconds
            idle_settled_window_seconds  = $settledSeconds
            idle_settle_seconds          = $settle
            cpu_per_100_keys_psmux_over_shell = $keyCpuRatio
            at_prompt                    = $atPrompt
            after_keys                   = $afterKeys
            cpu_ms_per_100_keys          = $keyCpu
            idle_cpu_pct_of_core         = $idleCpu
            idle_cpu_pct_of_core_settling = $idleEarly
            session_tree_settled         = $sessTree
        }
        $memLine = (@($atPrompt.Keys | ForEach-Object { "{0} {1:F1}/{2:F1}MB" -f $_, $atPrompt[$_].ws_mb, $atPrompt[$_].priv_mb }) -join "  ")
        $cpuLine = (@($keyCpu.Keys  | ForEach-Object { "{0} {1:F1}" -f $_, $keyCpu[$_] }) -join "  ")
        $idlLine = (@($idleCpu.Keys | ForEach-Object { "{0} {1:F2}%" -f $_, $idleCpu[$_] }) -join "  ")
        $earLine = (@($idleEarly.Keys | ForEach-Object { "{0} {1:F2}%" -f $_, $idleEarly[$_] }) -join "  ")
        Write-Host ("      mem ws/priv at prompt : $memLine") -ForegroundColor DarkGray
        Write-Host ("      cpu ms per 100 keys   : $cpuLine") -ForegroundColor DarkGray
        if ($null -ne $keyCpuRatio) {
            Write-Host ("      cpu per 100 keys, psmux over shell (not judged): {0:F2}x" -f $keyCpuRatio) -ForegroundColor DarkGray
        }
        Write-Host ("      idle cpu, first window ({0}s ramp down, not judged): {1}" -f $IdleSeconds, $earLine) -ForegroundColor DarkGray
        Write-Host ("      idle cpu, settled window ({0}s, T7 judges this)   : {1}" -f $settledSeconds, $idlLine) -ForegroundColor DarkGray
        if ($sessTree) { Write-Host ("      session tree, settled : {0} processes, {1:F1} MB working set, {2:F1} MB private  ({3})" -f $sessTree.processes, $sessTree.ws_mb, $sessTree.private_mb, (@($sessTree.by_name.Keys | ForEach-Object { "$_ x$($sessTree.by_name[$_])" }) -join ', ')) -ForegroundColor DarkGray }
        if ($sh.gui_shared) { Write-Host ("      host $($sh.gui_name) pid $($sh.gui) is shared with the user's own windows: its working set is not this cell's cost") -ForegroundColor DarkGray }
        }
    }
    $hosts = Find-CellHosts -Snap $snap -Since $since -Gui $GuiProcName -Ours @($p.Id, $shellPid)
    Close-Cell -Hosts $hosts -LauncherPid $p.Id -ShellPid $shellPid -Ns $(if ($IsPsmux) { $RunId } else { "" })
    # Up to three attempts. This cell is the one that can be lost to a transient
    # under load: keylat's AttachConsole can come back ERROR_ACCESS_DENIED, and a
    # psmux session can come up with its client already gone. Measured on a
    # machine running three benchmark agents at once, one cell in six runs needed
    # more than two attempts, and a lost cell costs the run its memory and CPU
    # thresholds (T0 says so out loud rather than printing dashes).
    if (-not $ok -and $Attempt -lt 2) {
        Warn ("  {0} : no data, attempt {1} of 3 failed, retrying" -f $Cell, ($Attempt + 1))
        return (Measure-KeyCell -Cell $Cell -Exe $Exe -Argv $Argv -MarkerFile $MarkerFile `
                -GuiProcName $GuiProcName -IsPsmux:$IsPsmux -Session $Session -Attempt ($Attempt + 1) -TraceBase $TraceBase -SplitPass:$SplitPass)
    }
    return $ok
}

$script:KeyCells = @()
if (-not $SkipKeys -and $KeyLat) {
    Head "2. KEYSTROKE TO SCREEN, MEMORY AND CPU  (n=$Keys keys per cell, ${IdleSeconds}s idle window)"
    Info "host cells watch the pane conhost's screen buffer, UPSTREAM of the pseudoconsole pipe"
    Info "psmux cells watch the psmux client's own console, DOWNSTREAM of it, so they also carry the ConPTY floor"
    Stop-OurPsmux

    # ── the ConPTY floor, measured in THIS run ──
    # conpty_echolat hosts a pseudoconsole with the same shell and the same single
    # key record, and no psmux anywhere in the path. Whatever it reports, no
    # ConPTY consumer on this machine can beat it: conhost's pseudoconsole
    # serializer emits a 6 byte ESC[?25l chunk about 0.6 ms after the keystroke and
    # then withholds the chunk carrying the character for a further 14.7 ms, so the
    # character reaches the PIPE about 15.8 ms late while sitting in the pane
    # conhost's screen buffer the whole time. Windows Terminal, WezTerm and
    # Alacritty all pay it; they are not measured paying it here only because their
    # cells watch the pane conhost's screen buffer, upstream of that pipe.
    $script:Floor = $null
    if ($EchoLat) {
        $floorOut = Join-Path $RunDir "floor.txt"
        Remove-Item $floorOut -Force -ErrorAction SilentlyContinue
        & $EchoLat --cmd "pwsh -NoLogo -NoProfile" --n $Keys --gap 120 --settle 4000 --label floor --out $floorOut 2>&1 | Out-Null
        if (Test-Path $floorOut) {
            $ftxt = Get-Content $floorOut -Raw
            $fm = [regex]::Match($ftxt, 'SUMMARY floor n=(\d+) min=([\d.]+) p25=([\d.]+) median=([\d.]+) mean=([\d.]+) p90=([\d.]+) p99=([\d.]+) max=([\d.]+)')
            if ($fm.Success) {
                $script:Floor = [pscustomobject]@{
                    n      = [int]$fm.Groups[1].Value
                    min    = [double]$fm.Groups[2].Value
                    median = [double]$fm.Groups[4].Value
                    mean   = [double]$fm.Groups[5].Value
                    p90    = [double]$fm.Groups[6].Value
                    p99    = [double]$fm.Groups[7].Value
                    max    = [double]$fm.Groups[8].Value
                }
                $split = [regex]::Match($ftxt, 'trials_with_char_in_first_chunk=(\d+) of (\d+)')
                $note = if ($split.Success) { ("the character arrived in the FIRST read chunk in {0} of {1} trials" -f $split.Groups[1].Value, $split.Groups[2].Value) } else { "" }
                Write-Host ("  {0,-22} n={1,-4} min={2,6:F2}  median={3,6:F2}  p90={4,6:F2}  p99={5,6:F2}  max={6,6:F2} ms" -f `
                    "conpty_floor", $script:Floor.n, $script:Floor.min, $script:Floor.median, $script:Floor.p90, $script:Floor.p99, $script:Floor.max) -ForegroundColor Green
                if ($note) { Write-Host "      $note" -ForegroundColor DarkGray }
            } else { Warn "the ConPTY floor probe produced no SUMMARY; the keystroke threshold falls back to its absolute ceiling" }
        } else { Warn "the ConPTY floor probe produced no output; the keystroke threshold falls back to its absolute ceiling" }
    }

    $mf = Join-Path $RunDir "k_bare.txt"
    $script:KeyCells += "bare_pwsh"
    $null = Measure-KeyCell -Cell "bare_pwsh" -Exe "pwsh" -Argv ((Shell-Argv $mf) | Select-Object -Skip 1) -MarkerFile $mf

    if ($WT) {
        $mf = Join-Path $RunDir "k_wt.txt"
        $w = New-Wrapper "kwt" (Shell-CmdLine $mf)
        $script:KeyCells += "wt_pwsh"
        $null = Measure-KeyCell -Cell "wt_pwsh" -Exe $WT -Argv @("-w",$WtWindow,"cmd","/c",$w) -MarkerFile $mf -GuiProcName "WindowsTerminal"
    }
    if ($WEZ) {
        $mf = Join-Path $RunDir "k_wez.txt"
        $w = New-Wrapper "kwez" (Shell-CmdLine $mf)
        $script:KeyCells += "wezterm_pwsh"
        $null = Measure-KeyCell -Cell "wezterm_pwsh" -Exe $WEZ -Argv @("start","--","cmd","/c",$w) -MarkerFile $mf -GuiProcName "wezterm-gui"
    }
    if ($ALAC) {
        $mf = Join-Path $RunDir "k_alac.txt"
        $w = New-Wrapper "kalac" (Shell-CmdLine $mf)
        $script:KeyCells += "alacritty_pwsh"
        $null = Measure-KeyCell -Cell "alacritty_pwsh" -Exe $ALAC -Argv @("-e","cmd","/c",$w) -MarkerFile $mf -GuiProcName "alacritty"
    }

    $mf = Join-Path $RunDir "k_psmux.txt"
    $script:KeyCells += "psmux_attached"
    $null = Measure-KeyCell -Cell "psmux_attached" -Exe $Psmux -Argv (@("-L",$RunId,"new-session","-s","ka") + (Shell-Argv $mf)) -MarkerFile $mf -IsPsmux -Session "ka"
    # the traced pass over the same path, for the split T3a and T3c judge
    $mf = Join-Path $RunDir "k_psmux_trace.txt"
    $null = Measure-KeyCell -Cell "psmux_attached" -Exe $Psmux -Argv (@("-L",$RunId,"new-session","-s","ka") + (Shell-Argv $mf)) -MarkerFile $mf -IsPsmux -Session "ka" -TraceBase (Join-Path $RunDir "ptrace_ka") -SplitPass

    if ($WT) {
        $mf = Join-Path $RunDir "k_psmuxwt.txt"
        $w = New-Wrapper "kpwt" ("`"$Psmux`" -L $RunId new-session -s kw " + (Shell-CmdLine $mf))
        $script:KeyCells += "psmux_in_wt"
        $null = Measure-KeyCell -Cell "psmux_in_wt" -Exe $WT -Argv @("-w",$WtWindow,"cmd","/c",$w) -MarkerFile $mf -GuiProcName "WindowsTerminal" -IsPsmux -Session "kw"
        $mf = Join-Path $RunDir "k_psmuxwt_trace.txt"
        $w = New-Wrapper "kpwtt" ("`"$Psmux`" -L $RunId new-session -s kw " + (Shell-CmdLine $mf)) -Trace (Join-Path $RunDir "ptrace_kw")
        $null = Measure-KeyCell -Cell "psmux_in_wt" -Exe $WT -Argv @("-w",$WtWindow,"cmd","/c",$w) -MarkerFile $mf -GuiProcName "WindowsTerminal" -IsPsmux -Session "kw" -TraceBase (Join-Path $RunDir "ptrace_kw") -SplitPass
    }

    Head "2. KEYSTROKE THRESHOLDS  (judged against the ConPTY floor measured in this run)"
    # WHY THIS IS A DELTA AND NOT AN ABSOLUTE NUMBER.
    # The old T3a asked for a 10 ms median and could never pass with a shell in
    # the pane, because about 15.8 ms of the number is conhost's pseudoconsole
    # serializer withholding the chunk that carries the character, and no change
    # to psmux can remove it. Traced and quantified by perfE on 2026-09-10: the
    # character is in the pane conhost's screen buffer 0.6 ms after the keystroke,
    # the pipe delivers it 14.7 ms later, and tests/conpty_echolat.cs measures
    # that floor at 15.72 to 15.85 ms with no psmux in the path at all. psmux's
    # own contribution above the floor is 0.74 ms median and 1.57 ms p99. At the
    # same probe point as the host cells, psmux measures 0.55 ms against the
    # 1.03 ms reported here for Windows Terminal.
    # So the budgets are 2.5 ms on the median and 6 ms on the p99, which is about
    # three times the measured overhead: enough headroom for the floor's own run to
    # run spread, tight enough that a psmux regression costing a whole 15.6 ms
    # timer tick cannot hide. The absolute p99 ceiling stays at 25 ms so the total
    # a user waits is still bounded, and a 30 ms median ceiling takes over if the
    # floor probe itself fails.
    #
    # SINCE 2026-10-01 T3a and T3c JUDGE PSMUX'S OWN HOPS, not end to end minus
    # the floor. On 2026-09-30 the standalone floor fell from ~15.7 to ~11.1 ms
    # on this machine while pwsh inside a psmux pane kept taking ~15 ms from
    # psmux's pty write to the read carrying the character, for an old and a new
    # build alike (interleaved A/B, identical to 0.1 ms). The subtraction then
    # charged psmux ~4.5 ms of machine timer state. So every psmux cell runs
    # with PSMUX_PTY_TRACE on and each keystroke is split with the server's own
    # stamps (Get-PerfShellSplit in perf_metrics_common.ps1): psmux's hops are
    # end to end minus the pane shell's echo time. The budgets are unchanged and
    # now stricter in effect, because the old subtraction hid about 1 ms of
    # shell time inside the "overhead". The floor still takes part: T3d holds
    # the pane shell's own echo time against it with a margin wider than the two
    # timer states sit apart (~4.5 ms) and narrower than the extra 15.6 ms tick
    # psmux would cost by answering a shell query late, which a split alone
    # cannot see. A cell whose trace did not split is reported end to end only
    # and says so; its floor subtraction is not judged.
    $best = $null
    foreach ($c in @("psmux_attached","psmux_in_wt")) {
        if ($script:Key[$c] -and ((-not $best) -or ($script:Key[$c].median -lt $best.median))) { $best = $script:Key[$c] }
    }
    $bestSplit = $null; $bestSplitCell = ""
    foreach ($c in @("psmux_attached","psmux_in_wt")) {
        $k = $script:KeySplit[$c]
        if (-not $k) { continue }
        if ((-not $bestSplit) -or ($k.psmux_median -lt $bestSplit.psmux_median)) { $bestSplit = $k; $bestSplitCell = $c }
    }
    foreach ($c in @("psmux_attached","psmux_in_wt")) {
        $k = $script:Key[$c]
        if ($k -and $script:Floor) {
            Info ("{0}: end to end minus the standalone floor, median {1:F2} ms, p99 {2:F2} ms (reported, not judged)" -f $c, ($k.median - $script:Floor.median), ($k.p99 - $script:Floor.p99))
        }
    }
    if ($best) {
        if ($bestSplit) {
            Check "T3a psmux keystroke median, psmux's own hops" $bestSplit.psmux_median 2.5 "ms" `
                ("{0}, {1}: end to end minus the pane shell's own echo time, per keystroke from the server's pty trace; pane shell echo median {2:F2} ms" -f $bestSplitCell, $bestSplit.server, $bestSplit.shell_median) -LoadSensitive
            # T3c used to hold the p99 of these 40 keystrokes to 6 ms. Over 40
            # samples the p99 IS the single worst key, and on 2026-10-04 that
            # key failed the gate in three runs out of five on two builds
            # (own hops 10.4, 8.6 and 9.0 ms) with the medians flat at 1.2 to
            # 1.7 ms and the pane shell's own echo p99 at 18 ms in the same
            # runs: the key had landed on an extra 15.6 ms conhost tick, and the
            # split charges the remainder of that tick to psmux. One key cannot
            # measure a tail. The p90 (the fourth worst of 40) keeps the 6 ms
            # line as the hard gate; the single worst key is reported beside it,
            # and its tail is judged where there are enough samples to judge
            # one: test_keystroke_latency_gate holds the p99 over 120 keys and
            # three runs to 8 ms.
            Check "T3c psmux keystroke p90, psmux's own hops" $bestSplit.psmux_p90 6 "ms" `
                ("{0}: the same split, p90 over {1} keystrokes" -f $bestSplitCell, $bestSplit.n)
            Info ("{0}: psmux's own hops, worst of {1} keystrokes (the p99 of this sample) {2:F2} ms; judged over 120 keys by test_keystroke_latency_gate, not here" -f $bestSplitCell, $bestSplit.n, $bestSplit.psmux_p99)
            if ($script:Floor) {
                Check "T3d pane shell echo under psmux over the standalone ConPTY floor" ($bestSplit.shell_median - $script:Floor.median) 7 "ms" `
                    ("pane shell {0:F2} ms against the floor's {1:F2} ms; a reply psmux answers late would add a whole 15.6 ms tick here" -f $bestSplit.shell_median, $script:Floor.median)
            }
        } else {
            Warn "T3a/T3c not judged: no psmux cell's keystroke trace could be split, so psmux's own hops were not measured; the end to end numbers above are reported only"
        }
        Check "T3b psmux keystroke p99 (absolute)" $best.p99 25 "ms" `
            "what a user actually waits for, floor included"
    } else { Warn "T3 not evaluated (no psmux keystroke samples)" }

    Head "2b. MEMORY AND CPU THRESHOLDS  (psmux_attached: one session, one window, one pane)"
    $r = $script:Resource["psmux_attached"]
    if ($r) {
        if ($r.at_prompt.Contains("server")) {
            Check "T6a psmux server working set, one pane" $r.at_prompt["server"].ws_mb 60 "MB" `
                ("private {0:F1} MB; the limit is about 2x the measurement, it is a leak alarm not a tuning target" -f $r.at_prompt["server"].priv_mb)
        } else { Warn "T6a not evaluated (no server sample)" }
        if ($r.at_prompt.Contains("client")) {
            Check "T6b psmux client working set" $r.at_prompt["client"].ws_mb 60 "MB" `
                ("private {0:F1} MB" -f $r.at_prompt["client"].priv_mb)
        } else { Warn "T6b not evaluated (no client sample)" }
        # T7 judges the SETTLED window, the second one, taken after the client's
        # adaptive poll has ramped back down. The first window is reported beside
        # it as the ramp down and is never judged. Both are printed per cell above,
        # labelled "first window (ramp down)" and "settled window (T7 judges
        # this)", because reading the wrong line off that block has already caused
        # one argument about which figure failed.
        # Resolution note: TotalProcessorTime moves in 15.6 ms scheduler ticks, so
        # one tick in a 3 s window is 0.52 percent of a core. This number is
        # meaningful to about half a percent, and the 2 percent gate is four ticks.
        $idleSum = Sum-Roles $r.idle_cpu_pct_of_core @("server","client")
        if ($r.idle_cpu_pct_of_core.Count -gt 0) {
        # T7 at 3 percent of one core. Recalibrated 2026-09-11: a healthy build reads
        # 0.5 to 1.6 percent on a quiet box and 2.2 to 2.9 percent inside a full
        # sweep (the remaining idle cost is the client's 16 ms input tick, about
        # 0.8 percent per process). The regression class this guards is a busy
        # poll, which reads 7 to 10 percent or more, so 3 percent still catches it
        # while leaving several scheduler ticks of headroom on a loaded machine.
            Check "T7 psmux idle CPU, server plus client (settled window)" $idleSum 3 "% of one core" `
                ("settled window, {0}s after the last keystroke, measured over {1}s with nothing typed, one scheduler tick being {2:F2}%. The ramp down window, not judged, was {3:F2}% of a core" -f $r.idle_settle_seconds, $r.idle_settled_window_seconds, (15.6 / ($r.idle_settled_window_seconds * 1000) * 100), (Sum-Roles $r.idle_cpu_pct_of_core_settling @("server","client")))
        } else { Warn "T7 not evaluated (no idle CPU samples)" }
        # T8 at 3000 ms per 100 keystrokes, ie 30 ms of CPU per key.
        # Recalibrated 2026-09-11 after the first two full sweeps: the same binary
        # measured 1016, 1172, 1289, 2110, 2148 and 2422 ms within a few hours with
        # the pane shell's own CPU moving in step (469 to 2032 ms), so 2000 flapped
        # on machine state alone. 3000 still catches the class of regression this
        # gate exists for (a busy poll or a frame per keystroke doubling lands
        # well above it) without failing a healthy build on a loaded box. The
        # deterministic form of this check is FRAMES per keystroke, which
        # tests/test_idle_socket_traffic.ps1 now pins at idle; a typing frames
        # per key gate is the next calibration step.
        # The measured cost is 1060 ms per 100 keys on a quiet machine and up to
        # 1680 ms loaded, and the cause is frame count, not spinning: at a shell
        # prompt conhost emits the cursor hide chunk and the text chunk 15 ms
        # apart, so psmux builds and pushes TWO frames per keystroke, the second
        # superseding the first, at roughly 4 to 5.7 ms of work per frame. With a
        # pane that writes one chunk per key it is one frame and about half the
        # CPU. 3000 ms leaves room for the loaded case and still catches a doubling.
        # The follow up, which is a change of its own and not a tuning tweak here,
        # is to defer a cursor-visibility-only frame by a short grace so the text
        # frame absorbs it; that would halve frames per keystroke on the typing
        # path. The client's console host costs more than either psmux process
        # (5100 to 5500 ms per 100 keys) and is REPORTED in the table but never
        # judged: it is a legacy console window repainting per write batch, and
        # inside Windows Terminal that work belongs to someone else's GPU.
        $keySum = Sum-Roles $r.cpu_ms_per_100_keys @("server","client")
        if ($r.cpu_ms_per_100_keys.Count -gt 0) {
            Check "T8 psmux CPU per 100 keystrokes, server plus client" $keySum 3000 "ms" `
                ("server {0:F0} plus client {1:F0}; two frames per keystroke at a shell prompt. The pane shell itself cost {2:F0} ms and the client's conhost {3:F0} ms, neither judged. psmux over shell, the machine state independent form, was {4}x" -f (Sum-Roles $r.cpu_ms_per_100_keys @("server")), (Sum-Roles $r.cpu_ms_per_100_keys @("client")), (Sum-Roles $r.cpu_ms_per_100_keys @("shell")), (Sum-Roles $r.cpu_ms_per_100_keys @("console_host")), $r.cpu_per_100_keys_psmux_over_shell)
        } else { Warn "T8 not evaluated (no keystroke CPU samples)" }
    } else { Warn "T6, T7 and T8 not evaluated (the psmux_attached cell produced nothing)" }
    if ($r) { Info ("roles sampled in psmux_attached: " + ((@($r.at_prompt.Keys) -join ", "))) }
    $null = Save-Metrics
} elseif (-not $SkipKeys) { Skip "keystroke, memory and CPU section (keylat.exe unavailable)" }

# ══════════════════════════════════════════════════════════════════
#  SECTION 3  CREATION LATENCY
# ══════════════════════════════════════════════════════════════════
function Connect-Control {
    param([string]$Session, [int]$TimeoutMs = 25000)
    $base = "${RunId}__$Session"
    $portFile = Join-Path $env:PSMUX_DATA_DIR "$base.port"
    $keyFile  = Join-Path $env:PSMUX_DATA_DIR "$base.key"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs -and -not (Test-Path $portFile)) { Start-Sleep -Milliseconds 3 }
    if (-not (Test-Path $portFile)) { return $null }
    for ($try = 0; $try -lt 60; $try++) {
        try {
            $port = [int]((Get-Content $portFile -Raw).Trim())
            $key = if (Test-Path $keyFile) { (Get-Content $keyFile -Raw).Trim() } else { "" }
            $tcp = [Net.Sockets.TcpClient]::new(); $tcp.NoDelay = $true
            $tcp.Connect("127.0.0.1", $port)
            $st = $tcp.GetStream(); $st.ReadTimeout = 8000
            $wr = [IO.StreamWriter]::new($st); $wr.AutoFlush = $true
            $c = @{ Tcp = $tcp; W = $wr; S = $st
                    Buf = [byte[]]::new(262144); Tail = ""; Newest = $null; Reply = $null }
            $wr.WriteLine("AUTH $key")
            $ok = $null
            $dl = [Diagnostics.Stopwatch]::StartNew()
            while ($dl.ElapsedMilliseconds -lt 5000 -and $null -eq $ok) {
                $ok = Get-Reply $c          # the handshake is a protocol reply, not a frame
                if ($null -eq $ok) { [Threading.Thread]::SpinWait(20000) }
            }
            if ($ok -ne "OK") { $tcp.Close(); return $null }
            $wr.WriteLine("PERSISTENT")
            Start-Sleep -Milliseconds 60
            return $c
        } catch { Start-Sleep -Milliseconds 40 }
    }
    return $null
}
function Close-Control { param($c) if ($c) { try { $c.Tcp.Close() } catch {} } }

# A PERSISTENT control connection is a PUSH channel: the server sends a whole
# frame whenever a pane changes. Three things follow, and getting any of them
# wrong costs seconds on the very measurement being taken.
#
# First, do NOT poll with dump-state. Every dump-state makes the server
# serialise a fresh frame, so a tight poll loop both saturates the server that
# is trying to boot the shell being timed AND queues a frame per request on top
# of the pushes. Reading the pushes is free: they arrive exactly when the pane
# changes, which is exactly when the prompt lands. One dump-state is still sent
# as a nudge if nothing has arrived for a while, so a prompt that was already on
# screen is still seen.
#
# Second, read the socket RAW. A StreamReader keeps its own buffer, so
# NetworkStream.DataAvailable can say "nothing pending" while a complete frame
# sits unread inside the reader, and the loop then works on a stale frame
# forever.
#
# Third, and this is the one that cost a whole section: the server answers every
# `dump-state` with a two byte `OK` line, and those acks are INTERLEAVED with the
# state frames on the same socket. A reader that keeps "the newest line" (or that
# queues lines and drains to the last, which is the same thing) therefore throws
# the state frame away and hands the caller `OK` instead. Measured on
# 2026-09-10: 604 "frames" seen in 30 s, every single one of them 2 bytes long,
# and the creation section reported nothing at all. A state frame is a JSON
# object, so lines are classified: one starting with "{" is a frame and is kept
# as the newest frame, anything else is a protocol reply and is kept separately
# for the AUTH handshake. Keep the parse LINEAR too, newest frame only: the
# earlier frames are stale by definition, and the previous version's
# StringBuilder.ToString() per extracted line was quadratic in the frame, which
# is how a run with twenty windows standing came to report 9 to 14 SECOND
# creations that were really the harness re-reading its own backlog.
function Pump-Control {
    param($c)
    try {
        while ($c.S.DataAvailable) {
            $got = $c.S.Read($c.Buf, 0, $c.Buf.Length)
            if ($got -le 0) { break }
            $s = $c.Tail + [Text.Encoding]::UTF8.GetString($c.Buf, 0, $got)
            $last = $s.LastIndexOf("`n")
            if ($last -lt 0) { $c.Tail = $s; continue }
            $c.Tail = $s.Substring($last + 1)
            $lines = $s.Substring(0, $last) -split "`n"
            $gotFrame = $false
            $gotReply = $false
            # backwards, so the FIRST hit of each kind is the newest of that kind
            for ($i = $lines.Count - 1; $i -ge 0; $i--) {
                if ($gotFrame -and $gotReply) { break }
                $line = $lines[$i].TrimEnd("`r")
                if ($line.Length -eq 0) { continue }
                if ($line[0] -eq '{') {
                    if (-not $gotFrame) { $c.Newest = $line; $gotFrame = $true }
                } elseif (-not $gotReply) { $c.Reply = $line; $gotReply = $true }
            }
        }
    } catch {}
}

# The freshest state frame the server has sent, or $null if it has sent no new
# one. Protocol replies never come back from here.
function Get-Frame {
    param($c)
    Pump-Control $c
    $f = $c.Newest
    $c.Newest = $null
    return $f
}

# The freshest protocol reply, for the AUTH handshake.
function Get-Reply {
    param($c)
    Pump-Control $c
    $r = $c.Reply
    $c.Reply = $null
    return $r
}

function Drain-Control {
    param($c)
    try { $c.W.WriteLine("dump-state") } catch {}
    Start-Sleep -Milliseconds 30
    $null = Get-Frame $c
    $null = Get-Reply $c
}

$PromptRe    = [regex]'PS [A-Za-z]:\\\\'
$ActiveWinRe = [regex]'\{"id":(\d+),"name":"[^"]*","active":true'
$WinIdRe     = [regex]'\{"id":(\d+),"name":"'
function Count-Prompts { param([string]$f) if (-not $f) { return 0 } return $PromptRe.Matches($f).Count }
# Every window appears TWICE in a frame (the window list and the pane tree), so
# the ids are de-duplicated. Counting them raw made the burst test think five
# windows had arrived when two had.
function Get-WindowIds {
    param([string]$f)
    if (-not $f) { return @() }
    $seen = [System.Collections.Generic.HashSet[int]]::new()
    foreach ($m in $WinIdRe.Matches($f)) { [void]$seen.Add([int]$m.Groups[1].Value) }
    return @($seen)
}
function Get-ActiveWinId {
    param([string]$f)
    if (-not $f) { return -1 }
    $m = $ActiveWinRe.Match($f)
    if ($m.Success) { return [int]$m.Groups[1].Value }
    return -1
}

# Poll the control connection until $Test is satisfied. A dump-state round trip
# is about 0.15 ms; the SpinWait keeps the server from being hammered while
# staying far below the 15.6 ms floor of Start-Sleep on this timer.
# Two consecutive frames have to agree before a prompt counts, so one racy
# snapshot (a window id already switched while the layout has not caught up)
# cannot score a false 20 ms. The confirming frame costs another 0.15 ms and
# the timestamp reported is still the first frame's.
#
# When it gives up it leaves $script:WaitInfo behind: how many frames it saw,
# how big the last one was, and whether the connection died under it. A bare
# "no prompt within 20 s" says nothing about WHY, and that is what made the
# creation section impossible to debug from a log.
function Wait-Frame {
    param($c, [long]$T0, [scriptblock]$Test, [int]$TimeoutMs = 20000)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $hit = -1.0
    $lastNudge = 0.0
    $frames = 0
    $lastLen = 0
    $script:WaitInfo = [ordered]@{ frames = 0; last_len = 0; reason = "timeout"; ms = 0 }
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        $f = Get-Frame $c
        if ($null -eq $f) {
            if (($sw.Elapsed.TotalMilliseconds - $lastNudge) -gt 50) {
                $lastNudge = $sw.Elapsed.TotalMilliseconds
                try { $c.W.WriteLine("dump-state") }
                catch {
                    $script:WaitInfo = [ordered]@{ frames = $frames; last_len = $lastLen; reason = "connection closed"; ms = [math]::Round($sw.Elapsed.TotalMilliseconds) }
                    return -1
                }
            }
            [Threading.Thread]::SpinWait(20000)
            continue
        }
        $frames++
        $lastLen = $f.Length
        if (& $Test $f) {
            if ($hit -ge 0) { return $hit }
            $hit = ([Diagnostics.Stopwatch]::GetTimestamp() - $T0) * 1000.0 / $Freq
            try { $c.W.WriteLine("dump-state") }
            catch {
                $script:WaitInfo = [ordered]@{ frames = $frames; last_len = $lastLen; reason = "connection closed after a match"; ms = [math]::Round($sw.Elapsed.TotalMilliseconds) }
                return -1
            }
            continue
        }
        $hit = -1.0
    }
    $script:WaitInfo = [ordered]@{ frames = $frames; last_len = $lastLen; reason = "timeout"; ms = [math]::Round($sw.Elapsed.TotalMilliseconds) }
    return -1
}

function Why-Failed {
    $w = $script:WaitInfo
    if (-not $w) { return "no diagnostics" }
    return ("{0} after {1} ms, {2} frames seen, last frame {3} bytes" -f $w.reason, $w.ms, $w.frames, $w.last_len)
}

# Kill the window that was just measured, but ONLY if the session has more than
# one. Killing a session's last window kills the session, the server exits, and
# every later measurement in the section then fails in 50 ms with a closed
# connection: measured 2026-09-10, split -h 0 passed and splits 1 to 4 all died
# that way after a new-window that had silently not arrived. So count first.
function Remove-MeasuredWindow {
    param($c, [string]$Session)
    $f = $null
    for ($i = 0; $i -lt 8 -and $null -eq $f; $i++) {
        try { $c.W.WriteLine("dump-state") } catch { return $false }
        Start-Sleep -Milliseconds 40
        $f = Get-Frame $c
    }
    $count = @(Get-WindowIds $f).Count
    # The pushed frame is not always there to be read at this instant, and a
    # frameless moment used to read as "0 windows", which made the guard decline
    # and let windows accumulate, which is the thing the guard exists to prevent:
    # 12 declines in one run on 2026-09-10. So when the frame cannot answer, ask
    # the server directly. list-windows costs one CLI round trip, about 20 ms, and
    # nothing is being timed at this point in the loop.
    if ($count -lt 1) {
        $listed = @(& $Psmux -L $RunId list-windows -t $Session 2>&1 | Where-Object { $_ -match '^\s*\d+:' })
        $count = $listed.Count
    }
    if ($count -lt 2) {
        Warn ("    not killing the measured window: the session has {0} window(s), killing it would end the session" -f $count)
        return $false
    }
    & $Psmux -L $RunId kill-window -t $Session 2>&1 | Out-Null
    return $true
}

# The control connection can be closed under us when a server is reassigned or
# restarted. Reconnecting costs a few milliseconds and saves the rest of the
# section; returning $null means the section gives up, which T0 then reports.
function Restore-Control {
    param($c, [string]$Session)
    Close-Control $c
    Start-Sleep -Milliseconds 300
    $fresh = Connect-Control $Session 8000
    if ($fresh) { Warn "    control connection was closed; reconnected" }
    else { Warn "    control connection was closed and could not be reopened" }
    return $fresh
}

function Report-Create {
    param([string]$Name, [double[]]$Samples)
    $st = Stat $Samples
    if (-not $st) { Warn "  $Name : no samples"; return }
    $bimodal = ($st.median -gt 0) -and ($st.max -gt 3 * $st.median)
    $st | Add-Member -NotePropertyName bimodal -NotePropertyValue $bimodal -Force
    $script:Create[$Name] = $st
    $tag = if ($bimodal) { "   BIMODAL (max {0:F0} is over 3x the median {1:F0})" -f $st.max, $st.median } else { "" }
    Write-Host ("  {0,-22} n={1,-3} min={2,7:F0}  median={3,7:F0}  p90={4,7:F0}  max={5,7:F0} ms{6}" -f `
        $Name, $st.n, $st.min, $st.median, $st.p90, $st.max, $tag) -ForegroundColor $(if ($bimodal) { "Yellow" } else { "Green" })
}

$script:CreateCells = @()
if (-not $SkipCreate) {
    Head "3. CREATION LATENCY  (default pane shell, so the warm pane pool is in play)"
    Info "timed to a visible prompt over one persistent control connection (about 0.15 ms per poll)"
    Info "each measured window is killed again at once, so the frame the harness re-reads stays small"
    Stop-OurPsmux
    $script:CreateCells = @("first_session","new_window_seq","new_window_burst","split_v_seq","split_h_seq")

    # Each repetition gets up to two attempts. This cell deliberately wipes the
    # registry to force a COLD server, and wiping it while the warm pool is
    # respawning a standby is a race: the standby rewrites its files, the next
    # new-session claims it instead of cold starting, and the control connection
    # is closed under the measurement when that server is reassigned. Measured
    # on 2026-09-10: "connection closed after 12211 ms, 4 frames seen". Sweeping
    # the namespace again after the wipe closes most of it, and a retry covers
    # the rest rather than losing the sample.
    $firstSamples = @()
    for ($i = 0; $i -lt [math]::Min($Creates, 3); $i++) {
        for ($att = 0; $att -lt 2; $att++) {
            Stop-OurPsmux
            Remove-Item -Recurse -Force (Join-Path $RunDir "data") -ErrorAction SilentlyContinue
            New-Item -ItemType Directory -Force -Path (Join-Path $RunDir "data") | Out-Null
            Start-Sleep -Milliseconds 500
            if (@(Get-OurPsmuxProcs).Count -gt 0) { Stop-OurPsmux; Start-Sleep -Milliseconds 300 }
            $s = "first${i}a$att"
            $t0 = [Diagnostics.Stopwatch]::GetTimestamp()
            & $Psmux -L $RunId new-session -d -s $s 2>&1 | Out-Null
            $c = Connect-Control $s
            if (-not $c) { Warn "    first session rep ${i}: no control connection"; continue }
            $ms = Wait-Frame $c $t0 { param($f) (Count-Prompts $f) -ge 1 } 30000
            Close-Control $c
            if ($ms -ge 0) {
                $firstSamples += $ms
                Write-Host ("    first session rep {0}: {1,7:F0} ms" -f $i, $ms) -ForegroundColor DarkGray
                break
            }
            Warn ("    first session rep {0} attempt {1}: no prompt within 30 s ({2})" -f $i, $att, (Why-Failed))
        }
    }
    Report-Create "first_session" $firstSamples
    Stop-OurPsmux

    # 80x25, not 200x50: the frame the harness reads back is proportional to
    # rows times columns times windows, and this session exists to be read from
    # thousands of times.
    $S = "crt"
    & $Psmux -L $RunId new-session -d -s $S -x 80 -y 25 2>&1 | Out-Null
    $conn = Connect-Control $S
    if (-not $conn) { Warn "no control connection; the window and split cells are skipped" }
    else {
        $crtReady = Wait-Frame $conn ([Diagnostics.Stopwatch]::GetTimestamp()) { param($f) (Count-Prompts $f) -ge 1 } 30000
        if ($crtReady -lt 0) { Warn ("  the measurement session never showed a prompt ({0}); the window and split cells will all fail" -f (Why-Failed)) }
        else { Info ("measurement session ready in {0:F0} ms" -f $crtReady) }

        Say "  new-window, $Creates sequential"
        $out = @()
        for ($i = 0; $i -lt $Creates; $i++) {
            $before = @(Get-WindowIds (Get-Frame $conn))
            Drain-Control $conn
            $t0 = [Diagnostics.Stopwatch]::GetTimestamp()
            & $Psmux -L $RunId new-window -t $S 2>&1 | Out-Null
            $ms = Wait-Frame $conn $t0 {
                param($f)
                $a = Get-ActiveWinId $f
                ($a -gt 0) -and ($before -notcontains $a) -and ((Count-Prompts $f) -ge 1)
            } 20000
            if ($ms -ge 0) { $out += $ms; Write-Host ("    new-window {0}: {1,7:F0} ms" -f $i, $ms) -ForegroundColor DarkGray }
            else {
                Warn ("    new-window {0}: no new window with a prompt within 20 s ({1})" -f $i, (Why-Failed))
                if ($script:WaitInfo.reason -like "connection closed*") {
                    $conn = Restore-Control $conn $S
                    if (-not $conn) { break }
                }
            }
            # kill the window just measured: -t <session> resolves to the
            # session's ACTIVE window, which is the one new-window just made.
            $null = Remove-MeasuredWindow $conn $S
            Start-Sleep -Milliseconds 500
            Drain-Control $conn
        }
        Report-Create "new_window_seq" $out

        Say "  new-window, burst of $Creates"
        $beforeIds = @(Get-WindowIds (Get-Frame $conn))
        $want = $beforeIds.Count + $Creates
        Drain-Control $conn
        $t0 = [Diagnostics.Stopwatch]::GetTimestamp()
        for ($i = 0; $i -lt $Creates; $i++) { Start-Process -FilePath $Psmux -ArgumentList @("-L",$RunId,"new-window","-t",$S) -NoNewWindow | Out-Null }
        $ms = Wait-Frame $conn $t0 { param($f) ((Get-WindowIds $f).Count -ge $want) -and ((Count-Prompts $f) -ge 1) } 40000
        if ($ms -ge 0) {
            $script:Create["new_window_burst"] = [pscustomobject]@{ n = $Creates; total_ms = [math]::Round($ms,2); per_window_ms = [math]::Round($ms / $Creates, 2) }
            Write-Host ("  {0,-22} {1} windows, last prompt at {2:F0} ms ({3:F0} ms each)" -f "new_window_burst", $Creates, $ms, ($ms / $Creates)) -ForegroundColor Green
        } else { Warn ("  new_window_burst: timed out ({0})" -f (Why-Failed)) }
        # back down to the one window the section started with
        for ($i = 0; $i -lt $Creates; $i++) { if (-not (Remove-MeasuredWindow $conn $S)) { break }; Start-Sleep -Milliseconds 120 }
        Start-Sleep -Milliseconds 600
        Drain-Control $conn

        foreach ($pair in @(@("-v","split_v_seq"), @("-h","split_h_seq"))) {
            $dir = $pair[0]; $label = $pair[1]
            Say "  split-window $dir, $Creates sequential"
            $out = @()
            for ($i = 0; $i -lt $Creates; $i++) {
                # a fresh window each time, so every split starts from one pane
                & $Psmux -L $RunId new-window -t $S 2>&1 | Out-Null
                $null = Wait-Frame $conn ([Diagnostics.Stopwatch]::GetTimestamp()) { param($f) (Count-Prompts $f) -ge 1 } 20000
                Start-Sleep -Milliseconds 400
                Drain-Control $conn
                $t0 = [Diagnostics.Stopwatch]::GetTimestamp()
                & $Psmux -L $RunId split-window $dir -t $S 2>&1 | Out-Null
                $ms = Wait-Frame $conn $t0 { param($f) (Count-Prompts $f) -ge 2 } 20000
                if ($ms -ge 0) { $out += $ms; Write-Host ("    split {0} {1}: {2,7:F0} ms" -f $dir, $i, $ms) -ForegroundColor DarkGray }
                else {
                    Warn ("    split {0} {1}: no second prompt within 20 s ({2})" -f $dir, $i, (Why-Failed))
                    if ($script:WaitInfo.reason -like "connection closed*") {
                        $conn = Restore-Control $conn $S
                        if (-not $conn) { break }
                    }
                }
                # the split window goes away again, panes and all
                $null = Remove-MeasuredWindow $conn $S
                Start-Sleep -Milliseconds 400
                Drain-Control $conn
            }
            Report-Create $label $out
        }
        Close-Control $conn
    }
    Stop-OurPsmux

    Head "3. CREATION THRESHOLDS"
    if ($script:Create["first_session"]) { Check "T4a first session to prompt (median)" $script:Create["first_session"].median 1000 "ms" "a cold server plus a cold default shell" }
    # T4b judges creations two onwards. Since b117e745 the first creation after
    # a cold launch boots alone by design (403 to 523 ms in every run on
    # 2026-10-06, the rest 16 to 26 ms), and with five samples the interpolated
    # p90 lands 0.6 of the way from the fourth value to that one, so the gate
    # flipped on whether the lone boot was 484 ms (p90 298, pass) or 523 ms
    # (p90 322, fail): sweep 2026-10-06_05-29-09 failed it on [523,17,17,20,18]
    # and an A/B against the pre batch binary gave the same shape. The first
    # creation is judged by T4a and by test_pane_startup_perf's cold check.
    if ($script:Create["new_window_seq"]) {
        $seq = $script:Create["new_window_seq"]
        $rest = @($seq.samples | Select-Object -Skip 1)
        $p90rest = if ($rest.Count -gt 0) { [math]::Round((Percentile ([double[]]($rest | Sort-Object)) 0.9), 2) } else { $seq.p90 }
        Check "T4b new-window p90 (creations two onwards)" $p90rest 300 "ms" "includes the 16 to 20 ms Windows needs to start the psmux.exe client; the first creation after launch is T4a's"
    }
    foreach ($k in @("split_v_seq","split_h_seq")) {
        if ($script:Create[$k]) { Check "T4c $k p90" $script:Create[$k].p90 300 "ms" }
    }
    $null = Save-Metrics
}

# ══════════════════════════════════════════════════════════════════
#  SUMMARY
# ══════════════════════════════════════════════════════════════════
$bareMed = if ($script:Launch["bare_pwsh"]) { $script:Launch["bare_pwsh"].median } else { 0 }
$rows = [System.Collections.ArrayList]::new()
foreach ($cell in @("bare_pwsh","wt_pwsh","wezterm_pwsh","alacritty_pwsh","psmux_attached","psmux_in_wt","psmux_attached_warm","psmux_in_wt_warm")) {
    $l = $script:Launch[$cell]; $k = $script:Key[$cell]; $r = $script:Resource[$cell]
    if (-not $l -and -not $k -and -not $r) { continue }
    $srv = if ($r -and $r.at_prompt.Contains("server")) { $r.at_prompt["server"] } else { $null }
    $cli = if ($r -and $r.at_prompt.Contains("client")) { $r.at_prompt["client"] } else { $null }
    $hst = if ($r -and $r.at_prompt.Contains("host")) { $r.at_prompt["host"] } else { $null }
    [void]$rows.Add([pscustomobject]@{
        host           = $cell
        launch_median  = if ($l) { $l.median } else { $null }
        launch_p90     = if ($l) { $l.p90 } else { $null }
        launch_vs_bare = if ($l -and $bareMed -gt 0) { [math]::Round($l.median / $bareMed, 2) } else { $null }
        key_median     = if ($k) { [math]::Round($k.median, 2) } else { $null }
        key_p90        = if ($k) { [math]::Round($k.p90, 2) } else { $null }
        key_p99        = if ($k) { [math]::Round($k.p99, 2) } else { $null }
        server_ws_mb   = if ($srv) { $srv.ws_mb } else { $null }
        client_ws_mb   = if ($cli) { $cli.ws_mb } else { $null }
        host_ws_mb     = if ($hst) { $hst.ws_mb } else { $null }
        host_shared    = if ($r) { [bool]$r.host_shared } else { $null }
        # Before 2026-10-09 psmux_ws_mb was server plus client only; that narrow
        # number is now psmux_ws_mb_server_client, and psmux_ws_mb is the whole
        # session tree (pane shells, warm pool spares, console hosts, standby
        # server, client). tests\perf_summary.ps1 reads the narrow key with a
        # fallback to psmux_ws_mb for files written before the split.
        psmux_ws_mb_server_client = if ($srv -or $cli) {
                             $a = 0.0; if ($srv) { $a += [double]$srv.ws_mb }
                             if ($cli) { $a += [double]$cli.ws_mb }
                             [math]::Round($a, 2)
                         } else { $null }
        psmux_ws_mb    = if ($r -and $r.session_tree_settled) { $r.session_tree_settled.ws_mb } else { $null }
        psmux_tree_processes = if ($r -and $r.session_tree_settled) { $r.session_tree_settled.processes } else { $null }
        cpu_per_100_keys_psmux = if ($r) { [math]::Round((Sum-Roles $r.cpu_ms_per_100_keys @("server","client")), 1) } else { $null }
        cpu_per_100_keys_host  = if ($r) { [math]::Round((Sum-Roles $r.cpu_ms_per_100_keys @("host","console_host","shell")), 1) } else { $null }
        idle_cpu_pct_psmux     = if ($r) { [math]::Round((Sum-Roles $r.idle_cpu_pct_of_core @("server","client")), 2) } else { $null }
        idle_cpu_pct_host      = if ($r) { [math]::Round((Sum-Roles $r.idle_cpu_pct_of_core @("host","console_host","shell")), 2) } else { $null }
    })
}

$fmt = { param($v, $f) if ($null -eq $v) { "-" } else { $f -f $v } }

Head "SUMMARY  (launch and keystroke in ms; vs_bare is launch versus bare pwsh)"
Write-Host "  keystroke: host rows watch the pane conhost's screen buffer (upstream of the pseudoconsole pipe), psmux rows watch a console downstream of it and so include the ConPTY floor printed below" -ForegroundColor DarkGray
Write-Host ("  {0,-22} {1,10} {2,10} {3,8} {4,9} {5,9} {6,9}" -f "host","launch_med","launch_p90","vs_bare","key_med","key_p90","key_p99") -ForegroundColor White
Write-Host ("  " + ("-" * 78)) -ForegroundColor DarkGray
foreach ($r in $rows) {
    Write-Host ("  {0,-22} {1,10} {2,10} {3,8} {4,9} {5,9} {6,9}" -f `
        $r.host,
        (& $fmt $r.launch_median "{0:F0}"), (& $fmt $r.launch_p90 "{0:F0}"), (& $fmt $r.launch_vs_bare "{0:F2}"),
        (& $fmt $r.key_median "{0:F2}"), (& $fmt $r.key_p90 "{0:F2}"), (& $fmt $r.key_p99 "{0:F2}"))
}

# Memory and CPU, measured on the cell that was typed into. The psmux columns
# are server plus client; the host column is the terminal emulator, and where
# it is a Windows Terminal TAB that process also holds the user's own windows,
# which is what shared means: read its CPU, not its working set.
if ($script:Resource.Count -gt 0) {
    Write-Host ""
    Write-Host ("  {0,-22} {1,9} {2,9} {3,9} {4,9} {5,7} {6,10} {7,10} {8,9} {9,9}" -f `
        "host","srv_ws","cli_ws","tree_ws","host_ws","shared","cpu/100k","host_cpu","idle%","host_idle%") -ForegroundColor White
    Write-Host ("  " + ("-" * 114)) -ForegroundColor DarkGray
    foreach ($r in $rows) {
        if ($null -eq $r.idle_cpu_pct_psmux -and $null -eq $r.host_ws_mb -and $null -eq $r.server_ws_mb) { continue }
        Write-Host ("  {0,-22} {1,9} {2,9} {3,9} {4,9} {5,7} {6,10} {7,10} {8,9} {9,9}" -f `
            $r.host,
            (& $fmt $r.server_ws_mb "{0:F1}"), (& $fmt $r.client_ws_mb "{0:F1}"), (& $fmt $r.psmux_ws_mb "{0:F0}"), (& $fmt $r.host_ws_mb "{0:F1}"),
            $(if ($r.host_shared) { "yes" } else { "no" }),
            (& $fmt $r.cpu_per_100_keys_psmux "{0:F1}"), (& $fmt $r.cpu_per_100_keys_host "{0:F1}"),
            (& $fmt $r.idle_cpu_pct_psmux "{0:F2}"), (& $fmt $r.idle_cpu_pct_host "{0:F2}"))
    }
    Write-Host ("  tree_ws is the whole psmux session (pane shells, warm pool spares, console hosts, standby server, client) once settled, the honest number to set beside a terminal's host_ws plus its shell") -ForegroundColor DarkGray
    Write-Host ("  srv_ws/cli_ws/host_ws are working set MB at prompt ready; cpu/100k is ms of CPU per 100 keystrokes; idle% is percent of ONE core over the settled window with nothing typed, and is the figure T7 judges") -ForegroundColor DarkGray
}

# The two numbers whoever works on startup and latency actually needs.
$deltas = [ordered]@{}
if ($bareMed -gt 0) {
    foreach ($c in @("psmux_attached","psmux_attached_warm")) {
        if ($script:Launch[$c]) { $deltas["$c launch minus bare pwsh"] = [math]::Round($script:Launch[$c].median - $bareMed, 1) }
    }
}
if ($script:Launch["wt_pwsh"]) {
    foreach ($c in @("psmux_in_wt","psmux_in_wt_warm")) {
        if ($script:Launch[$c]) { $deltas["$c launch minus wt"] = [math]::Round($script:Launch[$c].median - $script:Launch["wt_pwsh"].median, 1) }
    }
}
if ($script:Key["bare_pwsh"]) {
    foreach ($c in @("psmux_attached","psmux_in_wt")) {
        if ($script:Key[$c]) { $deltas["$c keystroke minus bare pwsh"] = [math]::Round($script:Key[$c].median - $script:Key["bare_pwsh"].median, 2) }
    }
}
# The number that actually says what psmux costs on a keystroke. The line above
# it, against bare pwsh, compares two DIFFERENT probe points and is kept only
# because it is the quantity the old threshold used: the host cells watch the
# pane conhost's screen buffer, the psmux cells watch a console downstream of the
# pseudoconsole pipe, and the 15.8 ms between those two points is conhost's.
if ($script:Floor) {
    $deltas["conpty floor, no psmux in the path"] = [math]::Round($script:Floor.median, 2)
    foreach ($c in @("psmux_attached","psmux_in_wt")) {
        if ($script:Key[$c]) { $deltas["$c keystroke minus the conpty floor"] = [math]::Round($script:Key[$c].median - $script:Floor.median, 2) }
    }
}
if ($deltas.Count -gt 0) {
    Write-Host ""
    Write-Host "  what psmux adds, median to median (ms)" -ForegroundColor White
    foreach ($k in $deltas.Keys) { Write-Host ("  {0,-46} {1,8:F1}" -f $k, $deltas[$k]) }
}

if ($script:Create.Count -gt 0) {
    Write-Host ""
    Write-Host ("  {0,-22} {1,10} {2,10} {3,10} {4,10}" -f "creation","min","median","p90","max") -ForegroundColor White
    Write-Host ("  " + ("-" * 68)) -ForegroundColor DarkGray
    foreach ($k in $script:Create.Keys) {
        $v = $script:Create[$k]
        if ($null -ne $v.median) {
            $bm = if ($v.bimodal) { "   BIMODAL" } else { "" }
            Write-Host ("  {0,-22} {1,10:F0} {2,10:F0} {3,10:F0} {4,10:F0}{5}" -f $k, $v.min, $v.median, $v.p90, $v.max, $bm)
        } else {
            Write-Host ("  {0,-22} {1,10} {2,10:F0} total, {3:F0} ms each" -f $k, "-", $v.total_ms, $v.per_window_ms)
        }
    }
}

# ─────────────────────────────────────── T0, did we measure anything ──
Head "0. DATA COMPLETENESS"
$missing = [System.Collections.ArrayList]::new()
if (-not $SkipLaunch) { foreach ($c in $script:LaunchCells) { if (-not $script:Launch[$c]) { [void]$missing.Add("launch:$c") } } }
if (-not $SkipKeys -and $KeyLat) {
    foreach ($c in $script:KeyCells) {
        if (-not $script:Key[$c]) { [void]$missing.Add("keystroke:$c") }
        if (-not $script:Resource[$c]) { [void]$missing.Add("resources:$c") }
    }
}
if (-not $SkipCreate) { foreach ($c in $script:CreateCells) { if (-not $script:Create[$c]) { [void]$missing.Add("creation:$c") } } }
# A cell whose terminal could not be started at all is T9's to report, once,
# with the reason; counting it again here would turn one machine fault into
# five failures.
$hostCellRe = @{ "WindowsTerminal" = '(^|:)(wt_|psmux_in_wt)'; "wezterm-gui" = '(^|:)wezterm'; "alacritty" = '(^|:)alacritty' }
$downRe = @($script:HostDown.Keys | ForEach-Object { $hostCellRe[$_] } | Where-Object { $_ })
$missing = [System.Collections.ArrayList]@(@($missing) | Where-Object { $m = $_; -not ($downRe | Where-Object { $m -match $_ }) })
Check "T0 every cell that was not skipped produced data" $missing.Count 0 "" `
    $(if ($missing.Count -gt 0) { "missing: " + ($missing -join ", ") } else { "launch, keystroke, memory, CPU and creation all populated" })
Check "T9 every installed terminal could be started" $script:HostDown.Count 0 "" `
    $(if ($script:HostDown.Count -gt 0) { "could not start: " + (@($script:HostDown.Keys | ForEach-Object { "$_ ($($script:HostDown[$_]))" }) -join "; ") } else { "every installed host launched" })

$script:Complete = $true
$null = Save-Metrics -Rows $rows -Deltas $deltas

# ───────────────────────────────────────────────────── teardown ──
# Nothing this suite opened may outlive it. Every window, tab, shell and server
# is closed by PID as its measurement is captured; this is the final audit, and
# a leftover is a FAILED THRESHOLD, not a warning, because a benchmark that
# litters the desktop is a benchmark nobody will run. Only processes this run
# can be shown to have started are counted, and only those are killed.
function Assert-NoLeftovers {
    param([switch]$Kill)
    $left = [System.Collections.ArrayList]::new()
    # anything still carrying this run's scratch dir: wrapper cmd, shell, client
    foreach ($ci in (Get-ProcInfosByName @("cmd","pwsh",$PsmuxImage))) {
        $who = [int]$ci.ProcessId
        if ($who -eq $PID) { continue }
        if ($script:Protected.Contains($who)) { continue }
        if ($ci.CommandLine -and $ci.CommandLine.Contains($RunDir)) {
            [void]$left.Add([pscustomobject]@{ pid = $who; name = [IO.Path]::GetFileNameWithoutExtension($ci.Name); why = "scratch dir on command line" })
            if ($Kill) { Kill-Pid $who }
        }
    }
    # psmux servers in either of this run's namespaces
    foreach ($ns in @($RunId, $ColdId)) {
        foreach ($ci in (Get-OurPsmuxProcs -Ns $ns)) {
            [void]$left.Add([pscustomobject]@{ pid = $ci.ProcessId; name = $PsmuxImage; why = "namespace $ns" })
            if ($Kill) { Kill-Pid $ci.ProcessId }
        }
    }
    # terminal windows and console hosts THIS RUN opened and attributed to
    # itself. A new window of one of these names that the suite cannot show it
    # opened is somebody else's and is neither blamed on us nor touched.
    $seen = [System.Collections.Generic.HashSet[int]]::new()
    foreach ($o in $script:Opened) {
        if (-not $seen.Add([int]$o.pid)) { continue }
        if ($script:Protected.Contains([int]$o.pid)) { continue }
        if (-not (Get-Process -Id $o.pid -ErrorAction SilentlyContinue)) { continue }
        if (-not (Is-SameHost $o)) { continue }   # the PID now belongs to somebody else's process
        if ($ConsoleNames -contains $o.name) {
            # What T5 is actually about is whether anything the user would have to
            # close is still running: a window, a tab, a shell, a server. A
            # console host is none of those on its own. It exits when its last
            # CLIENT exits, it has no window of its own once that client is gone,
            # and Windows is sometimes slow about it, so:
            #   no client left, or only other empty console hosts nested under it
            #     -> reap it, record it as reaped late, do NOT fail the run
            #   a real client still attached (a shell, a wrapper cmd, a psmux)
            #     -> that IS a leftover and the run fails for it
            # Measured 2026-09-10: an OpenConsole from a Windows Terminal tab
            # outlived its tab holding nothing but an empty conhost, and failing
            # T5 for that made the threshold a coin toss while telling the user
            # nothing.
            $realClients = @()
            foreach ($k in @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($o.pid)" -ErrorAction SilentlyContinue)) {
                $kn = [IO.Path]::GetFileNameWithoutExtension($k.Name)
                if ($ConsoleNames -contains $kn) {
                    if ((Count-Children ([int]$k.ProcessId)) -eq 0) {
                        [void]$script:Reaped.Add([pscustomobject]@{ pid = [int]$k.ProcessId; name = $kn; why = "empty console host nested under one of ours" })
                        if ($Kill) { Kill-Pid ([int]$k.ProcessId) }
                        continue
                    }
                }
                $realClients += "$($k.Name):$($k.ProcessId)"
            }
            if ($realClients.Count -eq 0) {
                [void]$script:Reaped.Add([pscustomobject]@{ pid = $o.pid; name = $o.name; why = "console host with no client left, reaped late" })
                if ($Kill) { Kill-Pid $o.pid }
                continue
            }
        }
        $kidList = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($o.pid)" -ErrorAction SilentlyContinue |
                     ForEach-Object { "$($_.Name):$($_.ProcessId)" })
        $why = "opened by this run and still alive"
        if ($kidList.Count -gt 0) { $why += "; children still attached: " + ($kidList -join ",") }
        [void]$left.Add([pscustomobject]@{ pid = $o.pid; name = $o.name; why = $why })
        if ($Kill) { Kill-Pid $o.pid }
    }
    return $left
}

# kill-server can leave the warm pool spawning one more standby behind us, so
# sweep until nothing carrying this run's namespace is left.
for ($sweep = 0; $sweep -lt 4; $sweep++) {
    Stop-OurPsmux -Ns $RunId
    Stop-OurPsmux -Ns $ColdId
    if ((@(Get-OurPsmuxProcs -Ns $RunId).Count + @(Get-OurPsmuxProcs -Ns $ColdId).Count) -eq 0) { break }
    Start-Sleep -Milliseconds 500
}
foreach ($id in ($script:Started | Sort-Object -Unique)) {
    # identity, not just liveness: a recorded PID that has since been reissued
    # to somebody else's process is not ours to kill
    if ((Get-Process -Id $id -ErrorAction SilentlyContinue) -and (Is-OurProcess $id)) { Kill-Pid $id }
}
foreach ($k in $SavedEnv.Keys) {
    if ($null -eq $SavedEnv[$k]) { Remove-Item "Env:\$k" -ErrorAction SilentlyContinue } else { Set-Item "Env:\$k" -Value $SavedEnv[$k] }
}

Head "CLEANUP AUDIT  (nothing this suite opened may outlive it)"
Stop-RunDirProcesses
Start-Sleep -Seconds 2
# A console host exits when its last client does, and a killed client can take a
# second or two to actually go. Wait for the console hosts we opened to become
# childless before auditing, or a slow exit is reported as a leak: measured
# 2026-09-10, one conhost still had a dying child at the audit and failed T5 on a
# run that had in fact closed everything. A host that still has a child after
# this grace period genuinely is a leak and is still reported.
for ($settle = 0; $settle -lt 12; $settle++) {
    $busy = 0
    foreach ($o in $script:Opened) {
        if ($ConsoleNames -notcontains $o.name) { continue }
        if (-not (Get-Process -Id $o.pid -ErrorAction SilentlyContinue)) { continue }
        if (-not (Is-SameHost $o)) { continue }
        $kids = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($o.pid)" -ErrorAction SilentlyContinue)
        if ($kids.Count -eq 0) { continue }
        $busy++
        # A child of a console host WE opened, which is descended from a process
        # we started, is ours too and is what keeps the host alive. The marker
        # shell is matched by its command line, but a shell that psmux started in
        # the pane carries nothing of ours on its command line at all, so the
        # attribution has to be by ancestry.
        foreach ($k in $kids) {
            $kid = [int]$k.ProcessId
            if ($script:Protected.Contains($kid)) { continue }
            $chain = Get-Chain $kid
            $ours = $false
            foreach ($id in $chain) { if (($script:Started -contains $id) -and (Is-OurProcess $id)) { $ours = $true; break } }
            if (-not $ours -and $k.CommandLine -and $k.CommandLine.Contains($RunDir)) { $ours = $true }
            if ($ours) { Kill-Pid $kid }
        }
    }
    if ($busy -eq 0) { break }
    Start-Sleep -Milliseconds 500
}
$leftovers = Assert-NoLeftovers -Kill
if ($script:Reaped.Count -gt 0) {
    Write-Host ("[INFO] {0} console host(s) had no client left and were reaped here; none of them had a window on screen" -f $script:Reaped.Count) -ForegroundColor DarkGray
    foreach ($rp in $script:Reaped) { Write-Host ("       pid {0} {1}" -f $rp.pid, $rp.name) -ForegroundColor DarkGray }
}
if ($leftovers.Count -eq 0) {
    Write-Host "[PASS] T5 no leftover windows, tabs, shells or servers" -ForegroundColor Green
    [void]$script:Thresholds.Add([pscustomobject]@{ name="T5 no leftover processes"; value=0; limit=0; unit=""; pass=$true; note="" })
} else {
    Write-Host ("[FAIL] T5 {0} leftover process(es); they have been killed, but the suite failed to close them itself" -f $leftovers.Count) -ForegroundColor Red
    foreach ($l in $leftovers) { Write-Host ("       pid {0} {1} : {2}" -f $l.pid, $l.name, $l.why) -ForegroundColor Red }
    $script:Fails++
    [void]$script:Thresholds.Add([pscustomobject]@{ name="T5 no leftover processes"; value=$leftovers.Count; limit=0; unit=""; pass=$false; note=($leftovers | ForEach-Object { "$($_.name):$($_.pid)" }) -join "," })
}
Remove-Item -Recurse -Force $RunDir -ErrorAction SilentlyContinue
$payload = Save-Metrics -Rows $rows -Deltas $deltas
try {
    $payload["leftovers"] = $leftovers
    $payload["console_hosts_reaped_late"] = @($script:Reaped)
    $payload | ConvertTo-Json -Depth 9 | Set-Content -Path $MetricsFile -Encoding UTF8
} catch {}
Info "metrics: $MetricsFile"

Write-Host ""
if ($script:Fails -eq 0) { Write-Host "  ALL THRESHOLDS PASSED" -ForegroundColor Green }
else { Write-Host ("  {0} THRESHOLD(S) FAILED" -f $script:Fails) -ForegroundColor Red }
exit $script:Fails
