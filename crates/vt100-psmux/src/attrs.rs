use crate::term::BufWrite as _;

/// Represents a foreground or background color for cells.
#[derive(Eq, PartialEq, Debug, Copy, Clone, Default)]
pub enum Color {
    /// The default terminal color.
    #[default]
    Default,

    /// An indexed terminal color.
    Idx(u8),

    /// An RGB terminal color. The parameters are (red, green, blue).
    Rgb(u8, u8, u8),
}

/// The style of underline drawn under a cell.
///
/// SGR 4 selects a single underline; the SGR 4 subparameter forms `4:0`
/// through `4:5` (and the legacy SGR 21) select the extended styles that
/// terminals such as Windows Terminal, `WezTerm` and `kitty` render.  tmux
/// models the same set as `GRID_ATTR_UNDERSCORE_2` through
/// `GRID_ATTR_UNDERSCORE_5` (tmux `input.c`, `grid.h`).
#[derive(Eq, PartialEq, Debug, Copy, Clone, Default)]
pub enum UnderlineStyle {
    /// No underline at all (SGR 24 or SGR 4:0).
    #[default]
    None,
    /// A single straight underline (SGR 4 or SGR 4:1).
    Single,
    /// A double straight underline (SGR 4:2 or SGR 21).
    Double,
    /// A curly (wavy) underline, commonly called undercurl (SGR 4:3).
    Curly,
    /// A dotted underline (SGR 4:4).
    Dotted,
    /// A dashed underline (SGR 4:5).
    Dashed,
}

impl UnderlineStyle {
    /// Maps an SGR 4 subparameter (`0` through `5`) onto a style.  Any other
    /// value is treated as a plain single underline, matching how terminals
    /// degrade unknown extended styles.
    #[must_use]
    pub fn from_sgr_subparam(n: u16) -> Self {
        match n {
            0 => Self::None,
            2 => Self::Double,
            3 => Self::Curly,
            4 => Self::Dotted,
            5 => Self::Dashed,
            _ => Self::Single,
        }
    }

    /// The SGR 4 subparameter that selects this style.
    #[must_use]
    pub fn sgr_subparam(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Single => 1,
            Self::Double => 2,
            Self::Curly => 3,
            Self::Dotted => 4,
            Self::Dashed => 5,
        }
    }
}

const TEXT_MODE_INTENSITY: u8 = 0b0000_0011;
const TEXT_MODE_BOLD: u8 = 0b0000_0001;
const TEXT_MODE_DIM: u8 = 0b0000_0010;
const TEXT_MODE_ITALIC: u8 = 0b0000_0100;
const TEXT_MODE_UNDERLINE: u8 = 0b0000_1000;
const TEXT_MODE_INVERSE: u8 = 0b0001_0000;
const TEXT_MODE_BLINK: u8 = 0b0010_0000;
const TEXT_MODE_HIDDEN: u8 = 0b0100_0000;
const TEXT_MODE_STRIKETHROUGH: u8 = 0b1000_0000;

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Attrs {
    pub fgcolor: Color,
    pub bgcolor: Color,
    /// The underline colour set by SGR 58 (`Default` until SGR 58 is seen, and
    /// reset by SGR 59 or SGR 0).  Mirrors tmux's `grid_cell.us`.
    pub ulcolor: Color,
    /// The extended underline style selected by SGR `4:0` through `4:5` (and
    /// SGR 21).  `TEXT_MODE_UNDERLINE` stays the "any underline at all" bit so
    /// every existing caller of `underline()` keeps working.
    pub ul_style: UnderlineStyle,
    pub mode: u8,
    /// SGR 53 overline, cleared by SGR 55 or SGR 0 (tmux
    /// `GRID_ATTR_OVERLINE`, input.c:2497-2501 at tag 3.8).  `mode` has no
    /// bit left; this takes padding `Attrs` already had, so a `Cell` stays the
    /// same size.
    pub overline: bool,
    /// OSC 8 hyperlink id (index into the screen's hyperlink store, +1).
    /// 0 means no hyperlink.  Part of Attrs so a link change is reflected in
    /// cell equality (forcing a redraw) and travels with the cell through all
    /// grid operations, mirroring tmux's grid_cell.link.  It is intentionally
    /// NOT emitted by write_escape_code_diff (SGR); psmux re-emits OSC 8 in its
    /// own pane renderer.
    pub link: u32,
}

impl Attrs {
    /// What an erase, an inserted line or cell, or a line scrolled in is
    /// filled with: the blank cell with the current background and nothing
    /// else, so no foreground, underline, reverse or link. tmux clears with
    /// `ictx->cell.cell.bg` alone (`grid_clear_cell`, grid.c:264 at tag 3.8,
    /// from input.c:1365-1425 and 1613-1852), and both console hosts show the
    /// same.
    #[must_use]
    pub fn erased(&self) -> Self {
        Self {
            bgcolor: self.bgcolor,
            ..Self::default()
        }
    }

    pub fn bold(&self) -> bool {
        self.mode & TEXT_MODE_BOLD != 0
    }

    pub fn dim(&self) -> bool {
        self.mode & TEXT_MODE_DIM != 0
    }

    /// SGR 1 adds bold and leaves dim as it is, and SGR 2 the other way
    /// round: they are two bits, as in tmux (`GRID_ATTR_BRIGHT` and
    /// `GRID_ATTR_DIM`, input.c:2420-2425 at tag 3.8) and in terminals, so
    /// `CSI 1 ; 2 m` is bold and dim. Only SGR 22 clears both.
    pub fn set_bold(&mut self) {
        self.mode |= TEXT_MODE_BOLD;
    }

