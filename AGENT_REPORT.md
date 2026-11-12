# Issue #669 — pane-border-status top shifts mouse release and drag one row down

Worktree: `C:\Users\godwin\Documents\workspace\psmux\.claude\worktrees\agent-ad04093410a9c6fca`
Base: master 37e2990. Branch head: e00aed5.

## 1. Reproduction (before reading any psmux code)

Both routes were reproduced against a binary built from 37e2990 (`psmux 3.3.8 (c7e352b, dirty)`,
kept at `scratchpad/i669_base/psmux.exe`). Ground truth is `tests/mouse_echo_child.cs`, which asks
for SGR mouse reporting and logs its own raw stdin.

### Route 1: raw control-port verbs, detached 80x24 session, no client at all

```
===== pane-border-status = off =====
  sent 'mouse-down 10 5'  -> RECV <ESC>[<0;11;6M
  sent 'mouse-up 10 5'    -> RECV <ESC>[<0;11;6m
  sent 'mouse-drag 10 6'  -> RECV <ESC>[<32;11;7M
===== pane-border-status = top =====
  sent 'mouse-down 10 5'  -> RECV <ESC>[<0;11;6M     (should be ;5)
  sent 'mouse-up 10 5'    -> RECV <ESC>[<0;11;6m     (should be ;5)
  sent 'mouse-drag 10 6'  -> RECV <ESC>[<32;11;7M    (should be ;6)
===== pane-border-status = bottom =====
  sent 'mouse-down 10 5'  -> RECV <ESC>[<0;11;6M
  sent 'mouse-up 10 5'    -> RECV <ESC>[<0;11;6m
  sent 'mouse-drag 10 6'  -> RECV <ESC>[<32;11;7M
```

Every raw verb reports the same row whatever `pane-border-status` says, while the pane's content
height drops from 24 to 23 the moment a label is asked for. `pane-mouse 1 0 10 5 M` (the verb the
client sends for a press) was correctly passed through untouched in all three cases.

### Route 2: attached client, real MOUSE_EVENT records (what the user actually feels)

An attached client launched through a `.cmd` that scrubs the nesting variables, with
`tests/click_injector.cs` (press and release at ONE cell) and `tests/mouse_drag_hold_injector.cs`
(press, motion one row down, release) writing real console records:

```
===== attached client, pane-border-status=off, screen row 5 =====
  click(10,5)          -> <ESC>[<0;11;6M   <ESC>[<0;11;6m
  drag(10,5)->(10,6)   -> <ESC>[<0;11;6M   <ESC>[<32;11;6M  <ESC>[<32;11;7M  <ESC>[<0;11;7m
===== attached client, pane-border-status=top, screen row 5 =====
  click(10,5)          -> <ESC>[<0;11;5M   <ESC>[<0;11;6m        <-- release one row low
  drag(10,5)->(10,6)   -> <ESC>[<0;11;5M   <ESC>[<32;11;6M  <ESC>[<32;11;7M  <ESC>[<0;11;7m
                                            ^^ both motions and the release one row low
===== attached client, pane-border-status=bottom, screen row 5 =====
  click(10,5)          -> <ESC>[<0;11;6M   <ESC>[<0;11;6m
```

One physical click, press and release at the identical cell, arrives at the application on two
different rows. The reporter's table is confirmed exactly, including that `bottom` is unaffected.
Bare motion (button 35) was correct under `top`, because the client resolves that one itself.

## 2. tmux parity

tmux cannot drift like this because the label row is charged to the pane's own geometry once and
every event then shares a single conversion. `layout_fix_panes` (layout.c:461-470) sets
`wp->yoff = lc->g.yoff` and, when `layout_add_horizontal_border` says this cell is the topmost one
and the status is `PANE_STATUS_TOP`, does `wp->yoff++` and `sy--`; so `wp->yoff` already points at
the first CONTENT row and `wp->sy` already excludes the label. Every mouse event — press, drag,
release, motion and the wheel — then reaches the pane through `input_key_mouse` (input-keys.c:797),
which calls the one helper `cmd_mouse_at` (cmd.c:764) and returns `y - wp->yoff`. There is exactly
one conversion site and it consults geometry that already knows about the label, so a press and its
release are incapable of disagreeing. psmux instead converts in two places, one per route, and only
one of them knew about the label row.

