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
    /// `terminal-features` entries, which the client matches against its own
    /// TERM for the `progressbar` feature. Skipped on the wire while the option
    /// holds its default, so a frame without it means the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "tfe")]
    pub terminal_features: Option<Vec<String>>,
    /// `word-separators`, which the client's prompts split words with for
    /// C-w, M-f and M-b, as tmux's prompt reads the option (status.c:1559,
    /// :1600, :1622).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "wsep")]
    pub word_separators: Option<String>,
    /// `user-keys` elements as (N, sequence), which the client's VT and pipe
    /// input reader turns into the key UserN (crate::user_keys). Skipped on
    /// the wire while the option is empty (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ukeys")]
    pub user_keys: Option<Vec<(u32, String)>>,
    /// `status-keys vi`: the client's command prompt has tmux's vi command
    /// mode, entered with Escape (prompt.c:1265 and :819 at tag 3.8), and is
    /// drawn with `message-command-style` while in it. Skipped on the wire
    /// for emacs, the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "skvi")]
    pub status_keys_vi: Option<bool>,
}
