# psmux Performance: How Fast Is tmux for Windows?

psmux is a native terminal multiplexer for Windows, and this page is its performance record: what a psmux command costs, how long a new pane takes to show a PowerShell prompt, what the server uses in memory per pane, and where the time actually goes. Every number below was measured on a real machine with the commands shown, so you can reproduce them on yours.

## Key facts

- **A psmux CLI command round trip is about 15 to 25 ms**, including starting the `psmux.exe` client process, reading the registry files, connecting over loopback TCP, and printing the reply.
- **`new-session -d` takes about 50 ms** when the warm pool has a standby server ready, about 215 ms when it has to cold start one.
- **A new window shows a pwsh prompt in about 100 ms** when the server's spare shell is ready, 400 to 550 ms when the shell has to boot from scratch. Bare `pwsh -NoProfile -c exit` takes about 210 ms on the same machine, so psmux is not the bottleneck; the shell is.
- **The server process needs about 30 MB working set (15 MB private) for a session with 11 windows and 16 panes.** Each pane adds three lightweight threads and its scrollback buffer. The shells themselves (about 90 MB per pwsh) dominate memory, exactly as they would outside psmux.
- **Output rendering is event driven.** The server pushes a frame within a few milliseconds of ConPTY output; the client never has to poll for it.
- **Release builds use `opt-level = 3`, full LTO, one codegen unit, and stripped symbols** (`[profile.release]` in `Cargo.toml`).

For how these numbers come about, read [How psmux Multiplexes Natively on Windows](architecture.md). For the warm pool, read [Warm Sessions](warm-sessions.md).

## Reference machine

All measurements on this page were taken on 2026-08-29 with psmux 3.3.8 (commit cbb9c10) installed from `cargo install --path .`:

- Windows 11, build 26200
- PowerShell 7.6.5 as the default shell
- AMD Ryzen AI MAX+ 395, 96 GB RAM
- Timed from a PowerShell script with `System.Diagnostics.Stopwatch` around each `& psmux ...` call, so every figure includes the client process start

Your numbers will differ. Shell startup in particular depends on your profile, PSReadLine, oh-my-posh, and antivirus scanning of `pwsh.exe`.

## How long does a psmux command take?

Ten runs each against a session with one window (`psmux new-session -d -s perf -x 200 -y 50`):

| Command | min | median | max |
|---------|----:|-------:|----:|
| `display-message -p '#{session_name}'` | 16 ms | 18 ms | 26 ms |
| `list-panes` | 15 ms | 18 ms | 23 ms |
| `send-keys 'echo hi' Enter` | 15 ms | 20 ms | 26 ms |
| `capture-pane -p` | 18 ms | 24 ms | 38 ms |
| `split-window -h` (command returns; shell keeps booting) | 25 ms | 36 ms | 70 ms |
| `new-window` (command returns; shell keeps booting) | 31 ms | 58 ms | 74 ms |
| `kill-session` (waits for the process tree to exit) | 250 ms | 255 ms | 283 ms |

What that means for scripts: a loop that sends 100 `send-keys` commands finishes in about two seconds, and most of that is Windows creating 100 `psmux.exe` client processes, not the server. Chain commands with `\;` or use [control mode](control-mode.md) to send many commands over one connection when that matters.

## How long until a new pane is usable?

The command returning is not the same as the prompt being visible. This measures `new-window` until `capture-pane` shows a `PS C:\...>` prompt, polling every 5 ms:

| Run | new-window to visible pwsh prompt |
|-----|----------------------------------:|
| 1 | 562 ms (spare shell not ready, cold pwsh start) |
| 2 | 106 ms (spare shell claimed) |
| 3 | 432 ms |
| 4 | 94 ms |
| 5 | 403 ms |

The two clusters are the warm pane pool at work. Every server keeps one spare shell booted; the first `new-window` or `split-window` after a pause gets it in about 100 ms, and a burst of creates falls back to cold shell starts of 400 to 550 ms while the pool refills. Baseline for comparison, on the same machine:

| Command | min | median | max |
|---------|----:|-------:|----:|
| `pwsh -NoProfile -c exit` (no psmux involved) | 206 ms | 211 ms | 246 ms |

A cold pane costs the shell's own startup plus a couple of hundred milliseconds of PSReadLine and prompt rendering inside a fresh console. psmux's share of that is the ConPTY creation and the first frame, well under 50 ms.

## How long does session creation take?

| Scenario | min | median | max |
|----------|----:|-------:|----:|
| `new-session -d` with the warm pool enabled (default) | 45 ms | 50 ms | 51 ms |
| `new-session -d` with `PSMUX_NO_WARM=1` (cold server) | 203 ms | 216 ms | 243 ms |

The warm path is a rename of the standby `__warm__` server's registry files plus a claim message, which is why it is four times faster than spawning a server, binding the listener, loading the config, and booting the first shell. See [Warm Sessions](warm-sessions.md).

## What does the server use in memory?

After creating 11 windows and splitting the first window into 5 panes (16 panes in total, all pwsh):

| Process | Working set | Private bytes | Threads |
|---------|------------:|--------------:|--------:|
| `psmux.exe server -s perf` | 29.5 MB | 14.4 MB | 55 |
| each `pwsh.exe` pane (average) | about 94 MB | | |

Per pane the server adds three threads (ConPTY reader, VT parser, write queue) and the scrollback grid, which is `history-limit` rows times the pane width. With the default history the server's private memory grows by well under a megabyte per pane. The shells dominate: 16 panes of pwsh is about 1.5 GB of working set, and a tab of the same shell in any other terminal costs the same, because it is the shell's memory, not the terminal's. For a one pane session measured side by side with Windows Terminal, WezTerm and Alacritty on the same machine, see [Measured against the terminals on your machine](#measured-against-the-terminals-on-your-machine) below. Use `cmd`, `nu`, or `pwsh -NoProfile` for panes that only need to run one command (see [Multi-Shell](multi-shell.md)).

