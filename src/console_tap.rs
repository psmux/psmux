//! Console input records crossterm 0.29 loses or misreads, taken before
//! crossterm reads them (issue #742).
//!
//! On the console input route (Windows Terminal, conhost, every host where
//! `needs_vt_input()` is false) the client reads its input through crossterm,
//! which calls `ReadConsoleInputW` and parses each `KEY_EVENT` record. Four kinds
//! of record do not come out of that parser as the client needs them. The first
//! two never come out at all:
//!
//! 1. Characters outside the Basic Multilingual Plane. The console hands one
//!    over as a key down AND a key up record per UTF-16 code unit, all with
//!    `vk = 0`. For U+1F60A that is `down D83D, up D83D, down DE0A, up DE0A`,
//!    whether it was pasted, committed by an IME or picked from the Win+.
//!    emoji panel. crossterm's `handle_surrogate` pairs consecutive surrogate
//!    values without looking at `bKeyDown`, so it pairs (high, high) and then
//!    (low, low), both fail to decode, and all four halves vanish. Upstream
//!    issues #787 #848 #1008 #1072, PRs #857 #1009 #1073, none merged, and
//!    0.29.0 is still the newest release.
//! 2. Control characters delivered as `vk = 0` with the character in
//!    `uChar`. crossterm sends those to `ToUnicodeEx(vk = 0)`, which has no
//!    character for them, so the key was dropped. This half is defensive: no
//!    terminal measured types a control key in this shape. A ConPTY host that
//!    writes bytes (mintty through the Cygwin pseudo console, winpty, sshd)
//!    gets a real virtual key from conhost (`VkKeyScanW` maps every C0 byte
//!    in every layout tried: 0x02 is Ctrl+B, 0x0A is Ctrl+Enter), and so does
//!    a conhost paste. What does produce it: WM_CHAR posted to a console
//!    window with no key behind it, a win32-input-mode report with Vk=0, and
//!    WriteConsoleInput. Those were dropped before and arrive now; nothing
//!    that carried a virtual key passes through here. A SendInput
//!    KEYEVENTF_UNICODE control character arrives as vk = VK_PACKET (0xE7),
//!    which is not this shape and is still crossterm's.
//! 3. An Alt code character (issue #766): Alt up carrying the character in
//!    `uChar`. crossterm reports a Release, which the client does not forward,
//!    so the character vanished. It is decoded here as a key press.
//! 4. Not lost, but misread (a normalization, not a repair of a loss): a key
//!    record with the AltGr signature (LEFT_CTRL and RIGHT_ALT held, LEFT_ALT
//!    not held) and a lowercase ASCII letter in `uChar`. The console reports
//!    AltGr that way (Turkish Q `i`, and every `i` of a paste on that layout).
//!    crossterm hands it on as Char with CONTROL | ALT, which the client reads
//!    as a Ctrl+Alt chord. It is decoded here as the plain letter, with no
//!    modifiers. This is a policy on the console route: such a record is
//!    treated as text, and any other Ctrl+Alt shape stays a chord.
//! 5. Misread on a non Latin layout: Ctrl (no Alt) on a letter key whose
//!    uChar is non ASCII, or a control character the active layout maps to a
//!    non ASCII letter. crossterm turns it into that letter (Ctrl+V on Russian
//!    is Ctrl+м), which has no control code, so the key was lost. It is decoded
//!    here as Ctrl plus the Latin letter the virtual key names. A Latin layout
//!    stays crossterm's.
//!
//! The seam: every read on the console route goes through
//! `crossterm::event::poll(Duration::ZERO)`, and with a zero timeout crossterm
//! reads at most ONE record per call. So before each such poll, psmux peeks at
//! the head of the console input buffer. When the head is one of the records
//! above, psmux consumes it itself and decodes it the way its own VT route
//! reader does (`ssh_input.rs`: key up records are skipped before pairing, and
//! control bytes map to the same keys; the AltGr letter of item 4 becomes the
//! plain letter). Every other record (ordinary keys, modifiers, mouse, resize,
//! focus, the win32 input mode keys, Ctrl+Alt records without the AltGr
//! signature or without a lowercase ASCII letter in `uChar`) stays in the
//! buffer for crossterm to read as before.
//!
//! The decision logic is [`ConsoleTap::classify`], pure and unit tested; the
//! Win32 peek and read live in [`take_head`].

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

