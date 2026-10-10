# Warm Sessions

psmux uses a background **warm session** (`__warm__`) to make new session creation nearly instant. This page explains how it works and how to interact with it if needed.

## What is a Warm Session?

When you create a session, psmux pre-spawns a hidden standby server called `__warm__`. This server loads your config, initializes a shell, and waits. When you run `psmux new-session` next time, psmux **claims** this warm server (renames it to your requested session name) instead of cold-starting a new process. This skips the entire server startup + config load + shell spawn cycle.

**Result:** New session creation drops from ~400-1000ms (shell startup) to near-instant.

## Why You Don't See It

The `__warm__` session is an internal implementation detail. It is hidden from:

- `psmux ls` / `psmux list-sessions`
- `prefix + s` (choose-session)
- `prefix + w` (choose-tree)
- `prefix + (` / `)` (session navigation)
- The `last_session` tracking file

Users should never need to interact with it directly.

## When It's Not Spawned

The warm server is **not** created when:

- The current session has `destroy-unattached on`, and keeping a hidden warm server alive would break the expectation that sessions die when you detach
- The current session **is** the warm session (no recursive warm spawning)
- Warm panes are explicitly disabled (see below)

## Pool Depth (`warm-pool-size`)

Inside a running server the spare shells live in a small pool. How deep that
pool is decides whether a *run* of creations is fast or only the first one is.

A pool of depth one is enough for the first `new-window` and useless for the
second: claiming the only spare triggers a refill, and the refill is a shell
that started milliseconds ago, so the next creation waits out its entire
startup. Opening five windows in a row that way alternates fast, slow, fast,
slow. Depth is the cure, because the spare handed to creation N+1 has then had
the whole of creation N to finish booting.

```
# default: keep three spare shells ready
set -g warm-pool-size 3

# the old default, one idle shell less
set -g warm-pool-size 2

# a machine with memory to spare and bursty window creation
set -g warm-pool-size 5

# no pool at all (same as set -g warm off)
set -g warm-pool-size 0
```

The value is clamped to 8. Each spare is a real shell process, so the cost is
linear: measured on Windows 11 with pwsh, one unit of depth costs one shell
plus its console host and about 100 MB of working set, and an idle server plus its spares burns under
0.1% of a 32 core machine. `PSMUX_WARM_POOL_SIZE` sets the boot time value for a
single run without touching the config.

### Why the default is three

The default was two until 2026-10-08. Two serves a creation now and then, and
it stalls the moment somebody opens a few windows in quick succession after
launching psmux. Measured with `tests/probe_pool_depth_cold_launch.ps1` on the
same build, depth set by `PSMUX_WARM_POOL_SIZE`, six runs per depth: a cold
`new-session`, 2 s for the user to see the prompt, then five `new-window` 300 ms
apart, each timed until its prompt is visible (p50 / max in ms):

| depth | cold launch | 1st | 2nd | 3rd       | 4th       | 5th | idle tree         |
|-------|-------------|-----|-----|-----------|-----------|-----|-------------------|
| 2     | 833 / 932   | 3/7 | 3/4 | 173 / 535 | 791 / 918 | 3/4 | 559 MB, 14 procs  |
| 3     | 834 / 850   | 3/3 | 3/3 | 3 / 4     | 3 / 3     | 3/3 | 662 MB, 16 procs  |
| 4     | 847 / 858   | 3/4 | 3/3 | 3 / 3     | 3 / 3     | 3/3 | 764 MB, 18 procs  |

The idle tree is the server, the standby server and every descendant (shells
and their console hosts) 14 s after launch. With a full second between
creations even depth two keeps up (every creation 3 to 7 ms), and with no gap
at all no depth is enough for every creation (see the bursts below). Three is
the least memory that removes the stall at a human pace, and cold launch did
not move. The standby server still waits for two ready spares, not three, before
it spawns after the boot hold (`STANDBY_OWED_READY` in `src/server/mod.rs`), so
the default behaves exactly like `PSMUX_WARM_POOL_SIZE=3` did on the old build.

