# Issue #668 investigation report

Worktree: `C:\Users\godwin\Documents\workspace\psmux\.claude\worktrees\agent-a1ab7d1d317daecad`
Base: master `37e2990`. Binary under test: `psmux 3.3.8 (37e2990 2026-09-16)`, built with
`cargo build --release` in the worktree and put first on PATH for every runner launch
(the runner log confirms
`psmux: C:\Users\godwin\Documents\workspace\psmux\.claude\worktrees\agent-a1ab7d1d317daecad\target\release\psmux.exe`).

**Outcome: NOT root caused. The failure did not reproduce today.** What follows is the
narrowing: four candidate mechanisms falsified with measurements, one positive finding
recovered from the 2026-09-16 logs, and a diagnostic left in the tree so the next
occurrence records the one reading that is still missing.

## 1. Did it reproduce today?

No. The unmodified suite scored 30/30 four times in a row under the sanctioned runner.

| run dir | conditions | Test E3 | total |
|---|---|---|---|
| `2026-09-18_00-56-04` | `-Only test_issue274_tui_wedge_repro` | `[PASS] send-keys works after 90s sustained output (echo visible in 42ms)` | 30 / 0 |
| `2026-09-18_00-59-04` | same | `[PASS] send-keys works after 90s sustained output (echo visible in 29ms)` | 30 / 0 |
| `2026-09-18_01-02-13` | same | `[PASS] send-keys works after 90s sustained output (echo visible in 321ms)` | 30 / 0 |
| `2026-09-18_01-11-05` | same, machine saturated (see 3.4) | `[PASS] send-keys works after 90s sustained output (echo visible in 478ms)` | 30 / 0 |

For comparison, the 2026-09-16 failures this issue is about, quoted from
`2026-09-16_13-00-04`:

```
[Test E3] send-keys after 90s sustained output
  [FAIL] send-keys FAILED after 90s sustained output (no echo within 15187ms)
  [DIAG] test274_tui:2 pane_pid=61836 current_command=node children=node.exe:63352
```

The passing Ctrl+C traces recorded today are byte for byte the trace the issue quotes for
the failing case, which confirms the report's central claim that the trace does not
discriminate:

```
[platform] ctrl_c: called: pid=60608 reattach=false had_console=false
[platform] ctrl_c: console process list n=3 members=[25020=psmux.exe ... | 53456=node.exe ppid=60608 ... | 60608=pwsh.exe ppid=25020 ...]
[platform] ctrl_c: console mode=0x01F7 PROCESSED_INPUT=true fg_is_shell=false
[platform] ctrl_c: GenerateConsoleCtrlEvent => ok=1 err=183
```

## 2. Bisect table

`tests\test_issue668_bisect.ps1` replays the #274 phases cumulatively (phase set from
`PSMUX_I668_PHASE`, flood rate from `PSMUX_I668_RATE`, because the runner discovers suites
by glob and cannot pass parameters) and then interrupts a flooding child in window 2 with
a 30 s deadline. M1 interrupts the child that phase E has been flooding for 90 s, M2 a
fresh child in the same pane, M3 a child in a window created after all phases.

| phase set | flood interval | run dir | M1 | M2 | M3 |
|---|---|---|---|---|---|
| A | 5 ms (10x the suite) | `2026-09-18_01-06-37` | killed in 11 ms | killed in 0 ms | killed in 0 ms |
| A+B+C+D+E | 5 ms (10x the suite) | `2026-09-18_01-07-40` | killed in 3 ms | killed in 0 ms | killed in 0 ms |

No phase flips the result. The full sequence at ten times the output rate of the real
suite interrupts its child as fast as a bare three window session does.

## 3. What was falsified, with the measurement

### 3.1 The child inherited "ignore Ctrl+C"