## Attached client memory under stalled output

If the terminal stops accepting output (a console selection, a stalled SSH
pipe), the attached client keeps reading from the server but keeps only the
newest frame: a frame that arrives behind a frame still waiting to be drawn
replaces it, the same thing the render loop does with a backlog. Directives
such as detach and session switch keep their order. Memory stays at one frame,
the server keeps counting the client as attached however long the stall lasts,
and the screen is redrawn from current state when the terminal catches up,
which is how tmux treats a tty that cannot keep up. This is separate from pane
scrollback storage.

On Windows, the isolated regression compares responsive and blocked terminal
output, checks private committed memory stays below 16 MiB with 13.7 KB frames,
then resumes output and verifies clean server-directed detach:

```powershell
python tests\test_client_frame_backpressure.py --exe target\release\tmux.exe
$env:PSMUX_TEST_EXE = "$PWD\target\release\psmux.exe"; .\tests\test_issue882_client_output_stall.ps1
```

The Python test uses its own data directory and namespace with a synthetic
loopback server and never starts a pane. The PowerShell test runs a real server
in its own data directory and `-L` namespace, floods a pane, leaves the client's
output unread past the server's 5 second write timeout, and checks memory,
`list-clients`, `destroy-unattached` and a clean detach.

## The extreme scale harness

`tests/test_extreme_perf.ps1` is the repository's stress benchmark. It creates 100 sequential windows, 50 windows in a burst, splits one window until psmux refuses, builds a 20 windows by 5 splits mixed session, and then measures command round trips and `dump-state` serialisation with 100 panes alive. It writes a JSON summary with these fields:

| Field | Meaning |
|-------|---------|
| `baseline_noprofile_ms`, `baseline_profile_ms` | Raw `pwsh` startup with and without the profile, no psmux involved |
| `cold_start_ms` | `new-session -d` on a cold server until its first prompt is visible |
| `seq_prompt_p50`, `seq_prompt_p90`, `seq_prompt_p99` | Prompt ready latency percentiles across 100 sequential `new-window` calls |
| `seq_cmd_avg` | Average `new-window` command return time in that run |
| `burst_total_ms` | Wall time to fire 50 `new-window` commands and see 50 prompts |
| `max_splits` | Splits accepted in one window before "pane too small" (depends on the terminal size you give the session) |
| `mixed_total_ms`, `mixed_mem_mb` | Wall time and server memory for the 100 pane mixed session |
| `rtt_avg_ms`, `rtt_p90_ms` | Command round trip with 100 panes alive |
| `dumpstate_avg_ms` | Server time to serialise a full frame with 100 panes alive |
| `throughput_wps` | `new-window` commands per second over 200 calls |
| `final_mem_mb`, `mem_after_kill_mb` | Server memory at the end and after `kill-session` |

Run it yourself after a release build (it looks for `target\release\psmux.exe`):

```powershell
cargo build --release
pwsh -NoProfile -File tests\test_extreme_perf.ps1
# smaller and faster:
pwsh -NoProfile -File tests\test_extreme_perf.ps1 -SequentialWindows 20 -BurstWindows 10 -SkipPromptCheck
```

A recorded run kept in the repository root (`test_extreme_perf_results.txt`, from an earlier build on a smaller terminal) shows `seq_prompt_p50` of 158 ms, `seq_prompt_p90` of 418 ms, `seq_prompt_p99` of 433 ms, `dumpstate_avg_ms` of 1 ms, and a server `final_mem_mb` of 73 MB with 100 panes alive. The p50 to p90 gap is the same warm versus cold shell split visible in the table above.

## Measured against the terminals on your machine

`tests/test_perf_vs_terminals.ps1` is the benchmark that does not quote anybody's documentation. It launches every terminal emulator installed on the machine, runs the same shell inside each of them, and times psmux beside them. It is part of the test suite and `tests\run_all_tests.ps1` runs it as a performance suite, so `-SkipPerf` skips it.

```powershell
cargo build --release
pwsh -NoProfile -File tests\test_perf_vs_terminals.ps1
# smaller and faster
pwsh -NoProfile -File tests\test_perf_vs_terminals.ps1 -Quick
```

Hosts measured, in this order: bare `pwsh` in its own console, Windows Terminal, WezTerm, Alacritty, psmux attached in its own console, and psmux inside Windows Terminal. A terminal that is not installed prints SKIP and costs nothing. The exit code is the number of thresholds that failed, and every sample lands in `%USERPROFILE%\.psmux-test-data\metrics\perf_vs_terminals-<timestamp>.json` (schema `psmux.perf_vs_terminals.v2`), never in the repository. The JSON is rewritten after every section with `"complete": false` until the run ends, so a run that is interrupted still leaves the data it had and says which sections were asked for.

Point it at a specific binary with `-Binary <path>`; with no argument it uses `PSMUX_TEST_BIN`, then this checkout's `target\release\psmux.exe`, then the psmux on PATH. A renamed copy for an A/B must be called `pmux.exe`, which psmux recognises as one of its own server images. Every psmux cell runs in a per run `-L` socket namespace with its own `PSMUX_DATA_DIR`, so the suite never touches a session you are using.

### How to read the four numbers