/// The fields of a `KEY_EVENT_RECORD` the tap decides on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeyRec {
    pub key_down: bool,
    pub vk: u16,
    pub u_char: u16,
    pub ctrl_state: u32,
}

/// What to do with the record at the head of the console input buffer.
#[derive(Debug, PartialEq)]
pub(crate) enum Verdict {
    /// Leave it for crossterm.
    NotOurs,
    /// Consume it; it completes this event, or nothing yet.
    Consumed(Option<Event>),
}

const VK_MENU: u16 = 0x12;
const ESC: u16 = 0x1B;

// dwControlKeyState bits.
const RIGHT_ALT_PRESSED: u32 = 0x0001;
const LEFT_ALT_PRESSED: u32 = 0x0002;
const RIGHT_CTRL_PRESSED: u32 = 0x0004;
const LEFT_CTRL_PRESSED: u32 = 0x0008;
const SHIFT_PRESSED: u32 = 0x0010;
const CAPSLOCK_ON: u32 = 0x0080;

/// Surrogate pairing state carried between records and between reads, so a
/// pair split across two `ReadConsoleInputW` batches still decodes.
#[derive(Default, Debug)]
pub(crate) struct ConsoleTap {
    high: Option<u16>,
}

fn press(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::empty(),
    })
}

/// The same modifier mapping crossterm applies to a character record, so a
/// non BMP character carries what a BMP one in the same position would.
fn modifiers(ctrl_state: u32) -> KeyModifiers {
    let mut m = KeyModifiers::empty();
    if ctrl_state & SHIFT_PRESSED != 0 { m |= KeyModifiers::SHIFT; }
    if ctrl_state & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0 { m |= KeyModifiers::CONTROL; }
    if ctrl_state & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0 { m |= KeyModifiers::ALT; }
    m
}

/// A control character as the key it stands for, matching the VT route's
/// ground state (`ssh_input.rs`): CR is Enter, HT is Tab, 0x01 to 0x1A are
/// C-a to C-z (so 0x08 is C-h and 0x0A is C-j, as tmux reads them), and 0x1C
/// to 0x1F are C-\ C-] C-^ C-_. NUL never gets here: a vk = 0 record with no
/// character carries nothing to decode.
fn control_key(u: u16) -> Option<Event> {
    Some(match u {
        0x0D => press(KeyCode::Enter, KeyModifiers::empty()),
        0x09 => press(KeyCode::Tab, KeyModifiers::empty()),
        0x01..=0x1A => press(KeyCode::Char((b'a' + (u as u8) - 1) as char), KeyModifiers::CONTROL),
        0x1C => press(KeyCode::Char('\\'), KeyModifiers::CONTROL),
        0x1D => press(KeyCode::Char(']'), KeyModifiers::CONTROL),
        0x1E => press(KeyCode::Char('^'), KeyModifiers::CONTROL),
        0x1F => press(KeyCode::Char('_'), KeyModifiers::CONTROL),
        _ => return None,
    })
}

/// The Alt UP record that completes an Alt code and carries its character
/// (a BMP character above the C0 range; surrogate halves and control
/// characters have their own branches). Shared with the VT route's reader,
/// which skips every other key up record.
pub(crate) fn alt_code_release_char(key_down: bool, vk: u16, u_char: u16) -> bool {
    !key_down && vk == VK_MENU && u_char > 0x1F && !(0xD800..=0xDFFF).contains(&u_char)
}

/// The AltGr signature (LEFT_CTRL and RIGHT_ALT held, LEFT_ALT not held) with
/// a lowercase ASCII letter in uChar: AltGr text (see the branch in
/// [`ConsoleTap::classify`]). Shift and the lock bits are not looked at.
fn altgr_ascii_letter(rec: &KeyRec) -> bool {
    rec.ctrl_state & LEFT_CTRL_PRESSED != 0
        && rec.ctrl_state & RIGHT_ALT_PRESSED != 0
        && rec.ctrl_state & LEFT_ALT_PRESSED == 0
        && (b'a' as u16..=b'z' as u16).contains(&rec.u_char)
}