## 3. Root cause

One sentence: psmux converts a screen cell into a pane cell in two places — the client subtracts
`pane_content_inner`'s top (label row excluded) before sending `pane-mouse`, while the server's
raw-coordinate handlers subtracted the pane's LAYOUT SLOT top (label row included) — so with
`pane-border-status top` every verb that travels as raw screen coordinates (`mouse-up`,
`mouse-drag`, `mouse-down`, `mouse-move`, `scroll-up`/`scroll-down`) landed one row below the press
the client had already converted.

## 4. Fix

`39bd94e` — `src/window_ops.rs`, `src/client.rs`.

* `client::DEFAULT_PANE_BORDER_FORMAT` is now a shared constant, so the #414 default (tmux ships a
  non-empty `pane-border-format`, so `set pane-border-status top` alone still costs the pane a row)
  is spelled once and used by both sides.
* New `window_ops::PaneLabelRow`: resolves `pane-border-status` / `pane-border-format` from the
  server's own options once per event and delegates to the very helper the client and the renderer
  use, `client::pane_content_inner`. `cell_0based` floors the row at 0 exactly as the client's
  `.max(0)` does, so a click on the label row itself still reports the pane's first content row
  rather than a negative one.
* Every server-side raw-coordinate conversion now goes through it: `remote_mouse_down` (app forward
  and copy mode), `remote_mouse_drag` (both, including the edge auto-scroll comparisons, which were
  also a row off), `remote_mouse_up` (both), `remote_mouse_button`, `remote_mouse_motion` and the
  wheel forward in `remote_scroll_wheel`. No conversion site is left using the raw slot.

The duplication is removed in the direction that keeps the geometry in one place: the server no
longer has an opinion about the label row, it asks the same function the client draws with. Zoomed
panes and multi-pane layouts need no special case because psmux reserves the label row per leaf
pane uniformly (`tree.rs` `border_status_rows`), which the unit tests pin.

## 5. Tests

* `tests-rs/test_issue669_border_status_mouse_rows.rs` (13 tests), registered from `src/window_ops.rs`.
  Content rect for off/top/bottom, agreement with `client::pane_content_inner`, the #414 unset-format
  default and an explicitly emptied format, a click on the label row, press/drag/release agreement,
  the lower pane of a vertical split, a zoomed pane, and press/drag/release driven through the real
  `remote_mouse_down`/`_drag`/`_up` in copy mode where the result is observable without a child.
* `tests/test_issue669_border_status_top_mouse_rows.ps1` (20 assertions), both routes.

### Both ways

| binary | new E2E suite |
| --- | --- |
| baseline, 37e2990 src | 16 passed, **4 failed**, 0 skipped |
| fixed, e00aed5 | **20 passed, 0 failed, 0 skipped** |

Baseline failures, all under `top` and on both routes:

```
[FAIL] top : press reported SGR row 6, expected 5
[FAIL] top : drag to screen row 6 reported SGR row 7, expected 6 (#669)
[FAIL] top : real click press <ESC>[<0;11;5M but release <ESC>[<0;11;6m — release is 1 row(s) off (#669)
[FAIL] top : drag reported <ESC>[<32;11;7M for one row below press <ESC>[<0;11;5M (#669)
```

Fixed, same assertions:

```
[PASS] top : press on screen row 5 reported SGR row 5
[PASS] top : release on the same screen row reported the same SGR row (5)
[PASS] top : drag to screen row 6 reported SGR row 6
[PASS] top : real click press <ESC>[<0;11;5M and release <ESC>[<0;11;5m share a row
[PASS] top : drag one screen row below the press reported one SGR row below it (<ESC>[<0;11;5M -> <ESC>[<32;11;6M)
```

