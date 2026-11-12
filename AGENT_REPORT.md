# PR batch review: #663 #664 #665 #666 #667 #670 #671 (tscodeplus)

Branch `pr-batch-2026-09-18`, based on master `37e2990`. Nothing pushed, nothing
posted to GitHub, master untouched. All seven contributor commits kept intact
with their original authorship (`git log --no-merges` shows 7 commits by
tscodeplus and 8 follow up commits by the repo owner account).

Final sha: **a506353** (see the sha table at the end).

**Headline:** all seven are worth taking. Five of them needed a follow up fix
before they actually did what the PR body claims, and one (#671) introduced a
`clear-history` regression that I would not ship without the fix. #667 and #670
are clean as submitted.

Build rule followed throughout: `cargo build --release` inside the worktree,
binary under test `target\release\psmux.exe`. No `cargo install`, `~\.cargo\bin`
never touched. Every session ran under a `-L pr<N>` namespace in an isolated
`USERPROFILE`/`HOME` (tests\psmux_test_helpers.ps1), torn down namespace scoped.

---

## Security review (all seven diffs)

Scanned every added line across the seven diffs for: dependency and manifest
changes, network use, process spawning, file writes, obfuscated strings, unsafe
blocks and environment reads.

- **No** `Cargo.toml`, `Cargo.lock`, `build.rs`, `.github/` or CI change in any
  of the seven. No dependency added, removed or bumped. `crates/vt100-psmux` is
  touched by #671 but only `src/parser.rs`, not its manifest.
- **No** `unsafe`, no `std::env` read or write, no `Command::new`, no
  `fs::write` / `File::create` / `OpenOptions`, no `transmute`, no
  `from_utf8_unchecked`, no base64/hex blob, no dynamic loading.
- **Network:** exactly one hit, the `tcp_pair` helper you spotted. Confirmed
  **test only and loopback only**: it lives in the new
  `tests-rs/test_copy_mode_snapshot.rs`, which is included behind `#[cfg(test)]`
  from `src/copy_mode.rs`, and it binds `127.0.0.1:0` (ephemeral loopback) purely
  to manufacture the reader/writer streams a `proxy_pane` needs so the test can
  build a real `Pane` with a real vt100 parser and no child process. Nothing is
  ever written to the socket and no outbound connection is made.
- `#670` mentions `ping.exe` in a comment; the test already spawned `ping.exe -t`
  as its parked process before this PR, and the PR adds no new spawn.

Nothing abnormal to flag.

---

## PR #663: `window-size latest` follows the active client

**Verdict: merge with the follow up commit `19bfbb9`.**

### tmux parity
Correct in intent and exactly tmux's model. `server_client_update_latest`
(server-client.c:1343) sets `w->latest = c` and, when `window-size` is `latest`,
calls `recalculate_size`. It is called from the key callback for every key
except `KEYC_FOCUS_OUT` (server-client.c:1655) and from `MSG_RESIZE`, skipping
CONTROL clients (server-client.c:2590). The PR's `note_client_activity` is the
psmux equivalent, and ignoring clients with no `client_sizes` entry covers
tmux's "CONTROL clients are skipped" plus psmux's one shot CLI clients.

Two divergences worth knowing:
1. tmux keeps `latest` **per window** (`w->latest`); psmux keeps one
   `app.latest_client_id` for the server. That is pre-existing psmux structure,
   not something this PR introduced.
2. tmux does **not** exclude bare pointer motion; psmux does, via
   `is_bare_motion_cmd`. That is a deliberate, conservative divergence
   protecting #604 (hovering must not steal the size) and I think it is the
   right call.

**Your question: can a small phone client shrink the window every time the
user types there? Yes, and that is tmux behaviour.** tmux moves `w->latest` on
every key, so typing on the phone hands the window back to the phone. Measured
below (`after phone TYPES 'b' : 80x24`).

### Defect found and fixed
The activity ping was only sent from the read at the **bottom** of the command
loop (`src/server/connection.rs`), which runs with a 10 ms read timeout right
after a command was dispatched. Real typing is spaced further apart than that,
so that read times out, the line is cleared, and the next command is picked up
by the read at the **top** of the loop, which sent nothing. The feature
therefore did not work end to end at all; the PR's unit tests pass because they
call `note_client_activity` directly.

E2E before the fix (two persistent clients, `window-size latest`):

```
after desktop client-size 200x50 : 200x50
after phone   client-size  80x24 : 80x24
after desktop TYPES 'a'         : 80x24     <- FAIL
```

After `19bfbb9` (same script, same binary path):

```
after desktop client-size 200x50 : 200x50
after phone   client-size  80x24 : 80x24
after desktop TYPES 'a'         : 200x50    <- latest followed the active client
after phone   TYPES 'b'         : 80x24     (tmux parity: latest follows keys)
after desktop bare mouse-move   : 80x24     (must stay 80x24, #604)
after one-shot 'psmux ls'       : 80x24     (sizeless client cannot drive it)
RESULT: E2E-663 PASS
```

### Tests
`cargo test --bin psmux resize_window` -> **23 passed; 0 failed** (matches the
PR's claim of 23).

### Conflicts
None.

---

## PR #664: `mouse-drag-enter-copy-mode`

**Verdict: merge with the follow up commit `873970a`.**

### tmux parity
Exact. tmux key-bindings.c:504:
`bind -n MouseDrag1Pane { if -F '#{||:#{pane_in_mode},#{mouse_any_flag}}' { send -M } { copy-mode -M } }`
In that binding, a left drag in a pane that is neither in a mode nor tracking
the mouse enters `copy-mode -M`. The PR's gate is `mouse_drag_enter_copy_mode && !pane_handles_mouse`
with `client_copy_mode ||` covering `pane_in_mode`. Release parity too:
`bind -Tcopy-mode MouseDragEnd1Pane { send -X copy-pipe-and-cancel }` yanks and
cancels, which is what the PR does. And `copy-mode -M` anchors at the drag
start, which is why the PR replays the press cell, right.
Default `off` keeps psmux's existing client side overlay, so nobody's muscle
memory changes unless they opt in.

Note for the contributor: the new gate sits **inside** the existing client side
selection block, so the option only takes effect while `mouse-selection` is on.
With `mouse-selection off` the option is inert. That is defensible (the option
is a replacement for the overlay) but it is worth a line in the docs.

Interaction with `pwsh-mouse-selection`: both can be on; the new option wins,
because the press clears `rsel_*` and the drag branch then consumes the pending
copy mode entry. Verified there is no refusal and both options report `on`
together.

### Defects found and fixed
1. **The option was not in `src/server/option_catalog.rs`**, so `set -gu
   mouse-drag-enter-copy-mode` was a silent rc-0 no-op. Measured against the
   sibling option in the same probe:
   ```
   after set on          : [mouse-drag-enter-copy-mode on]
   set -gu output        : []  rc=0
   after set -gu         : [mouse-drag-enter-copy-mode on]     <- wrong
   --- baseline sibling scroll-enter-copy-mode ---
   after set off         : [scroll-enter-copy-mode off]
   after set -gu         : [scroll-enter-copy-mode on]         <- correct
   ```
   After `873970a`: `after set -gu : [mouse-drag-enter-copy-mode off]`.
2. **The bulk `show-options -g` listing** (`src/server/mod.rs`) never printed
   the option. Only `commands.rs generate_show_options` had been updated;
   `show-options -g <name>` worked but the full listing omitted it.

A regression test for the catalog default was added to the PR's own test file.

### E2E
```
--- option plumbing ---
  OK   default #{mouse-drag-enter-copy-mode} = off
  OK   show-options -g lists it = mouse-drag-enter-copy-mode off
  OK   after set -g ... on = on
  OK   show-options -g reflects on = mouse-drag-enter-copy-mode on
  OK   toggles back off = off
--- the option reaches the attached client in the state JSON ---
  OK   state JSON carries mouse_drag_enter_copy_mode = true
--- the server half of the gesture (what the client now sends) ---
  OK   before the drag #{pane_in_mode} = 0
  OK   during the drag #{pane_in_mode} = 1
  OK   after the release #{pane_in_mode} = 0
  yanked buffer = [\Users\godwin\Do]
  OK   release yanked a selection
--- a plain press without a drag must NOT enter copy mode (tmux MouseDown1Pane) ---
  OK   after a plain click #{pane_in_mode} = 0
--- pwsh-mouse-selection can be set alongside it ---
  OK   pwsh-mouse-selection = pwsh-mouse-selection on
  OK   mouse-drag-enter-copy-mode still on = on
RESULT: E2E-664 PASS
```
The gesture was driven over the control port with the exact verbs the patched
client now emits (`copy-enter`, `pane-mouse <id> 0 col row M`, `pane-mouse <id>
32 col row M`, `pane-mouse <id> 0 col row m`).

### Tests
`cargo test --bin psmux mouse_drag_enter_copy_mode` -> **7 passed; 0 failed**
(6 from the PR plus the catalog regression test).
`cargo test --bin psmux client_entered_copy_mode_drag` -> **1 passed; 0 failed**.

### Conflicts
None on merge; the window_ops test anchor collided with #665 when #665 was
merged next (resolved there).

---

## PR #665: `g`/`G` vi only, add `M-<` / `M->`

**Verdict: merge with the follow up commit `195fe3d`.**

### tmux parity
Confirmed against key-bindings.c: `M-<` / `M->` are bound in the **copy-mode**
table (:619, :620) and `g` / `G` in **copy-mode-vi** only (:694, :678). psmux's
default `mode_keys` is `emacs` (types.rs:2088), so this does change the default
behaviour: a bare `g` in copy mode no longer jumps to history top. That is
correct tmux parity but it is a user visible change for anyone who relied on the
old psmux behaviour, and worth a release note.

One additive divergence: the PR makes `M-<` / `M->` work under **both**
`mode-keys` settings. tmux binds them in `copy-mode` only, not `copy-mode-vi`.
Nothing is displaced by this (psmux's vi copy mode has no other `M-<`), so it is
harmless, but it is not tmux behaviour. **Your question: does `mode-keys vi`
still allow `M-<`? Yes**, measured below. The docs change matches the emacs
behaviour correctly.

Minor: the gate is `app.mode_keys != "emacs"` rather than `== "vi"`, so a
nonsense `mode-keys foo` would keep `g`/`G` live. Cosmetic.

### Defect found and fixed
`M-<` / `M->` were added to `handle_key`'s `KeyCode::Char('<') + ALT` match, but
Alt keys reach copy mode as **named** keys through `send_key_to_active` in
`src/input.rs`, the same reason `M-x` has its own arm there, with a comment
saying exactly that. So the new binding was dead. Measured on a 380 line
scrollback with `mode-keys emacs`:

```
  send-keys 'M-v' : pos 1 -> 11   (a pre-existing Alt binding works)
  send-keys 'M-<' : pos 1 -> 1    <- dead
  send-keys 'M->' : pos 1 -> 1    <- dead
  send-keys -X history-top    : pos 1 -> 380   (the server command is fine)
```

After `195fe3d` (arms added to the named key dispatcher):

```
--- emacs (psmux default mode-keys) ---
  OK   emacs 'g' left the view alone            (scroll_position 1 -> 1)
  OK   M-< reached history top                  (1 -> 380, history_size 380)
  OK   M-> reached history bottom               (380 -> 0)
--- vi ---
  OK   vi 'g' reached history top               (1 -> 380)
  OK   vi 'G' reached history bottom            (-> 0)
  OK   M-< works in vi mode too                 (1 -> 380)
RESULT: E2E-665 PASS
```

### Tests
`cargo test --bin psmux history_top` -> **4 passed; 0 failed** (the PR's three
plus a pre-existing one).
`cargo test --bin psmux named_alt_less_than` -> **1 passed; 0 failed** (the new
regression test, which exercises both `mode-keys` tables).

### Conflicts
`src/window_ops.rs`: #664 and #665 each appended a test at the same anchor in
`window_ops_tests`, and git split the shared `#[test]` / closing brace. Resolved
by keeping both blocks (merge commit `edf7dd5` records this).

---

## PR #666: re-anchor the copy mode view across a pane resize

**Verdict: merge with the follow up commit `82a04e4`.**

### tmux parity
The arithmetic matches tmux. `window_copy_resize` (window-copy.c:1211) converts
the view offset into an absolute line before the resize
(`cy = gd->hsize + data->cy - data->oy`) and recomputes `oy = gd->hsize - cy`
against the new hsize, algebraically identical to `offset_after_trim`'s
`offset - (filled_before - filled_after)`.

One divergence: when the shifted offset is still deeper than the retained
history, the PR **exits copy mode**. tmux never leaves copy mode on a resize; it
pre-clamps `oy` (`if (data->oy > gd->hsize + data->cy) data->oy = gd->hsize +
data->cy;`) and, in the equivalent case, lands at `oy = 0`, i.e. the live bottom
with copy mode still up. Dropping the mode is a more surprising outcome than
tmux's clamp. Not a blocker, the state it recovers from is the corruption that
#671 removes at the root, but if the contributor wants strict parity, the
`None` arm should clamp to the live bottom and stay in copy mode.

### Defects found and fixed
1. **A stolen `#[cfg(test)]`.** The new module was inserted between an existing
   `#[cfg(test)]` and the `mod tests_copy_cancel_stale_state;` it belonged to,
   leaving a doubled attribute on the new module and **no** attribute on
   `tests_copy_cancel_stale_state`, which was therefore compiled into the
   release binary.
2. **Wrong window.** `reanchor_after_resize` addresses `app.active_idx`, but
   `resize_window_panes` is called per window (`resize_all_panes` walks every
   window). A background window's resize would measure its own pane's retained
   depth and apply that shift to the pane the user is looking at. The snapshot
   is now only taken when `window_index == app.active_idx`.

### E2E
Two runs, both PASS. The view stayed anchored on the exact same content line
across every resize:

```
before resize  pane_in_mode|scroll_position|history_size = 1|240|563
  top visible line before = [resize scrollback line 322]
after  resize  (120x40 -> 60x20)                        = 1|240|563
  top visible line after  = [resize scrollback line 322]
  OK   still in copy mode and NOT pinned at history top
  OK   the view still scrolls after the resize (one page-down -> 1|220|563)
  OK   not pinned after a grow (60x20 -> 140x45)
RESULT: E2E-666 PASS
```

With `history-limit 200` and the buffer at capacity (a second run designed to
force a trim):

```
before resize = 1|80|200   top=[trimline 682]
after resize to 40x12 = 1|80|200   top=[trimline 682]   OK not pinned
after resize to 30x8  = 1|80|200   top=[trimline 682]   OK not pinned
RESULT: E2E-666b PASS
```

**Honest caveat:** in neither E2E did psmux's vt100 actually trim retained
history on resize (`history_size` was identical before and after), so the trim
branch itself was exercised only by the PR's unit tests, not end to end. What
the E2E does prove is that the change introduces no regression and the view
stays exactly where the user put it. The contributor reproduced the live trim on
a phone client whose on screen keyboard toggles the pane height.

### Tests
`cargo test --bin psmux copy_mode_resize_reanchor` -> **5 passed; 0 failed**.

### Conflicts
None on merge; `src/copy_mode.rs` collided with #671 when #671 was merged next
(resolved there).

---

## PR #667: stop the Ctrl+V read-back pasting a CJK burst twice

**Verdict: merge as is. No follow up needed.**

### Review
Clean and well scoped. The guard records the text of the last paste shaped burst
plus a timestamp at every delivery site, and drops a clipboard read-back or a
late `Event::Paste` only when the **content matches** and it is inside a 300 ms
window. Content comparison, not bare timing, is the right design.

### Your question: does the 300 ms same string suppression ever eat a legitimately repeated paste?

**No.** Measured with a real attached client and WriteConsoleInput injected
Ctrl+V (`tests\injector.cs`, AttachConsole based so no focus stealing), each
case with a distinct payload so nothing had to be cleared between runs:

```
  CJK    '甲乙丙丁'  two Ctrl+V  120 ms apart -> 2 occurrence(s) on the pane
  CJK    '戊己庚辛'  two Ctrl+V  250 ms apart -> 2 occurrence(s) on the pane
  CJK    '壬癸子丑'  two Ctrl+V  600 ms apart -> 2 occurrence(s) on the pane
  ASCII  'ZQA'      two Ctrl+V  120 ms apart -> 2 occurrence(s) on the pane
  ASCII  'ZQB'      two Ctrl+V  250 ms apart -> 2 occurrence(s) on the pane
  ASCII  'ZQC'      two Ctrl+V  600 ms apart -> 2 occurrence(s) on the pane
```

The reason it is safe: the text always reaches the pane through the burst of
characters, which is never suppressed. Only the redundant read-back is dropped,
so a second Ctrl+V still delivers its own burst.

### E2E (the actual bug)
Real attached client, `Set-Clipboard` with CJK, injected `^v`:

```
attached client pid = 51352
--- one Ctrl+V with CJK on the clipboard must paste the text ONCE ---
  occurrences of '恭喜通关' on the pane = 1
  line = [PS C:\...\agent-a9edd984903fea9c9> 恭喜通关]
  OK   pasted exactly once
--- a SECOND deliberate Ctrl+V of the same text ---
  occurrences after the second paste = 2 (want 2)
  OK   a deliberate repeat paste still gets through
--- ASCII control: a short ASCII string pasted twice ---
  occurrences of 'ZQ' after two Ctrl+V = 2 (want 2)
RESULT: E2E-667 PASS
```

### Tests
`cargo test --bin psmux client::tests` -> **48 passed; 0 failed**.
`cargo test --bin psmux paste` -> **59 passed; 0 failed**.

### Conflicts
None (auto merged cleanly with #664 in `src/client.rs`).

---

## PR #670: deflake the #615 parked process cwd test

**Verdict: merge as is. No follow up needed.**

Test only, one file, one helper. Replaces an assumption about when a freshly
created process's PEB reports its working directory with a bounded poll (20 ms
interval, 15 s deadline, and it fails loudly if the reading never arrives).
No production code touched.

Ran the #615 tests **10 times**:

```
run  1..10: test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured
FAILED RUNS: 0 / 10
```

### Conflicts
None.

---

## PR #671: copy mode reads a snapshot of the pane grid

**Verdict: merge with the follow up commits `a929002`, `c1daa53` and `088fbad`,
and with the memory note below on the record.** The `clear-history` regression
below is the one thing in this batch I would not ship without the follow up.

### Design
`pane.term` is swapped to hold a frozen `Parser` snapshot while copy mode is up
and the live parser is parked in `pane.live_term`. The PTY reader thread holds
its own `Arc` to the live parser from pane creation, so it keeps feeding the
live screen; rendering and copy mode read the snapshot; `capture-pane` and its
variants go through the new `live_parser()`. `sync_copy_snapshot` reconciles per
frame so exactly the active pane holds a snapshot, exactly while copy mode is up
(see the scope limit below).
`each_term()` / `for_each_term_mut` make resize, `history-limit` and
`alternate-screen` reach both screens. `respawn`/`heal` reset `live_term` to
`None`. The design is sound and the reconciliation is the right defensive shape.

### tmux parity
- **capture-pane reads the live screen: correct.** cmd-capture-pane.c reads
  `wp->base` throughout (:134, :279, :287, :291) and never the copy-mode grid.
  The PR matches this exactly.
- **Copy mode on a copy of the grid: correct.** That is `window_copy_init`.
- **`#{history_size}` is a divergence.** tmux's `format_cb_history_size`
  (format.c:1895) returns `ft->wp->base.grid->hsize`, the **live** grid, even
  while the pane is in copy mode. The PR makes it report the frozen depth. The
  PR documents this deliberately in docs/features.md, and the frozen pair
  (`scroll_position` / `history_size` from the same grid) is arguably more
  useful, but a script written against tmux will see history stop growing in
  copy mode. Your call; I would keep it and keep the doc line.

**Scope limit worth telling the contributor about (code reading, not measured):**
`sync_copy_snapshot` gates on `want && is_active_window && Some(p.id) ==
active_id`, and `enter_copy_mode` only ever snapshots the active pane. psmux
allows more than one pane to be in copy mode at once (#607, and docs/features.md
says so on the line right above the one this PR adds). A **non-active** pane in
copy mode therefore gets no snapshot, `sync_copy_snapshot` actively calls
`leave_copy_snapshot` on it every frame, so it keeps the old behaviour and can
still be pushed around by its own output. Not a regression, and the active pane
is the case that matters in practice, but the docs line "the view is a snapshot"
currently overstates it.

There is a third consequence, wider than the PR body suggests: `capture-pane`
now stops following a **scrolled** copy-mode view, for any reason it was
scrolled, not just when new output arrives. It broke three assertions in two
shipped suites. See "The one behaviour change you should decide on consciously"
below, it is tmux correct, but it deserves a release note.

### Defect 1 found and fixed: `clear-history` stopped working in copy mode

This is the one that matters. `CtrlReq::ClearHistory` replaced the parser behind
`pane.term`. Since #671, `pane.term` is the **snapshot** while copy mode is up,
so `clear-history` wiped the screen the user was reading and left the live
scrollback completely untouched. Reproduced on the release binary:

```
--- baseline: clear-history OUTSIDE copy mode ---
  before: 0|0|473
  after clear-history: 0|0|0                       <- correct

--- clear-history WHILE in copy mode ---
  in copy mode: 1|60|502
  after clear-history: 1|60|0                      <- the SNAPSHOT was cleared,
                                                      and psmux stayed in copy
                                                      mode at scroll 60 of a
                                                      0 line history
  after leaving copy mode: 0|0|502                 <- the live history is BACK
  capture-pane -S -200 still shows 229 scrollback content lines
```

tmux does the opposite and in a specific order (cmd-capture-pane.c:418):
`window_pane_reset_mode_all(wp)` then `grid_clear_history(wp->base.grid)`, it
**leaves copy mode first**, then clears the **live** grid. Fixed in `c1daa53` by
extracting `window_ops::clear_active_pane_history`, which does exactly that, and
pointing the handler at it. A regression test
(`clear_history_leaves_copy_mode_and_clears_the_live_screen`) was added to the
PR's own test file.

### Defect 2 found and fixed: a stolen doc comment
The `snapshot` method was inserted between `pub fn process` and the doc comment
that documented it, so `process` became undocumented and `crates/vt100-psmux`
(which warns on missing docs) emitted
`warning: missing documentation for a method  --> crates\vt100-psmux\src\parser.rs:70`
on every release build. Fixed in `a929002`; the release build is warning free
again apart from the pre-existing multi-bin-target warning.

### E2E: the user visible claim
A pane streaming output continuously while the user sits in copy mode scrolled
up, polled every 100 ms for 4 seconds:

```
in copy mode while the pane streams: 1|200|531
  capture-pane top content line at entry = [stream line 188]
--- poll #{pane_in_mode}|#{scroll_position}|#{history_size} every 100 ms ---
    1|200|531            <- the ONLY distinct sample across all 40
  OK   never pinned at history top (40 samples)
  samples still in copy mode: 40/40
  OK   scroll_position never moved from 200
--- capture-pane reads the LIVE screen while copy mode shows the snapshot ---
  capture-pane top content line now = [stream line 510]
  OK   capture-pane moved on with the live screen while the view stayed frozen
--- leaving copy mode reveals everything that arrived ---
  before cancel 1|200|531   after cancel 0|0|968
  OK   history_size jumped from the frozen 531 to the live 968 on exit
  OK   scroll_position back to 0
  OK   the live screen is back
RESULT: E2E-671 PASS
```

That single line `1|200|531` repeated 40 times is the whole point of the PR: the
view is genuinely immovable while the application keeps printing. `capture-pane`
advancing from `stream line 188` to `stream line 510` over the same window is
the tmux `wp->base` behaviour, proven live.

### Memory cost, measured on a 50000 line history (you asked for this)

Session at 200 columns, `history-limit 60000`, filled to `history_size 49952`:

```
BEFORE copy mode : working set 190.7 MB  private 184.8 MB
IN     copy mode : working set 365.3 MB  private 364.9 MB   (snapshot cost 180.1 MB private)
AFTER copy mode  : working set 193.9 MB  private 186.1 MB   (delta vs before 1.3 MB)
```

**Entering copy mode roughly doubles the server's memory on a large scrollback:
+180 MB private here.** A single enter/leave cycle gives it all back (+1.3 MB).

Repeated cycles retain some of it, but it **plateaus**. That is allocator
retention, not a leak:

```
baseline private = 184.7 MB (history_size 49952)
  after  5 enter/leave cycles : private 196.1 MB  (delta 11.4 MB)
  after 10 enter/leave cycles : private 243.4 MB  (delta 58.7 MB)
  after 15 enter/leave cycles : private 244.0 MB  (delta 59.3 MB)
  after 20 enter/leave cycles : private 243.0 MB  (delta 58.3 MB)
  after 25 enter/leave cycles : private 243.3 MB  (delta 58.6 MB)
  after 30 enter/leave cycles : private 243.4 MB  (delta 58.7 MB)
```

Flat from cycle 10 through cycle 30. So the steady state cost is about +59 MB
retained plus a +180 MB spike whenever copy mode is open on a 50k line, 200
column pane. tmux pays the same class of cost by design, but tmux's grid is far
more compact than psmux's vt100 cell array. Worth telling the contributor, and
worth considering whether a future change can share the scrollback rows
copy-on-write instead of cloning them.

### Tests
`cargo test --bin psmux copy_mode_snapshot` -> **7 passed; 0 failed** (6 from
the PR plus the `clear-history` regression test).

### Conflicts
`src/copy_mode.rs`: #666 and #671 each added a test module declaration right
after `tests_capture_pane_fidelity`. Resolved by keeping both (merge commit
`a2aa40c`). `src/tree.rs`, `src/types.rs`, `src/server/mod.rs`,
`src/window_ops.rs` and `tests-rs/test_issue615_warm_claim_cwd_hint.rs` all auto
merged; the #666 re-anchor and the #671 `each_term()` resize loop coexist
correctly (the re-anchor now operates on the snapshot, which is the copy mode
view, which is right).

---

## Whole batch verification

### Rust tests

```
cargo test --no-run                      all targets built
cargo test --bin psmux                   3884 passed; 0 failed; 0 ignored
cargo test --bin psmux copy              164 passed; 0 failed
cargo test --bin psmux capture            59 passed; 0 failed
cargo test --bin psmux warm              119 passed; 0 failed
cargo test --bin psmux resize             67 passed; 0 failed
cargo test --bin psmux paste              59 passed; 0 failed
cargo test --bin psmux client::tests      48 passed; 0 failed
cargo test --bin psmux mouse             114 passed; 0 failed
```

`cargo test` (all bins) runs the same tests once per bin target. The `psmux` and
`pmux` bins both reported **0 failed**. The
`tmux` bin test executable could not be launched:

```
Caused by:
  Operation did not complete successfully because the file contains a virus or
  potentially unwanted software. (os error 225)
```

That is Windows Defender blocking `target\debug\deps\tmux-*.exe` on its name.
It is an environment artifact, not a batch defect: the identical test binary
under the `psmux` and `pmux` names ran green.

### PowerShell E2E suites

Every suite run against `WT\target\release\psmux.exe` (a copy of it was put first
on `PATH` so the suites that resolve psmux with `Get-Command` picked up the batch
build rather than the installed one, and `PSMUX_TEST_BIN` / `PSMUX_EXE` /
`PSMUX_BIN` / `PSMUX_TEST_EXE` were all pointed at it).

| suite | pass | fail |
| --- | --- | --- |
| test_issue596_copy_scroll_keys | 26 | 0 |
| test_issue612_copy_search_scrollback | 19 | 0 |
| test_issue494_copymode_freeze | 3 | 0 |
| test_capture_pane | 51 | 0 |
| test_copy_mode_advanced | 26 | 0 |
| test_copy_drag_autoscroll | 12 | 0 |
| test_copy_mode_bracket_paragraph | 16 | 0 |
| test_copy_cancel_and_dashdash_name | 10 | 0 |
| test_issue615_wsl_pane_path | 26 | 0 |
| test_issue604_wsl_nvim_mouse | 3 | 0 |
| test_issue570_wheel_coordinates | 29 | 0 |
| test_issue620_version_resource | 15 | 0 |
| test_issue621_wheel_stdin_block | 16 | 0 |
| test_issue622_hassession_timeout_reap | 15 | 0 |
| test_issue623_far_f1 | 10 | 0 |
| test_issue625_choose_tree_parity | 26 | 0 |
| test_issue626_border_attrs_default | 20 | 0 |
| test_issue627_unqualified_targets | 24 | 0 |
| test_issue629_ssh_vt_wheel | 11 | 0 |

**19 suites, 358 assertions, 0 failures.** That table is a full clean re-run of
all 19 suites against the final binary, not a patched up first pass.

### The one behaviour change you should decide on consciously

The first pass of that table was **not** clean: `test_issue612_copy_search_scrollback`
failed 2 assertions and `test_issue621_wheel_stdin_block` failed 1. All three had
the same cause, and it is the most far reaching consequence of #671:

```
[FAIL] capture-pane top line did not move into history, [LINE_379] -> [LINE_379]
[FAIL] TUI: capture-pane top [LINE_373] does not frame the match [LINE_129] on 29 rows
[FAIL] the view did not move; the wheel scrolled nothing
```

All three watched the copy-mode viewport by diffing a plain `capture-pane -p`.
That worked only because psmux used to scroll the pane's **live** parser in copy
mode, so capture-pane followed the view for free. #671 deliberately ends that:
copy mode scrolls the snapshot and capture-pane reads the live screen.

**This is tmux's behaviour**, and it goes beyond what the PR body advertises.
It is not only about "output arriving while in copy mode": it changes what
`capture-pane` returns for *any* scrolled copy-mode pane. tmux's capture reads
`wp->base` (cmd-capture-pane.c) and the copy-mode offset `oy` lives in the
mode's own data, so tmux's `capture-pane` does not follow a scrolled copy-mode
view either.

Nothing is lost, only the spelling. Measured on the release binary:

```
at the live bottom in copy mode:
  pane_in_mode|scroll_position|history_size|copy_cursor_line = 1|0|374|<prompt>
  capture-pane top = [LINE_373]
after -X search-backward LINE_120:
  pane_in_mode|scroll_position|history_size|copy_cursor_line = 1|276|374|LINE_120
  capture-pane      top = [LINE_373]   <- the LIVE screen, as tmux does
  capture-pane -S -276  top = [LINE_97]  <- the view's own region, as tmux users ask for it
```

The #612 search feature itself is completely intact: `scroll_position` still
goes 0 -> 276 and `copy_cursor_line` still reports `LINE_120`.

I judged the three assertions to be test defects against the new tmux-parity
contract and updated the two helpers to ask for the view's history range with
`-S -<scroll_position>` (commit `088fbad`, clearly labelled). Both suites then
went fully green (612: 17/2 -> 19/0; 621: 15/1 -> 16/0). **But this is your call
to ratify**, because any user script that scraped the copy-mode view with a
plain `capture-pane` will need the same change, and that deserves a release note.

### Skipped suite

`tests\test_copy_mode_full.ps1` was **not run**: it starts with
`taskkill /f /im psmux.exe`, a machine wide by-name kill that would take down the
other agents' servers running in parallel in this session. Everything it covers
is also covered by `test_copy_mode_advanced.ps1`,
`test_copy_mode_bracket_paragraph.ps1` and the 164 `copy` unit tests. There is no
`tests\test_resize_window*.ps1` in the tree (the resize coverage is
`test_issue102_resize_pane.ps1` and `test_zoom_resize.ps1`, plus the 67 `resize`
unit tests).

---

## Commits

| sha | what |
| --- | --- |
| `ebc53e2` | Merge PR #663 |
| `19bfbb9` | fix(#663): ping client activity from the idle read too, not just the batching read |
| `dff6d13` | Merge PR #664 |
| `873970a` | fix(#664): register mouse-drag-enter-copy-mode in the option catalog and the bulk listing |
| `edf7dd5` | Merge PR #665 (conflict in src/window_ops.rs resolved) |
| `195fe3d` | fix(#665): wire M-< / M-> into the named-key copy mode dispatcher |
| `81687e5` | Merge PR #666 |
| `82a04e4` | fix(#666): restore the stolen cfg(test) and scope the re-anchor to the active window |
| `17d7942` | Merge PR #667 |
| `89fdb09` | Merge PR #670 |
| `a2aa40c` | Merge PR #671 (conflict in src/copy_mode.rs resolved) |
| `a929002` | fix(#671): give Parser::process its doc comment back |
| `c1daa53` | fix(#671): clear-history must leave copy mode and clear the live screen |
| `088fbad` | test(#671): observe the copy mode view through -S, not through a plain capture-pane |
| `a506353` | style: no em dash in the clear-history doc comment |

Branch `pr-batch-2026-09-18`, final sha **a506353**.

Nothing was pushed, no PR was merged on GitHub, no comment was posted, master
was never touched, and no release was created.

## Leftovers

Zero. Checked after the final suite run:

```
psmux processes from the batch binary: NONE
orphan cmd.exe from my launches: NONE
processes running anything from my worktree: NONE
leftover shim dirs: NONE
```

The two temp homes my Ctrl+V probe left behind when it exited early
(`psmux_pr667p_*`) were removed. Every namespace used (`pr663`, `pr664`,
`pr664p`, `pr664q`, `pr665`, `pr665p`, `pr666`, `pr666b`, `pr667`, `pr667p`,
`pr667q`, `pr671`, `pr671b`, `pr671m`, `pr671n`, `prCH`, `prCV`) was shut down
with a namespace scoped `kill-server`. No process was ever killed by name; the
attached clients and their launcher consoles were stopped by the PIDs this agent
spawned. `~\.cargo\bin` was never touched and `cargo install` was never run.

The working tree is clean apart from this uncommitted `AGENT_REPORT.md`.
