//! strftime for status-left, status-right, `#{T:...}`, `#{t:...}` and the
//! log timestamps.
//!
//! psmux formatted these with chrono until chrono was flagged unmaintained
//! (RUSTSEC-2026-0543). The date fields now come from jiff, but the format
//! language is kept exactly as chrono 0.4.45 read it, because that is what
//! every existing psmux config was written against:
//!
//! * `%-x`, `%_x` and `%0x` change the padding of numeric specifiers only;
//!   a flag on a name, an offset or a composite (`%-a`, `%-z`, `%-D`) is
//!   refused.
//! * `%Z` prints the numeric offset (`+05:30`): chrono had no zone names.
//! * `%q` is the quarter, `%v` is `%e-%b-%Y`, `%+` is RFC 3339, `%.f`,
//!   `%.3f`, `%3f` and friends are fractions, `%:z`, `%::z`, `%:::z` are
//!   offset forms.
//! * An unknown specifier, a lone `%` at the end, or a truncated sequence
//!   refuses the whole string: [`format`] returns None and the caller keeps
//!   the text unformatted, as it did when chrono's Display failed.
//!
//! tests-rs/test_strftime_parity.rs holds the table recorded from chrono.

use std::fmt::Write;

use jiff::ZonedDateTime;

/// The current local time, in the system time zone.
pub fn now() -> ZonedDateTime {
    ZonedDateTime::now()
}

/// Local time for a Unix timestamp in seconds, or None when it is out of range.
pub fn local_from_unix(secs: i64) -> Option<ZonedDateTime> {
    let ts = jiff::Timestamp::from_second(secs).ok()?;
    Some(ts.to_zoned_datetime(jiff::tz::TimeZone::system()))
}

/// The current local time minus `ago`, saturating at "now" on overflow as
/// chrono's `Duration::from_std(..).unwrap_or_default()` did.
pub fn now_minus(ago: std::time::Duration) -> ZonedDateTime {
    let now = now();
    jiff::SignedDuration::try_from(ago)
        .ok()
        .and_then(|d| now.checked_sub(d).ok())
        .unwrap_or(now)
}

/// The current local time in `fmt`; the fixed log formats never fail, and a
/// refused format comes back as the format text itself.
pub fn now_string(fmt: &str) -> String {
    format(fmt, &now()).unwrap_or_else(|| fmt.to_string())
}

/// Formats `t` with `fmt`, or None when the format is refused.
pub fn format(fmt: &str, t: &ZonedDateTime) -> Option<String> {
    let f = Fields::new(t);
    let mut out = String::with_capacity(fmt.len() + 16);
    write_items(&mut out, fmt, &f)?;
    Some(out)
}

const WEEKDAY_SHORT: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const WEEKDAY_LONG: [&str; 7] =
    ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTH_SHORT: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const MONTH_LONG: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August",
    "September", "October", "November", "December",
];

struct Fields {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    nanos: i64,
    /// 0 is Sunday.
    weekday_from_sun: i64,
    ordinal: i64,
    iso_year: i64,
    iso_week: i64,
    unix: i64,
    offset: i32,
}

impl Fields {
    fn new(t: &ZonedDateTime) -> Fields {
        let iso = t.date().iso_week_date();
        Fields {
            year: t.year() as i64,
            month: t.month() as i64,
            day: t.day() as i64,
            hour: t.hour() as i64,
            minute: t.minute() as i64,
            second: t.second() as i64,
            nanos: t.subsec_nanosecond() as i64,
            weekday_from_sun: t.weekday().to_sunday_zero_offset() as i64,
            ordinal: t.day_of_year() as i64,
            iso_year: iso.year() as i64,
            iso_week: iso.week() as i64,
            unix: t.timestamp().as_second(),
            offset: t.offset().seconds(),
        }
    }
}

#[derive(Clone, Copy)]
enum Pad {
    None,
    Zero,
    Space,
}

#[derive(Clone, Copy)]
enum Num {
    Year,
    YearDiv100,
    YearMod100,
    IsoYear,
    IsoYearDiv100,
    IsoYearMod100,
    Quarter,
    Month,
    Day,
    WeekFromSun,
    WeekFromMon,
    IsoWeek,
    DaysFromSun,
    WeekdayFromMon,
    Ordinal,
    Hour,
    Hour12,
    Minute,
    Second,
    Nanosecond,
    Timestamp,
}

enum Item {
    Lit(&'static str),
    Num(Num, Pad),
    /// Specifiers that print text: names, offsets, fractions, RFC 3339.
    Fixed(char),
    /// Composite specifiers, expanded through the same reader.
    Recons(&'static str),
}

fn write_items(out: &mut String, fmt: &str, f: &Fields) -> Option<()> {
    let mut rest = fmt;
    while !rest.is_empty() {
        let Some(pos) = rest.find('%') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..pos]);
        rest = &rest[pos + 1..];
        let (item, after) = parse_spec(rest)?;
        rest = after;
        match item {
            Item::Lit(s) => out.push_str(s),
            Item::Num(n, pad) => write_num(out, n, pad, f),
            Item::Fixed(c) => write_fixed(out, c, f),
            Item::Recons(s) => write_items(out, s, f)?,
        }
    }
    Some(())
}