Refills run on a background thread, so a burst of creations never waits on a
`CreateProcess` and the server loop is never stalled by one.

### A spare only counts once its shell has started

A spare becomes a pool member about 25 ms after it is asked for, which is just
the `CreateProcess` and the ConPTY allocation. Its pwsh needs roughly another
400 ms to put a prompt on the screen. Handing one out in between is
indistinguishable from a cold spawn: the window opens and then sits blank for
the rest of the shell's startup.

So the pool tracks readiness, inferred from the spare's own output: a spare
counts as started once it has written something and then stayed quiet for
250 ms. A claim prefers a started spare and takes the oldest one, since the
oldest is the furthest through its startup. When nothing has started yet the
claim still takes the oldest warming spare, because a shell part way through
booting beats a cold spawn that is not started at all.

A `default-shell` that never writes anything would otherwise never be handed
out, so a spare older than 1500 ms counts as started regardless.

Measured on 2026-10-04, the quiet rule fires earlier than its name suggests.
Of 1009 promotions traced over nine full runs of the perf suite and twenty runs
of its depth five section, none needed the backstop, but 872 happened while
the spare's screen was still blank: the first output (`dv=1` or `dv=2`) is
the pseudoconsole's own setup sequence, and a Store pwsh then stays silent for
more than 250 ms while it loads, under load or when several start together.
Such a spare is counted as ready 300 to 800 ms after spawn without a prompt
yet. It matters only to creations that follow each other within about a
second; a spare that has idled for seconds, as in the depth five contract,
always has its prompt.

### Bursts (surge)

Depth on its own cannot serve a *run* of creations. Ten windows opened back to
back arrive roughly every 20 ms, and no fixed depth survives that, because each
claim is replaced by one shell that needs 400 ms.

When a claim finds nothing started **and** another claim happened within the
last 1.5 s, the pool treats that as a run and temporarily grows to four times
`warm-pool-size`, capped at 8. Those spawns go out concurrently, so the run pays
one shell startup between all of them instead of one each. Measured over ten
back to back `new-window` calls, that is nine creations at 15 to 30 ms and one
at about 500 ms, against every second one costing 400 to 600 ms before.

