//! Ground truth for what a console delivers when a terminal sends an extended
//! key (`CSI 13;2u`, the Shift+Enter of `set -s extended-keys`).
//!
//! `key_diag` shows what crossterm MADE of the input; this shows what arrived,
//! one `INPUT_RECORD` at a time, with the fields crossterm decides on:
//! `wVirtualKeyCode`, `UnicodeChar` and `dwControlKeyState`.  The distinction
//! matters because crossterm can drop a record entirely — a record whose
//! virtual key code is not one it names and whose `UnicodeChar` is a control
//! code goes to `ToUnicodeEx`, which answers nothing for a synthesised record,
//! and the event is discarded (crossterm 0.29 `event/sys/windows/parse.rs`).
//!
//! Run it in the terminal under test, NOT inside psmux, then press the key:
//!
//! ```text
//!   cargo run --example csi_u_diag
//! ```
//!
//! Press `q` (or Ctrl+C) to quit.  Lines also go to `$TEMP/psmux_csi_u_diag.log`.
#![cfg(windows)]

use std::ffi::c_void;
use std::io::Write;
use std::path::PathBuf;

#[repr(C)]
#[derive(Copy, Clone)]
struct KeyEventRecord {
    key_down: i32,
    repeat_count: u16,
    virtual_key_code: u16,
    virtual_scan_code: u16,
    u_char: u16,
    control_key_state: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct InputRecord {
    event_type: u16,
    _pad: u16,
    data: [u8; 16],
}

#[link(name = "kernel32")]
extern "system" {
    fn GetStdHandle(n: u32) -> *mut c_void;
    fn GetConsoleMode(h: *mut c_void, mode: *mut u32) -> i32;
    fn SetConsoleMode(h: *mut c_void, mode: u32) -> i32;
    fn ReadConsoleInputW(h: *mut c_void, buf: *mut c_void, len: u32, read: *mut u32) -> i32;
}

const STD_INPUT_HANDLE: u32 = -10i32 as u32;
const KEY_EVENT: u16 = 0x0001;
const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
const ENABLE_LINE_INPUT: u32 = 0x0002;
const ENABLE_ECHO_INPUT: u32 = 0x0004;
const ENABLE_WINDOW_INPUT: u32 = 0x0008;
const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

fn log_path() -> PathBuf {
    let dir = std::env::var("TEMP")
        .or_else(|_| std::env::var("TMP"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(dir).join("psmux_csi_u_diag.log")
}

fn emit(line: &str) {
    print!("{}\r\n", line);
    let _ = std::io::stdout().flush();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = writeln!(f, "{}", line);
    }
}

/// How crossterm 0.29 resolves this record, so the log says outright whether
/// the key survives the trip.  Mirrors `parse_key_event_record`: the named
/// virtual key codes win, and everything else falls back to `UnicodeChar` —
/// where a control code is handed to `ToUnicodeEx` and, for a record conhost
/// synthesised while flushing a sequence it did not recognise, comes back
/// empty.
fn crossterm_verdict(k: &KeyEventRecord) -> &'static str {
    match k.virtual_key_code {
        0x10..=0x12 => "dropped (bare modifier)",
        0x08 => "KeyCode::Backspace",
        0x1b => "KeyCode::Esc",
        0x0d => "KeyCode::Enter",
        0x09 => "KeyCode::Tab",
        0x25..=0x28 => "an arrow KeyCode",
        _ => match k.u_char {
            0x00..=0x1f => "ToUnicodeEx fallback -> DROPPED unless the layout answers",
            _ => "KeyCode::Char(u_char)",
        },
    }
}

fn main() {
    let _ = std::fs::write(log_path(), "");
    let h = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let mut original: u32 = 0;
    if unsafe { GetConsoleMode(h, &mut original) } == 0 {
        eprintln!("stdin is not a console");
        return;
    }
    // Raw, and with VTI on: the mode psmux's client runs its console in.
    let raw = (original & !(ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT))
        | ENABLE_WINDOW_INPUT
        | ENABLE_VIRTUAL_TERMINAL_INPUT;
    unsafe { SetConsoleMode(h, raw) };

    emit(&format!(
        "=== csi_u_diag: console mode {:#06x} -> {:#06x} (VTI {}), log {:?} ===",
        original,
        raw,
        if raw & ENABLE_VIRTUAL_TERMINAL_INPUT != 0 { "on" } else { "off" },
        log_path()
    ));
    emit("Press the key under test (Shift+Enter). Press q to quit.");

    let mut buf = [InputRecord { event_type: 0, _pad: 0, data: [0u8; 16] }; 32];
    loop {
        let mut read: u32 = 0;
        let ok = unsafe {
            ReadConsoleInputW(h, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut read)
        };
        if ok == 0 {
            emit("ReadConsoleInputW failed");
            break;
        }
        let mut quit = false;
        emit(&format!("--- batch of {} record(s) ---", read));
        for rec in buf.iter().take(read as usize) {
            if rec.event_type != KEY_EVENT {
                emit(&format!("  other event type {:#06x}", rec.event_type));
                continue;
            }
            let k: KeyEventRecord = unsafe { std::ptr::read(rec.data.as_ptr() as *const _) };
            let ch = char::from_u32(k.u_char as u32)
                .filter(|c| !c.is_control())
                .map(|c| format!("{:?}", c))
                .unwrap_or_else(|| "-".into());
            emit(&format!(
                "  {} vk={:#04x} scan={:#04x} uChar={:#06x} {:>4} ctrl={:#06x}  crossterm: {}",
                if k.key_down != 0 { "DOWN" } else { "UP  " },
                k.virtual_key_code,
                k.virtual_scan_code,
                k.u_char,
                ch,
                k.control_key_state,
                crossterm_verdict(&k),
            ));
            if k.key_down != 0 && (k.u_char == b'q' as u16 || k.u_char == 0x03) {
                quit = true;
            }
        }
        if quit {
            break;
        }
    }

    unsafe { SetConsoleMode(h, original) };
    emit("=== csi_u_diag done ===");
}