/// Reads one specifier after its `%`; None refuses the whole format.
fn parse_spec(s: &str) -> Option<(Item, &str)> {
    let mut chars = s.chars();
    let mut c = chars.next()?;
    let pad_override = match c {
        '-' => Some(Pad::None),
        '0' => Some(Pad::Zero),
        '_' => Some(Pad::Space),
        _ => None,
    };
    if pad_override.is_some() {
        c = chars.next()?;
    }
    let num0 = |n| Item::Num(n, Pad::Zero);
    let nums = |n| Item::Num(n, Pad::Space);
    let item = match c {
        'A' => Item::Fixed('A'),
        'B' => Item::Fixed('B'),
        'C' => num0(Num::YearDiv100),
        'D' => Item::Recons("%m/%d/%y"),
        'F' => Item::Recons("%Y-%m-%d"),
        'G' => num0(Num::IsoYear),
        'H' => num0(Num::Hour),
        'I' => num0(Num::Hour12),
        'M' => num0(Num::Minute),
        'P' => Item::Fixed('P'),
        'R' => Item::Recons("%H:%M"),
        'S' => num0(Num::Second),
        'T' => Item::Recons("%H:%M:%S"),
        'U' => num0(Num::WeekFromSun),
        'V' => num0(Num::IsoWeek),
        'W' => num0(Num::WeekFromMon),
        'X' => Item::Recons("%H:%M:%S"),
        'Y' => num0(Num::Year),
        'Z' => Item::Fixed('Z'),
        'a' => Item::Fixed('a'),
        'b' | 'h' => Item::Fixed('b'),
        'c' => Item::Recons("%a %b %e %H:%M:%S %Y"),
        'd' => num0(Num::Day),
        'e' => nums(Num::Day),
        'f' => num0(Num::Nanosecond),
        'g' => num0(Num::IsoYearMod100),
        'j' => num0(Num::Ordinal),
        'k' => nums(Num::Hour),
        'l' => nums(Num::Hour12),
        'm' => num0(Num::Month),
        'n' => Item::Lit("\n"),
        'p' => Item::Fixed('p'),
        'q' => Item::Num(Num::Quarter, Pad::None),
        'r' => Item::Recons("%I:%M:%S %p"),
        's' => Item::Num(Num::Timestamp, Pad::None),
        't' => Item::Lit("\t"),
        'u' => Item::Num(Num::WeekdayFromMon, Pad::None),
        'v' => Item::Recons("%e-%b-%Y"),
        'w' => Item::Num(Num::DaysFromSun, Pad::None),
        'x' => Item::Recons("%m/%d/%y"),
        'y' => num0(Num::YearMod100),
        'z' => Item::Fixed('z'),
        '+' => Item::Fixed('+'),
        '%' => Item::Lit("%"),
        ':' => {
            // %:z, %::z, %:::z
            let mut colons = 1;
            loop {
                match chars.next()? {
                    ':' if colons < 3 => colons += 1,
                    'z' => break,
                    _ => return None,
                }
            }
            Item::Fixed(match colons { 1 => '1', 2 => '2', _ => '3' })
        }
        '.' => match chars.next()? {
            'f' => Item::Fixed('.'),
            d @ ('3' | '6' | '9') => {
                if chars.next()? != 'f' {
                    return None;
                }
                Item::Fixed(match d { '3' => 'm', '6' => 'u', _ => 'N' })
            }
            _ => return None,
        },
        d @ ('3' | '6' | '9') => {
            if chars.next()? != 'f' {
                return None;
            }
            Item::Fixed(match d { '3' => 'o', '6' => 'O', _ => 'Q' })
        }
        _ => return None,
    };
    let item = match pad_override {
        None => item,
        Some(p) => match item {
            Item::Num(n, _) => Item::Num(n, p),
            _ => return None,
        },
    };
    Some((item, chars.as_str()))
}

