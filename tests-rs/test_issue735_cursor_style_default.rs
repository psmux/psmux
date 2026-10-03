// Issue #735: psmux asserts a cursor shape on every attach and defaults to
// `bar`, so attaching replaces the cursor shape the terminal was configured
// with. tmux defaults `cursor-style` to `default` and never asserts a shape of
// its own.
//
// tmux, tag 3.7c `e476c123`:
//   options-table.c:62-65    the choice list, `default` first
//   options-table.c:336-341  `cursor-style`, `.default_num = 0`
//   tty.c:332-386            `tty_start_tty` sends no DECSCUSR
//   tty.c:816-825            `tty_update_cursor` sends the reset for
//                            SCREEN_CURSOR_DEFAULT only when its own last
//                            style was not SCREEN_CURSOR_DEFAULT
//
// `SCREEN_CURSOR_DEFAULT` (tmux.h) is a third state beside block, underline
// and bar: "no opinion". psmux spells it 0 and now keeps it, instead of
// resolving it to a shape on the way out.
//
// These tests pin the two halves that are testable in process: what the
// configuration resolves to, and what the attach path writes for it. The
// client's per frame latch is the same rule one level up, and its starting
// value is the comment at `client.rs` `last_cursor_style`.

/// The cursor options are read from the process environment, not from
/// `AppState`, so a test that wants to know what psmux falls back to has to
/// remove them first and put them back afterwards. Same guard, same shared
/// lock as `test_option_default_parity.rs`, which explains at length why a
/// test that reads an environment variable it never set is measuring the
/// developer's shell.
struct CursorEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<String>)>,
}

impl CursorEnv {
    fn take() -> Self {
        let lock = crate::util::lock_test_env();
        let saved = ["PSMUX_CURSOR_STYLE", "PSMUX_CURSOR_BLINK"]
            .into_iter()
            .map(|name| {
                let previous = std::env::var(name).ok();
                std::env::remove_var(name);
                (name, previous)
            })
            .collect();
        Self { _lock: lock, saved }
    }

    fn set(&self, style: Option<&str>, blink: Option<&str>) {
        match style {
            Some(v) => std::env::set_var("PSMUX_CURSOR_STYLE", v),
            None => std::env::remove_var("PSMUX_CURSOR_STYLE"),
        }
        match blink {
            Some(v) => std::env::set_var("PSMUX_CURSOR_BLINK", v),
            None => std::env::remove_var("PSMUX_CURSOR_BLINK"),
        }
    }
}

