# Issue #662: the six tmux mouse tracking flags are now format variables

Worktree: `C:\Users\godwin\Documents\workspace\psmux\.claude\worktrees\agent-a77529a94a20a245f`
Base: master `37e2990`. Branch head: `44f2572`.

## 1. Reproduction, before any code was read for a fix

Unfixed binary built from `37e2990`, detached session under `-L i662`:

```
has-session rc=0
mouse_any_flag         rc=0 -> ''
mouse_standard_flag    rc=0 -> ''
mouse_button_flag      rc=0 -> ''
mouse_all_flag         rc=0 -> ''
mouse_utf8_flag        rc=0 -> ''
mouse_sgr_flag         rc=0 -> ''
alternate_on           rc=0 -> '0'
pane_in_mode           rc=0 -> '0'
tmux binding cond      -> '0'
```

All six render empty while `alternate_on` and `pane_in_mode` answer, exactly as reported.
`#{||:...}` reads an empty string as false, so tmux's default wheel binding could only
ever reach its `copy-mode -e` branch.

The tracking state itself was already there. With `PSMUX_MOUSE_DEBUG=1` and a pane child
that wrote `ESC[?1000h`, the unfixed server logged:

```
[00:52:47.749]   -> mouse proto AnyMotion on pane 1 (fg_shell=Some(false) alt=false app_owned=true)
[00:52:47.793]   -> wheel latch EARNED by pid 37664 in pane 1 (#613)
```

so the pane knew; it just had no way to say so.

## 2. After the fix, same session, same child

```
plain shell pane : 'any=0 std=0 btn=0 all=0 utf8=0 sgr=0 alt=0 mode=0 cond=0'
after ESC[?1000h : 'any=1 std=1 btn=0 all=0 utf8=0 sgr=1 alt=0 mode=0 cond=1'
after ESC[?1006h : 'any=1 std=1 btn=0 all=0 utf8=0 sgr=1 alt=0 mode=0 cond=1'
after ESC[?1003h : 'any=1 std=0 btn=0 all=1 utf8=0 sgr=1 alt=0 mode=0 cond=1'
after all DECRST : 'any=0 std=0 btn=0 all=0 utf8=0 sgr=0 alt=0 mode=0 cond=0'
```

Per mode, each child started fresh:

```
set=1000     -> any=1 std=1 btn=0 all=0 utf8=0 sgr=1
set=1002     -> any=1 std=0 btn=1 all=0 utf8=0 sgr=1
set=1003     -> any=1 std=0 btn=0 all=1 utf8=0 sgr=1
set=1005,1006 -> any=1 std=0 btn=0 all=1 utf8=1 sgr=1
then all off -> any=0 std=0 btn=0 all=0 utf8=0 sgr=0
```

## 3. A Windows fact the tests are written around

Measured on Windows 11 26200, one pane, three readings:

```
before child     -> any=0 std=0 btn=0 all=0 utf8=0 sgr=0
child, no DECSET -> any=1 std=0 btn=0 all=1 utf8=0 sgr=1
then ESC[?1000h  -> any=1 std=1 btn=0 all=0 utf8=0 sgr=1
```

Switching stdin to raw (`ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_EXTENDED_FLAGS`, and
explicitly NOT `ENABLE_MOUSE_INPUT`) makes conhost publish `ESC[?1003h ESC[?1006h`
upstream with the application having asked for nothing. The application's own DECSET is
tracked exactly when it comes after that. This is the same contamination #598 and #613
are about, and it is why `tests\issue662_mouse_mode_child.cs` sets the console mode
first and writes its DECSETs second, and why the `sgr` column reads 1 in every live pane
measurement above. It is documented in `docs/integration.md`.

## 4. tmux source references

