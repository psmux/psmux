//! One shot side effects carried by a frame: a clipboard write (OSC 52) and
//! a bell, host notifications and enabled tmux passthrough bodies.
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
//! clipboard, host events, then bell, written by [`append`] and read back by [`take`], so
//! nothing here has to parse the rest of the JSON.
//!
//! The sixel image blobs (#431) are carried the same way. The server sends
//! each image's bytes in one frame only (server::build_image_blobs_json) and
//! later frames name the image by id, so a superseded frame that held a blob
//! left the client with an image it could never draw. The frame builders
//! write `image_blobs` just before the clipboard and the bell.

/// The image blobs field as the frame builders write it.
const BLOBS_KEY: &str = ",\"image_blobs\":{";
/// The clipboard field as the frame builders write it.
const CLIP_KEY: &str = ",\"clipboard_osc52\":\"";
/// Exact host event bytes, encoded individually to preserve event boundaries.
const HOST_KEY: &str = ",\"host_events\":";
const RAW_FIELD: &str = ",\"host_raw\":true";

/// The bell field as the frame builders write it.
const BELL_FIELD: &str = ",\"bell\":true";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OneShots {
    /// The members of `image_blobs` (`"id":"base64",...`) without the braces.
    pub image_blobs: String,
    /// Base64 clipboard text for OSC 52.
    pub clip_b64: Option<String>,
    pub bell: bool,
    /// Exact event bytes, oldest first, bounded without partial truncation.
    pub host_events: Vec<Vec<u8>>,
    /// At least one passthrough can have altered the host screen.
    pub host_raw: bool,
}

impl OneShots {
    pub(crate) fn is_empty(&self) -> bool {
        self.image_blobs.is_empty() && self.clip_b64.is_none() && !self.bell && self.host_events.is_empty()
    }

    /// `newer` happened after `self`: the blobs of both are kept, a bell from
    /// either rings, and the newer clipboard write wins.
    pub(crate) fn merge_newer(&mut self, newer: OneShots) {
        if !newer.image_blobs.is_empty() {
            if !self.image_blobs.is_empty() {
                self.image_blobs.push(',');
            }
            self.image_blobs.push_str(&newer.image_blobs);
        }
        self.host_raw |= newer.host_raw;
        for event in newer.host_events { self.push_host_event(event); }
        self.bell |= newer.bell;
        if newer.clip_b64.is_some() {
            self.clip_b64 = newer.clip_b64;
        }
    }

    pub(crate) fn push_host_event(&mut self, event: Vec<u8>) {
        let bytes: usize = self.host_events.iter().map(Vec::len).sum();
        if self.host_events.len() < 256 && event.len() <= vt100::MAX_HOST_EVENT_BYTES.saturating_sub(bytes) {
            self.host_events.push(event);
        }
    }

    /// The image blobs as the client's `DumpState::image_blobs` holds them.
    pub(crate) fn image_blob_map(&self) -> std::collections::HashMap<String, String> {
        if self.image_blobs.is_empty() {
            return Default::default();
        }
        serde_json::from_str(&format!("{{{}}}", self.image_blobs)).unwrap_or_default()
    }
}

fn is_base64(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

/// Decimal ids and base64 bytes, quoted, so pane text (whose quotes are
/// escaped with a backslash) never reads as the field.
fn is_blob_members(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'"' | b':' | b','))
}

/// Append the one shots to a frame that ends with `}`. A line still carrying
/// its trailing newline (the client's reader hands those on) loses it.
pub(crate) fn append(frame: &mut String, shots: &OneShots) {
    let end = frame.trim_end().len();
    if shots.is_empty() || !frame[..end].ends_with('}') {
        return;
    }
    frame.truncate(end - 1);
    if !shots.image_blobs.is_empty() {
        frame.push_str(BLOBS_KEY);
        frame.push_str(&shots.image_blobs);
        frame.push('}');
    }
    if let Some(b64) = &shots.clip_b64 {
        frame.push_str(CLIP_KEY);
        frame.push_str(b64);
        frame.push('"');
    }
    if !shots.host_events.is_empty() {
        use base64::Engine;
        let encoded: Vec<String> = shots.host_events.iter().map(|bytes|
            base64::engine::general_purpose::STANDARD.encode(bytes)).collect();
        frame.push_str(HOST_KEY);
        frame.push_str(&serde_json::to_string(&encoded).expect("base64 strings"));
        if shots.host_raw { frame.push_str(RAW_FIELD); }
    }
    if shots.bell {
        frame.push_str(BELL_FIELD);
    }
    frame.push('}');
}

/// Remove the one shots from the end of a frame and return them. A frame
/// without them (or a line that is not a frame) is left as it is, except that
/// an empty `image_blobs` is removed as well.
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
    if frame[..end].ends_with(RAW_FIELD) {
        shots.host_raw = true;
        end -= RAW_FIELD.len();
    }
    let head = &frame[..end];
    if head.ends_with(']') {
        if let Some(pos) = head.rfind(HOST_KEY) {
            if let Ok(encoded) = serde_json::from_str::<Vec<String>>(&head[pos + HOST_KEY.len()..]) {
                use base64::Engine;
                for item in encoded {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(item) {
                        shots.push_host_event(bytes);
                    }
                }
                end = pos;
            }
        }
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
    // An empty `image_blobs` comes off too, so a frame that later gets
    // another frame's blobs appended never has the field twice.
    let head = &frame[..end];
    if head.ends_with('}') {
        if let Some(pos) = head.rfind(BLOBS_KEY) {
            let members = &head[pos + BLOBS_KEY.len()..head.len() - 1];
            if is_blob_members(members) {
                shots.image_blobs = members.to_string();
                end = pos;
            }
        }
    }
    if end != trimmed - 1 {
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