/// Ctrl held without Alt on a letter key (VK_A to VK_Z) that crossterm would
/// turn into a non ASCII character: a non ASCII uChar, or a control character
/// (or none) the active layout maps to a non ASCII letter (`layout_char` is
/// crossterm's ToUnicodeEx lookup). Gives the press or release of Ctrl plus the
/// Latin letter the virtual key names, cased the way crossterm cases it (Shift
/// XOR Caps Lock). A Latin layout stays crossterm's.
fn ctrl_letter(rec: &KeyRec, layout_char: impl Fn(&KeyRec) -> Option<char>) -> Option<Event> {
    let ctrl = rec.ctrl_state & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0;
    let alt = rec.ctrl_state & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0;
    if !ctrl || alt || !(0x41..=0x5A).contains(&rec.vk) {
        return None;
    }
    let u = rec.u_char;
    let misread = match u {
        0x80.. => true,
        0x00..=0x1F => layout_char(rec).is_some_and(|c| !c.is_ascii()),
        _ => false,
    };
    if !misread {
        return None;
    }
    let upper = (rec.ctrl_state & SHIFT_PRESSED != 0) ^ (rec.ctrl_state & CAPSLOCK_ON != 0);
    let letter = rec.vk as u8 as char;
    let letter = if upper { letter } else { letter.to_ascii_lowercase() };
    Some(Event::Key(KeyEvent {
        code: KeyCode::Char(letter),
        modifiers: modifiers(rec.ctrl_state),
        kind: if rec.key_down { KeyEventKind::Press } else { KeyEventKind::Release },
        state: KeyEventState::empty(),
    }))
}

impl ConsoleTap {
    pub fn new() -> Self { Self::default() }

    /// Decide on the record at the head of the buffer. `None` is a record that
    /// is not a `KEY_EVENT` (mouse, resize, focus, menu): never ours, and it
    /// leaves a pending high surrogate alone. With no layout lookup, every
    /// layout is taken for Latin.
    pub fn classify(&mut self, rec: Option<KeyRec>) -> Verdict {
        self.classify_with(rec, |_| None)
    }

