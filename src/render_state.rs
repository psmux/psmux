//! Typed options flattened into the server-to-client render-state payload.
//! The serde renames preserve the existing short wire keys.

use crate::pane_border::PaneBorderIndicators;

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct ClientRenderOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_left_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_right_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "wsa_style")]
    pub window_status_activity_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "wsb_style")]
    pub window_status_bell_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "wsl_style")]
    pub window_status_last_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_active_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_border_indicators: Option<PaneBorderIndicators>,
    /// `codepoint-widths` entries, carried so the CLIENT process resolves
    /// character widths the same way the server's emulator does.
    ///
    /// The client renders the status line, tab bar, pane labels and float
    /// titles itself, and measures them with its own width calls. If only the
    /// server honoured the override the two would disagree about how wide a
    /// glyph is, which is the same class of stranded-cell bug the option
    /// exists to fix. `None` (the common case, since the option is empty by
    /// default) is skipped on the wire entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "cpw")]
    pub codepoint_widths: Option<Vec<String>>,
    /// `terminal-overrides` entries (issue #700). The client matches them
    /// against its own TERM to decide whether to enter the alternate screen.
    /// Skipped on the wire while the option is empty (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "tov")]
    pub terminal_overrides: Option<Vec<String>>,
    /// `word-separators`, which the client's prompts split words with for
    /// C-w, M-f and M-b, as tmux's prompt reads the option (status.c:1559,
    /// :1600, :1622).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "wsep")]
    pub word_separators: Option<String>,
}