This was the leading structural candidate: `send_ctrl_c_event` sets
`SetConsoleCtrlHandler(NULL, TRUE)` on the server for the duration of the broadcast, the
server itself is spawned with `CREATE_NEW_PROCESS_GROUP` (`src/platform.rs:187`), and that
state is inherited by children and readable through no Win32 API. A pane child born with
the bit set would produce exactly the reported symptom: a correct broadcast, `ok=1`, and a
child that ignores it.

`tests\ctrlflags.cs` reads `RTL_USER_PROCESS_PARAMETERS.ConsoleFlags` out of the target's
PEB. Measured on every pane shell and every flooding child in every run today, including
live during a 90 s flood:

```
[DIAG] before 60608 ConsoleFlags=0x00000006 ignoreCtrlC=False ConsoleHandle=0x6C   (pane pwsh)
[DIAG] before 53456 ConsoleFlags=0x00000000 ignoreCtrlC=False ConsoleHandle=0x60   (flooding node)
```

Bit 0 is clear everywhere. Ctrl+C is enabled for these children; they are not deaf by
inheritance. The server itself reads `ConsoleFlags=0x00000000` as well
(`14792 ConsoleFlags=0x00000000 ignoreCtrlC=False`, sampled live mid flood), which is the
documented behaviour: `CREATE_NEW_PROCESS_GROUP` does not disable Ctrl+C when it is paired
with `CREATE_NEW_CONSOLE`, and `spawn_server_hidden` pairs them. So nothing downstream of
the server can inherit the bit either. (Not yet measured in the failing state, see
section 5.)

### 3.2 The pane's output backlog stalls the child or the console

The 2026-09-16 diagnostics show the pane's line counter at ~1800 when a live child would
have been near 2200, ie the pane was running roughly 20 s behind. The hypothesis was that
a child stuck in a console write cannot be handed a Ctrl+C.

Falsified. At a 5 ms flood interval the backlog reached more than three times that and the
interrupt still landed instantly:

```
[DIAG] M1 before: node age=97.5s screen_tick=6240 expected~19500 lag=66.3s
[DIAG] M1 before node pid=53456 age=97.6s cpu=1.34s mainthread state=Wait wait=EventPairLow
  [PASS] M1 C-c killed the flooding child in 3ms (send-keys rc=0)
```

A 66.3 s backlog, with the child's main thread sitting in the console LPC wait
(`EventPairLow`) at the moment of the interrupt, is not enough to lose the signal.

### 3.3 The suite's phase sequence leaves the session in a bad state

Falsified by the table in section 2: A and A+B+C+D+E are indistinguishable, and a window
created after every phase (M3) behaves like one created before any of them.

### 3.4 The machine was busy

24 CPU spinners on 32 logical processors, started before the run and killed by PID
afterwards. The load is visible in the suite's own numbers (CLI probe average 23 ms on an
idle machine, 460 ms here) and the interrupt still landed:

```
  [INFO]   +31s: CLI avg=454.8ms samples=32 failedCLI=0 failedTCP=0
  [PASS] send-keys works after 90s sustained output (echo visible in 478ms)
```

### 3.5 A reboot separates the failing window from the passing one

It does not. `LastBootUpTime = 09/15/2026 07:01:17` and the System log shows no restart
since. The eight consecutive failures of 2026-09-16 and today's passes are the same boot
of the same machine.

### 3.6 It is an artifact of running the suite alone

It is not. `2026-09-16_02-02-11` ran with `Only=''` (the full 722 suite sweep) and #274
failed there too, with the same two assertions.

## 4. The one positive finding

In the failing runs the child was not wedged; it never received the signal. The suite
captures the pane twice, once at E3 and once at E4 about twenty seconds later, and the
flood script numbers every line it prints, so those two captures are the child's own
clock. From `2026-09-16_13-00-04`:

```
[Test E3] ...
  [DIAG] |   function example_1800() { return 1800; }
[Test E4] ...
  [DIAG] | [/] Processing task 2145...
```