    /// [`Self::classify`] with the active layout's character for a key record
    /// passed in (crossterm's ToUnicodeEx lookup), so the Ctrl+letter rule can
    /// be tested without the layouts of the machine it runs on.
    pub fn classify_with(&mut self, rec: Option<KeyRec>, layout_char: impl Fn(&KeyRec) -> Option<char>) -> Verdict {
        let rec = match rec {
            Some(r) => r,
            None => return Verdict::NotOurs,
        };
        let u = rec.u_char;

        if (0xD800..=0xDFFF).contains(&u) {
            // A key up half is the console's echo of the key down half; the
            // pairing must never see it. The one release that carries a real
            // character is crossterm's Alt code exception (Alt key up with a
            // uChar), honoured here the same way.
            let counts = rec.key_down || rec.vk == VK_MENU;
            if !counts {
                return Verdict::Consumed(None);
            }
            if u <= 0xDBFF {
                // A high half. A second high replaces a first that never got
                // its low, the way decode_utf16_unit does on the VT route.
                self.high = Some(u);
                return Verdict::Consumed(None);
            }
            // A low half: it completes the pending high, or it is an orphan.
            return Verdict::Consumed(self.high.take().and_then(|hi| {
                let cp = 0x10000 + ((hi as u32 - 0xD800) << 10) + (u as u32 - 0xDC00);
                char::from_u32(cp).map(|ch| press(KeyCode::Char(ch), modifiers(rec.ctrl_state)))
            }));
        }

        // A control character with no virtual key.
        //
        // ESC included (issue #860): a Windows Terminal `sendInput` action
        // delivers every character it writes in this shape, so a key mapped
        // to "\u001b" (the usual way to make Ctrl+[ an Escape) reached
        // crossterm as a record it drops, and the key never arrived. It goes
        // out as the Escape key now, and an extended key typed the same way
        // (`ESC [ 32 ; 5 u`) still decodes: the Escape is handed to
        // EscCoalesce, which sees the `[` already queued behind it, holds it,
        // and lets recover_csi_u take the whole sequence with it.
        if rec.vk == 0 && u != 0 && u <= 0x1F {
            if !rec.key_down {
                return Verdict::Consumed(None);
            }
            self.high = None;
            if u == ESC {
                return Verdict::Consumed(Some(press(KeyCode::Esc, KeyModifiers::empty())));
            }
            return Verdict::Consumed(control_key(u));
        }

        // An Alt code character (issue #766). The console delivers a
        // character no key on the layout produces (an em dash written into a
        // pseudoconsole by node-pty, or Alt+0151 typed by hand) as Alt down,
        // numpad digits, then Alt UP carrying the character. crossterm reports
        // that as a Release, and the client forwards only Press and Repeat,
        // so the character vanished: `—` never reached the pane. It is decoded
        // here as the key press it stands for.
        if alt_code_release_char(rec.key_down, rec.vk, u) {
            self.high = None;
            return Verdict::Consumed(
                char::from_u32(u as u32).map(|ch| press(KeyCode::Char(ch), modifiers(rec.ctrl_state))),
            );
        }

        // A lowercase ASCII letter the layout typed with AltGr. Windows reports
        // AltGr as Ctrl+Alt, and the client tells AltGr text from a Ctrl+Alt
        // chord by the character (`is_altgr_char`): anything but a lowercase
        // ASCII letter is AltGr. Turkish Q also types `i` as AltGr+I, and a
        // pasted `i` arrives exactly that way (the console host picks the key
        // with VkKeyScanW), so every `i` of a paste went to the pane as C-M-i,
        // a Tab, and broke the paste in two. The policy: a record with the
        // AltGr signature (LEFT_CTRL | RIGHT_ALT, no LEFT_ALT, as the paste
        // records carry) and a lowercase ASCII letter in uChar is text. Every
        // other Ctrl+Alt shape, left Ctrl + left Alt above all, stays a chord.
        if altgr_ascii_letter(&rec) {
            // The key up goes on as the plain letter's release, not dropped:
            // the paste head rule (PasteHeadEvidence::head_released) reads a
            // host written pair by its release, so a paste opening on `i`
            // would otherwise go out as typing.
            let kind = if rec.key_down { KeyEventKind::Press } else { KeyEventKind::Release };
            if rec.key_down {
                self.high = None;
            }
            return Verdict::Consumed(Some(Event::Key(KeyEvent {
                code: KeyCode::Char(u as u8 as char),
                modifiers: KeyModifiers::empty(),
                kind,
                state: KeyEventState::empty(),
            })));
        }

        // Ctrl + a letter key on a non Latin layout. crossterm maps the
        // control character in uChar back to a letter with ToUnicodeEx on the
        // active layout, so Ctrl+V on Russian or Ukrainian arrives as Ctrl+м,
        // which has no control code and never reaches the pane. The virtual
        // key names the Latin letter in every layout (VK_A is 0x41), so the
        // letter comes from it. On a Latin layout the result is what crossterm
        // gives, and an ASCII letter in uChar (a layout that types it) stays
        // crossterm's.
        if let Some(ev) = ctrl_letter(&rec, layout_char) {
            if rec.key_down {
                self.high = None;
            }
            return Verdict::Consumed(Some(ev));
        }

        // Anything else is crossterm's. A character key arriving between two
        // halves breaks the pair, which is what crossterm itself does with its
        // buffer when any other key event comes first.
        if rec.key_down && u != 0 {
            self.high = None;
        }
        Verdict::NotOurs
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Copy, Clone)]
    pub struct KEY_EVENT_RECORD {
        pub key_down: i32,
        pub repeat_count: u16,
        pub virtual_key_code: u16,
        pub virtual_scan_code: u16,
        pub u_char: u16,
        pub control_key_state: u32,
    }

    #[repr(C)]
    pub struct INPUT_RECORD {
        pub event_type: u16,
        pub _pad: u16,
        pub data: [u8; 16],
    }

    pub const KEY_EVENT: u16 = 0x0001;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn PeekConsoleInputW(h: *mut c_void, buf: *mut c_void, len: u32, read: *mut u32) -> i32;
        // Same signature as the declarations in platform.rs and ssh_input.rs.
        pub fn ReadConsoleInputW(h: *mut c_void, buf: *mut c_void, len: u32, read: *mut u32) -> i32;
    }
}