fn write_num(out: &mut String, n: Num, pad: Pad, f: &Fields) {
    let (v, width): (i64, usize) = match n {
        Num::Year => (f.year, 4),
        Num::YearDiv100 => (f.year.div_euclid(100), 2),
        Num::YearMod100 => (f.year.rem_euclid(100), 2),
        Num::IsoYear => (f.iso_year, 4),
        Num::IsoYearDiv100 => (f.iso_year.div_euclid(100), 2),
        Num::IsoYearMod100 => (f.iso_year.rem_euclid(100), 2),
        Num::Quarter => ((f.month - 1) / 3 + 1, 1),
        Num::Month => (f.month, 2),
        Num::Day => (f.day, 2),
        Num::WeekFromSun => ((f.ordinal - f.weekday_from_sun + 6) / 7, 2),
        Num::WeekFromMon => ((f.ordinal - (f.weekday_from_sun + 6) % 7 + 6) / 7, 2),
        Num::IsoWeek => (f.iso_week, 2),
        Num::DaysFromSun => (f.weekday_from_sun, 1),
        Num::WeekdayFromMon => ((f.weekday_from_sun + 6) % 7 + 1, 1),
        Num::Ordinal => (f.ordinal, 3),
        Num::Hour => (f.hour, 2),
        Num::Hour12 => (if f.hour % 12 == 0 { 12 } else { f.hour % 12 }, 2),
        Num::Minute => (f.minute, 2),
        Num::Second => (f.second, 2),
        Num::Nanosecond => (f.nanos, 9),
        Num::Timestamp => (f.unix, 1),
    };
    // chrono signs years outside 0..=9999 and widens the field by one.
    if matches!(n, Num::Year | Num::IsoYear) && !(0..10000).contains(&v) {
        let _ = match pad {
            Pad::None => write!(out, "{:+}", v),
            Pad::Zero => write!(out, "{:+01$}", v, width + 1),
            Pad::Space => write!(out, "{:+1$}", v, width + 1),
        };
        return;
    }
    let _ = match pad {
        Pad::None => write!(out, "{}", v),
        Pad::Zero => write!(out, "{:01$}", v, width),
        Pad::Space => write!(out, "{:1$}", v, width),
    };
}

/// chrono's offset forms. `colons`: 0 `+hhmm`, 1 `+hh:mm`, 2 `+hh:mm:ss`,
/// 3 `+hh`. Minutes forms round seconds to the nearest minute, the hours form
/// truncates.
fn write_offset(out: &mut String, off: i32, colons: u8) {
    let sign = if off < 0 { '-' } else { '+' };
    let off = off.unsigned_abs();
    out.push(sign);
    match colons {
        3 => {
            let _ = write!(out, "{:02}", off / 3600);
        }
        2 => {
            let _ = write!(out, "{:02}:{:02}:{:02}", off / 3600, off / 60 % 60, off % 60);
        }
        _ => {
            let mins = (off + 30) / 60;
            let sep = if colons == 1 { ":" } else { "" };
            let _ = write!(out, "{:02}{}{:02}", mins / 60, sep, mins % 60);
        }
    }
}

/// chrono's `%.f`: no digits for whole seconds, else 3, 6 or 9 as needed.
fn write_auto_fraction(out: &mut String, nanos: i64) {
    if nanos == 0 {
    } else if nanos % 1_000_000 == 0 {
        let _ = write!(out, ".{:03}", nanos / 1_000_000);
    } else if nanos % 1_000 == 0 {
        let _ = write!(out, ".{:06}", nanos / 1_000);
    } else {
        let _ = write!(out, ".{:09}", nanos);
    }
}

fn write_fixed(out: &mut String, c: char, f: &Fields) {
    let wd = f.weekday_from_sun as usize;
    let mo = (f.month - 1) as usize;
    match c {
        'a' => out.push_str(WEEKDAY_SHORT[wd]),
        'A' => out.push_str(WEEKDAY_LONG[wd]),
        'b' => out.push_str(MONTH_SHORT[mo]),
        'B' => out.push_str(MONTH_LONG[mo]),
        'p' => out.push_str(if f.hour < 12 { "AM" } else { "PM" }),
        'P' => out.push_str(if f.hour < 12 { "am" } else { "pm" }),
        'z' => write_offset(out, f.offset, 0),
        '1' => write_offset(out, f.offset, 1),
        '2' => write_offset(out, f.offset, 2),
        '3' => write_offset(out, f.offset, 3),
        'Z' => {
            // chrono printed the FixedOffset Display: +hh:mm, plus :ss when
            // the offset has seconds.
            let sign = if f.offset < 0 { '-' } else { '+' };
            let off = f.offset.unsigned_abs();
            let _ = write!(out, "{}{:02}:{:02}", sign, off / 3600, off / 60 % 60);
            if !off.is_multiple_of(60) {
                let _ = write!(out, ":{:02}", off % 60);
            }
        }
        '.' => write_auto_fraction(out, f.nanos),
        'm' => { let _ = write!(out, ".{:03}", f.nanos / 1_000_000); }
        'u' => { let _ = write!(out, ".{:06}", f.nanos / 1_000); }
        'N' => { let _ = write!(out, ".{:09}", f.nanos); }
        'o' => { let _ = write!(out, "{:03}", f.nanos / 1_000_000); }
        'O' => { let _ = write!(out, "{:06}", f.nanos / 1_000); }
        'Q' => { let _ = write!(out, "{:09}", f.nanos); }
        '+' => {
            if (0..10000).contains(&f.year) {
                let _ = write!(out, "{:04}", f.year);
            } else {
                let _ = write!(out, "{:+05}", f.year);
            }
            let _ = write!(
                out,
                "-{:02}-{:02}T{:02}:{:02}:{:02}",
                f.month, f.day, f.hour, f.minute, f.second
            );
            write_auto_fraction(out, f.nanos);
            write_offset(out, f.offset, 1);
        }
        _ => {}
    }
}