**Launch to prompt.** Every cell runs the identical shell, `pwsh -NoLogo -NoProfile -NoExit -File marker.ps1`, and the marker script writes `[Diagnostics.Stopwatch]::GetTimestamp()` and its own PID into a file. QueryPerformanceCounter is system wide, so subtracting the timestamp the suite took immediately before `Start-Process` gives a sub millisecond, host neutral instant with no polling and no screen scraping. It is a shell readiness measure: it stops just before pwsh paints its first prompt, so it excludes each terminal's own first paint. The four GUI hosted cells go through one extra `cmd /c wrapper.cmd` hop, about 15 ms, so the wrapper can pin the child's environment; a `wt -w new` window is spawned by the Windows Terminal monarch process and would otherwise inherit that process's environment. The hop is in all four GUI cells, so GUI to GUI comparisons are exact.

Two psmux launch cells are reported. `psmux_attached` and `psmux_in_wt` cold start a server on every repetition, which is the first psmux window of the day. `psmux_attached_warm` and `psmux_in_wt_warm` run with a psmux server already alive, which is every launch after that, and are the cells the thresholds are judged on. Windows Terminal is usually already running, so its own figure is a window inside an existing process while WezTerm and Alacritty cold start a whole GUI process; `wt_was_running` in the JSON records which case was measured.

The cells are interleaved: repetition 0 of every host, then repetition 1 of every host, and so on. A machine can drift by a factor of two inside a minute, so five repetitions of Windows Terminal followed by five of psmux would be comparing two different machines. Round robin puts every cell in the same weather, which is what makes the psmux minus Windows Terminal delta worth quoting. The cold psmux cells run in a second socket namespace that is torn down after each repetition, which is what lets them be interleaved with the warm cells instead of run in separate blocks.

**Keystroke to screen.** Measured by `tests/keylat.cs` at the PTY level: a key record goes into the target console's input buffer and the same process watches that console's screen buffer for the echo, both timestamped from one QueryPerformanceCounter.

The host rows and the psmux rows are taken at different probe points, and that is the entire explanation for "1 ms versus 18 ms". For Windows Terminal, WezTerm and Alacritty the console being watched is the pane conhost's screen buffer, which is upstream of the pseudoconsole pipe, and which also contains none of the terminal's own GPU paint, another 8 to 16 ms of frame time in each of them. For psmux the console being watched is the psmux client's own console, downstream of that pipe, so the psmux number carries the whole psmux pipeline and the pipe with it.

The pipe is the expensive part and it is not psmux's. PSReadLine paints a keystroke in two console writes; conhost's pseudoconsole serializer emits the first as a 6 byte `ESC[?25l` chunk after about 0.6 ms and then withholds the chunk carrying the character for a further 14.7 ms. The character has been sitting in the pane conhost's screen buffer the whole time. `tests/conpty_echolat.cs` measures that floor at 15.7 to 16.2 ms with no psmux in the path at all, and every ConPTY consumer on Windows pays it, Windows Terminal included. At the same probe point as the host rows, psmux measures 0.55 ms against the 1.03 ms measured for Windows Terminal.

So the psmux rows are judged against that floor rather than against the host rows: the suite runs `conpty_echolat` in the same run on the same machine, prints its median and p99 as the `conpty_floor` row, and asserts how far above it psmux sits. This build sits 0.74 ms above on the median and 1.57 ms above on the p99. A useful corollary for anyone thinking about backends: the data is available 25 times earlier through the console API than through the pseudoconsole pipe.

**Memory and CPU.** Collected in the keystroke cells, because that is the only section that holds one cell alive long enough to watch it. For each cell the suite identifies the processes that make the cell work and samples each one twice, at prompt ready and again after the keystroke run: the psmux server (found from its `<namespace>__<session>.pid` anchor file, so a warm standby or another psmux on the machine is never sampled by mistake), the psmux client, the pwsh being typed into, the terminal emulator above it, and the conhost or OpenConsole that owns the console.

Three numbers come out of that. `srv_ws`, `cli_ws` and `host_ws` are working set in MB at prompt ready, with private bytes in the JSON beside them. `cpu/100k` is CPU time consumed across the keystroke run, normalised to milliseconds of CPU per 100 keystrokes, so cells that ran different key counts stay comparable. `idle%` is CPU over a quiet window with nothing typed, as a percentage of ONE core, and it is the number worth watching: busy polling is invisible in every latency figure on this page and shows up only here. psmux really did once ship 1 ms sleeps that Windows rounded up to a 15.6 ms timer tick, so this column exists to catch the next one.

Idle is measured in two windows, because the psmux client polls adaptively (1 ms while typing, then 5 ms, then 50 ms) and so does ConPTY. The first window opens half a second after the last keystroke and measures the ramp down, not idle; the JSON keeps it as `idle_cpu_pct_of_core_settling` for comparison. The second opens several seconds later and is the steady state figure, the one in the `idle%` column and the one T7 is judged on.

Read `host_ws` with the `shared` column next to it. When the cell is a Windows Terminal tab, the hosting WT process also holds the user's own windows, so its working set is not that cell's cost and `shared` says `yes`. Its CPU delta is only as clean as the rest of that window is quiet: anything happening in another tab of the same Windows Terminal lands in the same counter. The psmux server and client columns have no such caveat, because those processes exist only for the cell being measured.

**Creation latency.** `new-session`, `new-window`, `split-window -v` and `split-window -h`, timed to a visible prompt over one persistent control connection where a `dump-state` round trip costs about 0.15 ms, so detection is effectively continuous instead of the 16 ms a fresh `capture-pane` client costs. These cells use the default pane shell so the warm pane pool is in play, which is exactly why they are bimodal: a claimed spare shell lands near 60 ms and a cold shell start near 500 ms. The suite prints BIMODAL whenever the maximum is more than three times the median rather than hiding the split in an average.