Concurrently, and genuinely so since
[#686](https://github.com/psmux/psmux/issues/686). Every ConPTY spawn has to
park the process's std handle slots for the length of its `CreateProcessW`, and
that used to be done under an exclusive lock, so a surge of eight ran one spawn
at a time: 45 ms, then 90, then 135, up to 504 ms for the last one, and a claim
arriving between landings stalled 450 to 550 ms. Spawns now share that lock,
because they all want the same parked state; only a console identity change
(`FreeConsole`/`AttachConsole`, used for Ctrl+C delivery and input injection)
still takes it exclusively, and it is blocked by, and blocks, every spawn in
flight.

At most four spare shells are inside `CreateProcessW` at once. That cap is a
measurement, not caution: three concurrent spawns cost about 64 ms each on the
reference machine, five cost 99 ms and eight cost 130 ms and up, and once a
spawn costs more than the claim's wait budget every claim cold spawns, takes the
next pane id, and thereby retires the whole batch still in flight. A claim now
waits up to 250 ms for a spare that is already being spawned before cold
spawning, which covers the slowest spawn at that concurrency. Over the ten call
burst the two together took the p50 from 57 ms to about 25 ms and removed the
450 to 550 ms outliers entirely.

### The third quick creation at depth two

Five `new-window` calls in a row, each waiting for its prompt (TEST 2 of
`tests/test_pane_startup_perf.ps1`), come out like `[51, 34, 1118, 39, 34]` at
the default depth of two: the first two take settled spares, the third pays
0.9 to 1.2 s against a bare `pwsh -NoProfile` of about 335 ms. The trace says
why. The second call takes the last ready spare, which opens a surge, and the
pool schedules seven spawns at once (two waves of four `CreateProcessW`). The
third call is handed the one spare that was already starting, and that shell
boots alongside both waves.

Other shells starting nearby slow a shell's startup, and it is not the CPU.
Measured with no psmux (a C# probe starting the Store pwsh with `-NoProfile`
and timing it by its own exit time, 32 logical CPUs mostly idle, seven runs
each): alone 306 ms; four more started 0 to 50 ms after it 473 to 494; 100 to
200 ms after it about 410; 300 ms after it 306. Four started 300 ms before it
cost 384, 400 ms or more before it nothing. Eight cost about the same as four
(484). Starting the others at idle priority changed nothing. So the cost is
contention in the first 300 to 400 ms of each process's life, and the only lever
is when the other shells start.

That lever was tried and rejected, because it only moves the cost. Capping how
many spares may start while the shell a caller waits on is still starting
(three, five in a burst) took the third window to 450 to 726 ms over ten
interleaved runs, but the pool then had fewer started spares when the next
creations came: the four splits that follow the window run (TEST 3) went from a
worst of 33 to 80 ms on master to 160 to 653 ms, and once the depth five
contract failed (p90 202 ms). Protecting the waiting shell for a fixed time
instead (400, 700 or 1000 ms after it started) either lost the gain (1167, 892,
900 ms) or kept the split cost (779, 806, 414 ms). One wave of eight
`CreateProcessW` instead of two waves of four did not help either (1075,
1748 ms). The total amount of shell starting is fixed by how many creations
follow, and at depth two one of them pays for it; `warm-pool-size 5` is the
setting for a user who opens windows in runs, and its contract (every creation
fast, p90 150 ms) holds. The experiment is kept on the branch
`perf-refill-budget-experiment` with all of its numbers.

One side effect visible in the same traces was also tried on its own and
dropped. A claim that waits for in flight spares takes the first one to land,
while package activation returns a batch of concurrent spawns within a couple
of milliseconds of each other in no id order; the claim's id raises the pool's
floor, so the lower ids landing a millisecond later are killed on arrival
("refused spare pane=5 below floor 6") and spawned again. Landing the whole
batch before choosing (5 ms) is correct and unit testable, but over three
interleaved runs of `tests/test_issue686_pool_surge_and_reap.ps1` it changed
nothing measurable: burst mean 247, 246, 154 ms against 97, 136, 187 on
master, refusals 16, 48, 8 against 13, 11, 32. An earlier set of three had
shown the opposite, which is the size of the noise on this machine.

### A claimed spare keeps the directory it started in

A spare's shell can not be moved from outside once it is running, so a creation
that asks for a directory (`new-window -c <dir>`, and also a `new-window` or
`split-window` without `-c`, which asks for the calling client's working
directory) used to have the claim type ` cd '<dir>'; ...; cls` into the
transplanted shell. The shell has to read that line, run it and draw a fresh
prompt before the window shows anything again, so every warm creation carried a
whole shell round trip, and that round trip is exactly what slows down when the
machine is busy, for example while the claim's own refills are booting.

That is what made the depth five contract in `tests/test_pane_startup_perf.ps1`
(TEST 3b) flaky. Its five spares are all four seconds old, and the trace showed
the prompt on every one of them at claim time (100 of 100 claims over twenty
runs). Yet under load the fifth creation came out at 126 to 178 ms in six runs
of ten, against 50 to 80 ms for the first four, because its shell was answering
the injected `cd` alongside four booting refills. In some runs the first
`capture-pane` after `new-window` found the pane completely blank: the `cls`
had landed and the new prompt had not.

Spares are spawned in the server's working directory. A creation from a client
working in that same directory (the suite's case, and any script or shell that
started the server from where it runs) is handed a shell that is already where
a cold spawn would have put it, whose profile has already run there exactly as
it would after a cold spawn. The claim now records the directory each spare was
started in and only rehomes a spare that was started somewhere else. Over two
sets of ten runs under the same load the fifth creation is 55 to 107 ms, and
no creation of the hundred exceeds 107 ms. A creation asking for any other directory
still rehomes, as before.

### Nothing boots beside the first shell

A fresh server starts the session's own shell and then has background shells
to start: the two pool spares and the warm standby server, which brings its own
shell and spare. They used to go out the moment the loop started, so a cold
`new-session` booted four more pwsh processes while the user's shell was still
booting.

pwsh 7 does not tolerate that. With no psmux involved at all, one pwsh launched
together with four others reached its first line of script about 200 ms later
than alone (median boot 440 ms alone, 640 ms with four neighbours), and its own
CPU time was the same either way (about 420 ms), so it was waiting, not
computing. It is not the Store activation (launching the package's `pwsh.exe`
directly behaves the same) and not the startup profile file (decoys with their
own `LOCALAPPDATA` behave the same). Four Windows PowerShell 5.1 neighbours
cost the same shell only about 30 ms. The machine has 32 logical processors.

That is the whole of the old bimodal cold launch. Traced with
`PSMUX_STARTUP_TRACE`, `PSMUX_SPAWN_TRACE` and `PSMUX_WARM_TRACE` over 24 cold
reps, every step up to `srv.child.spawned` (about 327 ms, including the 185 ms
first `CreateProcessW` of the Store pwsh) was identical in the fast and the slow
mode; the 110 to 125 ms difference lay entirely between `CreateProcessW`
returning and the shell's first script line. Turning the pool off, or the
standby off, each removed the slow mode; turning both off removed the cost
entirely.

So the server now holds the pool refill and the standby spawn until the first
pane's shell has started, by the same test a spare's readiness uses (it has
written and then been quiet for 250 ms, or 1.5 s have passed). That first
version held only the idle refill tick and cost the first window dearly; the
next section has what the hold does now. Measured over 20 interleaved cold
reps, `new-session` with a pwsh
command, launch to the shell's first script line:

| build  | fast mode           | slow mode           |
|--------|---------------------|---------------------|
| before | 739 to 759 ms (12)  | 854 to 921 ms (8)   |
| after  | 663 to 709 ms (20)  | none                |

With the default shell and the user's profile, launch to the prompt visible in
`capture-pane` went from 989 to 1035 ms to 734 to 833 ms (14 interleaved reps
each). The standby and the spares arrive about 600 ms later than before; a
`new-window` three seconds after launch still claims a ready spare.

### The first window after launch

That first version moved the cost instead of removing it. A `new-window`
right after the prompt (what agent team tooling does: it creates panes the
moment `new-session` returns) arrived during the hold, found the pool empty
and cold spawned. And the trace showed it worse than one shell: the server's
own first window takes the early spare through the claim path, so the user's
first `new-window` counted as the second claim of a burst and surged eight
spares beside its cold spawn, plus the standby when the hold let go. Test 2
of `tests/test_pane_startup_perf.ps1` went from a first window of 37 to 283
ms to 1692 to 2066 ms.

What one companion costs, measured the same way as above: one pwsh 7 beside
the measured one adds ~70 ms, two add ~110 ms, four ~200 ms. A neighbour that
starts later costs less: two started 150, 300 and 450 ms after the measured
shell added ~90, ~50 and ~0 ms. Releasing the hold earlier than the prompt
(at a fixed 150 or 250 ms into the boot) made the first window 200 to 490 ms
but put 60 to 110 ms back on every launch, so it was not taken.

What the hold does now:

* It watches every pane that exists while it is on, not only the first, so a
  window or split created during the hold cold spawns alone and the hold
  waits for that shell too. Nothing is refilled during the hold, from the
  tick or from a claim.
* A shell counts as started at its first visible text (the prompt, or
  whatever its profile prints first; ConPTY's own startup sequences draw
  nothing), so the release no longer waits 250 ms of quiet after the prompt.
  Written then quiet for 250 ms still counts, for a shell that draws nothing,
  and 1.5 s after the last pane joined the hold lets go regardless.
* On release the pool trickles: one spare booting at a time until one is
  ready or somebody claims. The standby is spawned last, once the pool has
  its spares or 2 s after the release; only the next `new-session` needs it.
* The server's own first window no longer counts as a claim for the surge.

Interleaved, test 2's sequence (detached session, prompt, then five
`new-window` back to back, 15 ms prompt polling), first window over 8 reps:

