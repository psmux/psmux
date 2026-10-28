//! The shared server's paste buffers, one set for the whole server (#925).
//!
//! tmux keeps paste buffers in file statics (`paste_by_name`, `paste_by_time`,
//! `paste_next_index`, paste.c:32 to :36), so every session of a server sees
//! the same buffers: `set-buffer` in one session and `paste-buffer` in another
//! paste the same text. psmux keeps them in `AppState` (`paste_buffers`,
//! `named_buffers`, `paste_next_index`), one set per session, and all the per
//! session code (the buffer commands, copy mode yanks, OSC 52, the buffer
//! formats) reads and writes them there. Rather than change that code (owner
//! ruling: the default path stays untouched), the shared server lends its one
//! set to a session for exactly the span of one request, as it lends the id
//! counters (`ids.rs`): `lend` moves the buffers into the session before the
//! request runs and `take_back` moves them out after it. Every request of
//! every session runs on the one loop thread, so no two sessions ever hold the
//! buffers at once. Outside a lent span a session's own fields are empty.

use crate::types::{AppState, PasteBuffer};
use std::collections::HashMap;

/// The server wide paste buffers.
#[derive(Debug, Default)]
pub struct ServerBuffers {
    automatic: Vec<PasteBuffer>,
    named: HashMap<String, String>,
    next_index: u32,
}

impl ServerBuffers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Move the buffers into a session before its request runs. Anything the
    /// session held on its own (it never should, outside a lent span) is kept
    /// rather than dropped: it joins the server set.
    pub fn lend(&mut self, app: &mut AppState) {
        let own_auto = std::mem::replace(&mut app.paste_buffers, std::mem::take(&mut self.automatic));
        let own_named = std::mem::replace(&mut app.named_buffers, std::mem::take(&mut self.named));
        for b in own_auto {
            if !app.paste_buffers.iter().any(|x| x.name == b.name) && !app.named_buffers.contains_key(&b.name) {
                app.paste_buffers.push(b);
            }
        }
        for (k, v) in own_named {
            if !app.paste_buffers.iter().any(|x| x.name == k) {
                app.named_buffers.entry(k).or_insert(v);
            }
        }
        app.paste_next_index = app.paste_next_index.max(self.next_index);
    }

    /// Move the buffers back out of the session after its request, leaving
    /// the session's own fields empty. The name counter never goes backwards,
    /// so an automatic name once handed out is not handed out again.
    pub fn take_back(&mut self, app: &mut AppState) {
        self.automatic = std::mem::take(&mut app.paste_buffers);
        self.named = std::mem::take(&mut app.named_buffers);
        self.next_index = self.next_index.max(app.paste_next_index);
    }

    /// Number of buffers held (tests).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.automatic.len() + self.named.len()
    }

    /// A buffer's data by name, automatic or named (tests).
    #[cfg(test)]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.automatic
            .iter()
            .find(|b| b.name == name)
            .map(|b| b.data.as_str())
            .or_else(|| self.named.get(name).map(String::as_str))
    }

    /// The newest automatic buffer's data (tests).
    #[cfg(test)]
    pub fn top(&self) -> Option<&str> {
        self.automatic.first().map(|b| b.data.as_str())
    }
}

#[cfg(test)]
#[path = "../../../tests-rs/test_issue925_shared_buffers.rs"]
mod tests_issue925_shared_buffers;