    pub fn set_dim(&mut self) {
        self.mode |= TEXT_MODE_DIM;
    }

    /// SGR 22 (input.c:2450-2451): neither bold nor dim.
    pub fn set_normal_intensity(&mut self) {
        self.mode &= !TEXT_MODE_INTENSITY;
    }

    pub fn overline(&self) -> bool {
        self.overline
    }

    pub fn set_overline(&mut self, overline: bool) {
        self.overline = overline;
    }

    pub fn italic(&self) -> bool {
        self.mode & TEXT_MODE_ITALIC != 0
    }

    pub fn set_italic(&mut self, italic: bool) {
        if italic {
            self.mode |= TEXT_MODE_ITALIC;
        } else {
            self.mode &= !TEXT_MODE_ITALIC;
        }
    }

    pub fn underline(&self) -> bool {
        self.mode & TEXT_MODE_UNDERLINE != 0
    }

    pub fn set_underline(&mut self, underline: bool) {
        if underline {
            self.mode |= TEXT_MODE_UNDERLINE;
            self.ul_style = UnderlineStyle::Single;
        } else {
            self.mode &= !TEXT_MODE_UNDERLINE;
            self.ul_style = UnderlineStyle::None;
        }
    }

    pub fn underline_style(&self) -> UnderlineStyle {
        self.ul_style
    }

    /// Selects an extended underline style.  `UnderlineStyle::None` clears the
    /// underline bit, everything else sets it, so the plain `underline()`
    /// predicate keeps reporting "this cell has some underline".
    pub fn set_underline_style(&mut self, style: UnderlineStyle) {
        self.ul_style = style;
        if style == UnderlineStyle::None {
            self.mode &= !TEXT_MODE_UNDERLINE;
        } else {
            self.mode |= TEXT_MODE_UNDERLINE;
        }
    }

    pub fn ulcolor(&self) -> Color {
        self.ulcolor
    }

    pub fn set_ulcolor(&mut self, color: Color) {
        self.ulcolor = color;
    }

    pub fn inverse(&self) -> bool {
        self.mode & TEXT_MODE_INVERSE != 0
    }

    pub fn set_inverse(&mut self, inverse: bool) {
        if inverse {
            self.mode |= TEXT_MODE_INVERSE;
        } else {
            self.mode &= !TEXT_MODE_INVERSE;
        }
    }

    pub fn blink(&self) -> bool {
        self.mode & TEXT_MODE_BLINK != 0
    }

    pub fn set_blink(&mut self, blink: bool) {
        if blink {
            self.mode |= TEXT_MODE_BLINK;
        } else {
            self.mode &= !TEXT_MODE_BLINK;
        }
    }

    pub fn hidden(&self) -> bool {
        self.mode & TEXT_MODE_HIDDEN != 0
    }

    pub fn set_hidden(&mut self, hidden: bool) {
        if hidden {
            self.mode |= TEXT_MODE_HIDDEN;
        } else {
            self.mode &= !TEXT_MODE_HIDDEN;
        }
    }

    pub fn strikethrough(&self) -> bool {
        self.mode & TEXT_MODE_STRIKETHROUGH != 0
    }

    pub fn set_strikethrough(&mut self, strikethrough: bool) {
        if strikethrough {
            self.mode |= TEXT_MODE_STRIKETHROUGH;
        } else {
            self.mode &= !TEXT_MODE_STRIKETHROUGH;
        }
    }

    pub fn write_escape_code_diff(
        &self,
        contents: &mut Vec<u8>,
        other: &Self,
    ) {
        if self != other && self == &Self::default() {
            crate::term::ClearAttrs.write_buf(contents);
            return;
        }

        let attrs = crate::term::Attrs::default();

        let attrs = if self.fgcolor == other.fgcolor {
            attrs
        } else {
            attrs.fgcolor(self.fgcolor)
        };
        let attrs = if self.bgcolor == other.bgcolor {
            attrs
        } else {
            attrs.bgcolor(self.bgcolor)
        };
        // Bold and dim are two bits a terminal sets one by one, and only SGR
        // 22 takes either away, both at once. So when one goes, 22 comes
        // first and whatever is still on is set again; otherwise only what is
        // new is set.
        let attrs = if self.bold() == other.bold() && self.dim() == other.dim() {
            attrs
        } else {
            let reset = (other.bold() && !self.bold()) || (other.dim() && !self.dim());
            attrs.intensity(crate::term::Intensity {
                reset,
                bold: self.bold() && (reset || !other.bold()),
                dim: self.dim() && (reset || !other.dim()),
            })
        };
        let attrs = if self.italic() == other.italic() {
            attrs
        } else {
            attrs.italic(self.italic())
        };
        let attrs = if self.ul_style == other.ul_style {
            attrs
        } else {
            attrs.underline_style(self.ul_style)
        };
        let attrs = if self.ulcolor == other.ulcolor {
            attrs
        } else {
            attrs.ulcolor(self.ulcolor)
        };
        let attrs = if self.inverse() == other.inverse() {
            attrs
        } else {
            attrs.inverse(self.inverse())
        };
        let attrs = if self.blink() == other.blink() {
            attrs
        } else {
            attrs.blink(self.blink())
        };
        let attrs = if self.hidden() == other.hidden() {
            attrs
        } else {
            attrs.hidden(self.hidden())
        };
        let attrs = if self.strikethrough() == other.strikethrough() {
            attrs
        } else {
            attrs.strikethrough(self.strikethrough())
        };
        let attrs = if self.overline == other.overline {
            attrs
        } else {
            attrs.overline(self.overline)
        };

        attrs.write_buf(contents);
    }
}