impl Drop for CursorEnv {
    fn drop(&mut self) {
        for (name, previous) in &self.saved {
            match previous {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

fn attach_bytes() -> Vec<u8> {
    let mut sink: Vec<u8> = Vec::new();
    crate::rendering::apply_cursor_style(&mut sink).expect("writing to a Vec cannot fail");
    sink
}

#[test]
fn nothing_configured_resolves_to_no_opinion() {
    let env = CursorEnv::take();
    env.set(None, None);
    assert_eq!(
        crate::rendering::configured_cursor_code(),
        0,
        "with nothing set, psmux must have no opinion about the cursor shape"
    );
}

#[test]
fn the_three_shapes_resolve_to_their_decscusr_codes() {
    let env = CursorEnv::take();
    // DECSCUSR: 1/2 block, 3/4 underline, 5/6 bar, odd blinking, even steady.
    for (style, blink, code) in [
        ("block", "1", 1u8),
        ("block", "0", 2),
        ("underline", "1", 3),
        ("underline", "0", 4),
        ("bar", "1", 5),
        ("bar", "0", 6),
        // `beam` is an accepted spelling of `bar`.
        ("beam", "1", 5),
    ] {
        env.set(Some(style), Some(blink));
        assert_eq!(
            crate::rendering::configured_cursor_code(),
            code,
            "style {} with blink {}",
            style,
            blink
        );
    }
}

/// tmux has no `cursor-blink`: its `cursor-style` carries the blink in the
/// value (`blinking-block`, `blinking-underline`, `blinking-bar`,
/// options-table.c:62-65 and tmux.1). A config written for tmux has to name a
/// shape psmux understands, so the prefix is accepted and names the shape.
#[test]
fn the_tmux_spellings_name_the_same_three_shapes() {
    let env = CursorEnv::take();
    for (style, plain, code) in [
        ("blinking-block", "block", 1u8),
        ("blinking-underline", "underline", 3),
        ("blinking-bar", "bar", 5),
    ] {
        env.set(Some(style), Some("1"));
        assert_eq!(crate::rendering::configured_cursor_code(), code, "{}", style);
        env.set(Some(plain), Some("1"));
        assert_eq!(crate::rendering::configured_cursor_code(), code, "{}", plain);
    }
}

/// A value that names the blink blinks on its own. `cursor-blink` is a psmux
/// option a tmux config does not carry, so `cursor-style blinking-bar` by
/// itself has to mean a blinking bar, not whatever that option defaults to.
#[test]
fn a_value_that_names_the_blink_blinks_with_the_option_unset() {
    let env = CursorEnv::take();
    for (style, code) in [
        ("blinking-block", 1u8),
        ("blinking-underline", 3),
        ("blinking-bar", 5),
    ] {
        env.set(Some(style), None);
        assert_eq!(
            crate::rendering::configured_cursor_code(),
            code,
            "{} with cursor-blink unset must blink",
            style
        );
    }
}

/// Set, `cursor-blink` decides: it is the setting that speaks about nothing
/// but blinking, so it wins over the blink in the value's name.
#[test]
fn the_blink_option_wins_over_the_blink_in_the_value() {
    let env = CursorEnv::take();
    for (style, steady) in [
        ("blinking-block", 2u8),
        ("blinking-underline", 4),
        ("blinking-bar", 6),
    ] {
        env.set(Some(style), Some("0"));
        assert_eq!(
            crate::rendering::configured_cursor_code(),
            steady,
            "{} with cursor-blink off must not blink",
            style
        );
    }
}

/// INCOMPATIBLE with psmux before this change, and the reason it is here:
/// `cursor-blink` defaulted to `on`, so a bare `block` blinked. tmux means the
/// STEADY block by that word (DECSCUSR 2) and spells the blinking one
/// `blinking-block`, so the default is now `off` and the three bare shapes
/// mean what tmux means. A config that wants the old cursor adds
/// `set -g cursor-blink on`.
#[test]
fn a_bare_shape_is_steady_with_the_option_unset() {
    let env = CursorEnv::take();
    for (style, steady) in [("block", 2u8), ("underline", 4), ("bar", 6)] {
        env.set(Some(style), None);
        assert_eq!(
            crate::rendering::configured_cursor_code(),
            steady,
            "a bare {} must be the steady shape tmux means by the word",
            style
        );
    }
}

/// The option takes words, the variable it is stored in takes digits, and the
/// two have to agree. `set -g cursor-blink off` writes `0`, but a shell can
/// export the word itself, and `PSMUX_CURSOR_BLINK=off` must not come out
/// meaning blinking just because it is not the literal `0`.
#[test]
fn the_variable_is_read_the_way_the_option_value_is_parsed() {
    let env = CursorEnv::take();
    for yes in ["1", "on", "true"] {
        env.set(Some("bar"), Some(yes));
        assert_eq!(crate::rendering::configured_cursor_code(), 5, "blink [{}]", yes);
    }
    for no in ["0", "off", "false", "no", ""] {
        env.set(Some("bar"), Some(no));
        assert_eq!(crate::rendering::configured_cursor_code(), 6, "blink [{}]", no);
    }
    // Unset is its own answer, not a value, which is what lets a bare shape
    // be steady while `blinking-bar` blinks.
    env.set(Some("bar"), None);
    assert_eq!(crate::rendering::cursor_blink_option(), None);
    env.set(Some("bar"), Some("0"));
    assert_eq!(crate::rendering::cursor_blink_option(), Some(false));
    env.set(Some("bar"), Some("on"));
    assert_eq!(crate::rendering::cursor_blink_option(), Some(true));
}

/// The fallback for an unset `cursor-blink` is the option's own default, so
/// the two must not drift apart.
#[test]
fn the_blink_fallback_is_the_catalog_default() {
    let catalog = crate::server::option_catalog::default_for("cursor-blink")
        .expect("cursor-blink must be in the catalog");
    assert_eq!(
        crate::rendering::CURSOR_BLINK_DEFAULT_BLINKS,
        catalog == "on",
        "the catalog says cursor-blink defaults to [{}]",
        catalog
    );
}

#[test]
fn default_and_anything_unrecognised_resolve_to_no_opinion() {
    let env = CursorEnv::take();
    for style in ["default", "wobble", "", "blinking-wobble"] {
        // The blink value must not rescue a style that names no shape: there
        // is nothing to blink.
        for blink in ["1", "0"] {
            env.set(Some(style), Some(blink));
            assert_eq!(
                crate::rendering::configured_cursor_code(),
                0,
                "style {} with blink {}",
                style,
                blink
            );
        }
    }
}

#[test]
fn the_attach_writes_nothing_when_nothing_asked_for_a_shape() {
    let env = CursorEnv::take();
    env.set(None, None);
    assert!(
        attach_bytes().is_empty(),
        "attaching must not touch the cursor the terminal was configured with"
    );
    env.set(Some("default"), None);
    assert!(
        attach_bytes().is_empty(),
        "`cursor-style default` is the same state spelled out"
    );
}

/// The second reported defect: `cursor-blink on` with no `cursor-style` did
/// nothing at all. A shape carries its blink in the DECSCUSR code, and with no
/// shape there is no code to carry it, so the blink goes out on its own as DEC
/// private mode 12. Only when it was asked for: an unset `cursor-blink` still
/// writes nothing.
#[test]
fn the_blink_goes_out_on_its_own_when_no_shape_is_named() {
    let env = CursorEnv::take();
    env.set(None, Some("on"));
    assert_eq!(crate::rendering::blink_only_mode(), Some(true));
    assert_eq!(attach_bytes(), b"\x1b[?12h");

    env.set(Some("default"), Some("off"));
    assert_eq!(crate::rendering::blink_only_mode(), Some(false));
    assert_eq!(attach_bytes(), b"\x1b[?12l");

    env.set(None, None);
    assert_eq!(crate::rendering::blink_only_mode(), None);
    assert!(attach_bytes().is_empty());
}

/// A shape says everything about itself, so the blink must not go out twice.
#[test]
fn a_named_shape_carries_its_own_blink_and_sends_no_mode_12() {
    let env = CursorEnv::take();
    for (style, blink, code) in [
        ("bar", "on", b"\x1b[5 q".as_slice()),
        ("bar", "off", b"\x1b[6 q".as_slice()),
        ("blinking-block", "on", b"\x1b[1 q".as_slice()),
    ] {
        env.set(Some(style), Some(blink));
        assert_eq!(
            crate::rendering::blink_only_mode(),
            None,
            "{} names a shape, so the blink rides in its code",
            style
        );
        assert_eq!(attach_bytes(), code, "{} with blink {}", style, blink);
    }
}

/// The third reported defect: a blink this client set stayed set after it
/// exited. DECSCUSR 0 puts the SHAPE back and leaves mode 12 alone, measured
/// on Windows Terminal, so the blink has to be put back as mode 12, and mode
/// 12 has only its two states, no "back to the default". The state it had is
/// read at attach with DECRQM, which Windows Terminal answers:
/// `\x1b[?12$p` came back as `\x1b[?12;1$y`.
#[test]
fn the_terminal_is_asked_what_the_blink_was() {
    assert_eq!(
        crate::platform::parse_decrqm_blink(b"\x1b[?12;1$y"),
        Some(true),
        "1 is set"
    );
    assert_eq!(crate::platform::parse_decrqm_blink(b"\x1b[?12;3$y"), Some(true), "3 is permanently set");
    assert_eq!(crate::platform::parse_decrqm_blink(b"\x1b[?12;2$y"), Some(false), "2 is reset");
    assert_eq!(crate::platform::parse_decrqm_blink(b"\x1b[?12;4$y"), Some(false), "4 is permanently reset");
    // 0 is "the terminal does not know this mode", which is not an answer.
    assert_eq!(crate::platform::parse_decrqm_blink(b"\x1b[?12;0$y"), None);
    // The reply arrives in the same drain as the colour reports and the DA1,
    // so it has to be found among them, and a reply for another mode is not
    // this one.
    assert_eq!(
        crate::platform::parse_decrqm_blink(b"\x1b]11;rgb:0c0c/0c0c/0c0c\x1b\x1b[?25;1$y\x1b[?12;2$y\x1b[?61;6c"),
        Some(false)
    );
    assert_eq!(crate::platform::parse_decrqm_blink(b"\x1b[?25;1$y"), None, "mode 25 is not mode 12");
    assert_eq!(crate::platform::parse_decrqm_blink(b""), None, "no reply at all");
}

/// The shape is put back the same way, and for the same reason: `ESC [ 0 q`
/// goes to the TERMINAL's default, not to the cursor the user had. tmux stops
/// there (`tty_stop_tty`, tty.c, sends `Se`, which the cstyle feature defines
/// as `ESC [ 2 q`, or `Ss 0`), and Windows Terminal will say what it is
/// drawing, so psmux puts that back instead.
#[test]
fn the_shape_is_put_back_to_what_the_terminal_was_drawing() {
    let _env = CursorEnv::take();

    crate::rendering::forget_cursor_state_for_test();
    assert_eq!(crate::rendering::shape_restore(), None, "nothing asserted, nothing to put back");

    // The terminal was drawing a steady underline before psmux started.
    crate::rendering::forget_cursor_state_for_test();
    crate::rendering::note_host_shape_before(Some(4));
    crate::rendering::note_cursor_code(5);
    assert_eq!(crate::rendering::shape_restore(), Some(4));

    // 0 is a real answer, the terminal's own default, and most terminals give
    // it: it is not the same as having said nothing.
    crate::rendering::forget_cursor_state_for_test();
    crate::rendering::note_host_shape_before(Some(0));
    crate::rendering::note_cursor_code(5);
    assert_eq!(crate::rendering::shape_restore(), Some(0));

    // A terminal that would not say: fall back to the reset, which is where
    // tmux stops.
    crate::rendering::forget_cursor_state_for_test();
    crate::rendering::note_host_shape_before(None);
    crate::rendering::note_cursor_code(5);
    assert_eq!(crate::rendering::shape_restore(), Some(0));

    // The debt outlives the shape here too: the frame loop writes `ESC [ 0 q`
    // when a pane lets its shape go, which has already left the terminal on
    // its default rather than on what the user had.
    crate::rendering::forget_cursor_state_for_test();
    crate::rendering::note_host_shape_before(Some(4));
    crate::rendering::note_cursor_code(5);
    crate::rendering::note_cursor_code(0);
    assert_eq!(crate::rendering::shape_restore(), Some(4));

    crate::rendering::forget_cursor_state_for_test();
}

/// The shape reply, like the blink one, arrives in the same drain as the
/// colour reports.
#[test]
fn the_terminal_is_asked_what_the_shape_was() {
    assert_eq!(crate::platform::parse_decrqss_cursor_style(b"\x1bP1$r0 q\x1b\\"), Some(0));
    assert_eq!(crate::platform::parse_decrqss_cursor_style(b"\x1bP1$r6 q\x1b\\"), Some(6));
    // An empty Ps is 0, which is what the standard says and what a terminal
    // reporting its default may send.
    assert_eq!(crate::platform::parse_decrqss_cursor_style(b"\x1bP1$r q\x1b\\"), Some(0));
    // A refusal is `DCS 0 $ r ST`, measured from psmux's own emulation, and is
    // not an answer.
    assert_eq!(crate::platform::parse_decrqss_cursor_style(b"\x1bP0$r\x1b\\"), None);
    assert_eq!(crate::platform::parse_decrqss_cursor_style(b""), None);
    // Nothing above DECSCUSR 6 is a cursor style.
    assert_eq!(crate::platform::parse_decrqss_cursor_style(b"\x1bP1$r9 q\x1b\\"), None);
    // Among its neighbours in one drain.
    assert_eq!(
        crate::platform::parse_decrqss_cursor_style(
            b"\x1b[?12;2$y\x1bP1$r6 q\x1b\\\x1b[?61;6c"),
        Some(6)
    );
}

/// Teardown puts back what the terminal said it had, and when the terminal
/// said nothing, the opposite of what was asserted, which undoes the change
/// whenever there was one.
#[test]
fn the_blink_is_put_back_to_what_the_terminal_had() {
    let _env = CursorEnv::take();
    crate::rendering::forget_cursor_state_for_test();
    assert_eq!(crate::rendering::blink_restore(), None, "nothing asserted, nothing to put back");

    crate::rendering::forget_cursor_state_for_test();
    crate::rendering::note_host_blink_before(Some(false));
    crate::rendering::note_blink_asserted(true);
    assert_eq!(crate::rendering::blink_restore(), Some(false), "it was off, so it goes back off");

    crate::rendering::forget_cursor_state_for_test();
    crate::rendering::note_host_blink_before(Some(true));
    crate::rendering::note_blink_asserted(false);
    assert_eq!(crate::rendering::blink_restore(), Some(true), "it was on, so it goes back on");

    // A terminal that answered nothing: undo our own change by its opposite.
    crate::rendering::forget_cursor_state_for_test();
    crate::rendering::note_blink_asserted(true);
    assert_eq!(crate::rendering::blink_restore(), Some(false));
    crate::rendering::forget_cursor_state_for_test();
    crate::rendering::note_blink_asserted(false);
    assert_eq!(crate::rendering::blink_restore(), Some(true));

    crate::rendering::forget_cursor_state_for_test();
}

/// The reported defect: `cursor-style bar` with the blinking off, and after
/// psmux exited the cursor blinked. A shape is a DECSCUSR code that says
/// whether it blinks, and the reset that takes it back, `ESC [ 0 q`, returns
/// the terminal to ITS default, which on Windows Terminal blinks. So a shape
/// owes the blink a restore just as mode 12 does, even once the shape itself
/// has been let go of.
#[test]
fn a_shape_puts_the_blink_back_as_well_as_itself() {
    let _env = CursorEnv::take();

    crate::rendering::forget_cursor_state_for_test();
    crate::rendering::note_host_blink_before(Some(false));
    crate::rendering::note_cursor_code(6); // a steady bar
    assert_eq!(
        crate::rendering::blink_restore(),
        Some(false),
        "the blinking was off before, so it goes back off"
    );

    // Letting the shape go writes `ESC [ 0 q` from the frame loop, which moves
    // the blinking again, so the debt outlives the shape.
    crate::rendering::note_cursor_code(0);
    assert_eq!(
        crate::rendering::blink_restore(),
        Some(false),
        "the blinking still has to be put back"
    );

    // A terminal that never answered DECRQM: a shape alone leaves nothing
    // honest to say about the blink.
    crate::rendering::forget_cursor_state_for_test();
    crate::rendering::note_cursor_code(6);
    assert_eq!(crate::rendering::blink_restore(), None);

    crate::rendering::forget_cursor_state_for_test();
}

/// The reported defect: attaching turned the cursor's blinking on whatever
/// the terminal was set to, because the attach sequence carried crossterm's
/// `EnableBlinking`, which is DEC private mode 12. Leaving the cursor alone
/// has to mean the blink as well as the shape, so no mode 12 is written at
/// attach, none at teardown, and the shape reset on the way out goes only to
/// undo a shape this client asserted (`tty_stop_tty`, tty.c).
#[test]
fn an_attach_that_asserts_nothing_owes_the_terminal_no_reset() {
    let env = CursorEnv::take();
    crate::rendering::forget_cursor_state_for_test();
    env.set(None, None);
    assert!(attach_bytes().is_empty());
    assert_eq!(
        crate::rendering::shape_restore(),
        None,
        "nothing was asserted, so teardown must write no reset"
    );
    assert_eq!(crate::rendering::blink_restore(), None);
}

#[test]
fn an_attach_that_asserts_a_shape_owes_the_terminal_a_reset() {
    let env = CursorEnv::take();
    crate::rendering::forget_cursor_state_for_test();
    env.set(Some("block"), Some("0"));
    assert_eq!(attach_bytes(), b"\x1b[2 q");
    assert_eq!(
        crate::rendering::shape_restore(),
        Some(0),
        "a shape was asserted and the terminal said nothing, so teardown falls          back to the reset tmux sends"
    );
}

#[test]
fn the_attach_writes_the_shape_that_was_asked_for() {
    let env = CursorEnv::take();
    env.set(Some("block"), Some("0"));
    assert_eq!(attach_bytes(), b"\x1b[2 q", "steady block is DECSCUSR 2");
    env.set(Some("bar"), Some("1"));
    assert_eq!(attach_bytes(), b"\x1b[5 q", "blinking bar is DECSCUSR 5");
}

#[test]
fn the_catalog_default_is_the_state_that_asserts_nothing() {
    let env = CursorEnv::take();
    env.set(
        Some(
            crate::server::option_catalog::default_for("cursor-style")
                .expect("cursor-style must be in the catalog"),
        ),
        None,
    );
    assert_eq!(
        crate::rendering::configured_cursor_code(),
        0,
        "the catalog default must be the value that leaves the terminal alone"
    );
}

/// The overlay the user reads (Prefix + ?) prints its own table of defaults,
/// and it drifted: it said `cursor-style` defaulted to the empty string and
/// `cursor-blink` to `off`, while the catalog said `bar` and `on`.
///
/// This is not a sweep of the whole table. Nine more of its entries disagree
/// with the catalog today: `pane-border-style`, `set-titles-string`,
/// `status-left-style`, `status-right-style`, `window-status-style`,
/// `window-status-current-style` and `window-status-last-style` print an empty
/// default where the catalog has a real one, and `main-pane-width` and
/// `main-pane-height` print prose (`0 (60% heuristic)`) where it has `0`. Each
/// needs its own decision, so this pins the two options the issue is about.
#[test]
fn the_help_overlay_prints_the_catalog_defaults_for_the_cursor_options() {
    let lines = crate::help::options_lines();
    for name in ["cursor-style", "cursor-blink"] {
        let catalog = crate::server::option_catalog::default_for(name)
            .unwrap_or_else(|| panic!("{} must be in the catalog", name));
        let line = lines
            .iter()
            .find(|l| l.trim_start().starts_with(name))
            .unwrap_or_else(|| panic!("the overlay must list {}", name));
        let printed = line.trim_start()[name.len()..].trim();
        assert_eq!(
            printed, catalog,
            "the overlay prints [{}] as the default of {}, the catalog says [{}]",
            printed, name, catalog
        );
    }
}