The unit tests were likewise written against the defect: with the handlers' conversion reverted the
copy-mode handler cases fail (`("top", 5, 9)` yields `(5, 9)` instead of `(4, 8)`), while the pure
resolver cases pass either way since they test the new helper directly.

## 6. Commits

| sha | what |
| --- | --- |
| 39bd94e | fix(#669): convert raw mouse coordinates against the pane's content rect |
| c7e352b | test(#669): pin the pane-border-status row conversion for press, drag and release |
| 106cd17 | test(#669): E2E suite for pane-border-status mouse rows on both mouse routes |
| e00aed5 | test(#669): judge press against release case sensitively and wait out the namespace teardown |

`e00aed5` fixes two defects in my own suite, both caught by running it: PowerShell's `-eq` is
case-insensitive so the release filter matched the bare-motion `M` report and the `top` case passed
on an unfixed binary (now `-ceq`), and a previous run's server is still reaping when the next
`new-session` starts (the teardown now waits for the port file to go).

## 7. Test runs

| run | result |
| --- | --- |
| `cargo test --bin psmux issue669` | 13 passed, 0 failed |
| `cargo test --bin psmux mouse` | 120 passed, 0 failed |
| `cargo test --bin psmux copy` | 147 passed, 0 failed |
| `cargo test --bin psmux border` | 134 passed, 0 failed |
| `cargo test --no-run` | compiles clean, all test binaries built |
| `tests/test_issue669_...ps1` (new) | 20 / 0 / 0 |
| `tests/test_pr595_mouse_route_parity.ps1` | 37 / 0 / 0 |
| `tests/test_pr590_drag_autoscroll.ps1` | 33 / 0 / 0 |
| `tests/test_copy_drag_autoscroll.ps1` | 12 / 0 |
| `tests/test_mouse_hover.ps1` | 7 / 0 |
| `tests/test_issue570_wheel_coordinates.ps1` | 29 / 0 (equals the baseline's clean run) |
| `tests/test_issue60_native_tui_mouse.ps1` | 16 / 4 — **baseline on the same suite: 15 / 5** |

Note on the last two: while other agents were running suites in the DEFAULT data root, servers there
were dying mid-run for both binaries. Proven with a control: a plain `new-session -d -s i669ctl`
started by the BASELINE binary and then left completely alone was gone within 10 seconds and stayed
gone for all six checks over a minute. `test_issue570` and `test_issue60` both use the default
namespace, and both flaked on the baseline binary exactly as they did on mine (baseline 570: one
clean 29/0, then "session did not start" on two of three further attempts; baseline issue60: 15/5,
dying one scroll INTO part 3 where mine survived to 3.7). Every failure in those two suites is a
server that vanished, never a coordinate assertion: on my binary every coordinate assertion that
executed passed, and the final calm 570 run was 29/0. My own suite runs under its own `-L i669m`
namespace and is unaffected.

Two things I littered and then cleaned: stale `i669__t.pid` / `.sid` satellites from a first attempt
that ran a RENAMED binary (a renamed `pmux.exe`/`psmux_baseline.exe` could not create the session),
whose PID anchor then made that one namespace+session pair permanently refuse new servers until the
files were removed. Renamed A/B binaries must be named `psmux.exe` in their own directory.

## 8. Leftovers

None. After teardown: no file under `%USERPROFILE%\.psmux` matches `i669`, `test_i570` or
`issue60test`; no `psmux.exe` process from this worktree is alive; no `cmd.exe` launcher window of
mine remains. The only live psmux is another agent's `__warm__` standby from the INSTALLED
`~\.cargo\bin\psmux.exe`, which I did not touch. No `cargo install` was run and `~\.cargo\bin` was
never written to.
