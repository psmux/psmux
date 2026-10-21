//! One shot side effects carried by a frame: a clipboard write (OSC 52) and
//! a bell.
//!
//! A frame is a full snapshot, so wherever frames queue up only the newest is
//! kept: the server's per client frame slot (`types::push_frame`), the
//! attached client's line queue (`client_frame_queue`) and the client main
//! loop's drain all replace an unread frame with the next one. That is right
//! for the screen and wrong for these two fields, which describe something
//! that HAPPENED rather than a state: a frame that carried an application's
//! OSC 52 and was then superseded took the clipboard write with it, so the
//! host terminal never got it (and a bell went silent the same way). tmux
//! writes each one to the client's tty as it happens (input.c:3415-3440
//! input_osc_52 calls screen_write_setselection, screen-write.c:3014-3024;
//! the bell is alerts.c:314 tty_putcode(TTYC_BEL)), not as part of a redraw.
//!
//! So every place that supersedes a frame carries the older frame's one
//! shots into the newer one ([`supersede`]): a bell survives as a bell, and a
//! clipboard write survives unless the newer frame carries its own, which is
//! the later write and the one the clipboard ends up holding either way (the
//! server's forward slot already collapses back to back writes to the newest,
//! helpers::drain_osc52). The fields are always the LAST ones in the frame,
//! clipboard then bell, written by [`append`] and read back by [`take`], so
//! nothing here has to parse the rest of the JSON.

/// The clipboard field as the frame builders write it.
const CLIP_KEY: &str = ",\"clipboard_osc52\":\"";
/// The bell field as the frame builders write it.
const BELL_FIELD: &str = ",\"bell\":true";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OneShots {
    /// Base64 clipboard text for OSC 52.
    pub clip_b64: Option<String>,
    pub bell: bool,
}

impl OneShots {
    pub(crate) fn is_empty(&self) -> bool {
        self.clip_b64.is_none() && !self.bell
    }

    /// `newer` happened after `self`: a bell from either rings, and the newer
    /// clipboard write wins.
    pub(crate) fn merge_newer(&mut self, newer: OneShots) {
        self.bell |= newer.bell;
        if newer.clip_b64.is_some() {
            self.clip_b64 = newer.clip_b64;
        }
    }
}

fn is_base64(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

/// Append the one shots to a frame that ends with `}`. A line still carrying
/// its trailing newline (the client's reader hands those on) loses it.
pub(crate) fn append(frame: &mut String, shots: &OneShots) {
    let end = frame.trim_end().len();
    if shots.is_empty() || !frame[..end].ends_with('}') {
        return;
    }
    frame.truncate(end - 1);
    if let Some(b64) = &shots.clip_b64 {
        frame.push_str(CLIP_KEY);
        frame.push_str(b64);
        frame.push('"');
    }
    if shots.bell {
        frame.push_str(BELL_FIELD);
    }
    frame.push('}');
}

/// Remove the one shots from the end of a frame and return them. A frame
/// without them (or a line that is not a frame) is left as it is.
pub(crate) fn take(frame: &mut String) -> OneShots {
    let mut shots = OneShots::default();
    let trimmed = frame.trim_end().len();
    if !frame[..trimmed].ends_with('}') {
        return shots;
    }
    let mut end = trimmed - 1;
    if frame[..end].ends_with(BELL_FIELD) {
        shots.bell = true;
        end -= BELL_FIELD.len();
    }
    let head = &frame[..end];
    if head.ends_with('"') {
        if let Some(pos) = head.rfind(CLIP_KEY) {
            let b64 = &head[pos + CLIP_KEY.len()..head.len() - 1];
            if is_base64(b64) {
                shots.clip_b64 = Some(b64.to_string());
                end = pos;
            }
        }
    }
    if !shots.is_empty() {
        frame.truncate(end);
        frame.push('}');
    }
    shots
}

/// `newer` replaces the unread `older`: keep what `older` carried.
pub(crate) fn supersede(older: &mut String, newer: &mut String) {
    let mut carried = take(older);
    if carried.is_empty() {
        return;
    }
    carried.merge_newer(take(newer));
    append(newer, &carried);
}

#[cfg(test)]
#[path = "../tests-rs/test_frame_oneshot.rs"]
mod tests;