/// The character the foreground window's layout types on `vk` with no
/// modifier held: the lookup crossterm 0.29 makes for a Ctrl+letter record
/// (`get_char_for_key`), so the tap takes exactly the records crossterm would
/// turn into a non ASCII letter.
#[cfg(windows)]
fn layout_char(vk: u16) -> Option<char> {
    #[link(name = "user32")]
    extern "system" {
        fn GetForegroundWindow() -> isize;
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        fn GetKeyboardLayout(thread: u32) -> isize;
        fn ToUnicodeEx(vk: u32, scan: u32, state: *const u8, buf: *mut u16, len: i32, flags: u32, hkl: isize) -> i32;
    }
    let state = [0u8; 256];
    let mut buf = [0u16; 4];
    // 0x4: do not change the kernel keyboard state (dead keys stay pending).
    let n = unsafe {
        let hwnd = GetForegroundWindow();
        let hkl = GetKeyboardLayout(GetWindowThreadProcessId(hwnd, std::ptr::null_mut()));
        ToUnicodeEx(vk as u32, 0, state.as_ptr(), buf.as_mut_ptr(), buf.len() as i32, 0x4, hkl)
    };
    if n != 1 {
        return None;
    }
    char::from_u32(buf[0] as u32)
}

#[cfg(windows)]
thread_local! {
    static TAP: std::cell::RefCell<ConsoleTap> = std::cell::RefCell::new(ConsoleTap::new());
}

/// Consume the records at the head of the console input buffer that crossterm
/// would lose or misread (the four kinds in the module notes), and return the
/// first event they complete. Returns `None` as soon as the head is a record
/// crossterm should read, or the buffer is empty.
///
/// Costs one `PeekConsoleInputW` when the head is not ours, which is every
/// ordinary keystroke.
#[cfg(windows)]
pub(crate) fn take_head() -> Option<Event> {
    use win::*;
    let h = crate::ssh_input::frame_wake::conin()?;
    TAP.with(|tap| {
        let mut tap = tap.borrow_mut();
        loop {
            let mut rec: INPUT_RECORD = unsafe { std::mem::zeroed() };
            let mut n: u32 = 0;
            let ok = unsafe { PeekConsoleInputW(h, &mut rec as *mut _ as *mut _, 1, &mut n) };
            if ok == 0 || n == 0 {
                return None;
            }
            let key = if rec.event_type == KEY_EVENT {
                let k = unsafe { &*(rec.data.as_ptr() as *const KEY_EVENT_RECORD) };
                Some(KeyRec {
                    key_down: k.key_down != 0,
                    vk: k.virtual_key_code,
                    u_char: k.u_char,
                    ctrl_state: k.control_key_state,
                })
            } else {
                None
            };
            match tap.classify_with(key, |r| layout_char(r.vk)) {
                Verdict::NotOurs => return None,
                Verdict::Consumed(ev) => {
                    // Remove exactly the record just peeked. This thread is the
                    // only reader of the buffer, so the head is unchanged.
                    let mut gone: INPUT_RECORD = unsafe { std::mem::zeroed() };
                    let mut m: u32 = 0;
                    let ok = unsafe { ReadConsoleInputW(h, &mut gone as *mut _ as *mut _, 1, &mut m) };
                    if let Some(k) = key {
                        crate::debug_log::input_log(
                            "tap",
                            &format!(
                                "{} vk=0x{:02X} uChar=0x{:04X} -> {:?}",
                                if k.key_down { "down" } else { "up" }, k.vk, k.u_char, ev
                            ),
                        );
                    }
                    if ok == 0 || m == 0 {
                        return None;
                    }
                    if ev.is_some() {
                        return ev;
                    }
                }
            }
        }
    })
}

#[cfg(test)]
#[path = "../tests-rs/test_issue742_console_tap.rs"]
mod tests_issue742_console_tap;

#[cfg(test)]
#[path = "../tests-rs/test_issue766_alt_code_char.rs"]
mod tests_issue766_alt_code_char;

#[cfg(test)]
#[path = "../tests-rs/test_altgr_letter_console_tap.rs"]
mod tests_altgr_letter_console_tap;

#[cfg(test)]
#[path = "../tests-rs/test_ctrl_letter_non_latin_layout.rs"]
mod tests_ctrl_letter_non_latin_layout;
