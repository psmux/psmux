#![allow(dead_code)]
//! This crate parses a terminal byte stream and provides an in-memory
//! representation of the rendered contents.
//!
//! # Overview
//!
//! This is essentially the terminal parser component of a graphical terminal
//! emulator pulled out into a separate crate. Although you can use this crate
//! to build a graphical terminal emulator, it also contains functionality
//! necessary for implementing terminal applications that want to run other
//! terminal applications - programs like `screen` or `tmux` for example.
//!
//! # Synopsis
//!
//! ```
//! # use vt100_psmux as vt100;
//! let mut parser = vt100::Parser::new(24, 80, 0);
//!
//! let screen = parser.screen().clone();
//! parser.process(b"this text is \x1b[31mRED\x1b[m");
//! assert_eq!(
//!     parser.screen().cell(0, 13).unwrap().fgcolor(),
//!     vt100::Color::Idx(1),
//! );
//!
//! let screen = parser.screen().clone();
//! parser.process(b"\x1b[3D\x1b[32mGREEN");
//! assert_eq!(
//!     parser.screen().contents_formatted(),
//!     &b"\x1b[?25h\x1b[m\x1b[H\x1b[Jthis text is \x1b[32mGREEN"[..],
//! );
//! assert_eq!(
//!     parser.screen().contents_diff(&screen),
//!     &b"\x1b[1;14H\x1b[32mGREEN"[..],
//! );
//! ```

#![warn(missing_docs)]
#![warn(clippy::cargo)]
#![warn(clippy::pedantic)]
#![warn(clippy::nursery)]
#![warn(clippy::as_conversions)]
#![warn(clippy::get_unwrap)]
#![allow(clippy::cognitive_complexity)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::similar_names)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::type_complexity)]

#[cfg(test)]
extern crate self as vt100;

mod attrs;
mod callbacks;
mod cell;
mod grid;
mod palette;
mod parser;
mod perform;
mod program_status;
mod queries;
mod row;
mod screen;
mod term;
pub mod width;

pub use attrs::{Color, UnderlineStyle};
pub use callbacks::Callbacks;
pub use cell::Cell;
pub use palette::{parse_palette_index, parse_x11_colour, ColourPalette};
pub use parser::Parser;
pub use queries::QueryDetector;
pub use program_status::{ProgramKind, ProgramRecord, ProgramState, ProgramStatus, ProgramStatuses, TerminalQuery, PROGRAM_STATUS_REPLY};
pub use screen::{extended_keys_option, set_extended_keys_option, xtmodkeys_off_mode, xtmodkeys_set_mode, MouseProtocolEncoding, MouseProtocolMode, Screen};
pub use width::{
    char_width, clear_codepoint_widths, has_overrides, parse_entry as parse_codepoint_width_entry,
    set_codepoint_widths, str_cells, str_width, WidthOverride, MAX_OVERRIDE_WIDTH,
};