| build                    | first window         | the whole vector, one rep      |
|--------------------------|----------------------|--------------------------------|
| before the hold          | 37 to 283 ms         | 39, 32, 1374, 78, 36           |
| first hold               | 1692 to 2066 ms      | 2020, 232, 40, 35, 34          |
| this                     | 611 to 701 ms        | 613, 79, 25, 1351, 37          |

The first window is now one shell booting alone, about what a bare pwsh with
the profile costs here. The 1.2 to 1.7 s creation later in the sequence is the
surge (two quick claims that both missed): it was the third creation before
the hold and is the fourth now, and the suite's budget of two slow creations
covers it. Cold launch keeps its single mode: 682 to 724 ms over 20 reps
against 666 to 706 for the first hold and 744 to 938 (8 fast, 12 slow) before
it; a second interleaved set put the first hold at 695 to 717 and this at 675
to 718. `tests/test_perf_vs_terminals.ps1` T4b, whose first window is one
shell booting alone right after the prompt (350 to 456 ms), came out 281.7,
220.1, 280.8 and 247.3 ms against its 300 ms limit; with five samples the p90
is 0.6 of that one creation, so the margin is that shell's boot.

### Creations right after launch: two trickle variants tried and dropped

A deeper pool does nothing for a `new-window` issued the moment pane one shows
its prompt: the pool is still empty behind the boot hold, the claim takes the
one trickled spare, and its refill starts the whole `warm-pool-size` beside it
(at depth 8 that first window took 1.6 to 1.7 s; at depth 3 five windows each
issued the moment the last showed its prompt came out p50 [788, 91, 2, 2,
1648] ms with the fifth up to 2 s). Two ways of keeping the trickle on past
that claim were measured on 2026-10-08, interleaved, depth 3, eight runs:

* Keep the trickle after a claim that found nothing ready (a ready claim still
  ends it). The first window went from about 790 to 510 ms, but the stall moved
  to the third: [528, 90, 1622, 2, 3] and up to 2.2 s.
* Keep the trickle while creations keep coming (one shell booting at a time
  until claims stop for 1.5 s). Back to back windows became [500, 95, 500,
  100, 505] with nothing over 546 ms, and eight issued at once right after the
  prompt came up in 1.65 to 1.73 s instead of 3.5 to 4.5 s. It failed
  `tests/test_pane_startup_perf.ps1` three runs of three (new-window median
  over creations two to five 345 ms against 300; the splits after it
  [34, 527, 282, 531] against a budget of one slow), which passed three of
  three without it, and `tests/test_issue661_warm_pool_depth_holds.ps1` once in
  three, because the pool never fills while creations keep coming.

Both only move the shell starting around, as the experiments in "The third
quick creation at depth two" found: the total is fixed by how many creations
follow, and a run issued faster than a shell boots pays for it somewhere. The
pace a person creates windows at (see "Why the default is three") never meets
the trickle at all.

### Teardown

A spare that is still being spawned belongs to nobody: the shell exists, its
conhost exists, and the server has not seen it yet. `kill-server` used to kill
the windows and the pooled spares only, so anything in flight was orphaned,
parented to a dead psmux and idle at a prompt forever (six of them over ten
rounds of "new-session, six new-window, kill-server").