Each measured window is killed again as soon as its sample is taken. That is part of the measurement, not tidiness: every window left standing grows the frame the control connection has to read and rescan on every push, and a run that left twenty windows open reported 9 to 14 SECOND creations that were really the harness reading its own backlog.

### The thresholds

| Threshold | Limit | Why |
|-----------|-------|-----|
| T0 every cell that was not skipped produced data | 0 missing | a pretty summary full of dashes is worse than a failing run, because it looks like a pass |
| T1, T1b psmux attached launch minus bare pwsh, cold and warm server | 350 ms | an absolute delta, not a ratio: bare pwsh itself ranges from 350 to 600 ms here, so the same build scored 1.7x and 2.5x on two runs with its own cost unchanged. Measured 175 to 213 ms; the double shell bug this replaces showed 440 to 530 ms |
| T2 psmux in Windows Terminal over plain Windows Terminal, warm | 300 ms | the steady state launch, the one a user meets all day |
| T2b the same, cold server | 700 ms | the first psmux window of the day also pays a server spawn |
| T3a psmux keystroke median minus the measured ConPTY floor | 2.5 ms | about three times the 0.74 ms this build costs. An absolute 10 ms median could never pass with a shell in the pane, because 15.8 of those 18 ms are conhost's pseudoconsole serializer and no psmux change removes them |
| T3c psmux keystroke p99 minus the floor's p99 | 6 ms | same reasoning, about four times the 1.57 ms measured |
| T3b psmux keystroke p99, absolute | 25 ms | kept, so the total a user waits is still bounded, floor included. If the floor probe fails, T3a falls back to an absolute 30 ms median ceiling |
| T4a first session to a prompt | 1000 ms | a cold server plus a cold default shell |
| T4b `new-window` p90 | 300 ms | includes the 16 to 20 ms Windows needs to start the CLI client |
| T4c `split-window -v` and `-h` p90 | 300 ms | same |
| T5 leftover windows, tabs, shells or servers | 0 | a benchmark that litters the desktop is a benchmark nobody will run |
| T6a psmux server working set, one pane | 60 MB | measured at 14 to 25 MB, so the limit is about 2x the measurement: a leak alarm, not a tuning target |
| T6b psmux client working set | 60 MB | same reasoning; server plus client under 120 MB is the answer to "a multiplexer is heavy" |
| T7 psmux idle CPU, server plus client, settled window | 3 percent of one core | tmux on Unix is about 0 percent idle, and a Windows port that polls is the known failure mode. No latency test can see this. Judged on the settled window, never on the ramp down one, and that window is 8 s rather than 3 s because CPU time advances in 15.6 ms scheduler ticks: at 3 s one tick is 0.52 percent and the gate sat between two steps, so the same binary scored 2.08 and 3.12 ten minutes apart |
| T8 psmux CPU per 100 keystrokes, server plus client | 3000 ms | measured 1016 to 2422 ms on one healthy binary within a few hours, and the cause is frame count, not spinning: two frames per keystroke at a shell prompt, because the cursor hide chunk and the text chunk arrive 15 ms apart. This number moves with machine state by nearly 2x on one binary, and the pane shell's own CPU moves with it, so the suite also reports psmux CPU divided by the shell's CPU in the same cell (1.68 to 1.94 measured), which cancels most of that and is the number worth calibrating against next. The client's console host costs more than either psmux process; reported, not judged |

A number that moves is a regression or a machine change, and the JSON keeps every sample so the two can be told apart by rerunning an old binary in the same time window.

### A recorded run, 2026-09-10

psmux 3.3.8 at 4897b20, the installed binary, one full run on the reference machine with nothing else opening terminals: n=5 per launch cell, 40 keystrokes per latency cell, 5 creations per kind, 457 s. Every cell produced data, nothing was left behind, and every threshold passed.

| Host | launch median | launch p90 | keystroke median | keystroke p99 |
|------|--------------:|-----------:|-----------------:|--------------:|
| bare `pwsh` in its own console | 355 ms | 722 ms | 1.66 ms | 3.10 ms |
| Windows Terminal | 429 ms | 900 ms | 0.89 ms | 2.91 ms |
| WezTerm | 555 ms | 1043 ms | 0.95 ms | 2.75 ms |
| Alacritty | 559 ms | 569 ms | 0.93 ms | 2.82 ms |
| psmux attached, server already running | 551 ms | 558 ms | 16.29 ms | 17.12 ms |
| psmux in Windows Terminal, server already running | 614 ms | 646 ms | 16.28 ms | 16.72 ms |
| psmux attached, cold server | 579 ms | 582 ms | | |
| psmux in Windows Terminal, cold server | 649 ms | 656 ms | | |
| **ConPTY floor, no psmux in the path** | | | **15.91 ms** | **16.17 ms** |

What psmux adds: **196 ms to a launch** with a server already running, 224 ms when it has to spawn one, 185 ms inside a Windows Terminal tab. On the keystroke path it adds **0.4 ms to the median and 0.5 ms to the p99 over the ConPTY floor**, which is the only comparison that means anything here: the 14.6 ms that separates the psmux rows from the host rows is the pseudoconsole pipe, measured in the floor row, and every ConPTY consumer pays it.

Memory and CPU, one session with one window and one pane:

| Process | working set | private | CPU per 100 keystrokes | idle CPU, percent of one core |
|---------|------------:|--------:|-----------------------:|------------------------------:|
| psmux server | 15.6 MB | 4.0 MB | 664 ms | 0.78 |
| psmux client | 8.7 MB | 2.0 MB | 1094 ms | 0.78 |
| the pane's pwsh | 91 MB | 32 MB | 1055 ms | 0.00 |
| conhost hosting the psmux client | 17.9 MB | 2.5 MB | 1758 ms | 0.20 |
| WezTerm | 114 MB | | 1445 ms | 0.20 |
| Alacritty | 108 MB | | 1133 ms | 0.00 |

Server plus client is **24 MB**, against 108 MB for Alacritty and 114 MB for WezTerm hosting the same shell. Idle, server plus client hold **1.56 percent of one core**, ie 0.78 each, which is four scheduler ticks in the 8 s window and agrees exactly with the figure measured independently by the keystroke gate suite. CPU on the typing path is 1758 ms per 100 keystrokes for server plus client, or 1.67 times what the pane's own shell spends; most of it is the two frames per keystroke that a shell prompt produces.

Creation latency: first session to a prompt 570 ms, `new-window` median 62 ms with p90 79 ms, `split-window -v` p90 80 ms, `split-window -h` p90 73 ms, five windows in a burst all prompting in 589 ms.

For comparison, the same suite at d69c310 before the launch and keystroke work landed: psmux added 450 ms to a launch rather than 196, and its keystroke median was 18.94 ms against a floor of about 15.9 rather than 16.29. Two earlier runs of this suite against 4897b20 also recorded T8 at 1445 and 2734 ms per 100 keystrokes where this run recorded 1758, with the pane shell's own CPU moving in step, which is why the conditioned psmux-over-shell ratio is reported beside it.

### What it opens, it closes

T5 is a real threshold because this suite opens GUI windows. Everything it opens is closed by PID as soon as its measurement is captured, and the rule is attribution: a process is killed only when the run can show it started it, either because the suite spawned it, because it carries the run's scratch directory (which contains the suite's own PID) on its command line, or because it is a console host whose parent is one of the run's processes, or the GUI process the cell opened, and whose image is one the suite actually launches. That last clause matters more than it looks: PIDs are recycled within seconds on a busy machine, and without the image check an unrelated process that inherited a freed PID gets its console host blamed on the benchmark.

What T5 counts is what a user would have to close: a window, a tab, a shell, a psmux server. A conhost or OpenConsole that has outlived its client has no window of its own, so it is reaped and listed instead of failing the run. One that still holds a real client does fail it, and the failure line names the client.

The run's own process ancestry and every terminal that existed before it started are protected from every kill, and the suite refuses to start if that baseline looks wrong. It is worth knowing why: an earlier version built the baseline in a loop whose variable `$n` was the same variable as its own `[int]$N` parameter, so the baseline stayed empty, and the final audit then treated every terminal on the machine as one the benchmark had opened and killed it, including the window the benchmark was being run from.

## Where the time goes

- **Shell startup dominates.** pwsh takes 200 to 1000 ms to a prompt depending on the profile; psmux spends tens of milliseconds per pane. The warm pool exists to overlap the two.
- **Client process start is most of a CLI round trip.** The server answers a query in about a millisecond; the other 15 ms is Windows creating `psmux.exe`, loading it, and reading three small files. This is why control mode and `\;` chains are faster for automation.
- **Frame serialisation is about 1 ms.** `dump_layout_json_fast` (`src/layout.rs`) snapshots each pane's cells under its parser mutex for about a millisecond and serialises outside the lock, so rendering never blocks a pane's reader thread.
- **`kill-session` is slow on purpose.** It walks the process tree of every pane, verifies each pid's creation time so a recycled pid is never killed, and waits for the exits, which is where the 250 ms goes.

## How psmux keeps latency low

All of these are in the source today; the file is named so you can check.