Same pane, same child (`children=node.exe:63352` in both), counter rising by 345 lines
between the two. The child kept producing output at its normal rate for at least twenty
seconds after a `CTRL_C_EVENT` that the console server accepted (`ok=1`) on a console
whose member list contained it. `[Test E5] Keystrokes work in final fresh attach` passed in
the same run, so the server and a fresh client were healthy throughout: the only thing
lost was that one pane child's interrupt.

That narrows the remaining hypothesis space to two: the console host did not dispatch the
event to that client, or the client had a handler that swallowed it. Section 3.1 rules out
the inherited form of the second, but only for runs that passed.

## 5. What is still missing, and what was left in the tree to catch it

The decisive reading is `ConsoleFlags` for the pane shell and the child *at the moment of
a failing interrupt*, together with whether the child's line counter is still rising. Both
are now recorded automatically:

- `ba3ed2b` extends `Show-PaneDiagnostics` in `tests\test_issue274_tui_wedge_repro.ps1`, so
  the next time E3 or E4 fails the log carries `[DIAG] ctrlflags <pid> ConsoleFlags=...
  ignoreCtrlC=...` for the pane shell and every descendant, plus
  `[DIAG] ... screen line counter=N`.
- `adff668` adds `tests\ctrlflags.cs` (the PEB reader) and `tests\test_issue668_bisect.ps1`
  (the phase and rate harness), which can re-run the bisect in minutes if the state
  returns.

Neither changes product code, so nothing here can mask or alter the failure.

## 6. Commits

| sha | what |
|---|---|
| `adff668` | `test(#668)`: bisect harness plus the `ConsoleFlags` PEB reader |
| `ba3ed2b` | `test(#274)`: record the ignore-Ctrl+C bit and the child's line counter on failure |
| `bdd8a5c` | `test(#668)`: default the harness to phase A so the runner's glob can afford it |

No product source was modified; `src/` is untouched on this branch.

## 7. Regression suites

Run under the sanctioned runner with the worktree binary, `-Only "issue274|issue346|issue579"`
(the alternation has to be quoted inside a `.cmd`, a bare `|` is a pipe to cmd and the run
silently starts nothing). Run dir `2026-09-18_01-18-15`:

| suite | result | passed | failed |
|---|---|---|---|
| `test_issue274_pane_isolation` | PASS | 14 | 0 |
| `test_issue274_sustained_load` | PASS | 8 | 0 |
| `test_issue274_tui_wedge_repro` | PASS | 30 | 0 |
| `test_issue274_wezterm_repro` | PASS | 21 | 0 |
| `test_issue346_ctrlc_ping` | PASS | 10 | 0 |
| `test_issue579_ctrlc_console_bridge` | FAIL | 2 | 1 |

The #579 failure is the known injector flake, not a product result. Arm 2's assertion is
`$after -ge $before -and $cap -match '\^C'`; the WSL half held (`fg WSL arm failed (2 -> 2)`,
so the session survived as required) and it was the injected `^c` that never rendered in the
pane. Re-run alone in `2026-09-18_01-29-12`:
`[PASS] fg WSL survived, ^C forwarded (4 -> 6)`, suite 3 / 0. Nothing on this branch touches
product code, so no result here can be attributed to it.

## 8. Leftovers

Verified after the last run: zero processes running from this worktree's binary
(`ExecutablePath -like "*agent-a1ab7d1d317daecad*"` returns nothing), zero leftover flood
children (no `node.exe` whose command line mentions `psmux_274` or `psmux_668`), no leftover
runner console window. Every runner launcher exited by itself under `PSMUX_RUN_NOPAUSE=1`.
The 24 load generators of section 3.4 were spawned by this agent and stopped by PID; none
survive. The six `psmux.exe` processes alive at the end all run from other agents'
worktrees (`agent-a24aef6d4805f4e93`, `agent-ad04093410a9c6fca`, `agent-ad99a977a1772f253`)
and were left alone.

One caveat worth passing on: the sanctioned runner kills every `psmux.exe` by image name
between suites, machine wide. Seven runner launches during this investigation will have
killed other agents' psmux sessions each time they started a suite.