Every spawn is therefore tracked from the moment it is issued and gains its pid
as soon as `CreateProcessW` returns one. A teardown closes that registry, kills
the pids in it through the same creation time validated kill guard the panes
use, and a spawn that finishes afterwards is told on arrival to kill the child
it just created. The shutdown also kills its children *first*, before the client
notifications and their sleeps, and the server no longer acknowledges a
`kill-server` before it has actually gone: the caller treats the closed socket
as "the server is dead" and force-kills the pid shortly after, which used to
cut the shutdown off before it killed anything at all.

One isolated slow creation does not surge: a cold `new-session` misses by
definition, since its only spare was born moments earlier, and surging there
fired eight shell spawns alongside the session's own starting shell and cost
about 100 ms of startup. The extra spares are released 5 s after the last miss,
so the idle footprint stays at `warm-pool-size`.

A `__warm__` standby is held at one spare and never surges: it creates no
windows of its own, and it may sit around for days.

### Tracing

To see what the pool is doing, set `PSMUX_WARM_TRACE=1` before starting the
server. Every claim, refill, landing and readiness flip is appended to
`%TEMP%\psmux_warm_trace.log` (override with `PSMUX_WARM_TRACE_FILE`) with a
millisecond timeline, including the age of the spare each claim received, which
is the number that explains a slow open.

A spawn line carries its own breakdown, which is how a serialised surge is told
apart from a slow machine:

```
pool: spawned spare pane=6 pid=Some(28164) in 99.9ms (pty 6.2ms, proc 93.2ms, console wait 0.0ms, in CreateProcessW 92.4ms)
```

`pty` is the ConPTY allocation, `proc` is the whole process spawn, `console
wait` is how long that spawn waited to get into the console state, and `in
CreateProcessW` is the part of `proc` spent inside the operating system's
`CreateProcessW`. A non zero console wait means something is holding it
exclusively. Spares that started together and differ only in `in
CreateProcessW` were held up by Windows, not by psmux: the Store packaged pwsh
is created through package activation, which returns concurrent creations in
batches.

A claim line describes the spare it took and every spare left behind as
`id:age:state:data_version:quiet`, state being `Q` (started, its output went
quiet), `B` (counted as started only by the 1500 ms backstop) or `W` (still
starting), followed by the last line on the taken spare's screen. A `READY`
line says which rule promoted the spare (`via=quiet` or `via=BACKSTOP`) and
what its screen showed at that moment. `rehome:` lines show every claim that
typed a `cd` into its shell.

`PSMUX_SPAWN_TRACE=1` goes one level deeper and times every step of each
spawn (job object, attribute list, console state, each `CreateProcessW`
attempt, job assignment, resume) into `%TEMP%\psmux_spawn_trace.log`
(override with `PSMUX_SPAWN_TRACE_FILE`).

## Disabling Warm Sessions

If you prefer every session, window, and pane to start with a completely fresh shell invocation (no pre-spawned state), you can disable warm entirely.

### Via config file

Add this to your `.psmux.conf`, `.tmux.conf`, or `~/.config/psmux/psmux.conf`:

```
set -g warm off
```

### Via environment variable

```powershell
$env:PSMUX_NO_WARM = "1"
```

When warm is disabled:
- No `__warm__` background server is spawned
- No warm panes are pre-spawned inside sessions
- Every `new-session`, `new-window`, and `split-window` cold-starts a fresh shell
- Startup latency increases slightly (shell profile load is not parallelized)

You can re-enable warm at runtime with `set -g warm on`.

## What a Claim Carries Over

A warm server and a warm pane are spawned ahead of time, so they know nothing about the client
that later claims them. psmux carries the parts that matter across the claim:

- **Start directory.** `new-session -c`, `new-window -c` and `split-window -c` cannot set the
  working directory of a shell that is already running, so psmux types a `cd` line into the warm
  shell and clears the screen. The line uses the syntax of the shell in the pane, chosen from the
  effective `default-shell`: PowerShell, cmd.exe and the POSIX shells each get their own form.
  Before [#600](https://github.com/psmux/psmux/issues/600) every Windows pane got the PowerShell
  form, which Git Bash and cmd.exe rejected. See
  [multi-shell.md](multi-shell.md#start-directories-and-warm-panes) for the exact lines.
- **Process priority.** The claiming client's `PSMUX_PRIORITY` (or its config file `priority`
  line) is applied to the claimed server, so `show-options -g priority` and the real scheduling
  class agree whether the session was cold started or claimed
  ([#608](https://github.com/psmux/psmux/issues/608)). See
  [configuration.md](configuration.md#process-priority).
- **Config.** The claimed server reloads your config file on the claim, so a `set -g` line you
  added since the standby was spawned is honoured.
- **Environment.** The claimed server adopts the claiming client's environment, which is what a
  cold started server inherits anyway, so `run-shell` children, hooks, plugin scripts and every
  pane spawned afterwards see the environment of the shell you ran psmux in. Before
  [#659](https://github.com/psmux/psmux/issues/659) a standby kept the environment of whatever
  spawned it for life, and a standby born without the psmux directory on `PATH` (a shell that
  predates the install, psmux reached through WSL interop) made every plugin fail with "the term
  'psmux' is not recognized". Two variables stay the server's own: `PSMUX_TARGET_SESSION`, which
  is its identity, and `PSMUX_DATA_DIR`, which names the directory its registry files already
  live in.

What does not carry over: `-e VAR=value` on `new-session`, `new-window` or `split-window` cannot
reach a shell that already has its environment, so a spawn with `-e` skips the warm pool and starts
cold. For the same reason the standby's first shell, the one you land in, keeps the environment it
was born with: a running process's environment block cannot be edited from outside. The one
exception is `PATH`: when your shell's `PATH` differs from the standby's, the claim respawns that
first shell (it is one nobody has typed into yet) so a dev build or portable tool you put first on
`PATH` is found there too ([#773](https://github.com/psmux/psmux/issues/773)). That respawn makes
the claim cost about what a cold start does; a claim from a shell with the same `PATH` as the
standby, the common case, stays instant. Any other variable you exported in your own shell reaches
the panes you open next, not that first one. tmux behaves the same way, for the same reason.

Every pane inherits the environment of the server, which is the environment of the shell you
started psmux from, like tmux. The registry environment (`HKLM\...\Session Manager\Environment`
and `HKCU\Environment`) only fills variables that environment does not have at all; before
[#773](https://github.com/psmux/psmux/issues/773) it replaced them, so your shell's `PATH`, `TEMP`,
`GOPATH` and the like were swapped for the registry values in every pane.

## One Warm Server per Registry

The warm server belongs to the registry that spawned it. Each `-L <name>` namespace keeps its own
(`<name>____warm__`), and each `PSMUX_DATA_DIR` keeps its own as well: the single server guard that
stops two servers from publishing the same session name is keyed by the resolved data root, so two
registries can each hold a `__warm__` without refusing one another
([#599](https://github.com/psmux/psmux/issues/599)).

## Accessing the Warm Session (Advanced)

If you need to inspect or manage the warm session directly (debugging, development):

```powershell
# Check if a warm session is running
Test-Path "$HOME\.psmux\__warm__.port"

# List all sessions including warm (raw port files)
Get-ChildItem "$HOME\.psmux\*.port" | Select-Object Name

# Send a command to the warm server
psmux -t __warm__ list-windows

# Kill just the warm session
psmux -t __warm__ kill-session

# With -L namespace: warm session is stored as "<namespace>____warm__"
Test-Path "$HOME\.psmux\myns____warm__.port"
```

## File Layout

| File | Purpose |
|------|---------|
| `~\.psmux\__warm__.port` | TCP port of the warm server |
| `~\.psmux\__warm__.key` | Auth key for the warm server |
| `~\.psmux\<ns>____warm__.port` | Warm server under `-L <ns>` namespace |