| Technique | Effect | Where |
|-----------|--------|-------|
| Server push rendering | A dirty state pushes a frame to attached clients within a few milliseconds instead of waiting for the next client poll | `src/server/mod.rs` |
| Adaptive client polling | 10 ms while typing, 16 ms idle with pushed frames, 1 ms while assembling a paste | `src/client.rs` |
| Reader and parser split | A 64 KB reader thread never takes the parser lock, and the parser coalesces bursts in 1 ms ticks | `src/pane.rs` |
| Per pane write queue | Keystrokes are queued and written by a dedicated thread, so a wedged child cannot stall the server loop | `src/pane.rs` (PR #543) |
| Lazy pane resize | Only the active window's panes are resized; background windows resize when shown, avoiding O(n) `ResizePseudoConsole` calls | `src/tree.rs` |
| Cached shell resolution | The default shell's path is resolved once per server in an `OnceLock` | `src/pane.rs` |
| Early port file write | The server binds its listener and writes `.port` before loading config or spawning shells, so an attaching client connects immediately | `src/server/mod.rs` |
| 10 ms attach polling | The client watches for the `.port` beacon in 10 ms ticks | `src/main.rs` |
| Warm servers and warm panes | Pre booted standby server and spare shell per server | [Warm Sessions](warm-sessions.md) |
| ConPTY passthrough | On Windows 11 22H2+ conhost forwards VT output as written instead of re rendering it | `crates/portable-pty-psmux` |
| Above normal priority | psmux's own server and client processes get `ABOVE_NORMAL_PRIORITY_CLASS` so a compile on every core cannot starve keystrokes (issue #608) | `src/platform.rs` |
| Release profile | `opt-level = 3`, `lto = true`, `codegen-units = 1`, `strip = "symbols"` | `Cargo.toml` |

## Why native multiplexing suits scripted TUIs and terminal agents

Running a dozen terminal agents, a build watcher, a log tail, and a couple of editors from one script is the workload psmux is tuned for:

- **Dozens of panes are cheap on the psmux side.** The server cost is a few threads and a screen buffer per pane; the harness routinely runs 100 panes in one server. What you pay for is the programs in the panes, which you would pay for anyway.
- **Commands are local and cheap.** `send-keys`, `capture-pane`, `wait-for`, and `pipe-pane` are 15 to 25 ms loopback calls. There is no WSL boundary and no shell wrapper between your script and the pane.
- **Detached servers keep running.** A supervisor script can create sessions at boot, hand agents their panes, and attach from any terminal later. See [Windows Use Cases](use-cases.md).
- **Output arrives as it happens.** Push rendering means an attached client sees an agent's output within milliseconds, and `capture-pane` reads the same screen the parser holds.
- **The interactive path is protected.** Above normal priority for psmux's own processes keeps the client responsive while agents saturate the CPU.

The step by step guide is [Running Terminal Agents and TUIs in psmux](tutorials/terminal-agents-and-tuis.md), and the command reference is [Scripting and Automation](scripting.md).

## Measure it yourself

Command round trip and session creation, in PowerShell 7:

```powershell
function Time-Psmux([string[]]$cmd, [int]$n = 10) {
    $t = 1..$n | ForEach-Object {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $null = & psmux @cmd
        $sw.Stop(); $sw.ElapsedMilliseconds
    }
    $s = $t | Sort-Object
    "{0,-40} min={1} med={2} max={3}" -f ($cmd -join ' '), $s[0], $s[[int]($s.Count / 2)], $s[-1]
}

psmux new-session -d -s perf -x 200 -y 50
Time-Psmux @('display-message', '-p', '-t', 'perf', '#{session_name}')
Time-Psmux @('send-keys', '-t', 'perf', 'echo hi', 'Enter')
Time-Psmux @('capture-pane', '-p', '-t', 'perf')
Time-Psmux @('new-window', '-t', 'perf') 5
psmux kill-session -t perf

# Session creation, warm versus cold
Time-Psmux @('new-session', '-d', '-s', 'perf2') 1; psmux kill-session -t perf2
$env:PSMUX_NO_WARM = '1'
Time-Psmux @('new-session', '-d', '-s', 'perf3') 1; psmux kill-session -t perf3
Remove-Item Env:PSMUX_NO_WARM
```

Prompt ready latency for a new window:

```powershell
$sw = [Diagnostics.Stopwatch]::StartNew()
psmux new-window -t perf -n probe
while ($sw.ElapsedMilliseconds -lt 20000) {
    $screen = (psmux capture-pane -p -t perf:probe) -join "`n"
    if ($screen -match 'PS [A-Z]:') { break }
    Start-Sleep -Milliseconds 5
}
"new-window to prompt: $($sw.ElapsedMilliseconds) ms"
```

Server memory:

```powershell
Get-CimInstance Win32_Process -Filter "Name='psmux.exe'" |
    Where-Object CommandLine -match 'server -s perf' |
    ForEach-Object { Get-Process -Id $_.ProcessId } |
    Select-Object Id, @{n='WS_MB';e={[math]::Round($_.WorkingSet64/1MB,1)}},
                      @{n='Private_MB';e={[math]::Round($_.PrivateMemorySize64/1MB,1)}}, Threads