| Reference | What it says |
|---|---|
| `tmux/format.c:1940` | `format_cb_mouse_all_flag` -> `wp->base.mode & MODE_MOUSE_ALL` |
| `tmux/format.c:1952` | `format_cb_mouse_any_flag` -> `wp->base.mode & ALL_MOUSE_MODES` |
| `tmux/format.c:1964` | `format_cb_mouse_button_flag` -> `MODE_MOUSE_BUTTON` |
| `tmux/format.c:1991` | `format_cb_mouse_sgr_flag` -> `MODE_MOUSE_SGR` |
| `tmux/format.c:2003` | `format_cb_mouse_standard_flag` -> `MODE_MOUSE_STANDARD` |
| `tmux/format.c:2015` | `format_cb_mouse_utf8_flag` -> `MODE_MOUSE_UTF8` |
| `tmux/format.c:3633-3664` | the six table entries, all `FORMAT_TABLE_STRING` |
| `tmux/tmux.h:680-687` | `MODE_MOUSE_STANDARD 0x20`, `MODE_MOUSE_BUTTON 0x40`, `MODE_MOUSE_UTF8 0x100`, `MODE_MOUSE_SGR 0x200`, `MODE_MOUSE_ALL 0x1000` |
| `tmux/tmux.h:698` | `ALL_MOUSE_MODES (MODE_MOUSE_STANDARD\|MODE_MOUSE_BUTTON\|MODE_MOUSE_ALL)`, the encodings deliberately left out |
| `tmux/input.c:2050-2063` | DECSET 1000/1002/1003 each clear `ALL_MOUSE_MODES` before setting their own bit, so the three are exclusive |
| `tmux/input.c:1955-1960` | DECRST 1000/1001/1002/1003 all clear `ALL_MOUSE_MODES`, so any one of them turns the family off |
| `tmux/input.c:1964-1969`, `2067-2072` | 1005 and 1006 set and clear their own bit only |
| `tmux/key-bindings.c:510` | `bind -n WheelUpPane { if -F '#{\|\|:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}' { send -M } { copy-mode -e } }` |
| `tmux/key-bindings.c:513` | `bind -n MouseDown2Pane { select-pane -t=; if -F '#{\|\|:#{pane_in_mode},#{mouse_any_flag}}' { send -M } { paste -p } }` |

## 5. What changed

`crates/vt100-psmux/src/screen.rs`
- new `mouse_encoding_flags` field: 1005 and 1006 are two independent bits, as in tmux,
  kept alongside the existing single `mouse_protocol_encoding` that drives report
  generation. Clearing one now falls back to the other if it is still on.
- six new `Screen` accessors: `mouse_any_flag`, `mouse_standard_flag`,
  `mouse_button_flag`, `mouse_all_flag`, `mouse_utf8_flag`, `mouse_sgr_flag`.
- `clear_mouse_mode` now clears the whole tracking family for any of 9/1000/1002/1003,
  matching `input.c:1955-1960`. It used to clear only an exact match, so a pane that
  enabled 1003 and was disabled with a bare `ESC[?1000l` kept reporting tracking.

`src/format.rs`
- the six variables, read from the TARGET pane (`target_pane()`, the accessor
  `pane_in_mode` uses) rather than the window's active pane, so `-t %N` and
  `list-panes -F` answer per pane the way tmux's `ft->wp` does.

`src/help.rs`
- a "Mouse" row in the built-in format variable catalogue.

