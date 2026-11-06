//! The DEC Special Graphics character set (the line drawing set), designated
//! with `ESC ( 0` into G0 or `ESC ) 0` into G1, `ESC ( B` / `ESC ) B` for
//! ASCII again, and chosen with SI (G0) and SO (G1), as tmux reads them
//! (input.c:241-249, :1372-1377, :1442-1453 at tag 3.8).
//!
//! tmux keeps the ASCII letter with `GRID_ATTR_CHARSET` and turns it into a
//! box drawing character only when it draws (tty-acs.c:31-68). psmux turns it
//! into that character when it is printed instead, as the Windows console
//! hosts do: the inbox conhost hands psmux `┌──┐` for `ESC(0 lqqk`, and
//! `OpenConsole` keeps the same characters in its own buffer, so the pane
//! holds what the console host holds on both routes (Discussion #937). The
//! table is the one both hosts use, the DEC set itself: `_` is a blank and
//! `` ` `` is ♦, and `+ , - . 0`, which tmux's table also turns into arrows
//! and a block (ncurses' extensions), stay as they are.

/// Which sets G0 and G1 hold, and which one is in use.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct Charsets {
    /// G0 is the line drawing set (`ESC ( 0`).
    g0: bool,
    /// G1 is the line drawing set (`ESC ) 0`).
    g1: bool,
    /// SO chose G1; SI chooses G0 again.
    shifted: bool,
    /// The set in use is the line drawing set, kept ready so that every
    /// printed character pays for one test of it.
    line_drawing: bool,
}

impl Charsets {
    /// `ESC ( 0` or `ESC ( B` (`g1` false), `ESC ) 0` or `ESC ) B`.
    pub fn designate(&mut self, g1: bool, line_drawing: bool) {
        if g1 {
            self.g1 = line_drawing;
        } else {
            self.g0 = line_drawing;
        }
        self.settle();
    }

    /// SO (`shifted`) or SI.
    pub fn shift(&mut self, shifted: bool) {
        self.shifted = shifted;
        self.settle();
    }

    fn settle(&mut self) {
        self.line_drawing = if self.shifted { self.g1 } else { self.g0 };
    }

    /// The character `c` stands for in the set in use.
    #[inline]
    pub fn map(self, c: char) -> char {
        if self.line_drawing {
            line_drawing_char(c)
        } else {
            c
        }
    }
}

/// `c` in the DEC Special Graphics set: 0x5F to 0x7E change, anything else
/// stays (measured on the inbox conhost and `OpenConsole` on Windows 11 26300).
#[cold]
#[inline(never)]
fn line_drawing_char(c: char) -> char {
    match c {
        '_' => ' ',
        '`' => '\u{2666}', // diamond
        'a' => '\u{2592}', // checker board
        'b' => '\u{2409}', // HT
        'c' => '\u{240C}', // FF
        'd' => '\u{240D}', // CR
        'e' => '\u{240A}', // LF
        'f' => '\u{00B0}', // degree
        'g' => '\u{00B1}', // plus or minus
        'h' => '\u{2424}', // NL
        'i' => '\u{240B}', // VT
        'j' => '\u{2518}', // lower right corner
        'k' => '\u{2510}', // upper right corner
        'l' => '\u{250C}', // upper left corner
        'm' => '\u{2514}', // lower left corner
        'n' => '\u{253C}', // crossing lines
        'o' => '\u{23BA}', // scan line 1
        'p' => '\u{23BB}', // scan line 3
        'q' => '\u{2500}', // horizontal line
        'r' => '\u{23BC}', // scan line 7
        's' => '\u{23BD}', // scan line 9
        't' => '\u{251C}', // tee pointing right
        'u' => '\u{2524}', // tee pointing left
        'v' => '\u{2534}', // tee pointing up
        'w' => '\u{252C}', // tee pointing down
        'x' => '\u{2502}', // vertical line
        'y' => '\u{2264}', // less than or equal
        'z' => '\u{2265}', // greater than or equal
        '{' => '\u{03C0}', // pi
        '|' => '\u{2260}', // not equal
        '}' => '\u{00A3}', // pound sign
        '~' => '\u{00B7}', // centred dot
        _ => c,
    }
}