```

If a number looks wrong, the debug and crash logs described in [Diagnostics](diagnostics.md) show where the time went.

## The metrics folder

Every performance suite writes one JSON file per run into `%USERPROFILE%\.psmux-test-data\metrics\`, never into the repository. Nothing is ever overwritten, so the folder is a history: a suspected regression is compared against the run that last passed instead of against a number in a comment.

Each file carries the same envelope, written by `tests/perf_metrics_common.ps1`:

| field | what it is |
| --- | --- |
| `schema` | envelope version, so a reader can tell old files apart |
| `suite` | the suite that wrote the file |
| `timestamp` | round trip format, local time with offset |
| `binary` | full path of the psmux that was measured |
| `git_sha` | short HEAD of the tree that binary was built in, found by walking up from the binary itself, or `installed` when it is a `cargo install` copy that sits in no work tree |
| `version` | the binary's own `psmux -V` line |
| `machine`, `os`, `cpu`, `cpu_count`, `ram_gb` | where it was measured |
| `load` | what else the machine was doing: `n`, `min_pct`, `p50_pct`, `max_pct` and every sample, each one a reading of `\Processor(_Total)\% Processor Time` with the process count beside it |

`git_sha` is taken from the binary's own directory and not from the current checkout, which matters in the one case that matters: comparing a fresh build against the installed one. A run of the installed psmux is honestly labelled `installed` rather than tagged with whatever `HEAD` the shell happened to be sitting on.

These are the files that carry numbers:

| file | what is in it |
| --- | --- |
| `launch-to-prompt-<stamp>.json` | psmux against a bare pwsh, per iteration samples plus p50 / p90 / p99 for both arms |
| `keystroke-latency-<stamp>.json` | the echo cell, pooled percentiles and every sample, plus the memory and CPU block |
| `keystroke-latency-pwsh-<stamp>.json` | the shell cell and the ConPTY floor measured in the same run |
| `creation_latency_gate-<stamp>.json` | thirteen cells: `new-window`, `split-window -v` and `-h` at ten samples each, the `-f`, `-b`, `-bh`, `-bv` and with a command variants at five, `new-session` with and without the warm pool, and `kill-pane`, `kill-window` and `kill-session`, each with samples and percentiles, plus the memory and CPU block |
| `pane_startup_perf-<stamp>.json` | first session, warm pool depth and burst creation |
| `idle-socket-traffic-<stamp>.json` | what an idle attached pair puts on the socket, in lines per second, with the CPU beside it |
| `perf_vs_terminals-<stamp>.json` | the head to head against Windows Terminal, WezTerm and Alacritty, with every threshold and its verdict |

## The gates, and what fails a sweep

Five suites are gates rather than benchmarks: they assert, and a sweep goes red when one of them does not hold. Every one of them runs in its own `-L` socket namespace and tears down only that namespace, so none of them can disturb a session you are using.

| suite | what it asserts | budget | writes |
| --- | --- | --- | --- |
| `test_launch_to_prompt_gate` | psmux launch to a visible prompt divided by bare pwsh launch to a visible prompt | `<= 2.0x` (hard) | `launch-to-prompt-*` |
| | psmux median minus bare pwsh median | `<= 400 ms`, hard on a quiet machine and a warning otherwise | |
| `test_keystroke_latency_gate` | echo cell, pooled median over every keystroke of every run | `< 3 ms` | `keystroke-latency-*` |
| | echo cell, pooled p99 | `< 8 ms` | |
| | shell cell, median above the ConPTY floor measured in the same run | `< 2.5 ms` | `keystroke-latency-pwsh-*` |
| | shell cell, p99 above that floor | `< 6 ms` | |
| | shell cell, absolute median ceiling | `< 30 ms` | |
| `test_creation_latency_gate` | p50 per cell for `new-window` and both splits | `<= 150 ms` (hard) | `creation_latency_gate-*` |
| | p50 for the `-f`, `-b`, `-bh` and `-bv` variants | `<= 200 ms` | |
| | p50 for a split that carries its own command | `<= 2000 ms` | |
| | p50 for `new-session` with the warm pool, and with `PSMUX_NO_WARM=1` | `<= 2000 ms` and `<= 3000 ms` | |
| | p50 for `kill-pane`, `kill-window`, `kill-session` | `<= 400 ms`, `<= 400 ms`, `<= 3000 ms` | |
| | how many of ten creations exceed 150 ms, plus p90 and max | `<= 2 of 10`, `<= 300 ms`, `<= 1500 ms`, hard on a quiet machine and warnings otherwise | |
| `test_idle_socket_traffic` | lines per second an idle attached pair puts on the socket | see the suite | `idle-socket-traffic-*` |
| `test_perf_vs_terminals` | T1 psmux launch over bare pwsh, T2 psmux in Windows Terminal over plain Windows Terminal, both load aware | `350 ms`, `300 ms` | `perf_vs_terminals-*` |
| | T3 keystroke over the ConPTY floor measured in the same run, and the absolute p99 | `2.5 ms` and `25 ms` | |
| | T4 first session to prompt, and creation p90 | `1000 ms` and `300 ms` | |
| | T6 server and client working set, T7 idle CPU, T8 CPU per 100 keystrokes | `60 MB` each, `3% of one core`, `3000 ms` | |
| | T5 no leftover processes | `0` | |

**Why some budgets are ratios and some are load aware.** Both were measured, not guessed. Against the same installed binary on the same machine, once quiet and once with five other build jobs running:

| | quiet | loaded | |
| --- | ---: | ---: | --- |
| bare pwsh launch | 393 ms | 892 ms | the machine |
| psmux launch | 625 ms | 1374 ms | the machine |
| **delta** | **232 ms** | **482 ms** | doubles with load, so it cannot be a gate on its own |
| **ratio** | **1.59x** | **1.54x** | moves 3 percent, so it can |
| `new-window` p50 | 25 ms | 29 ms | the warm pool's own path |
| `new-window` p90 | 110 ms | 416 ms | a creation that waits out a cold shell waits out whatever a cold shell costs |

So the hard assertions are the ones that do not move with the machine: a ratio for launch, a p50 for creation, a keystroke p99, and everything measured against a floor taken in the same run. The rest are still asserted, but every suite samples `\Processor(_Total)\% Processor Time` around its cells, and when the machine was above 25 percent of total CPU those assertions are recorded as warnings in the run output and in the JSON (`tail_warnings`, or `soft` on a threshold row) instead of failing the sweep. The load itself is in the envelope, so a warning can always be checked against what the machine was doing.

Which assertions are load aware, and why each one is or is not:

| assertion | load aware | because |
| --- | --- | --- |
| launch ratio | no, hard always | both arms stretch together, so the ratio does not move |
| launch absolute delta, and `test_perf_vs_terminals` T1, T1b, T2, T2b | yes | a difference between two timings stretches with the machine |
| creation p50 per cell | no, hard always | it is the warm pool's own path and it moved 4 ms between a quiet box and one at 60 percent |
| creation slow count, p90, max | yes | a creation that waits out a cold shell waits out whatever a cold shell costs today |
| keystroke echo p50 | yes | under load a healthy median lands on top of the defect's median and stops discriminating |
| keystroke echo p99 | no, hard always | under the same load it was still half the defect's |
| keystroke over the ConPTY floor | no, hard always | the floor is measured in the same run and moves with the machine too |
| memory, idle CPU, CPU per keystroke, leftover processes | no, hard always | not timings |

A CPU counter sampled once per section is not on its own enough to tell a trustworthy run from an untrustworthy one. The head to head run of 2026-09-22 failed T1 and T2 while its own load samples read a median of 6.5 percent of total CPU: the machine was not steadily busy, it was stalling individual launches, and its first interleaved round measured every host at two to three times the rest of the run. So `test_perf_vs_terminals` also checks the spread of the reference arms it subtracts from, `p90 / median` on `bare_pwsh` and `wt_pwsh`, which is measured inside the very same interleaved window as the timings it judges:

| | bare pwsh median | bare pwsh p90 | spread |
| --- | ---: | ---: | ---: |
| a trustworthy run | 434 ms | 477 ms | 1.10x |
| the run that failed T1 and T2 | 428 ms | 736 ms | 1.72x |

Above 1.35x the baseline is declared unstable, the difference thresholds report as warnings, and `ref_spread` and `ref_unstable` go into the JSON beside `load`.

Memory and CPU are recorded by every gate and gated in one place. The keystroke gate samples the server and the attached client at the prompt and again after a burst of keystrokes, and reports working set, private bytes, CPU as ms per 100 keys, and CPU over a quiet window with nothing typed. The creation gate does the same at one pane and again after twenty windows and three splits, which is what shows a per pane poll: a number that grew with the pane count while the one pane sample looked fine. The launch gate samples the last iteration at its prompt. Idle CPU is a percentage of one core and its resolution is one scheduler tick, 15.6 ms, about 0.5 percent over a three second window. The thresholds on all of these live in `test_perf_vs_terminals` (T6, T7, T8), so there is only one place to argue with; the other gates assert only that the section produced data, because a JSON full of nulls that still says PASS is worse than a failure.

## Reading the trend

`tests/perf_summary.ps1` reads the metrics folder and prints it back:

```powershell
pwsh -NoProfile -File tests\perf_summary.ps1              # last 8 runs of every metric
pwsh -NoProfile -File tests\perf_summary.ps1 -Last 20     # a longer window
pwsh -NoProfile -File tests\perf_summary.ps1 -Metric B    # one section: A, B, C, D or T
pwsh -NoProfile -File tests\perf_summary.ps1 -Metric T    # the trend table on its own
pwsh -NoProfile -File tests\perf_summary.ps1 -Csv perf.csv
```

Sections **A** to **D** are the four questions run by run: **A** launch to a usable prompt, psmux against a bare pwsh and against every terminal a head to head run found; **B** keystroke to screen at p50, p90 and p99, with the shell cell judged against the ConPTY floor measured in the same run; **C** creation latency at p50 and p90 for every cell; **D** working set, private bytes and CPU for the server and the client.

Section **T** is the trend. One row per headline number, every one of them a number where lower is better, showing the minimum, the median and the maximum over the window, the newest value, and the median of the five runs before it:

```
  metric                             unit    runs       min    median       max    newest   prev5med  flag
  launch psmux / bare pwsh           x          8     1.533     1.589     1.621     1.533      1.611  ok -5%
  keystroke echo p50                 ms         8      1.64       1.7      1.83      1.71       1.69  ok +1%
  creation new-window p50            ms         8        15        25        36        29         25  ok +20%
  new-session warm p50               ms         6        69        82        96        82         83  ok -1%
  new-session no-warm p50            ms         6       780       802       940       802        810  ok -1%
  server working set at prompt       MB         8      15.6      15.7      15.8      15.7       15.7  ok 0%
