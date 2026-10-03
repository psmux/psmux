/// Static catalog of all supported tmux options for customize-mode.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OptionScope {
    Server,
    Session,
    Window,
    Pane,
}

impl OptionScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Session => "session",
            Self::Window => "window",
            Self::Pane => "pane",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumberKind {
    I64,
    U64,
    Usize,
    Index,
    RepeatTime,
}

impl NumberKind {
    /// Inclusive bounds of the destination type, widened so every kind fits.
    fn range(self) -> (i128, i128) {
        match self {
            Self::I64 => (i64::MIN as i128, i64::MAX as i128),
            Self::U64 => (0, u64::MAX as i128),
            Self::Usize => (0, usize::MAX as i128),
            Self::Index => (0, isize::MAX as i128),
            Self::RepeatTime => (0, crate::server::options::REPEAT_TIME_MAX_MS as i128),
        }
    }

    /// tmux validates a number option with strtonum and reports the errstr it
    /// hands back: "value is too small: N", "value is too large: N" or
    /// "value is invalid: X" (options.c:1295). psmux keeps its own wording for
    /// a value that is not an integer at all, because that message names the
    /// option, but a value that IS an integer and merely falls outside the
    /// destination range now gets tmux's out of range wording instead of the
    /// misleading "expected a number".
    fn validate(self, name: &str, value: &str) -> Result<(), String> {
        let Ok(parsed) = value.trim().parse::<i128>() else {
            return Err(format!(
                "invalid value '{}' for option '{}' (expected a number)",
                value, name,
            ));
        };
        let (minimum, maximum) = self.range();
        if parsed < minimum {
            return Err(format!("value is too small: {}", value));
        }
        if parsed > maximum {
            return Err(format!("value is too large: {}", value));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChoiceKind {
    Priority,
    Unvalidated,
    PaneBorderIndicators,
    /// tmux `options_table_copy_mode_line_numbers_list`: a CHOICE option, so
    /// tmux refuses anything else with `unknown value` (#706).
    CopyModeLineNumbers,
}

impl ChoiceKind {
    fn validate_value(self, value: &str) -> Result<(), String> {
        match self {
            Self::CopyModeLineNumbers => {
                if crate::copy_line_numbers::CHOICES.contains(&value.trim()) {
                    Ok(())
                } else {
                    Err(format!(
                        "value for 'copy-mode-line-numbers' must be one of {}, got '{}'",
                        crate::copy_line_numbers::CHOICES.join(", "),
                        value,
                    ))
                }
            }
            Self::Priority if crate::platform::normalize_priority(value).is_none() => Err(format!(
                "value for 'priority' must be one of {}, got '{}'",
                crate::platform::PRIORITY_VALUES.join(", "),
                value,
            )),
            Self::PaneBorderIndicators => {
                crate::pane_border::PaneBorderIndicators::parse(value).map(|_| ())
            }
            Self::Priority | Self::Unvalidated => Ok(()),
        }
    }

    const fn allows_append(self) -> bool {
        match self {
            Self::Priority | Self::Unvalidated | Self::CopyModeLineNumbers => true,
            Self::PaneBorderIndicators => false,
        }
    }

    const fn allows_local_window_override(self) -> bool {
        match self {
            Self::Priority | Self::Unvalidated | Self::CopyModeLineNumbers => true,
            Self::PaneBorderIndicators => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OptionType {
    String,
    Boolean,
    Choice(ChoiceKind),
    Number(NumberKind),
}

impl OptionType {
    fn validate_value(self, name: &str, value: &str) -> Result<(), String> {
        match self {
            Self::Choice(kind) => kind.validate_value(value),
            Self::Number(kind) => kind.validate(name, value),
            Self::String | Self::Boolean => Ok(()),
        }
    }

    const fn allows_append(self) -> bool {
        match self {
            Self::Choice(kind) => kind.allows_append(),
            Self::String | Self::Boolean | Self::Number(_) => true,
        }
    }

    const fn allows_local_window_override(self) -> bool {
        match self {
            Self::Choice(kind) => kind.allows_local_window_override(),
            Self::String | Self::Boolean | Self::Number(_) => true,
        }
    }
}

pub struct OptionDef {
    pub name: &'static str,
    pub scope: OptionScope,
    pub option_type: OptionType,
    pub default: &'static str,
    pub description: &'static str,
}

impl OptionDef {
    pub fn validate_value(&self, value: &str) -> Result<(), String> {
        self.option_type.validate_value(self.name, value)
    }

    pub fn validate_append(&self) -> Result<(), String> {
        if self.option_type.allows_append() {
            Ok(())
        } else {
            Err(format!("{} does not support append", self.name))
        }
    }

    pub fn validate_local_window_override(&self) -> Result<(), String> {
        if self.option_type.allows_local_window_override() {
            Ok(())
        } else {
            Err(format!(
                "{} does not support local window overrides",
                self.name,
            ))
        }
    }

}

pub struct ValidationOnlyOptionDef {
    pub name: &'static str,
    pub option_type: OptionType,
}

impl ValidationOnlyOptionDef {
    fn validate_value(&self, value: &str) -> Result<(), String> {
        self.option_type.validate_value(self.name, value)
    }
}

use ChoiceKind::{Priority, Unvalidated};
use NumberKind::{I64, Index, RepeatTime, U64, Usize};
use OptionScope::{Pane, Server, Session, Window};
use OptionType::{Boolean, Choice, Number};

const UNVALIDATED_CHOICE: OptionType = Choice(Unvalidated);

pub static OPTION_CATALOG: &[OptionDef] = &[
    // ── Server options ──
    OptionDef { name: "escape-time", scope: Server, option_type: Number(U64), default: "500", description: "Time in ms to wait for escape sequence" },
    OptionDef { name: "focus-events", scope: Server, option_type: Boolean, default: "off", description: "Send focus events to applications" },
    OptionDef { name: "bold-is-bright", scope: Server, option_type: Boolean, default: "on", description: "Rewrite crossterm's 256-indexed basic colors to standard SGR so the terminal applies bold-is-bright (issue #425); off keeps explicit 256-indexed low colors byte-accurate" },
    OptionDef { name: "history-limit", scope: Server, option_type: Number(Usize), default: "2000", description: "Maximum scrollback lines per pane" },
    OptionDef { name: "alternate-screen", scope: Server, option_type: Boolean, default: "on", description: "Honour DEC 47/1049 alt-screen mode (off = TUI output goes to scrollback, #88)" },
    OptionDef { name: "set-clipboard", scope: Server, option_type: UNVALIDATED_CHOICE, default: "on", description: "OSC 52 clipboard integration" },
    OptionDef { name: "default-shell", scope: Server, option_type: OptionType::String, default: "", description: "Default shell for new panes" },
    OptionDef { name: "default-terminal", scope: Server, option_type: OptionType::String, default: "xterm-256color", description: "TERM value for new panes" },
    OptionDef { name: "copy-command", scope: Server, option_type: OptionType::String, default: "", description: "External copy command (pipe selection)" },
    OptionDef { name: "codepoint-widths", scope: Server, option_type: OptionType::String, default: "", description: "Array of override widths for Unicode codepoints, e.g. U+2500-U+257F=2 (comma separated; widths 0, 1 or 2)" },
    OptionDef { name: "terminal-overrides", scope: Server, option_type: OptionType::String, default: "", description: "Array of pattern:cap overrides matched against the client TERM; smcup@/rmcup@ keep the attached client off the host's alternate screen (#700)" },
    OptionDef { name: "exit-empty", scope: Server, option_type: Boolean, default: "on", description: "Exit server when no sessions remain" },
    OptionDef { name: "priority", scope: Server, option_type: Choice(Priority), default: "above-normal", description: "Scheduling class for psmux's own server and client processes (normal/above-normal/high). Pane children are never raised" },
    // ── Session options ──
    OptionDef { name: "prefix", scope: Session, option_type: OptionType::String, default: "C-b", description: "Primary prefix key" },
    OptionDef { name: "prefix2", scope: Session, option_type: OptionType::String, default: "none", description: "Secondary prefix key" },
    OptionDef { name: "base-index", scope: Session, option_type: Number(Index), default: "0", description: "Starting index for windows" },
    OptionDef { name: "pane-base-index", scope: Session, option_type: Number(Index), default: "0", description: "Starting index for panes" },
    OptionDef { name: "display-time", scope: Session, option_type: Number(U64), default: "750", description: "Duration of messages in ms" },
    OptionDef { name: "display-panes-time", scope: Session, option_type: Number(U64), default: "1000", description: "Duration of pane numbers display in ms" },
    OptionDef { name: "repeat-time", scope: Session, option_type: Number(RepeatTime), default: "500", description: "Repeat timeout for prefix keys in ms" },
    OptionDef { name: "mouse", scope: Session, option_type: Boolean, default: "on", description: "Enable mouse support" },
    OptionDef { name: "scroll-enter-copy-mode", scope: Session, option_type: Boolean, default: "on", description: "Enter copy mode on mouse scroll up at shell prompt" },
    OptionDef { name: "mouse-drag-enter-copy-mode", scope: Session, option_type: Boolean, default: "off", description: "Enter copy mode and select there on a left drag in a pane that does not track the mouse (tmux's MouseDrag1Pane -> copy-mode -M)" },
    OptionDef { name: "pwsh-mouse-selection", scope: Session, option_type: Boolean, default: "off", description: "Windows 11 PowerShell-style drag selection (pane-aware, right-click to copy, word/line multi-click)" },
    OptionDef { name: "mouse-selection", scope: Session, option_type: Boolean, default: "on", description: "Enable psmux's client-side drag-selection overlay. Set to off so apps inside a pane (opencode, etc.) can implement their own mouse selection without psmux drawing on top." },
    OptionDef { name: "mouse-selection-force", scope: Session, option_type: Boolean, default: "off", description: "Keep psmux drag selection active in mouse-aware apps; replay plain clicks while consuming drags" },
    OptionDef { name: "paste-detection", scope: Session, option_type: Boolean, default: "on", description: "Detect Ctrl+V paste from console host and send as bracketed paste (disable to let Ctrl+V reach child apps)" },
    OptionDef { name: "mode-keys", scope: Session, option_type: UNVALIDATED_CHOICE, default: "emacs", description: "Key bindings in copy mode (vi/emacs)" },
    OptionDef { name: "copy-mode-line-numbers", scope: Window, option_type: Choice(ChoiceKind::CopyModeLineNumbers), default: "off", description: "Line number mode in copy mode (off/default/absolute/relative/hybrid)" },
    OptionDef { name: "copy-mode-line-number-style", scope: Window, option_type: OptionType::String, default: "fg=brightblack", description: "Style for copy-mode line numbers" },
    OptionDef { name: "copy-mode-current-line-number-style", scope: Window, option_type: OptionType::String, default: "fg=yellow,bold", description: "Style for the current copy-mode line number" },
    OptionDef { name: "status", scope: Session, option_type: Boolean, default: "on", description: "Show/hide the status bar" },
    OptionDef { name: "status-position", scope: Session, option_type: UNVALIDATED_CHOICE, default: "bottom", description: "Status bar position (top/bottom)" },
    OptionDef { name: "status-interval", scope: Session, option_type: Number(U64), default: "15", description: "Status bar refresh interval in seconds" },
    OptionDef { name: "status-justify", scope: Session, option_type: UNVALIDATED_CHOICE, default: "left", description: "Window list alignment (left/centre/right)" },
    OptionDef { name: "status-left", scope: Session, option_type: OptionType::String, default: "[#S] ", description: "Left side of the status bar" },
    OptionDef { name: "status-right", scope: Session, option_type: OptionType::String, default: "#{?window_bigger,[#{window_offset_x}#,#{window_offset_y}] ,}\"#{=21:pane_title}\" %H:%M %d-%b-%y", description: "Right side of the status bar" },
    OptionDef { name: "status-left-length", scope: Session, option_type: Number(Usize), default: "10", description: "Max width of left status section" },
    OptionDef { name: "status-right-length", scope: Session, option_type: Number(Usize), default: "40", description: "Max width of right status section" },
    OptionDef { name: "status-style", scope: Session, option_type: OptionType::String, default: "bg=green,fg=black", description: "Status bar style" },
    OptionDef { name: "status-left-style", scope: Session, option_type: OptionType::String, default: "default", description: "Left status section style" },
    OptionDef { name: "status-right-style", scope: Session, option_type: OptionType::String, default: "default", description: "Right status section style" },
    OptionDef { name: "message-style", scope: Session, option_type: OptionType::String, default: "bg=yellow,fg=black", description: "Command prompt / message style" },
    OptionDef { name: "message-command-style", scope: Session, option_type: OptionType::String, default: "bg=black,fg=yellow", description: "Command prompt editing style" },
    OptionDef { name: "mode-style", scope: Session, option_type: OptionType::String, default: "bg=yellow,fg=black", description: "Copy mode selection style" },
    OptionDef { name: "bell-action", scope: Session, option_type: UNVALIDATED_CHOICE, default: "any", description: "Bell handling (any/none/current/other)" },
    OptionDef { name: "visual-bell", scope: Session, option_type: Boolean, default: "off", description: "Show visual indicator on bell" },
    OptionDef { name: "activity-action", scope: Session, option_type: UNVALIDATED_CHOICE, default: "other", description: "Activity alert action" },
    OptionDef { name: "silence-action", scope: Session, option_type: UNVALIDATED_CHOICE, default: "other", description: "Silence alert action" },
    OptionDef { name: "monitor-silence", scope: Window, option_type: Number(U64), default: "0", description: "Seconds of silence before alert (0=off)" },
    OptionDef { name: "destroy-unattached", scope: Session, option_type: Boolean, default: "off", description: "Destroy session when last client detaches" },
    OptionDef { name: "renumber-windows", scope: Session, option_type: Boolean, default: "off", description: "Renumber windows on close" },
    OptionDef { name: "set-titles", scope: Session, option_type: Boolean, default: "off", description: "Set terminal title" },
    OptionDef { name: "set-titles-string", scope: Session, option_type: OptionType::String, default: "#S:#I:#W", description: "Terminal title format string" },
    OptionDef { name: "word-separators", scope: Session, option_type: OptionType::String, default: " -_@", description: "Characters treated as word boundaries" },
    OptionDef { name: "allow-passthrough", scope: Session, option_type: UNVALIDATED_CHOICE, default: "off", description: "Allow passthrough escape sequences" },
    OptionDef { name: "allow-rename", scope: Session, option_type: Boolean, default: "on", description: "Allow programs to rename windows" },
    OptionDef { name: "allow-set-title", scope: Session, option_type: Boolean, default: "off", description: "Allow programs to set pane title via escape sequences" },
    OptionDef { name: "update-environment", scope: Session, option_type: OptionType::String, default: "DISPLAY KRB5CCNAME SSH_ASKPASS SSH_AUTH_SOCK SSH_AGENT_PID SSH_CONNECTION WINDOWID XAUTHORITY", description: "Environment variables to update on attach" },
    OptionDef { name: "synchronize-panes", scope: Session, option_type: Boolean, default: "off", description: "Send input to all panes simultaneously" },
    // `set-option -u` restores catalog defaults, so every resettable option lives here.
    OptionDef { name: "choose-tree-preview", scope: Session, option_type: Boolean, default: "off", description: "Show a live pane preview in choose-tree" },
    // ── psmux extensions (session scope) ──
    OptionDef { name: "prediction-dimming", scope: Session, option_type: Boolean, default: "off", description: "Dim PSReadLine prediction text" },
    OptionDef { name: "allow-predictions", scope: Session, option_type: Boolean, default: "off", description: "Allow PSReadLine predictions" },
    OptionDef { name: "warm", scope: Session, option_type: Boolean, default: "on", description: "Pre-spawn warm shell for fast window creation" },
    OptionDef { name: "warm-pool-size", scope: Session, option_type: Number(Usize), default: "2", description: "How many spare shells to keep pre-spawned (0 disables, max 8)" },
    OptionDef { name: "cursor-style", scope: Session, option_type: UNVALIDATED_CHOICE, default: "default", description: "Cursor shape asked of the terminal: default (ask for none), block, underline, bar (steady), blinking-block, blinking-underline, blinking-bar" },
    OptionDef { name: "cursor-blink", scope: Session, option_type: Boolean, default: "off", description: "Add or remove the blink of the cursor-style shape, or ask for the blink alone when no shape is named" },
    OptionDef { name: "claude-code-fix-tty", scope: Session, option_type: Boolean, default: "on", description: "Fix TTY for Claude Code" },
    OptionDef { name: "claude-code-force-interactive", scope: Session, option_type: Boolean, default: "on", description: "Force interactive mode for Claude Code" },
    // ── Window options ──
    OptionDef { name: "automatic-rename", scope: Window, option_type: Boolean, default: "on", description: "Auto-rename windows based on running command" },
    OptionDef { name: "monitor-activity", scope: Window, option_type: Boolean, default: "off", description: "Monitor for activity in window" },
    OptionDef { name: "remain-on-exit", scope: Window, option_type: Boolean, default: "off", description: "Keep pane open after command exits" },
    OptionDef { name: "aggressive-resize", scope: Window, option_type: Boolean, default: "off", description: "Resize window to smallest attached client" },
    // tmux types both of these OPTIONS_TABLE_STRING (options-table.c:1474) and
    // documents a percentage form ("10%"), so they cannot be validated as a
    // number without rejecting input real tmux accepts.
    OptionDef { name: "main-pane-width", scope: Window, option_type: OptionType::String, default: "0", description: "Width of main pane in main-* layouts (0 sizes it automatically, percentages accepted)" },
    OptionDef { name: "main-pane-height", scope: Window, option_type: OptionType::String, default: "0", description: "Height of main pane in main-* layouts (0 sizes it automatically, percentages accepted)" },
    OptionDef { name: "window-size", scope: Window, option_type: UNVALIDATED_CHOICE, default: "latest", description: "Window sizing strategy" },
    OptionDef { name: "window-status-format", scope: Window, option_type: OptionType::String, default: "#I:#W#{?window_flags,#{window_flags}, }", description: "Window status bar format" },
    OptionDef { name: "window-status-current-format", scope: Window, option_type: OptionType::String, default: "#I:#W#{?window_flags,#{window_flags}, }", description: "Active window status bar format" },
    OptionDef { name: "window-status-separator", scope: Window, option_type: OptionType::String, default: " ", description: "Separator between window entries" },
    OptionDef { name: "window-status-style", scope: Window, option_type: OptionType::String, default: "default", description: "Inactive window style" },
    OptionDef { name: "window-status-current-style", scope: Window, option_type: OptionType::String, default: "default", description: "Active window style" },
    OptionDef { name: "window-status-activity-style", scope: Window, option_type: OptionType::String, default: "reverse", description: "Window style on activity alert" },
    OptionDef { name: "window-status-bell-style", scope: Window, option_type: OptionType::String, default: "reverse", description: "Window style on bell alert" },
    OptionDef { name: "window-status-last-style", scope: Window, option_type: OptionType::String, default: "default", description: "Previously active window style" },
    OptionDef { name: "pane-border-indicators", scope: Window, option_type: Choice(ChoiceKind::PaneBorderIndicators), default: crate::pane_border::INDICATORS_DEFAULT, description: "Active pane indicator mode (off/colour/arrows/both)" },
    // ── Pane options ──
    // The default here is what `set -u` and customize-mode restore, so it has
    // to be a style that renders like a fresh server, not the word tmux prints
    // for a style nobody overrode. tmux gives the option a real default style
    // (options-table.c:1605, `fg=themelightgrey`) precisely so unset and
    // startup always agree; `fg=brightblack` is the psmux spelling of the
    // built in grey the client falls back to (client::pane_border_default_style).
    // The old literal "default" parsed to an EMPTY style, which the renderer
    // pins to the terminal default foreground, so unsetting the option left the
    // inactive border colourless while a fresh session drew it grey (#626).
    OptionDef { name: "pane-border-style", scope: Pane, option_type: OptionType::String, default: "fg=brightblack", description: "Inactive pane border style" },
    // Already agreed with AppState::default() and with the renderer fallback.
    OptionDef { name: "pane-active-border-style", scope: Pane, option_type: OptionType::String, default: "fg=green", description: "Active pane border style" },
    OptionDef { name: "pane-border-lines", scope: Pane, option_type: UNVALIDATED_CHOICE, default: "single", description: "Pane border line style (single/double/heavy/simple/number/spaces/none)" },
    // See the `set-option -u` note above (#619).
    OptionDef { name: "pane-border-hover-style", scope: Pane, option_type: OptionType::String, default: "fg=yellow", description: "Pane border style under the mouse pointer" },
];

/// Options validated on assignment but omitted from catalog listing and defaults.
pub static VALIDATION_ONLY_OPTIONS: &[ValidationOnlyOptionDef] = &[
    ValidationOnlyOptionDef { name: "message-limit", option_type: Number(I64) },
    ValidationOnlyOptionDef { name: "history-file-limit", option_type: Number(I64) },
];

/// Ordered names emitted by `show-options -w`, not every Window-scoped catalog entry.
pub static WINDOW_OPTION_NAMES: &[&str] = &[
    "automatic-rename",
    "monitor-activity",
    "monitor-silence",
    "remain-on-exit",
    "window-status-format",
    "window-status-current-format",
    "window-status-separator",
    "window-status-style",
    "window-status-current-style",
    "window-status-activity-style",
    "window-status-bell-style",
    "window-status-last-style",
    "pane-border-indicators",
    "main-pane-width",
    "main-pane-height",
    "window-size",
];

pub fn option_definition(name: &str) -> Option<&'static OptionDef> {
    OPTION_CATALOG.iter().find(|definition| definition.name == name)
}

pub fn validate_option_value(name: &str, value: &str) -> Result<(), String> {
    if let Some(definition) = option_definition(name) {
        return definition.validate_value(value);
    }
    if let Some(definition) = VALIDATION_ONLY_OPTIONS
        .iter()
        .find(|definition| definition.name == name)
    {
        return definition.validate_value(value);
    }
    Ok(())
}

/// True for every name psmux recognises as an option: a catalog entry or a
/// validation only name. The config parser uses it to tell an option its own
/// match does not route (hand it to the runtime setter) from a genuine typo.
pub fn is_known_option(name: &str) -> bool {
    option_definition(name).is_some()
        || VALIDATION_ONLY_OPTIONS.iter().any(|definition| definition.name == name)
}

pub fn validate_option_append(name: &str) -> Result<(), String> {
    option_definition(name).map_or(Ok(()), OptionDef::validate_append)
}

pub fn validate_local_window_override(name: &str, local_window: bool) -> Result<(), String> {
    if !local_window {
        return Ok(());
    }
    option_definition(name).map_or(Ok(()), OptionDef::validate_local_window_override)
}

/// Build the flattened option list for CustomizeMode using live values from AppState.
pub fn build_option_list(app: &crate::types::AppState) -> Vec<(String, String, String)> {
    use crate::server::options::get_option_value;
    OPTION_CATALOG.iter().map(|def| {
        let value = get_option_value(app, def.name);
        (def.name.to_string(), value, def.scope.as_str().to_string())
    }).collect()
}

/// Look up the default value for a given option name.
pub fn default_for(name: &str) -> Option<&'static str> {
    option_definition(name).map(|definition| definition.default)
}

/// Names of the options this catalog marks server scope, sorted.
///
/// tmux keeps a separate server option table (options-table.c,
/// OPTIONS_TABLE_SERVER) and `options_scope_from_flags` points `-s` at it, so a
/// bare `show-options -s` there prints server options only. psmux runs one
/// server per session and keeps a single option store, so `-s` cannot select a
/// different store; it selects this SLICE of the one store, which is the part a
/// caller passing `-s` is actually asking about (#618).
pub fn server_option_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = OPTION_CATALOG
        .iter()
        .filter(|d| d.scope == Server)
        .map(|d| d.name)
        .collect();
    names.sort_unstable();
    names
}

/// True when the catalog marks `name` server scope. `set-option -s` accepts any
/// option name (tmux does the same: for a table option `options_scope_from_name`
/// ignores `-s` entirely rather than erroring), so this only ever narrows a
/// listing, never rejects a write.
pub fn is_server_option(name: &str) -> bool {
    OPTION_CATALOG
        .iter()
        .any(|d| d.name == name && d.scope == Server)
}

#[cfg(test)]
#[path = "../../tests-rs/test_option_default_parity.rs"]
mod tests_option_default_parity;