`docs/scripting.md`, `docs/integration.md`
- the six rows in the variable table, and a ConPTY note covering both the conhost
  republish above and the one place the flags and psmux's own wheel gate can disagree
  (PSReadLine enabling tracking at a prompt, #548/#598).

`tests/test_issue657_wheel_copy_mode_nonshell.ps1`
- the in-file note that said `#{mouse_any_flag}` "is not a psmux format variable yet"
  now points at the new suite.

## 6. Commits

| sha | subject |
|---|---|
| `8b6dcc2` | fix(#662): publish the six tmux mouse tracking flags as format variables |
| `a045f30` | test(#662): pin each mouse flag to its own DECSET, and the tmux wheel binding condition |
| `44f2572` | test(#662): E2E over a live pane, and document the six flags |

## 7. Tests

New files:
- `tests-rs/test_issue662_mouse_flag_formats.rs`, registered in `src/format.rs` as
  `#[cfg(test)] #[path] mod tests_issue662_mouse_flag_formats;`
- `tests/test_issue662_mouse_flag_formats.ps1` (picked up automatically by
  `tests/run_all_tests.ps1`, which globs `test_*.ps1`)
- `tests/issue662_mouse_mode_child.cs`, a pane program that enables exactly the modes it
  is told to, one DECSET at a time, and logs what it receives

Rust, both ways:

```
this tree   cargo test --bin psmux issue662
            test result: ok. 13 passed; 0 failed; 0 ignored; 3849 filtered out

37e2990     cargo test --bin psmux issue662   (format.rs + screen.rs checked out at 37e2990)
            test result: FAILED. 0 passed; 13 failed
            assertion `left == right` failed
              left: "     "
             right: "1 0 0 1 0 0"
```

Five spaces: six empty variables joined by the format string's separators.

E2E, both ways (same script, `PSMUX_EXE` pointed at each binary):

```
unfixed binary (37e2990 sources)    Passed: 8   Failed: 16
  [FAIL] #{mouse_any_flag} -> '[]', expected '[0]' (#662 prints '[]')
  ... all six ...
  [INFO] pane B (ESC[?1000h) any= std= btn= all= utf8= sgr= alt=0 mode=0 cond=0
  [FAIL] the binding predicts '0' over pane B, expected 1
  [PASS] the wheel reached pane B's application, as the condition said (RECV <ESC>[<64;31;15M)
  [FAIL] list-panes did not report the flags per pane (%1: %2:)

this tree                           Passed: 24  Failed: 0
  [PASS] #{mouse_any_flag} -> '[0]'          (and the other five)
  [PASS] DECSET 1000 lights exactly std      (1002 btn, 1003 all, 1005+1006 utf8+sgr)
  [PASS] DECRST after 1000 clears every flag (and after 1002, 1003, 1005,1006)
  [PASS] source-file took tmux's binding lines unchanged (rc=0, no output)
  [PASS] the binding predicts copy-mode -e over pane A
  [PASS] the binding predicts send -M over pane B
  [PASS] the wheel entered copy mode over pane A, as the condition said (before=0 after=1)
  [PASS] the wheel reached pane B's application, as the condition said (RECV <ESC>[<64;31;15M)
  [PASS] list-panes reports the flags per pane (%1:0 %2:1)
```

The unfixed row is the issue in one line: the wheel really did belong to pane B, and
tmux's binding line would have sent that pane into copy mode instead.

Targeted Rust runs on this tree:

| command | result |
|---|---|
| `cargo test --bin psmux format` | 241 passed, 0 failed |
| `cargo test --bin psmux issue662` | 13 passed, 0 failed |
| `cargo test --bin psmux mouse` | 120 passed, 0 failed |
| `cargo test --bin psmux wheel` | 53 passed, 0 failed |
| `cargo test --bin psmux decset` | 10 passed, 0 failed |
| `cargo test -p vt100-psmux` | 142 passed across 10 targets, 0 failed |
| `cargo test --test test_vt100_mouse` | 1 passed, 0 failed |
| `cargo test --no-run` | every target compiled, no errors |
| `cargo test --bin psmux` (whole binary suite) | stopped at 1373 tests, 0 failures; it blocks in `copy_mode::tests_issue443_blank_cell_capture` (clipboard yank, "running for over 60 seconds"), which is unrelated to this change |

Regression suites, run with this worktree's `target\release\psmux.exe`:

| suite | result |
|---|---|
| `tests\test_issue613_wheel_gate_durability.ps1` | Passed: 22, Failed: 0 |
| `tests\test_issue657_wheel_copy_mode_nonshell.ps1` | Passed: 17, Failed: 0 |
| `tests\test_issue598_wheel_mouse_gate.ps1` | Passed: 14, Failed: 0 |

## 8. Leftovers

Every session lived under `-L i662` (one unfixed comparison run used `-L i662old`).
Four server processes survived the scripts' own cleanup and were killed by PID
(52088, 47824, 63112, 23024, all `psmux.exe server ...` started by this agent). After
that:

```
--- any process from my worktree, temp children, or i662 sessions ---
(empty)
--- alacritty / wezterm / stray console hosts ---
(empty)
--- default root, i662 namespace files ---
(empty)
```

Temporary data roots, the renamed unfixed binary, the compiled test children, the temp
config and the child logs were all removed; `%TEMP%\psmux_i662*` is empty. No window
this agent opened is still open.