```

A row is flagged **REGRESSED** when the newest run is worse than the median of the previous five by more than 20 percent, which `-RegressPct` changes. The comparison is against a median of five rather than against last time on purpose: any single previous run can be the one that ran while something else was linking, and a rule that fires on that stops being read, while a real regression is present in every run after it lands and so moves the median with it. Twenty percent is above this machine's run to run spread on every metric in the table.

`-FailOnRegression` makes the script exit non zero when anything is flagged, for a pipeline that wants the trend itself to be a gate. By default it only prints, because the gates are the thing that fails a sweep.

Before believing a flag, open the file it came from and look at `load`. A tail that grew on a machine at 70 percent is the afternoon; a p50 that grew on a quiet machine is the build.

All of these suites are part of `tests\run_all_tests.ps1`, which runs every `tests\test_*.ps1` file, so a default sweep runs all of them and nothing has to be opted into. `-SkipPerf` skips the ones listed as performance suites. They take the build in `target\release` by default, ahead of the psmux on `PATH`; `-Binary <path>` or `PSMUX_TEST_BINARY` points them somewhere else, which is how two builds are compared against each other.

## FAQ

### Is psmux faster than tmux inside WSL?

For Windows shells, yes by construction: a pwsh pane in psmux is a direct ConPTY child, while reaching pwsh from tmux in WSL means `wsl.exe` to `pwsh.exe` interop on every pane and every script call. For Linux shells inside WSL the two are comparable; psmux runs `wsl.exe` in a pane and the Linux side is unchanged.

### Why does the first split after a pause feel instant and a burst of splits does not?

The spare shell. Each server keeps one shell booted for the next `split-window` or `new-window`; a burst uses it up and the rest cold start while the pool refills. Windows created in a loop still return in about 50 ms each; only the prompt takes longer to appear.

### Does psmux add input latency?

A keystroke goes client to server over loopback (sub millisecond), into the pane's write queue, into ConPTY, and the echo comes back through the reader, parser, and a pushed frame. End to end this is a few milliseconds, below the 16 ms frame time of the terminal you are typing into.

### How many panes can one server hold?

The harness runs 100 panes in one session as a routine test. The practical limit is memory for the shells and your patience with `list-panes`, not psmux.

### What should I change to make it faster?

1. Trim your PowerShell profile or use `pwsh -NoProfile` for utility panes; the shell is the slow part.
2. Leave the warm pool on (default).
3. Prefer `\;` chains or control mode over hundreds of separate `psmux` invocations in tight loops.
4. Keep `history-limit` reasonable if you run hundreds of panes; scrollback is the only per pane memory psmux itself allocates.
