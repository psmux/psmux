# Issue #597 follow up (gabri-ns, 2026-09-16): the XTVERSION round trip

Worktree: `C:\Users\godwin\Documents\workspace\psmux\.claude\worktrees\agent-a24aef6d4805f4e93`
Branch: `worktree-agent-a24aef6d4805f4e93`, base `37e2990`.
Every E2E ran `target\release\psmux.exe` built in this worktree. No `cargo install`.

## 1. Byte level reproduction

Probe: `tests/query_probe_child.cs` (new). It enables VT processing on its console
output, writes each query with `WriteFile` on the raw handle, and records every byte
that arrives on stdin in the following 900 to 1200 ms.

Environments: (a) a pane of a detached session under `-L i597`, (b) a plain conhost
window opened by PID, (c) a Windows Terminal window (`wt -w new`). No attached client
was needed, because the detached result already reproduces.

| query | bytes sent | psmux pane BEFORE | psmux pane AFTER | plain conhost | Windows Terminal |
|---|---|---|---|---|---|
| XTVERSION | `1b 5b 3e 30 71` (`ESC [ > 0 q`) | **0 bytes** | `1b 50 3e 7c 74 6d 75 78 20 33 2e 33 2e 38 1b 5c` = `ESC P > \| tmux 3.3.8 ESC \` | **0 bytes** | **0 bytes** |
| XTVERSION no param | `1b 5b 3e 71` | **0 bytes** | same 16 byte reply | 0 bytes | 0 bytes |
| `CSI > 1 q` (control) | `1b 5b 3e 31 71` | 0 bytes | 0 bytes | 0 bytes | 0 bytes |
| DA1 | `ESC [ c` | `ESC [ ? 61 ; 6 ; 7 ; 21 ; 22 ; 23 ; 24 ; 28 ; 32 ; 42 c` | unchanged | `ESC [ ? 61 ; 1 ; 6 ; 7 ; 21 ; 22 ; 23 ; 24 ; 28 ; 32 ; 42 ; 52 c` | `ESC [ ? 61 ; 4 ; 6 ; 7 ; 14 ; 21 ; 22 ; 23 ; 24 ; 28 ; 32 ; 42 ; 52 c` |
| DA2 | `ESC [ > c` | `ESC [ > 0 ; 10 ; 1 c` | unchanged | `ESC [ > 0 ; 10 ; 1 c` | `ESC [ > 0 ; 10 ; 1 c` |
| DSR CPR | `ESC [ 6 n` | `ESC [ 1 ; 1 R` | unchanged | `ESC [ 1 ; 1 R` | `ESC [ 1 ; 1 R` |
| DECRQM 2026 | `ESC [ ? 2026 $ p` | `ESC [ ? 2026 ; 0 $ y` | unchanged | `ESC [ ? 2026 ; 0 $ y` | `ESC [ ? 2026 ; 2 $ y` |
| DECRQM 1006 | `ESC [ ? 1006 $ p` | `ESC [ ? 1006 ; 2 $ y` | unchanged | `ESC [ ? 1006 ; 2 $ y` | `ESC [ ? 1006 ; 2 $ y` |
| XTGETTCAP `TN` | `ESC P + q 544e ESC \` | 0 bytes | 0 bytes | 0 bytes | 0 bytes |

Two corrections to assumptions in the brief:

* Windows Terminal does **not** answer XTVERSION to a program running inside it.
  Measured 0 bytes, same as psmux before the fix. The DA1/DA2/DSR/DECRQM answers a
  program gets in WT come from the openconsole VT adapter in front of it, not from WT.
* DECRQM under psmux is **already answered**, by ConPTY, with `;0` for 2026 ("mode not
  recognised"). That is very likely what Claude Code logs as "no reply to DECRQM 2026".

## 2. Where the bytes died

`PSMUX_PANE_RAW=1` (docs/diagnostics.md) capture of the probe run, the complete
pre-parser pane stream, 221 bytes:

```
1b 5b 3f 39 30 30 31 68  ESC[?9001h   (conhost, because the probe set VT input mode)
1b 5b 3f 31 30 30 34 68  ESC[?1004h
1b 5b 3e 30 71           ESC[>0q      <-- XTVERSION, arrives verbatim
1b 5b 3f 32 35 6c ...    ESC[?25l ESC[2J ESC[m ESC[H  OSC 0 title ... ESC[?25h
1b 5b 3e 71              ESC[>q       <-- XTVERSION without parameter, also verbatim
1b 5b 3f 39 30 30 31 6c  ESC[?9001l
1b 5b 3f 31 30 30 34 6c  ESC[?1004l
```

The DA1, DA2, DSR, DECRQM and XTGETTCAP bytes are **not in that stream at all**.

Causal chain, one sentence: the ConPTY host answers DA1, DA2, DSR and DECRQM itself and
consumes them, but it does not know XTVERSION, so `CSI > q` passes through to psmux's
pane reader, where nothing recognised it and it was dropped by the vt100 parser (the
crate has no `>`/`q` arm at all), leaving the asking program to time out.

## 3. tmux parity facts

Source read: `C:\Users\godwin\Documents\workspace\tmux\input.c`, tree at `AC_INIT([tmux], next-3.8)`.

| what | line | text |
|---|---|---|
| XTVERSION table entry | 343 | `{ 'q', ">", INPUT_CSI_XDA }` |
| XTVERSION handler | 1884-1888 | `n = input_get(ictx, 0, 0, 0); if (n == 0) input_reply(ictx, 1, "\033P>\|tmux %s\033\\", getversion());` |
| DA1 table entry | 326 | `{ 'c', "",  INPUT_CSI_DA }` |
| DA1 reply | 1587 / 1589 | `\033[?1;2;4c` with sixel, else `\033[?1;2c` |
| DA2 table entry | 327 | `{ 'c', ">", INPUT_CSI_DA_TWO }` |
| DA2 reply | 1602 | `\033[>84;0;0c` |
| DSR | 1727 / 1730 | `\033[0n` for Ps 5, `\033[%u;%uR` for Ps 6 |
| DECRQM (private) table entry | 341 | `{ 'p', "?$", INPUT_CSI_QUERY_PRIVATE }` |
| DECRQM handler | 1650-1719 | replies `\033[?%d;%d$y`, with arms for 1, 3, 6, 7, 12, 25, 47/1047/1049, 1000, 1002, 1003, 1004, 1005, 1006, 2004, **2026** (line 1709), 2031 |
| DECRQM (ANSI) table entry | 340 | `{ 'p', "$", INPUT_CSI_QUERY }`, reply `\033[%d;%d$y` |

The brief said tmux has no DECRQM handler. That is not true of this tree: tmux next-3.8
answers DECRQM including 2026. psmux still cannot, because the query never reaches it
(ConPTY consumes it), and ConPTY already answers it. Nothing was changed there.

DA1 and DA2 both reach a pane child already, answered by ConPTY, so neither needed a
psmux handler; adding one would have produced a second, conflicting reply.

## 4. Identity chosen

`tmux`, giving `ESC P > | tmux 3.3.8 ESC \` (version from `psmux -V`, whose first line is
`tmux 3.3.8`).

Measured reason: a live pane's environment contains `TMUX=/tmp/psmux-42444/i597,59305,0`
and `TMUX_PANE=%1`, and `TERM=xterm-256color`. psmux sets no `TERM_PROGRAM` at all. The
identity a pane already sees is tmux, so the XTVERSION answer spells the same thing,
exactly as tmux does. `PSMUX_XTVERSION_NAME` overrides the name (documented in
docs/configuration.md).

Both spellings were measured against Claude Code (section 5); they made no observable
difference, so parity decided it.

## 5. Claude Code 2.1.274, measured

Pane of an `-L i597` session, `PSMUX_PANE_RAW=1`, `claude` started in a scratch folder,
left to reach its prompt, then the session killed. Sequence timeline taken from the raw
capture (offsets in bytes).

Unfixed binary, folder already trusted (3914 bytes of startup traffic):

```
ESC[<u ESC[>5u ESC[>4;2m ESC[>0q ... ESC[?1049h ... ESC[?1000h ESC[?1002h ESC[?1003h ESC[?1006h
```

Fixed binary, same conditions (3912 bytes):

```
ESC[<u ESC[>5u ESC[>4;2m ESC[>0q ... ESC[?1049h ... ESC[?1000h ESC[?1002h ESC[?1003h ESC[?1006h
```

Honest result: **no behavioural difference on this Claude Code version.**

* XTVERSION asks: 2 before, 2 after.
* Mouse: `ESC[?1000h ESC[?1002h ESC[?1003h ESC[?1006h` are emitted **in both cases**, so
  the reporter's "Claude never sends ?100xh" symptom does **not** reproduce with
  2.1.274 here. Whatever gates his build's conservative fallback, it is not only the
  XTVERSION reply.
* `ESC[?9001h` in the raw stream is **not** Claude asking for win32 input mode: the plain
  C# probe produces it too, because conhost emits it when a child turns on
  `ENABLE_VIRTUAL_TERMINAL_INPUT`.
* Time to prompt, 2 runs each: unfixed avg 2547 ms (2534, 2560), fixed `tmux` avg 2857 ms
  (2548, 3166), fixed `psmux` avg 3232 ms (2896, 3567). Noise dominated, no signal.
* No stray reply text ever appeared on Claude's screen, and typing still echoes
  (`send-keys "hello from psmux"` shows up in its prompt).

So the fix closes a real, measured protocol gap and matches tmux, but it did not change
what Claude Code 2.1.274 does in this environment. The wheel and win32-input follow ups
in the brief were conditional on Claude starting to enable mouse after the fix; it
already did so before, so nothing changed there to verify, and the wheel path stays
covered by the #598/#613 suites below.

## 6. The fix

`src/pane.rs`

* `xtversion_reply()` builds `ESC P > | <name> <version> ESC \`, name from
  `PSMUX_XTVERSION_NAME` or `tmux`.
* `scan_xtversion_query_from(data, min_end)` finds `ESC [ > <digits and ;> q` and accepts
  it only when the first parameter is absent or 0, mirroring tmux's `input_get(ictx, 0,
  0, 0)`. `min_end` lets the boundary rescan ignore a query it already reported.
* `XtversionScanner` carries 15 bytes between reads so a query split across two reads is
  still answered once and only once.
* The reader thread answers it inline, next to the issue #556 colour query answer, with
  `platform::mouse_inject::send_vt_response` (WriteConsoleInput into the child's
  console). Not the PTY writer: the ConPTY runs with `PSEUDOCONSOLE_WIN32_INPUT_MODE`,
  where a raw VT response wedges the Win32 input parser (issue #313, which is why
  `conpty_preemptive_dsr_response` is a no-op today).

A unit test caught a real double reply while writing it: with `KEEP = 15` a complete
5 byte query stays inside the carried tail, so the next batch re-reported it. That is
what `min_end` fixes.

## 7. Commits

| sha | subject |
|---|---|
| `67dfe3a` | `fix(#597): answer the XTVERSION round trip inside a pane` |
| `b5743eb` | `docs(#597): document PSMUX_XTVERSION_NAME` |

Files: `src/pane.rs`, `tests-rs/test_issue597_xtversion_reply.rs` (new),
`tests/query_probe_child.cs` (new), `tests/test_issue597_xtversion_reply.ps1` (new),
`docs/configuration.md`.

## 8. Proof both ways

`tests/test_issue597_xtversion_reply.ps1`, same machine, same probe:

| binary | result |
|---|---|
| unfixed (master `37e2990`) | **5 passed, 4 failed** — `XTVERSION got 0 bytes`, reply bytes `[]` against expected `[1b 50 3e 7c 74 6d 75 78 20 33 2e 33 2e 38 1b 5c]` |
| fixed | **9 passed, 0 failed** — `XTVERSION got 16 bytes`, byte exact, `ESC[>q` answered too, `ESC[>1q` still silent, DA1/DA2/DSR/DECRQM unchanged |

Unit tests: `cargo test --bin psmux issue597_xtversion` 17 passed, 0 failed.

## 9. Regression suites

| suite | result |
|---|---|
| `cargo test -p vt100-psmux` | 3 passed + 1 doc test, 0 failed |
| `cargo test --bin psmux issue597` | 35 passed, 0 failed |
| `cargo test --bin psmux dsr` | 1 passed, 0 failed |
| `cargo test --bin psmux cpr` | 15 passed, 0 failed |
| `cargo test --no-run` | builds clean, exit 0 |
| `tests/test_issue646_wezterm_osc_leak.ps1` (DA1 settle window) | 3 passed, 0 failed |
| `tests/test_issue556_yazi_probe_race.ps1` (the colour query path this change sits next to) | 4 passed, 0 failed, OSC 11 still answered in 19.2 ms |
| `tests/test_nested_client_color_leak.ps1` | 12 passed, 0 failed |
| `tests/test_issue597_legacy_mouse_bypass.ps1` | 10 passed, 0 failed |
| `tests/test_issue597_mouse_keepalive.ps1` | fixed: 12/1 then 10/1; unfixed baseline: 12/1 |
| `tests/test_issue613_wheel_gate_durability.ps1` | fixed: 21/2; unfixed baseline: 21/1 |

The last two are the WriteConsoleInput mouse injection suites. They fail on the
**unfixed** baseline too, with a different assertion on every run ("mouse-reporting child
did not start", "the htop-like model app never started", "can't find pane: %3"), which is
the known agent shell flakiness for foreground window injection. Nothing in the failing
set is specific to the fixed binary.

## 10. Leftovers

Zero. After `kill-server` on `i597`, `i597xtv`, `i597ab` and `test_i597`:
no process runs out of this worktree's path, no `query_probe_child.exe` survives, no
`cmd /c pause` window is open, and the two `WindowsTerminal.exe` PIDs (5548, 14924) are
the same ones that existed before the session started. The conhost window and the
`wt -w new` window opened for the baseline measurements closed themselves when the probe
exited. Nothing was killed by name; every teardown went through psmux `kill-server`.

Note for the next agent: a detached psmux server started from the PowerShell tool shell
dies when that tool call returns, so every E2E has to live inside one call.
