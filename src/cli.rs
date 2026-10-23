use crate::types::{ParsedTarget, VERSION, build_version_string};

// ---------------------------------------------------------------------------
// Dangling value-taking flags (issue #635)
//
// tmux rejects `kill-window -t` (a value-taking flag with nothing after it)
// GENERICALLY, in arguments.c:
//
//     xasprintf(cause, "-%c expects an argument", flag);
//     return (-1);
//
// The command never runs.  psmux instead used the idiom
// `if let Some(v) = args.get(i + 1)` at ~60 call sites, so a valueless flag
// was silently dropped and the command executed against the DEFAULT target:
// `kill-window -t` killed the current window and `kill-session -t` destroyed
// the current session (PSMUX_TARGET_SESSION is inherited by pane children),
// both at exit 0.  The realistic trigger is ordinary shell scripting --
// `psmux kill-session -t $TARGET` with `$TARGET` unset.
//
// The fix mirrors tmux: ONE table of per-command flag templates and ONE
// validator, called from every command ingress.
// ---------------------------------------------------------------------------

/// Per-command flag templates, transcribed verbatim from the `.args` field of
/// each `struct cmd_entry` in tmux's `cmd-*.c`.
///
/// Template grammar (tmux `args_parse_flags`): a bare letter is a boolean
/// flag, `X:` takes a REQUIRED value, `X::` takes an OPTIONAL value.
///
/// Entries are `(command, alias, template)`.  Extra psmux aliases that tmux
/// does not define are listed in [`EXTRA_ALIASES`].
const ARGS_TEMPLATES: &[(&str, &str, &str)] = &[
    ("attach-session",        "attach",     "c:dEf:rt:x"),
    ("bind-key",              "bind",       "nrN:T:"),
    ("break-pane",            "breakp",     "abdPF:n:s:t:Wx:X:y:Y:"),
    ("capture-pane",          "capturep",   "ab:CeE:FHJLMNpPqRS:Tt:"),
    ("choose-buffer",         "chooseb",    "F:f:K:kNO:rt:yZ"),
    ("choose-client",         "",           "F:f:hiK:kNO:rt:yZ"),
    ("choose-tree",           "",           "F:f:GhK:kNO:rst:wyZ"),
    ("clear-history",         "clearhist",  "Ht:"),
    ("clear-prompt-history",  "clearphist", "T:"),
    ("clock-mode",            "",           "t:"),
    ("command-prompt",        "",           "1CbeFiklI:NPp:t:T:"),
    ("confirm-before",        "confirm",    "bc:p:t:y"),
    ("copy-mode",             "",           "dekHMqSs:t:u"),
    ("customize-mode",        "",           "F:f:kNt:yZ"),
    ("delete-buffer",         "deleteb",    "b:"),
    ("detach-client",         "detach",     "aE:s:t:P"),
    ("display-menu",          "menu",       "b:c:C:H:s:S:MOt:T:x:y:"),
    ("display-message",       "display",    "aCc:d:lINpt:F:v"),
    ("display-panes",         "displayp",   "d:kNs:t:Z"),
    ("display-popup",         "popup",      "Bb:Cc:d:e:Eh:kNs:S:t:T:w:x:y:"),
    ("find-window",           "findw",      "CiNrt:TZ"),
    ("has-session",           "has",        "t:"),
    ("if-shell",              "if",         "bFt:"),
    ("join-pane",             "joinp",      "bdfhvp:l:s:t:"),
    ("kill-pane",             "killp",      "af:t:"),
    ("kill-server",           "",           ""),
    ("kill-session",          "",           "aCgf:t:"),
    ("kill-window",           "killw",      "af:t:"),
    ("last-pane",             "lastp",      "det:Z"),
    ("last-window",           "last",       "t:"),
    ("link-window",           "linkw",      "abdks:t:"),
    ("list-buffers",          "lsb",        "F:f:O:r"),
    ("list-clients",          "lsc",        "F:f:O:rt:"),
    ("list-commands",         "lscm",       "F:"),
    ("list-keys",             "lsk",        "1aF:NO:P:rT:"),
    ("list-panes",            "lsp",        "aF:f:O:rst:"),
    ("list-sessions",         "ls",         "F:f:O:r"),
    ("list-windows",          "lsw",        "aF:f:O:rt:"),
    ("load-buffer",           "loadb",      "b:t:w"),
    ("lock-client",           "lockc",      "t:"),
    ("lock-server",           "lock",       ""),
    ("lock-session",          "locks",      "t:"),
    ("move-pane",             "movep",      "bdD::fhMvl:L::P:R::s:t:U::X:Y:z:"),
    ("move-window",           "movew",      "abdkrs:t:"),
    ("new-session",           "new",        "Ac:dDe:EF:f:n:Ps:t:x:Xy:"),
    ("new-window",            "neww",       "abc:de:EF:kn:PSt:"),
    ("next-layout",           "nextl",      "t:"),
    ("next-window",           "next",       "at:"),
    ("paste-buffer",          "pasteb",     "db:prSs:t:"),
    ("pipe-pane",             "pipep",      "IOot:"),
    ("previous-layout",       "prevl",      "t:"),
    ("previous-window",       "prev",       "at:"),
    ("refresh-client",        "refresh",    "A:B:cC:Df:r:F:lLRSt:U"),
    ("rename-session",        "rename",     "t:"),
    ("rename-window",         "renamew",    "t:"),
    ("resize-pane",           "resizep",    "D::L::MR::Tt:U::x:y:Z"),
    ("resize-window",         "resizew",    "aADLRt:Ux:y:"),
    ("respawn-pane",          "respawnp",   "c:e:Ekt:"),
    ("respawn-window",        "respawnw",   "c:e:Ekt:"),
    ("rotate-window",         "rotatew",    "Dt:UZ"),
    ("run-shell",             "run",        "bd:Ct:Es:c:"),
    ("save-buffer",           "saveb",      "ab:"),
    ("select-layout",         "selectl",    "Enopt:"),
    ("select-pane",           "selectp",    "DdegLlMmP:RT:t:UZ"),
    ("select-window",         "selectw",    "lnpTt:"),
    ("send-keys",             "send",       "c:FHKlMN:Rt:X"),
    ("send-prefix",           "",           "2t:"),
    ("server-access",         "",           "adglrw"),
    ("set-buffer",            "setb",       "ab:t:n:w"),
    ("set-environment",       "setenv",     "Fhgrt:u"),
    ("set-hook",              "",           "agpERTt:uB:w"),
    ("set-option",            "set",        "aFgopqst:uUw"),
    ("set-window-option",     "setw",       "aFgoqt:u"),
    ("show-buffer",           "showb",      "b:"),
    ("show-environment",      "showenv",    "hgst:"),
    ("show-hooks",            "",           "Bgpt:w"),
    ("show-messages",         "showmsgs",   "JTt:"),
    ("show-options",          "show",       "AgHpqst:vw"),
    ("show-prompt-history",   "showphist",  "T:"),
    ("show-window-options",   "showw",      "gvt:"),
    ("source-file",           "source",     "t:Fnqv"),
    ("split-window",          "splitw",     "bB:c:de:EfF:hIkl:m:p:PR:s:S:t:T:vWZ"),
    ("start-server",          "start",      ""),
    ("suspend-client",        "suspendc",   "t:"),
    ("swap-pane",             "swapp",      "dDs:t:UZ"),
    ("swap-window",           "swapw",      "ds:t:"),
    ("switch-client",         "switchc",    "c:EFlnO:pt:rT:Z"),
    ("switch-mode",           "",           "F:kst:wZ"),
    ("unbind-key",            "unbind",     "anqT:"),
    ("unlink-window",         "unlinkw",    "kt:"),
    ("wait-for",              "wait",       "EF:LSUlvw:"),
    ("new-pane",              "newp",       "bB:c:de:EfF:hIkl:LMm:Op:PR:s:S:t:T:vWx:X:y:Y:Z"),
    // psmux-only commands.  tmux has no cmd_entry for these, so the templates
    // come from psmux's own handlers (docs/tmux_args_reference.md is the
    // per-command flag list psmux actually parses).
    ("choose-session",        "",           "F:f:K:kNO:rt:yZ"),
    ("choose-window",         "",           "F:f:K:kNO:rt:yZ"),
    ("server-info",           "info",       ""),
    ("send-paste",            "",           "t:"),
];

/// Value-taking flags psmux adds to a command that tmux's template does not
/// declare.  Kept separate so ARGS_TEMPLATES stays a verbatim transcription of
/// tmux and every psmux-specific rule is visible in one place.
const PSMUX_EXTRA_FLAGS: &[(&str, &str)] = &[
    // psmux's new-window takes `-T <title>`; tmux's does not.
    ("new-window", "T:"),
    // psmux routes `-t <target>` on these three; tmux's templates have no `t`.
    ("unbind-key", "t:"),
    ("list-buffers", "t:"),
    ("list-keys", "t:"),
];

/// Aliases psmux accepts that tmux does not define, mapped to the canonical
/// command whose template they share (docs/tmux_args_reference.md).
const EXTRA_ALIASES: &[(&str, &str)] = &[
    ("split-pane", "split-window"),
    ("splitp", "split-window"),
    ("kill-ses", "kill-session"),
    ("a", "attach-session"),
    ("at", "attach-session"),
    ("resp", "respawn-pane"),
    ("send-key", "send-keys"),
    ("show-option", "show-options"),
    ("show-window-option", "show-window-options"),
    ("warmup", "start-server"),
];

/// How a flag letter consumes a value, per its command's tmux template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlagValue {
    /// Boolean flag, or a letter this command does not declare at all.
    None,
    /// `X:` — tmux errors with "-X expects an argument" when it is dangling.
    Required,
    /// `X::` — tmux silently records "no value" when it is dangling, and also
    /// when the next token looks like another flag.
    Optional,
}

/// Resolve a psmux-only alias to the command whose template it shares.
fn canonical_command(command: &str) -> &str {
    EXTRA_ALIASES
        .iter()
        .find(|(alias, _)| *alias == command)
        .map(|(_, target)| *target)
        .unwrap_or(command)
}

/// The canonical entry `(name, alias, template)` for `command`, if any.
fn args_entry(command: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    let canonical = canonical_command(command);
    ARGS_TEMPLATES
        .iter()
        .find(|(name, alias, _)| *name == canonical || (!alias.is_empty() && *alias == canonical))
}

/// The tmux `.args` template for `command`, resolving aliases.
pub fn args_template(command: &str) -> Option<&'static str> {
    args_entry(command).map(|(_, _, template)| *template)
}

/// Value-taking flags psmux adds to `command` on top of tmux's template.
fn psmux_extra_flags(command: &str) -> &'static str {
    let Some((name, _, _)) = args_entry(command) else { return "" };
    PSMUX_EXTRA_FLAGS
        .iter()
        .find(|(cmd, _)| cmd == name)
        .map(|(_, extra)| *extra)
        .unwrap_or("")
}

/// Classify `flag` against `template`, mirroring tmux's
/// `strchr(parse->template, flag)` plus its `found[1] == ':'` /
/// `found[2] == ':'` tests.
fn flag_value_kind(template: &str, flag: char) -> FlagValue {
    let bytes = template.as_bytes();
    // Only ASCII alphanumerics are flag letters in tmux; a ':' in the
    // template is punctuation and must never be matched as a flag.
    if !flag.is_ascii_alphanumeric() {
        return FlagValue::None;
    }
    let needle = flag as u8;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == needle {
            // Skip a letter that is really the tail of a `::` run? There is
            // none: `:` is never a flag letter, so a match here is genuine.
            return if bytes.get(i + 1) == Some(&b':') {
                if bytes.get(i + 2) == Some(&b':') {
                    FlagValue::Optional
                } else {
                    FlagValue::Required
                }
            } else {
                FlagValue::None
            };
        }
        i += 1;
    }
    FlagValue::None
}

/// True when `token` is what tmux's optional-value path treats as "another
/// flag" rather than a value: `-` followed by `-` or an alphabetic character
/// (`args_parse_flag_argument`: `as[0] == '-' && (as[1] == '-' || isalpha(as[1]))`).
/// A negative NUMBER (`-5`) deliberately fails this test, so `resize-pane -x -5`
/// still passes `-5` through as the value.
fn looks_like_another_flag(token: &str) -> bool {
    let b = token.as_bytes();
    b.len() >= 2 && b[0] == b'-' && (b[1] == b'-' || b[1].is_ascii_alphabetic())
}

/// Reject a value-taking flag that has no value, exactly where tmux does.
///
/// `command` is the command name (canonical or alias); `args` is everything
/// AFTER it. Returns `Err("-t expects an argument")` for the offending flag,
/// or `Ok(())` when the command line is well formed (including when the
/// command is unknown to the table, so nothing new is ever rejected).
///
/// Faithful to `args_parse_flags`:
///   * flag scanning stops at the first token that is not `-`-prefixed, at a
///     bare `-`, and at `--`;
///   * clustered short flags (`-at`) are walked letter by letter;
///   * an attached value (`-tfoo`) satisfies the flag;
///   * a REQUIRED value consumes the next token WHATEVER it looks like, so
///     `kill-window -t -a` uses `-a` as the target (tmux then reports
///     "can't find window: -a") rather than erroring here;
///   * an OPTIONAL value (`::`) is never mandatory, and is not consumed when
///     the next token looks like another flag.
pub fn validate_flag_arguments<S: AsRef<str>>(command: &str, args: &[S]) -> Result<(), String> {
    let Some(template) = args_template(command) else {
        return Ok(());
    };
    let extra = psmux_extra_flags(command);
    let kind_of = |flag: char| match flag_value_kind(template, flag) {
        FlagValue::None => flag_value_kind(extra, flag),
        kind => kind,
    };
    let mut i = 0;
    while i < args.len() {
        let token = args[i].as_ref();
        if !token.starts_with('-') || token == "-" || token == "--" {
            break; // positional, bare '-', or end-of-options: flags are over
        }
        i += 1; // the flag token itself is consumed
        let letters: Vec<char> = token.chars().skip(1).collect();
        let mut k = 0;
        while k < letters.len() {
            let flag = letters[k];
            k += 1;
            match kind_of(flag) {
                FlagValue::None => continue, // boolean, or not ours to police
                kind => {
                    if k < letters.len() {
                        break; // attached value: `-tfoo`, `-atfoo`
                    }
                    match args.get(i).map(|a| a.as_ref()) {
                        Some(next) => {
                            if !(kind == FlagValue::Optional && looks_like_another_flag(next)) {
                                i += 1; // the next token is this flag's value
                            }
                        }
                        None => {
                            if kind == FlagValue::Required {
                                return Err(format!("-{} expects an argument", flag));
                            }
                        }
                    }
                    break;
                }
            }
        }
    }
    Ok(())
}

/// The first flag letter in `args` (the tokens after the command name) that
/// `command`'s template does not declare, read the way tmux's args_parse
/// reads them: only the flag region counts (it ends at `--`, a bare `-` and
/// the first operand), a value-taking letter ends its cluster and takes the
/// rest of the token or the next token. tmux refuses such a command with
/// "unknown flag -X"; psmux used to print the token as message text
/// (`display -p -x hello` printed `-x hello`). `None` for a command the table
/// does not know.
pub fn unknown_flag<S: AsRef<str>>(command: &str, args: &[S]) -> Option<char> {
    let template = args_template(command)?;
    let extra = psmux_extra_flags(command);
    let known = |flag: char| flag.is_ascii_alphanumeric() && (template.contains(flag) || extra.contains(flag));
    let kind_of = |flag: char| match flag_value_kind(template, flag) {
        FlagValue::None => flag_value_kind(extra, flag),
        kind => kind,
    };
    let mut i = 0;
    while i < args.len() {
        let token = args[i].as_ref();
        if !token.starts_with('-') || token == "-" || token == "--" {
            return None;
        }
        i += 1;
        let letters: Vec<char> = token.chars().skip(1).collect();
        for (k, &flag) in letters.iter().enumerate() {
            if !known(flag) {
                return Some(flag);
            }
            let kind = kind_of(flag);
            if kind == FlagValue::None {
                continue;
            }
            if k + 1 == letters.len() {
                if let Some(next) = args.get(i).map(|a| a.as_ref()) {
                    if !(kind == FlagValue::Optional && looks_like_another_flag(next)) {
                        i += 1;
                    }
                }
            }
            break;
        }
    }
    None
}

/// `display-message` arguments read the way tmux's args_parse reads them with
/// the template `aCc:d:lINpt:F:v` (cmd-display-message.c).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DisplayMessageArgs {
    /// `-a`: list the format variables and their values.
    pub list_all: bool,
    /// `-C`: keep updating the pane while the message shows (psmux never
    /// freezes a pane for a message, so this is what it always does).
    pub no_freeze: bool,
    /// `-I`: forward stdin to an empty pane.
    pub input: bool,
    /// `-l`: print the message unchanged, no format expansion.
    pub literal: bool,
    /// `-N`: key presses do not close the message.
    pub ignore_keys: bool,
    /// `-p`: print to stdout instead of the status line.
    pub print: bool,
    /// `-v`: log the format expansion.
    pub verbose: bool,
    pub client: Option<String>,
    pub delay: Option<String>,
    pub target: Option<String>,
    /// `-F format`: the template given as a flag instead of an argument.
    pub format: Option<String>,
    /// The operands. tmux takes at most one; psmux has always joined extra
    /// words with a space, which is kept.
    pub message: Vec<String>,
}

impl DisplayMessageArgs {
    /// The template to expand: the argument, else `-F`, else `None` for the
    /// default (cmd-display-message.c: `template = args_string(args, 0)` /
    /// `args_get(args, 'F')`).
    pub fn template(&self) -> Option<String> {
        if !self.message.is_empty() {
            Some(self.message.join(" "))
        } else {
            self.format.clone()
        }
    }
}

/// The server's error in a `display-message -p` reply, if the reply is one
/// (#865). The one shot reply carries output and errors as the same text, so
/// a printed message that starts with "ERROR" used to exit 1 with the text on
/// stderr, where tmux prints it and exits 0. The CLI parses the flags and
/// checks the target before it sends, so the only errors the server can still
/// answer here are a target that went away in between (one line,
/// `ERROR: can't find ...`) and the few refusals listed below. Anything else
/// is output. A template the server
/// prints as written (no `#`, or `-l`) is output even when it reads like that
/// error, since the reply is exactly that text.
pub fn display_reply_error(reply: &str, template: &str, literal: bool) -> Option<String> {
    let line = reply.trim_end_matches(['\r', '\n']);
    if line.contains('\n') {
        return None;
    }
    let message = line.trim_start().strip_prefix("ERROR:")?.trim();
    // What the server's target check (connection.rs validate_target) and
    // its AUTH step can answer before the command runs.
    const SERVER_ERRORS: &[&str] = &[
        "can't find ",
        "server is shutting down",
        "no response from server",
        "Authentication required",
        "Invalid session key",
    ];
    if !SERVER_ERRORS.iter().any(|p| message.starts_with(p)) {
        return None;
    }
    if (literal || !template.contains('#'))
        && line == crate::util::visual_escape_message(template)
    {
        return None;
    }
    Some(message.to_string())
}

/// Parse `display-message` arguments (everything after the command name).
/// Errors are tmux's: "unknown flag -X", "-X expects an argument", and "only
/// one of -F or argument must be given".
pub fn parse_display_message_args<S: AsRef<str>>(args: &[S]) -> Result<DisplayMessageArgs, String> {
    if let Some(flag) = unknown_flag("display-message", args) {
        return Err(format!("unknown flag -{}", flag));
    }
    validate_flag_arguments("display-message", args)?;
    let mut out = DisplayMessageArgs::default();
    let mut i = 0;
    while i < args.len() {
        let token = args[i].as_ref();
        if token == "--" {
            i += 1;
            break;
        }
        if !token.starts_with('-') || token == "-" {
            break;
        }
        i += 1;
        let body = &token[1..];
        for (pos, flag) in body.char_indices() {
            let rest = &body[pos + flag.len_utf8()..];
            let mut take_value = || -> String {
                if !rest.is_empty() {
                    rest.to_string()
                } else {
                    let v = args.get(i).map(|a| a.as_ref().to_string()).unwrap_or_default();
                    i += 1;
                    v
                }
            };
            match flag {
                'a' => out.list_all = true,
                'C' => out.no_freeze = true,
                'I' => out.input = true,
                'l' => out.literal = true,
                'N' => out.ignore_keys = true,
                'p' => out.print = true,
                'v' => out.verbose = true,
                'c' => { out.client = Some(take_value()); break; }
                'd' => { out.delay = Some(take_value()); break; }
                't' => { out.target = Some(take_value()); break; }
                'F' => { out.format = Some(take_value()); break; }
                _ => {}
            }
        }
    }
    out.message = args[i.min(args.len())..].iter().map(|a| a.as_ref().to_string()).collect();
    if out.format.is_some() && !out.message.is_empty() {
        return Err("only one of -F or argument must be given".to_string());
    }
    Ok(out)
}

/// The format variables `display-message -a` lists, sorted like tmux's: the
/// names tmux 3.4 prints for `display -p -a` (measured) that psmux's format
/// expander also knows. Each is printed as `name=value`.
pub const DISPLAY_ALL_VARIABLES: &[&str] = &[
    "alternate_on", "alternate_saved_x", "alternate_saved_y", "command", "config_files",
    "cursor_character", "cursor_flag", "cursor_x", "cursor_y", "history_bytes", "history_limit",
    "history_size", "host", "host_short", "insert_flag", "keypad_cursor_flag", "keypad_flag",
    "mouse_all_flag", "mouse_any_flag", "mouse_button_flag", "mouse_sgr_flag",
    "mouse_standard_flag", "mouse_utf8_flag", "origin_flag", "pane_active", "pane_at_bottom",
    "pane_at_left", "pane_at_right", "pane_at_top", "pane_bg", "pane_bottom",
    "pane_current_command", "pane_current_path", "pane_dead", "pane_fg", "pane_format",
    "pane_height", "pane_id", "pane_in_mode", "pane_index", "pane_input_off", "pane_last",
    "pane_left", "pane_marked", "pane_marked_set", "pane_path", "pane_pid", "pane_pipe",
    "pane_right", "pane_search_string", "pane_start_command", "pane_start_path",
    "pane_synchronized", "pane_tabs", "pane_title", "pane_top", "pane_tty",
    "pane_unseen_changes", "pane_width", "pid", "scroll_region_lower", "scroll_region_upper",
    "server_sessions", "session_activity", "session_alerts", "session_attached",
    "session_created", "session_format", "session_grouped", "session_id",
    "session_last_attached", "session_many_attached", "session_name", "session_path",
    "session_stack", "session_windows", "socket_path", "start_time", "user", "version",
    "window_active", "window_activity", "window_activity_flag", "window_bell_flag",
    "window_cell_height", "window_cell_width", "window_end_flag", "window_flags",
    "window_format", "window_height", "window_id", "window_index", "window_last_flag",
    "window_layout", "window_linked", "window_linked_sessions", "window_linked_sessions_list",
    "window_name", "window_panes", "window_raw_flags", "window_silence_flag",
    "window_stack_index", "window_start_flag", "window_visible_layout", "window_width",
    "window_zoomed_flag", "wrap_flag",
];

/// Same check for a whole command LINE (command name first), used by the
/// config-file and key-binding routes where the command arrives as a string.
pub fn validate_command_line_flags<S: AsRef<str>>(tokens: &[S]) -> Result<(), String> {
    let Some(command) = tokens.first() else {
        return Ok(());
    };
    validate_flag_arguments(command.as_ref(), &tokens[1..])
}

/// True when `-<flag>` takes a value for `command`, per [`ARGS_TEMPLATES`].
///
/// Same classification [`validate_flag_arguments`] applies, exposed so a parser
/// that has to step OVER a flag's value cannot drift from the one flag table.
/// Without it a hand-written parser has to keep its own list of value-taking
/// flags, and the flag it forgets turns its value into a positional argument.
pub fn flag_takes_value(command: &str, flag: char) -> bool {
    let Some(template) = args_template(command) else {
        return false;
    };
    let kind = match flag_value_kind(template, flag) {
        FlagValue::None => flag_value_kind(psmux_extra_flags(command), flag),
        kind => kind,
    };
    kind != FlagValue::None
}

/// The first operand of `command` in `args` (the tokens after the command
/// name), read the way tmux's getopt reads them (arguments.c:207): flags end at
/// `--`, at a bare `-` or at the first token that does not start with `-`, a
/// value-taking flag consumes the rest of its token or the token after it, and
/// one token can carry several flags.
///
/// `rename-window -- '%%'` is the shape that needs it: tmux names the window
/// after the operand, where reading the first token named it `--`.
pub fn first_operand<'a>(command: &str, args: &[&'a str]) -> Option<&'a str> {
    let mut i = 0;
    while i < args.len() {
        let token = args[i];
        if token == "--" {
            return args.get(i + 1).copied();
        }
        let Some(rest) = token.strip_prefix('-').filter(|rest| !rest.is_empty()) else {
            return Some(token);
        };
        for (at, flag) in rest.char_indices() {
            if flag_takes_value(command, flag) {
                if at + flag.len_utf8() == rest.len() {
                    i += 1;
                }
                break;
            }
        }
        i += 1;
    }
    None
}

/// Split clustered short flags into one token per flag, as tmux's getopt
/// reads them: `-dt %1` becomes `-d -t %1`, and `-t%1` becomes `-t %1`.
///
/// For the client arms that rebuild a command line from exact tokens
/// (`select-pane`, `last-pane`), which otherwise drop a cluster without a word
/// (`select-pane -dt %2` reached the server as a bare `select-pane`).  A
/// value-taking flag ends its cluster and takes the rest of the word or the
/// next argument; `--` and the first non-option end the flags, and what follows
/// is passed through untouched.  Only for commands whose templates have no
/// OPTIONAL (`::`) values, which both callers satisfy.
pub fn expand_flag_clusters<S: AsRef<str>>(command: &str, args: &[S]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_ref();
        if a == "--" || !a.starts_with('-') || a.len() < 2 {
            out.extend(args[i..].iter().map(|s| s.as_ref().to_string()));
            break;
        }
        for (pos, c) in a.char_indices().skip(1) {
            out.push(format!("-{}", c));
            if flag_takes_value(command, c) {
                let rest = &a[pos + c.len_utf8()..];
                if !rest.is_empty() {
                    out.push(rest.to_string());
                } else if let Some(v) = args.get(i + 1) {
                    out.push(v.as_ref().to_string());
                    i += 1;
                }
                break;
            }
        }
        i += 1;
    }
    out
}

/// Normalize `-x=VALUE` short-flag forms into `["-x", "VALUE"]`, in FLAG
/// position only.
///
/// psmux accepts `-t=VALUE` as a spelling of `-t VALUE` (issue #196:
/// `has-session -t=NAME`).  The split applies only where tmux's `args_parse`
/// would read a flag: the global options before the subcommand, and each
/// command's flag region, which ends at `--`, at a bare `-`, or at the first
/// operand (arguments.c).  A value-taking flag's own value is never touched
/// either (`new-window -n -x=foo` names the window `-x=foo`).  Everything
/// from the first operand on is data: `set @k -x=foo` stores `-x=foo`
/// verbatim, as tmux does (issue #795; before it the value was rewritten to
/// `-x foo` at rc 0).
///
/// Shape of `args`: either argv (binary name first, then global flags, then
/// the subcommand) or a command line whose first token is the command.  The
/// first token is taken as the command when [`args_template`] knows it.
///
/// Token rules (unchanged from #196):
///   - Only tokens starting with a single `-` (not `--`) are split.
///   - The flag letter must be ASCII alphabetic (`-t=foo` yes, `-1=bar` no).
///   - Long flags (`--name=value`), bare `-` and degenerate `-=` pass through.
pub fn normalize_flag_equals(args: Vec<String>) -> Vec<String> {
    normalize_argv_flags(args, true, false)
}

/// The flag letter of a `-X=value` token, when `token` has that shape.
fn equals_flag_letter(token: &str) -> Option<char> {
    let b = token.as_bytes();
    (b.len() >= 4 && b[0] == b'-' && b[1].is_ascii_alphabetic() && b[2] == b'=')
        .then(|| b[1] as char)
}

/// Does `token` end a command in an argv command list (`a \; b`)?  tmux's
/// `cmd_parse_from_arguments` ends a command at an argument that ends in `;`
/// unless the `;` is escaped (cmd-parse.y).  A lone `\;` is a separator too,
/// as in [`split_command_queue`]: a Windows shell passes it through unchanged.
fn ends_command(token: &str) -> bool {
    token == "\\;" || (token.ends_with(';') && !token.ends_with("\\;"))
}

/// Shared walker for [`normalize_flag_equals`] and
/// [`normalize_attached_target_flag`]: rewrites only flag-position tokens.
fn normalize_argv_flags(args: Vec<String>, equals: bool, attached_t: bool) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len() + 2);
    let mut i = 0;
    // argv shape: a first token that is neither a flag nor a known command is
    // the binary name.
    if let Some(first) = args.first() {
        if !first.starts_with('-') && args_template(first).is_none() {
            out.push(first.clone());
            i = 1;
        }
    }
    // Global region: the flags before the subcommand.
    while i < args.len() {
        let tok = &args[i];
        if !tok.starts_with('-') || tok == "-" {
            break;
        }
        if tok == "--" {
            out.push(tok.clone());
            i += 1;
            break;
        }
        i += 1;
        if equals {
            if let Some(c) = equals_flag_letter(tok) {
                out.push(format!("-{}", c));
                out.push(tok[3..].to_string());
                continue;
            }
        }
        out.push(tok.clone());
        // A detached value of a value-taking global option is not the
        // subcommand (`-L ns`).
        if tok.len() == 2 && matches!(tok.as_bytes()[1], b'L' | b'f' | b'S' | b't') {
            if let Some(v) = args.get(i) {
                out.push(v.clone());
                i += 1;
            }
        }
    }
    // Commands: each runs to the end of argv or to a `;` terminator.
    while i < args.len() {
        let command = args[i].clone();
        out.push(command.clone());
        i += 1;
        if ends_command(&command) {
            continue;
        }
        let end = args[i..]
            .iter()
            .position(|t| ends_command(t))
            .map(|p| i + p + 1)
            .unwrap_or(args.len());
        normalize_command_flags(&command, &args[i..end], equals, attached_t, &mut out);
        i = end;
    }
    out
}

/// Rewrite one command's flag region (`args` is everything after the command
/// name, up to its terminator).  See [`normalize_flag_equals`].
fn normalize_command_flags(
    command: &str,
    args: &[String],
    equals: bool,
    attached_t: bool,
    out: &mut Vec<String>,
) {
    let Some(template) = args_template(command) else {
        // A command the flag table does not know (user alias, typo): its
        // operands cannot be told apart, so keep the #196 behaviour up to
        // `--`.  The command itself errors as unknown anyway.
        let mut literal = false;
        for tok in args {
            if tok == "--" {
                literal = true;
            }
            if !literal {
                if equals {
                    if let Some(c) = equals_flag_letter(tok) {
                        out.push(format!("-{}", c));
                        out.push(tok[3..].to_string());
                        continue;
                    }
                }
                if attached_t && tok.len() > 2 && tok.starts_with("-t") && !tok.starts_with("--") {
                    out.push("-t".to_string());
                    out.push(tok[2..].to_string());
                    continue;
                }
            }
            out.push(tok.clone());
        }
        return;
    };
    let extra = psmux_extra_flags(command);
    let kind_of = |flag: char| match flag_value_kind(template, flag) {
        FlagValue::None => flag_value_kind(extra, flag),
        kind => kind,
    };
    let mut i = 0;
    while i < args.len() {
        let tok = args[i].as_str();
        if tok == "--" || tok == "-" || !tok.starts_with('-') {
            break; // end of options, or the first operand: the rest is data
        }
        i += 1;
        if equals {
            // Only a value-taking letter is split: `-u=1` for a boolean `-u`
            // (or a letter the command does not have) would otherwise turn
            // into a flag plus a stray operand. Left whole, the command's
            // parser sees the token as written.
            if let Some(c) = equals_flag_letter(tok).filter(|c| kind_of(*c) != FlagValue::None) {
                out.push(format!("-{}", c));
                out.push(tok[3..].to_string());
                continue;
            }
        }
        if attached_t
            && tok.len() > 2
            && tok.starts_with("-t")
            && !tok.starts_with("--")
            && kind_of('t') != FlagValue::None
        {
            out.push("-t".to_string());
            out.push(tok[2..].to_string());
            continue;
        }
        out.push(tok.to_string());
        // Walk the cluster: a value-taking letter ends it and takes the rest
        // of the token, or the NEXT token verbatim (tmux consumes a required
        // value whatever it looks like: `-n -x=foo` names a window `-x=foo`).
        let rest = &tok[1..];
        for (pos, flag) in rest.char_indices() {
            let kind = kind_of(flag);
            if kind == FlagValue::None {
                continue;
            }
            if pos + flag.len_utf8() == rest.len() {
                if let Some(next) = args.get(i) {
                    if !(kind == FlagValue::Optional && looks_like_another_flag(next)) {
                        out.push(next.clone());
                        i += 1;
                    }
                }
            }
            break;
        }
    }
    out.extend(args[i..].iter().cloned());
}

/// Split tmux-style ATTACHED short-option arguments in the GLOBAL (pre
/// subcommand) region of the command line: `-Lsockname` -> `-L sockname`
/// (discussion #571).
///
/// tmux parses its program options with getopt(3) (tmux.c: spec string
/// `2c:CDdf:hlL:NqS:T:uUvV`), and its bundled compat getopt treats
/// characters left attached after a value-taking option letter as that
/// option's argument (`optarg = place` in the "no white space" branch of
/// compat/getopt_long.c), so `-L sockname` and `-Lsockname` are
/// equivalent.  Tools that drive tmux emit the
/// attached form (libtmux builds `-L{name}`), and psmux's exact-match
/// scanners treated the whole token as an unknown flag and SILENTLY
/// dropped it — a `-Lfoo new-session` landed in the DEFAULT namespace.
///
/// Rules:
///   - Only the region BEFORE the subcommand is rewritten; the first token
///     not starting with `-` ends the scan, so command flags such as
///     `select-pane -L` (no value) are never touched.
///   - Only the value-taking global option letters psmux's own scanners
///     consume are split: L, f, S, and psmux's global `-t`.  Letters psmux
///     does not handle globally (tmux's `-c`/`-T`) pass through unchanged —
///     splitting those would leave a stray value token that the scanner
///     mistakes for the subcommand.  `-C`/`-CC` and boolean flags pass
///     through unchanged.
///   - A detached form (`-L name`) passes through unchanged: the letter is
///     followed by nothing, so there is no attached value to split.
pub fn normalize_attached_global_args(args: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut in_globals = true;
    let mut skip_value = false;
    for (idx, arg) in args.into_iter().enumerate() {
        // args[0] is the binary name, not the subcommand.
        if idx > 0 && in_globals {
            if skip_value {
                // Previous token was a detached value-taking flag: this token
                // is its value, not the subcommand.
                skip_value = false;
                out.push(arg);
                continue;
            }
            if !arg.starts_with('-') || arg == "-" {
                in_globals = false; // subcommand (or bare -): stop rewriting
                out.push(arg);
                continue;
            }
            let bytes = arg.as_bytes();
            if bytes.len() >= 3
                && !arg.starts_with("--")
                && matches!(bytes[1], b'L' | b'f' | b'S' | b't')
            {
                out.push(format!("-{}", bytes[1] as char));
                out.push(arg[2..].to_string());
                continue;
            }
            if bytes.len() == 2 && matches!(bytes[1], b'L' | b'f' | b'S' | b't') {
                skip_value = true; // detached form: the NEXT token is the value
            }
        }
        out.push(arg);
    }
    out
}

/// Split tmux-style ATTACHED `-t` target arguments AFTER the subcommand:
/// `kill-session -tname` -> `kill-session -t name` (discussion #571,
/// command-level slice).
///
/// tmux's command parser (arguments.c args_parse_flag_argument: the
/// `if (*string != '\0')` branch) treats characters left attached after a
/// value-taking flag letter as that flag's argument.  psmux's exact-match
/// command parsers ignored such tokens, and the global routing fallback then
/// picked the most recent session: `kill-session -tvictimA` killed victimB.
///
/// Rules:
///   - Only the command's FLAG REGION is rewritten, read from its tmux flag
///     template: it ends at `--`, at a bare `-` and at the first operand, and
///     a value-taking flag's value is skipped.  tmux's `args_parse` stops at
///     the first operand too, so `set @k -tfoo` stores `-tfoo` and
///     `send-keys -t a -- -tliteral` sends `-tliteral` (issue #795; this pass
///     used to split every `-t...` token up to `--`).
///   - The global region is `normalize_attached_global_args`'s job.
///   - A bare `-t` (detached form) and every other flag pass through.
pub fn normalize_attached_target_flag(args: Vec<String>) -> Vec<String> {
    normalize_argv_flags(args, false, true)
}

pub fn deferred_command_start<S: AsRef<str>>(command: &str, args: &[S]) -> Option<usize> {
    let value_options: &[&str] = match command {
        "bind-key" | "bind" => &["-T", "-N"],
        "set-hook" => &["-t"],
        "confirm-before" | "confirm" => &["-p", "-t"],
        _ => return None,
    };

    let mut i = 0;
    while i < args.len() {
        let token = args[i].as_ref();
        if token == "--" {
            i += 1;
            break;
        }
        if !token.starts_with('-') || token == "-" {
            break;
        }
        i += if value_options.contains(&token) { 2 } else { 1 };
    }

    match command {
        "bind-key" | "bind" | "set-hook" => (i < args.len()).then_some(i + 1),
        "confirm-before" | "confirm" => (i < args.len()).then_some(i),
        _ => None,
    }
}

pub fn outer_target_scan_end<S: AsRef<str>>(command: &str, args: &[S]) -> usize {
    deferred_command_start(command, args)
        .into_iter()
        .chain(args.iter().position(|arg| arg.as_ref() == "--"))
        .min()
        .unwrap_or(args.len())
}

/// Split a command line that is already in argv form into its commands, the
/// way tmux's `cmd_parse_from_arguments` does (cmd-parse.y:1063-1130): an
/// argument ending in `;` ends the current command, the text before the `;`
/// (if any) staying its last argument, and `\;` at the end of an argument is a
/// literal `;`. Empty commands are dropped. Used for the queue a
/// `start-server` carries (issue #734).
pub fn split_command_queue<S: AsRef<str>>(args: &[S]) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for arg in args {
        let mut copy = arg.as_ref().to_string();
        let mut end = false;
        if copy == "\\;" {
            // A lone `\;` is the separator a Windows shell passes through
            // unchanged where a POSIX shell would have removed the
            // backslash; psmux's own command splitter reads it the same way.
            commands.push(std::mem::take(&mut current));
            continue;
        }
        if copy.ends_with(';') {
            copy.pop();
            if copy.ends_with('\\') {
                copy.pop();
                copy.push(';');
            } else {
                end = true;
            }
        }
        if !end || !copy.is_empty() {
            current.push(copy);
        }
        if end {
            commands.push(std::mem::take(&mut current));
        }
    }
    commands.push(current);
    // The first group is kept even when empty (it belongs to the command
    // the caller already dispatched on); later empty ones are dropped.
    let mut out = Vec::with_capacity(commands.len());
    for (i, c) in commands.into_iter().enumerate() {
        if i == 0 || !c.is_empty() {
            out.push(c);
        }
    }
    out
}

/// Split a one-shot CLI command list (`argv` from the command name on) into
/// its first command and the commands after it, or `None` when the argv is to
/// be dispatched whole, exactly as before command lists were recognised.
///
/// tmux runs `tmux cmd1 \; cmd2` as one command list. psmux's CLI dispatches
/// on the FIRST command's name, and most of those arms rebuild the request
/// from the flags they know, so everything after the separator was silently
/// dropped (`select-pane -t p \; set-option ...` never set the option) or read
/// as the first command's own arguments. The caller sends the whole list to
/// the server as one line instead (see [`command_list_wire_line`]).
///
/// Only a lone `;` or `\;` argument separates, the same whole-token rule
/// [`crate::config::split_chained_commands_pub`] applies to the line the
/// server reads; an argument that merely ends in `;` stays data, as it always
/// has on this route. Commands that hand their arguments, separators and
/// all, to the server's own splitter (`bind-key`, `set-hook` and the
/// prompts), that run their own queue (`start-server`, #734), or that never
/// hand control back (an attaching client) keep their whole argv.
/// `new-session`, `run-shell` and `if-shell` are split: the caller runs them
/// through their own arm first (see [`command_list_runs_alone`]).
pub fn split_cli_command_list<S: AsRef<str>>(
    argv: &[S],
) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let command = argv.first()?.as_ref();
    let keeps_whole_line = matches!(
        command,
        // The server splits these lines itself, as it always has.
        "bind-key" | "bind" | "set-hook" | "confirm-before" | "confirm"
        | "command-prompt" | "display-menu" | "menu"
        // Runs its own queue against the server it starts (#734).
        | "start-server" | "start" | "warmup"
        // An attaching client does not come back until it detaches.
        | "attach-session" | "attach" | "at" | "a" | "server"
    );
    if keeps_whole_line {
        return None;
    }
    let mut groups: Vec<Vec<String>> = vec![Vec::new()];
    for arg in argv {
        let arg = arg.as_ref();
        if arg == ";" || arg == "\\;" {
            groups.push(Vec::new());
        } else if let Some(last) = groups.last_mut() {
            last.push(arg.to_string());
        }
    }
    let mut groups = groups.into_iter();
    let first = groups.next().unwrap_or_default();
    let queued: Vec<Vec<String>> = groups.filter(|g| !g.is_empty()).collect();
    if first.is_empty() || queued.is_empty() {
        return None;
    }
    Some((first, queued))
}

/// Commands whose new pane process takes the PATH of the one-shot client that
/// ran them (issue #829). tmux's spawn_pane (spawn.c) replaces the session's
/// PATH with the issuing client's when that client is not attached
/// (`c->session == NULL`), "because otherwise running tmux new myprogram
/// wouldn't work if myprogram isn't in the session's path". Every pane spawn
/// goes through spawn_pane there: new-window, split-window (and its new-pane
/// form) and respawn-pane/respawn-window. run-shell and display-popup build
/// their environment from the session alone (job.c), so they are not here.
pub fn spawns_pane_with_client_path(cmd: &str) -> bool {
    matches!(
        cmd,
        "new-window" | "neww"
            | "split-window" | "splitw" | "split-pane" | "splitp"
            | "new-pane" | "newp"
            | "respawn-pane" | "respawnp" | "resp"
            | "respawn-window" | "respawnw"
    )
}

/// Does this spawning command's own `-e` already set PATH? Only the options
/// before a `--` are read; what follows it is the pane's command.
fn has_own_path_env<S: AsRef<str>>(args: &[S]) -> bool {
    let is_path = |kv: &str| {
        kv.trim_matches('"')
            .split_once('=')
            .map_or(false, |(k, _)| k.eq_ignore_ascii_case("PATH"))
    };
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_ref();
        if a == "--" {
            break;
        }
        if a == "-e" {
            if args.get(i + 1).map_or(false, |v| is_path(v.as_ref())) {
                return true;
            }
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("-e") {
            if is_path(v) {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// The `KEY=VALUE` operand of the `-e` a one-shot CLI adds to a spawning
/// command so its pane gets the caller's PATH (issue #829), or None.
///
/// `args` is the whole command, `args[0]` its name. None when the command
/// spawns nothing, the caller has no PATH, or the caller's own `-e` already
/// names PATH: an explicit `-e PATH=...` keeps working as it always has.
/// `-e` is the carrier because every server that knows these commands already
/// applies it to the new pane, so a newer CLI talking to an older running
/// server still gets the caller's PATH into the pane. The server drops it
/// again when it equals what the pane inherits anyway, so the warm pane pool
/// stays usable in the common case (see `pane::client_path_override`).
pub fn client_path_env_value<S: AsRef<str>>(args: &[S], path: Option<&std::ffi::OsStr>) -> Option<String> {
    let cmd = args.first()?.as_ref();
    if !spawns_pane_with_client_path(cmd) || has_own_path_env(&args[1..]) {
        return None;
    }
    let path = path?.to_string_lossy();
    if path.is_empty() {
        return None;
    }
    Some(format!("PATH={}", path))
}

/// `command` with the caller's PATH added as `-e PATH=...` right after the
/// command name when [`client_path_env_value`] asks for it.
pub fn with_client_path(command: &[String], path: Option<&std::ffi::OsStr>) -> Vec<String> {
    match client_path_env_value(command, path) {
        Some(kv) => {
            let mut out = Vec::with_capacity(command.len() + 2);
            out.push(command[0].clone());
            out.push("-e".to_string());
            out.push(kv);
            out.extend(command[1..].iter().cloned());
            out
        }
        None => command.to_vec(),
    }
}

/// The one line a CLI command list goes to the server as: each command's
/// arguments quoted where the server's tokenizer needs it (the same
/// [`crate::util::quote_arg_if_needed`] rule the CLI arms use, so a value such
/// as `a ; b` stays one quoted argument), commands joined by ` ; `. The
/// server's connection loop splits it back with
/// [`crate::config::split_chained_commands_pub`] and runs the commands in
/// order, stopping at the first that fails.
pub fn command_list_wire_line(first: &[String], queued: &[Vec<String>]) -> String {
    std::iter::once(first)
        .chain(queued.iter().map(Vec::as_slice))
        .map(|command| {
            command
                .iter()
                .map(|arg| crate::util::quote_arg_if_needed(arg))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(" ; ")
}

/// Commands of a CLI command list that run as their own invocation of the
/// CLI rather than on the connection of the session the list is routed to:
/// their arm does its work on the client or across every server of the
/// namespace (`list-sessions`, `kill-server`, `has-session`, `kill-session`,
/// `run-shell`, `if-shell`'s condition), routes in its own way
/// (`switch-client`, `detach-client`), or creates or attaches a session.
/// Sent down one session's connection instead, `ls` listed that session
/// alone and `kill-server` ended only it.
pub fn command_list_runs_alone(command: &str) -> bool {
    matches!(
        command,
        "list-sessions" | "ls" | "kill-server" | "has-session" | "has"
        | "kill-session" | "kill-ses" | "run-shell" | "run" | "if-shell" | "if"
        | "new-session" | "new" | "attach-session" | "attach" | "at" | "a"
        | "start-server" | "start" | "warmup" | "switch-client" | "switchc"
        | "detach-client" | "detach" | "server"
    )
}

/// The values a command's `-t` and `-s` flags name, spelled apart (`-t x`),
/// attached (`-tx`) or ending a cluster (`-dt x`). Scanning stops at `--`.
/// Reading too much is harmless: the caller only uses these to decide that a
/// command must be routed on its own.
pub fn command_list_target_values(command: &[String]) -> Vec<String> {
    let mut values = Vec::new();
    let mut i = 1;
    while i < command.len() {
        let arg = command[i].as_str();
        if arg == "--" {
            break;
        }
        if let Some(letters) = arg.strip_prefix('-') {
            if let Some(at) = letters.find(|c| c == 't' || c == 's') {
                let rest = &letters[at + 1..];
                if rest.is_empty() {
                    if let Some(value) = command.get(i + 1) {
                        values.push(value.clone());
                    }
                    i += 1;
                } else {
                    values.push(rest.to_string());
                }
            }
        }
        i += 1;
    }
    values
}

/// Whether a later command of a CLI command list can go down the same
/// connection as the commands before it, to the session `routed_base` (a
/// registry base name) the list is routed to.
///
/// psmux runs one server per session, so a command naming another session
/// sent down this connection acted on THIS one: `set -t x @a 1 \; set -t y
/// @b 2` set both on x, and `send-keys -t y ...` typed into x. Anything that
/// names a session other than this one, an id that may live elsewhere
/// (`%N`, `@N`, `$N`: the CLI resolves their owner, #627), a command outside
/// the tmux catalog (whose CLI arm decides if it exists), or a command that
/// runs alone ([`command_list_runs_alone`]) is not shared.
pub fn command_list_shares_session(command: &[String], ns: Option<&str>, routed_base: &str) -> bool {
    let Some(name) = command.first() else { return false };
    if command_list_runs_alone(name) || !crate::server::connection::is_catalog_command(name) {
        return false;
    }
    command_list_target_values(command).iter().all(|value| {
        let value = value.trim_start_matches('=');
        if value.is_empty() || value.starts_with(':') || value.starts_with('.') {
            return true;
        }
        if value.starts_with(['%', '@', '$']) {
            return false;
        }
        match parse_target(value).session {
            Some(session) => crate::session::same_session_identity(ns, &session, routed_base),
            None => true,
        }
    })
}

/// Split a command list's reply into what its commands printed, verbatim,
/// and their error messages: a line starting with `ERROR` (`ERROR: msg`, or
/// switch-client's `ERROR msg`) is a failed command, the server's one-shot
/// reply convention every CLI arm reads.
pub fn command_list_reply(reply: &str) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut errors = Vec::new();
    for line in reply.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("ERROR") {
            errors.push(rest.strip_prefix(':').unwrap_or(rest).trim().to_string());
        } else {
            out.push_str(line);
        }
    }
    (out, errors)
}

/// Same as [`normalize_flag_equals`] but operates on `Vec<&str>`, returning
/// owned strings (needed where the caller already has borrowed slices).
pub fn normalize_flag_equals_borrowed(args: &[&str]) -> Vec<String> {
    normalize_flag_equals(args.iter().map(|a| a.to_string()).collect())
}

pub fn get_program_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "psmux".to_string())
        .to_lowercase()
        .replace(".exe", "")
}

pub fn print_help() {
    let prog = get_program_name();
    println!(r#"{prog} v{ver} - Terminal multiplexer for Windows (tmux alternative)

USAGE:
    {prog} [COMMAND] [OPTIONS]

SESSION COMMANDS:
    (no command)            Start a new session or attach to existing one
    new-session, new        Create a new session
        -s <name>           Session name (default: "default")
        -d                  Start detached (in background)
        -n <winname>        Name for the initial window
        -- <cmd> [args]     Run a specific command instead of default shell
    a, at, attach, attach-session
                            Attach to an existing session
        -t <name>           Target session name
    ls, list-sessions       List all active sessions
    has-session, has        Check if a session exists (exit code 0 = yes)
        -t <name>           Target session name
    kill-session, kill-ses  Kill a session
        -t <name>           Target session name
    kill-server             Kill every session on this socket (-L namespace,
                            or the default one when no -L is given)
        -a, --all           Kill every session in every -L namespace
    rename-session, rename  Rename the current session
    switch-client, switchc  Switch to another session
    list-clients, lsc       List connected clients
    detach-client, detach   Detach attached client(s); session keeps running
        -t <client>         Target a specific client (tty path or %id)
        -s <session>        Detach all clients of a specific session
        -a                  Detach all other clients (or all from CLI)
        -P                  Also kill the parent shell on detach
    server-info, info       Show server information

WINDOW COMMANDS:
    new-window, neww        Create a new window in current session
        -n <name>           Window name
        -d                  Create but don't switch to it
        -c <dir>            Start directory
    kill-window, killw      Close the current window
    rename-window, renamew  Rename current window
    select-window, selectw  Select a window by index
        -t <index>          Target window index
    next-window, next       Go to next window
    previous-window, prev   Go to previous window
    last-window, last       Go to last active window
    move-window, movew      Move window to a different index
    swap-window, swapw      Swap two windows
    find-window, findw      Search for a window by name
    link-window, linkw      Link a window to another session
    unlink-window, unlinkw  Unlink a window
    list-windows, lsw       List windows in a session

PANE COMMANDS:
    split-window, splitw    Split current pane
        -h                  Split horizontally (side by side)
        -v                  Split vertically (top/bottom, default)
        -p <percent>        Size as percentage
        -c <dir>            Start directory
    kill-pane, killp        Close the current pane
    select-pane, selectp    Select a pane
        -U / -D / -L / -R  Direction (up/down/left/right)
        -t <id>             Target pane (e.g. %3)
        -m / -M             Mark / unmark pane
    resize-pane, resizep    Resize a pane
        -U/-D/-L/-R <n>    Direction and amount
        -Z                  Toggle zoom
        -x <cols> -y <rows> Absolute size
    swap-pane, swapp        Swap two panes
        -U / -D             Direction
    join-pane, joinp        Join a pane to a window
    break-pane, breakp      Break pane into a new window
    rotate-window, rotatew  Rotate panes in a window
    display-panes, displayp Display pane numbers
    zoom-pane               Toggle pane zoom (alias for resizep -Z)
    respawn-pane, respawnp  Restart the pane's shell
    pipe-pane, pipep        Pipe pane output to a command
    list-panes, lsp         List panes in current window
    capture-pane, capturep  Capture pane content to buffer
        -p                  Print to stdout

COPY & PASTE COMMANDS:
    copy-mode               Enter copy/scroll mode
    set-buffer, setb        Set paste buffer content
    paste-buffer, pasteb    Paste from buffer to active pane
    list-buffers, lsb       List paste buffers
    show-buffer, showb      Display paste buffer content
    delete-buffer, deleteb  Delete a paste buffer
    choose-buffer, chooseb  Interactive buffer chooser
    save-buffer, saveb      Save buffer to file
    load-buffer, loadb      Load buffer from file
    clear-history, clearhist Clear pane scrollback history

KEY BINDING COMMANDS:
    bind-key, bind          Bind a key to a command
    unbind-key, unbind      Unbind a key
    list-keys, lsk          List all key bindings
    send-keys, send         Send keys to a pane
        -l                  Send literally (no key parsing)
        -p                  Paste text (legacy compatibility)
        -t <target>         Target pane

CONFIGURATION COMMANDS:
    set-option, set         Set a session/window option
        -g                  Set globally
        -u                  Unset (reset to default)
        -a                  Append to current value
        -q                  Quiet (no error on unknown option)
    show-options, show      Show all options and values
    show-window-options, showw  Show window-scoped options
    source-file, source     Execute commands from a config file
    set-environment, setenv Set an environment variable
    show-environment, showenv Show environment variables
    set-hook                Set a hook command for an event
    show-hooks              Show all defined hooks
    list-commands, lscm     List all available commands

LAYOUT COMMANDS:
    select-layout, selectl  Apply a layout preset
                            Presets: even-horizontal, even-vertical,
                            main-horizontal, main-vertical, tiled
    next-layout             Cycle to next layout
    previous-layout         Cycle to previous layout

DISPLAY COMMANDS:
    display-message, display  Display a message or format variable
    display-menu, menu      Display an interactive menu
    display-popup, popup    Display a popup window
    confirm-before, confirm Run command after y/n confirmation
    clock-mode              Display a big clock
    run-shell, run          Run a shell command
    if-shell, if            Conditional command execution
    wait-for, wait          Wait for / signal a named channel

MISC:
    help                    Show this help message
    version                 Show version information

OPTIONS:
    -h, --help              Show this help message
    -V, --version           Show version information
    -f <file>               Use <file> as the configuration file
    -L <name>               Name the server socket (namespace isolation)
    -S <path>               Specify server socket path
    -t <target>             Target session, window, or pane

TARGET SYNTAX (-t):
    session:window.pane     Full target path
    :2                      Window 2 in current session
    :2.1                    Pane 1 of window 2
    %3                      Pane by pane ID
    @4                      Window by window ID
    work:2                  Window 2 in session "work"

CONFIGURATION:
    psmux reads config on startup from the first file found:
        %USERPROFILE%\.psmux.conf
        %USERPROFILE%\.psmuxrc
        %USERPROFILE%\.tmux.conf
        %USERPROFILE%\.config\psmux\psmux.conf

    Config syntax is tmux-compatible. Example ~/.psmux.conf:

        # Change prefix to Ctrl+a
        set -g prefix C-a

        # Use a different shell
        set -g default-shell "C:/Program Files/PowerShell/7/pwsh.exe"
        # or: set -g default-command pwsh
        # or: set -g default-command cmd

        # Status bar
        set -g status-left "[#S] "
        set -g status-right "%H:%M %d-%b-%y"
        set -g status-style "bg=green,fg=black"

        # Key bindings
        bind-key -T prefix h split-window -h
        bind-key -T prefix v split-window -v

SHELL CONFIGURATION:
    psmux launches PowerShell 7 (pwsh) by default. To change:

    Use cmd.exe:
        set -g default-shell cmd
        set -g default-command "cmd /K"

    Use PowerShell 5 (Windows built-in):
        set -g default-shell powershell

    Use PowerShell 7 (pwsh):
        set -g default-shell pwsh

    Use Git Bash:
        set -g default-shell "C:/Program Files/Git/bin/bash.exe"

    Use Nushell:
        set -g default-shell nu

    Launch a window with a specific command:
        psmux new-window -- cmd /K echo hello
        psmux new-session -- python

SET OPTIONS (use with: set -g <option> <value>):
    prefix              Key  Prefix key (default: C-b)
    base-index          Int  First window number (default: 1)
    pane-base-index     Int  First pane number (default: 0)
    escape-time         Int  Escape delay in ms (default: 10)
    repeat-time         Int  Repeat key timeout in ms (default: 500)
    history-limit       Int  Scrollback lines (default: 2000)
    display-time        Int  Message display time in ms (default: 750)
    display-panes-time  Int  Pane number display time in ms (default: 1000)
    status-interval     Int  Status refresh interval in sec (default: 15)
    mouse               Bool Mouse support (default: on)
    status              Bool Show status bar (default: on)
    status-position     Str  "top" or "bottom" (default: bottom)
    priority            Str  psmux process scheduling class: "normal",
                             "above-normal" or "high" (default: above-normal)
    focus-events        Bool Pass focus events to apps (default: off)
    mode-keys           Str  "vi" or "emacs" (default: emacs)
    renumber-windows    Bool Auto-renumber on close (default: off)
    automatic-rename    Bool Auto-rename from foreground process (default: on)
    monitor-activity    Bool Flag windows with new output (default: off)
    monitor-silence     Int  Seconds before silence flag (default: 0)
    synchronize-panes   Bool Send input to all panes (default: off)
    remain-on-exit      Bool Keep panes after process exits (default: off)
    aggressive-resize   Bool Resize to smallest client (default: off)
    set-titles          Bool Update terminal title (default: off)
    set-titles-string   Str  Terminal title format
    default-shell       Str  Shell to launch (default: pwsh)
    default-command     Str  Alias for default-shell
    word-separators     Str  Copy-mode word delimiters (default: ASCII punctuation but _)
    prediction-dimming  Bool Dim predictive text (default: on)
    cursor-style        Str  Cursor shape asked of the terminal. "block",
                             "underline" and "bar" are steady;
                             "blinking-block", "blinking-underline" and
                             "blinking-bar" blink. "default" asks for no
                             shape, so the cursor you had before starting
                             psmux is the one you keep, and with neither
                             option set that is what you get
                             (default: default)
    cursor-blink        Bool Adds or removes the blink of the shape above:
                             "on" makes a steady shape blink, "off" stops a
                             blinking one. Set on its own, with no shape
                             named, it asks for the blink alone and leaves
                             the shape as it was (default: off)
    bell-action         Str  Bell handling: any, none, current, other
    visual-bell         Bool Visual bell indicator (default: off)

    STATUS / STYLE OPTIONS:
    status-left         Str  Left status content (default: "[#S] ")
    status-right        Str  Right status content
    status-style        Str  Status bar style (default: bg=green,fg=black)
    status-bg           Str  Status background color (deprecated, use status-style)
    status-fg           Str  Status foreground color (deprecated, use status-style)
    status-left-style   Str  Left status area style
    status-right-style  Str  Right status area style
    status-justify      Str  Tab alignment: left, centre, right
    message-style       Str  Message bar style
    message-command-style Str Command prompt style
    mode-style          Str  Copy-mode highlight style
    pane-border-style   Str  Inactive pane border style
    pane-active-border-style Str Active pane border style
    pane-border-hover-style Str Border hover highlight style
    pane-border-indicators Str Active pane indicator (off/colour/arrows/both)
    window-status-format        Str  Inactive window tab format
    window-status-current-format Str  Active window tab format
    window-status-separator     Str  Separator between tabs
    window-status-style         Str  Inactive tab style
    window-status-current-style Str  Active tab style
    window-status-activity-style Str Activity tab style
    window-status-bell-style    Str  Bell tab style
    window-status-last-style    Str  Last-active tab style

    Style format: "fg=colour,bg=colour,bold,dim,underscore,italics,reverse"
    Colours: default, black, red, green, yellow, blue, magenta, cyan, white,
             colour0-colour255, #RRGGBB

FORMAT VARIABLES (use in status-left, status-right, display-message, etc.):
    #S  session_name          #I  window_index
    #W  window_name           #F  window_flags
    #P  pane_index            #T  pane_title
    #D  pane_id               #H  hostname
    #h  host_short

    Conditionals:  #{{?window_active,yes,no}}
    Comparison:    #{{==:#I,1}}  #{{!=:#W,bash}}
    Substitution:  #{{s/old/new/:variable}}
    Truncation:    #{{=20:variable}}
    Basename:      #{{b:pane_current_path}}
    Dirname:       #{{d:pane_current_path}}
    Literal:       #{{l:text}}

KEY BINDINGS (default prefix: Ctrl+B):
    prefix + c          Create new window
    prefix + n          Next window
    prefix + p          Previous window
    prefix + l          Last window
    prefix + "          Split pane top/bottom
    prefix + %          Split pane left/right
    prefix + o          Switch to next pane
    prefix + ;          Last pane
    prefix + x          Kill current pane
    prefix + &          Kill current window
    prefix + z          Toggle pane zoom
    prefix + {{          Swap pane up
    prefix + }}          Swap pane down
    prefix + !          Break pane to new window
    prefix + d          Detach from session
    prefix + [          Enter copy/scroll mode
    prefix + ]          Paste from buffer
    prefix + =          Buffer chooser
    prefix + :          Enter command mode
    prefix + ?          List keybindings
    prefix + ,          Rename current window
    prefix + '          Select window by index
    prefix + $          Rename session
    prefix + w          Window/pane chooser
    prefix + s          Session chooser
    prefix + q          Display pane numbers
    prefix + i          Display pane info
    prefix + t          Clock mode
    prefix + Space      Next layout
    prefix + Arrow      Navigate between panes
    prefix + 0-9        Select window by number
    prefix + M-1..5     Preset layouts
    prefix + C-Arrow    Resize pane by 1
    prefix + M-Arrow    Resize pane by 5

COPY MODE KEYS (prefix + [):
    ↑/k  Scroll up         ↓/j  Scroll down
    PgUp/b  Page up        PgDn/f  Page down
    g  Top of scrollback   G  Bottom
    ←/h  Cursor left       →/l  Cursor right
    w/W  Next word          b/B  Previous word
    0  Start of line       $  End of line
    ^  First non-blank     H/M/L  Top/Mid/Bot
    f/F  Find char fwd/bwd t/T  Till char fwd/bwd
    %  Matching bracket    {{/}}  Prev/next paragraph
    /  Search forward      ?  Search backward
    n  Next match          N  Previous match
    v  Rectangle toggle    V  Line selection
    Space  Begin selection y/Enter  Yank (copy)
    D  Copy to end of line "a-z  Named registers
    o  Swap cursor/anchor  1-9  Numeric prefix
    q/Esc  Exit copy mode

ENVIRONMENT VARIABLES:
    PSMUX_SESSION_NAME       Default session name
    PSMUX_DEFAULT_SESSION    Fallback default session name
    PSMUX_CURSOR_STYLE       Cursor style (default, block, underline, bar,
                             blinking-block, blinking-underline, blinking-bar)
    PSMUX_CURSOR_BLINK       Cursor blinking (1/on/true, else off; unset
                             is not the same as 0)
    PSMUX_DIM_PREDICTIONS    Prediction dimming (1 to enable)
    TMUX                     Set inside psmux panes (tmux-compatible)
    TMUX_PANE                Current pane ID (e.g. %1)

EXAMPLES:
    {prog}                          Start or attach to default session
    {prog} new -s work              Create session named "work"
    {prog} new -s dev -- cmd /K     Create session running cmd.exe
    {prog} new -s py -- python      Create session running Python REPL
    {prog} attach -t work           Attach to session "work"
    {prog} ls                       List all sessions
    {prog} split-window -h          Split pane side by side
    {prog} send-keys -t %1 "ls" Enter
                                    Send keystrokes to pane %1
    {prog} set -g default-shell cmd Use cmd.exe as default shell
    {prog} source-file ~/.psmux.conf Reload config

NOTE: psmux ships as 'psmux', 'pmux', and 'tmux' - use whichever you prefer!

For more information: https://github.com/psmux/psmux
"#, prog = prog, ver = VERSION);
}

pub fn print_version() {
    // First line MUST stay "tmux <version>" (and nothing else) for
    // compatibility with tools like libtmux/tmuxp that read the first line of
    // `-V` output and parse the version token right after the "tmux " prefix.
    println!("tmux {}", VERSION);
    // Second line carries the exact build provenance for humans: the git commit
    // the binary was built from (short hash + date, plus a "dirty" marker when
    // built from a modified tree). Tools that parse only the first line ignore
    // it, so this stays fully backward compatible. Example:
    //   psmux 3.3.7 (a1b2c3d 2026-07-20)
    println!("{}", build_version_string());
}

pub fn print_commands() {
    println!(r#"Available commands:
  attach-session (attach)   - Attach to a session
  bind-key (bind)           - Bind a key to a command
  break-pane                - Break a pane into a new window
  capture-pane              - Capture the contents of a pane
  choose-buffer (chooseb)   - Choose a paste buffer interactively
  choose-tree               - Choose a session, window or pane from a tree
  clear-history (clearhist) - Clear pane scrollback history
  clock-mode                - Display a large clock in current pane
  confirm-before (confirm)  - Run command after confirmation
  copy-mode                 - Enter copy mode
  delete-buffer             - Delete a paste buffer
  detach-client (detach)    - Detach from the current session
  display-menu (menu)       - Display a menu
  display-message           - Display a message in the status line
  display-panes             - Display pane numbers
  display-popup (popup)     - Display a popup window
  find-window (findw)       - Search for a window by name
  has-session               - Check if a session exists
  if-shell (if)             - Conditional command execution
  join-pane                 - Join a pane to a window
  kill-pane                 - Kill a pane
  kill-server               - Kill the servers on this socket (-a: all namespaces)
  kill-session              - Kill a session
  kill-window               - Kill a window
  last-pane                 - Select the previously active pane
  last-window               - Select the previously active window
  link-window (linkw)       - Link a window to another session
  list-buffers (lsb)        - List paste buffers
  list-clients (lsc)        - List connected clients
  list-commands (lscm)      - List commands
  list-keys (lsk)           - List key bindings
  list-panes (lsp)          - List panes in a window
  list-sessions (ls)        - List sessions
  list-windows (lsw)        - List windows in a session
  load-buffer (loadb)       - Load buffer from file
  lock-client (lockc)       - Lock the client
  move-pane (movep)         - Move a pane to another window
  move-window (movew)       - Move a window to a different index
  new-session (new)         - Create a new session
  new-window (neww)         - Create a new window
  next-layout (nextl)       - Cycle to next layout
  next-window (next)        - Move to the next window
  paste-buffer              - Paste from a buffer
  pipe-pane (pipep)         - Pipe pane output to a command
  previous-window (prev)    - Move to the previous window
  refresh-client (refresh)  - Refresh client display
  rename-session            - Rename a session
  rename-window (renamew)   - Rename a window
  resize-pane (resizep)     - Resize a pane
  respawn-pane              - Respawn a pane
  rotate-window (rotatew)   - Rotate panes in a window
  run-shell (run)           - Run a shell command
  save-buffer (saveb)       - Save buffer to file
  select-layout (selectl)   - Apply a layout preset
  select-pane (selectp)     - Select a pane
  select-window (selectw)   - Select a window
  send-keys                 - Send keys to a pane
  set-buffer (setb)         - Set a paste buffer
  set-environment (setenv)  - Set an environment variable
  set-hook                  - Set a hook command
  set-option (set)          - Set a session or window option
  show-buffer (showb)       - Display the contents of a paste buffer
  show-environment (showenv)- Show environment variables
  show-hooks                - Show defined hooks
  show-options (show)       - Show session or window options
  show-window-options (showw)- Show window options
  source-file (source)      - Execute commands from a file
  split-window (splitw)     - Split a window into panes
  start-server (warmup)     - Pre-spawn a warm server for instant session creation
  suspend-client (suspendc) - Suspend the client
  swap-pane (swapp)         - Swap two panes
  swap-window (swapw)       - Swap two windows
  switch-client (switchc)   - Switch to another session
  unbind-key (unbind)       - Unbind a key
  unlink-window (unlinkw)   - Unlink a window
  wait-for (wait)           - Wait for a signal
  zoom-pane (zoom)          - Toggle pane zoom
"#);
}

/// Strip the tmux exact-match marker from a raw target specification.
///
/// tmux target grammar allows a leading '=' meaning "match this session name
/// exactly, no fuzzy prefix matching". psmux only ever matches exactly, so
/// the marker carries no information here, but any code that keeps the RAW
/// -t string (for relative pane forms like :.+) and later compares it as a
/// session name must see the plain name: `"name" == "=name"` silently
/// matching nothing is issue #558 (kill-session -t =name burned its 5s
/// settle deadline and exited 1 while the session survived).
pub fn strip_exact_match_prefix(target: &str) -> &str {
    target.strip_prefix('=').unwrap_or(target)
}

/// Does a bare `-t` token name a WINDOW rather than a session?
///
/// tmux's window table (`cmd-find.c:51`, `cmd_find_window_table`) is
/// `{start} ^`, `{last} !`, `{end} $`, `{next} +`, `{previous} -`, and
/// `cmd-find.c:443` tries a plain number as a window index before anything
/// else. A bare NAME stays ambiguous and keeps psmux's session meaning.
pub fn bare_target_names_a_window(target: &str) -> bool {
    if target.parse::<usize>().is_ok() {
        return true;
    }
    if matches!(target, "!" | "^" | "$") {
        return true;
    }
    if target.starts_with('{') && target.ends_with('}') && target.len() > 2 {
        return true;
    }
    if let Some(n) = target.strip_prefix(['+', '-']) {
        return n.is_empty() || n.chars().all(|c| c.is_ascii_digit());
    }
    false
}

/// A `-t` with no session, window or pane separator and no id prefix: a
/// name, an index or a window symbol. For a window or pane target tmux reads
/// it in the current session before it tries it as a session name
/// (cmd-find.c:1093 to :1110 file it as the window or pane part), so it is
/// resolved on live state rather than routed as a session.
pub fn bare_target_token(target: &str) -> bool {
    let t = strip_exact_match_prefix(target.trim());
    !t.is_empty()
        && !t.contains(':')
        && !t.contains('.')
        && !t.starts_with(['%', '@', '$', '~', '='])
}

/// Commands whose `-t` is a window target, where a bare token must be read as
/// a window before it is read as a session.
///
/// tmux resolves such a `-t` through `cmd_find_get_window` (`cmd-find.c:328`),
/// which uses the CURRENT session and asks
/// `cmd_find_get_window_with_session` first; only if that fails does it try
/// the token as a session itself (`cmd-find.c:348`). psmux's `parse_target`
/// reads any bare token as a session name (`"A bare string without ':' or
/// '.' is always a session name, even if numeric"`), so `select-window -t 0`
/// looked for a SESSION called `0`: on the CLI that died with
/// `no server running on session '<ns>__0'`, and from a binding, the control
/// socket or the `:` prompt it silently selected nothing (issue #692).
///
/// `move-window` and `swap-window` were coerced here first (issue #602).
/// `select-window` joins them; commands that resolve their own `-t`
/// (kill-window, break-pane, join-pane) are deliberately left alone.
pub fn window_target_command(cmd: &str) -> bool {
    matches!(
        cmd,
        "move-window" | "movew" | "swap-window" | "swapw" | "select-window" | "selectw"
            // new-window's -t is CMD_FIND_WINDOW with CMD_FIND_WINDOW_INDEX
            // (cmd-new-window.c), so `new-window -t 3` is index 3 here.
            | "new-window" | "neww"
    )
}

/// Put the colon back on a bare window target, so the generic parser reads it
/// as a window in the current session (tmux `cmd_find_get_window`).
///
/// Returns the target unchanged when the command's `-t` is not a window
/// target or the token is not window-shaped.
///
/// `select-window` took only the plain index when issue #692 landed, because
/// the relative and symbolic forms (`+2`, `!`, `{end}`) were resolved server
/// side by `move-window`/`swap-window`'s own #602 resolver and by nothing
/// else, so coercing them would only have turned a loud CLI error into a
/// silent no-op. Issue #693 item 4 routes `select-window` through that same
/// resolver on every route, so it now takes the whole shape, exactly as
/// tmux's `cmd_find_window_table` (cmd-find.c:51-58) and the offset branch
/// (cmd-find.c:390-417) do.
pub fn coerce_bare_window_target(cmd: &str, target: &str) -> String {
    let plain = strip_exact_match_prefix(target);
    let window_shaped = bare_target_names_a_window(plain);
    if window_target_command(cmd) && window_shaped {
        format!(":{}", plain)
    } else {
        target.to_string()
    }
}

/// Parse a tmux-style target specification
pub fn parse_target(target: &str) -> ParsedTarget {
    let mut result = ParsedTarget::default();

    // Strip leading '=' prefix (tmux exact-match semantics)
    let target = strip_exact_match_prefix(target);
    
    if target.starts_with('%') {
        if let Ok(pid) = target[1..].parse::<usize>() {
            result.pane = Some(pid);
            result.pane_is_id = true;
        }
        return result;
    }
    if target.starts_with('@') {
        // Allow a ".pane" suffix after the window id (e.g. "@2.0" or "@2.%3")
        let (wid_part, pane_part) = match target.find('.') {
            Some(dot) => (&target[1..dot], Some(&target[dot + 1..])),
            None => (&target[1..], None),
        };
        if let Ok(wid) = wid_part.parse::<usize>() {
            result.window = Some(wid);
            result.window_is_id = true;
            if let Some(pp) = pane_part {
                if let Some(pid) = pp.strip_prefix('%').and_then(|s| s.parse::<usize>().ok()) {
                    result.pane = Some(pid);
                    result.pane_is_id = true;
                } else if let Ok(p) = pp.parse::<usize>() {
                    result.pane = Some(p);
                }
            }
        }
        return result;
    }
    // $N is a tmux session ID (e.g., "$0"). Resolve it to the actual
    // session name by looking up the .sid file that maps this ID.
    if target.starts_with('$') {
        if let Ok(id) = target[1..].parse::<usize>() {
            // Set session to the resolved name, or to the literal "$N"
            // (which won't match any real session) so callers don't
            // fall through to "most recent session" for invalid IDs.
            result.session = Some(
                crate::session::resolve_session_by_id(id)
                    .unwrap_or_else(|| target.to_string())
            );
            return result;
        }
    }
    
    let (session_part, window_pane_part) = if let Some(colon_pos) = target.find(':') {
        let session = if colon_pos == 0 {
            None
        } else {
            let s = &target[..colon_pos];
            // $N session IDs (e.g. "$0:1") — resolve to session name
            if s.starts_with('$') {
                if let Ok(id) = s[1..].parse::<usize>() {
                    Some(crate::session::resolve_session_by_id(id)
                        .unwrap_or_else(|| s.to_string()))
                } else {
                    Some(s.to_string())
                }
            } else {
                Some(s.to_string())
            }
        };
        (session, Some(&target[colon_pos + 1..]))
    } else if target.starts_with('.') {
        (None, Some(target))
    } else if let Some(dot_pos) = target.find('.') {
        // Handle tmux-style session.pane syntax (e.g., "default.1")
        // Only treat as session.pane if the part after the dot is numeric
        let after_dot = &target[dot_pos + 1..];
        if after_dot.parse::<usize>().is_ok() {
            let session = target[..dot_pos].to_string();
            // Construct ".pane" so the window_pane_part parser handles it
            (Some(session), Some(&target[dot_pos..]))
        } else {
            // Dot is part of the session name (e.g., "my.session")
            (Some(target.to_string()), None)
        }
    } else {
        // A bare string without ':' or '.' is always a session name, even if numeric.
        // Window/pane specifiers require explicit syntax like ":0" or ".1"
        (Some(target.to_string()), None)
    };
    
    result.session = session_part;
    
    if let Some(wp) = window_pane_part {
        if wp.starts_with('%') {
            if let Ok(pid) = wp[1..].parse::<usize>() {
                result.pane = Some(pid);
                result.pane_is_id = true;
            }
        } else if wp.starts_with('@') {
            // Allow a ".pane" suffix after the window id (e.g. "ses:@2.0")
            let (wid_part, pane_part) = match wp.find('.') {
                Some(dot) => (&wp[1..dot], Some(&wp[dot + 1..])),
                None => (&wp[1..], None),
            };
            if let Ok(wid) = wid_part.parse::<usize>() {
                result.window = Some(wid);
                result.window_is_id = true;
                if let Some(pp) = pane_part {
                    if let Some(pid) = pp.strip_prefix('%').and_then(|s| s.parse::<usize>().ok()) {
                        result.pane = Some(pid);
                        result.pane_is_id = true;
                    } else if let Ok(p) = pp.parse::<usize>() {
                        result.pane = Some(p);
                    }
                }
            }
        } else if let Some(dot_pos) = wp.find('.') {
            if dot_pos > 0 {
                let win_part = &wp[..dot_pos];
                if let Ok(w) = win_part.parse::<usize>() {
                    result.window = Some(w);
                } else if !win_part.is_empty() {
                    result.window_name = Some(win_part.to_string());
                }
            }
            // The pane slot accepts a %id as well as an index, exactly as the
            // "@window.pane" branch above already does. Parsing it with a bare
            // parse::<usize>() silently failed on the '%', leaving pane = None,
            // and the command then fell back to the ACTIVE pane. That is not a
            // no-op: "kill-pane -t sess:.%4" killed the active pane %2 in a
            // different window and still exited 0.
            let pane_str = &wp[dot_pos + 1..];
            if let Some(pid) = pane_str.strip_prefix('%').and_then(|s| s.parse::<usize>().ok()) {
                result.pane = Some(pid);
                result.pane_is_id = true;
            } else if let Ok(p) = pane_str.parse::<usize>() {
                result.pane = Some(p);
            }
        } else {
            if let Ok(w) = wp.parse::<usize>() {
                result.window = Some(w);
            } else if !wp.is_empty() {
                result.window_name = Some(wp.to_string());
            }
        }
    }
    
    result
}

/// Extract the session name from a target string (for port file lookup)
pub fn extract_session_from_target(target: &str) -> String {
    let parsed = parse_target(target);
    parsed.session.unwrap_or_else(|| "default".to_string())
}

/// Split set-option flags from operands. Ordinary flags end at the option
/// name; `-t <target>` remains recognized until `--`.
pub(crate) struct ParsedSetOptionArgs<'a> {
    pub flag_chars: String,
    pub target: Option<&'a str>,
    pub missing_target: bool,
    pub positionals: Vec<&'a str>,
}

impl ParsedSetOptionArgs<'_> {
    pub fn validate(&self, window_command: bool) -> Result<(), String> {
        if let Some(flag) = invalid_set_option_flag(&self.flag_chars) {
            return Err(format!("unknown flag -{}", flag));
        }
        if self.missing_target {
            return Err("target required after -t".to_string());
        }
        let unset = self.flag_chars.contains('u') || self.flag_chars.contains('U');
        let append = self.flag_chars.contains('a');
        let pane_scope = self.flag_chars.contains('p');
        let local_window = (window_command || self.flag_chars.contains('w'))
            && !self.flag_chars.contains('g')
            && !self.flag_chars.contains('s');
        let Some(option) = self.positionals.first().copied() else {
            return Err("too few arguments (need at least 1)".to_string());
        };
        // tmux cmd-set-option.c:262-270: a name options_match cannot find is
        // "invalid option: NAME", and `-q` turns that into a silent success
        // that sets nothing (#846). Every route (CLI, TCP, control mode)
        // validates through here; with -q the server's apply_set_option
        // refuses the name again, so nothing is stored.
        if !crate::server::option_catalog::is_valid_option_name(option) {
            if self.flag_chars.contains('q') {
                return Ok(());
            }
            return Err(format!("invalid option: {}", option));
        }
        crate::server::option_catalog::validate_local_window_override(option, local_window)?;
        if pane_scope {
            if unset || self.positionals.len() >= 2 {
                return Ok(());
            }
            return Err("set-option -p: option and value required".to_string());
        }
        if unset {
            return Ok(());
        }
        if self.positionals.get(1).is_none() {
            if !append && crate::server::options::missing_value_toggles(option) {
                return Ok(());
            }
            return Err(format!("empty value for '{}'", option));
        }
        if append {
            crate::server::option_catalog::validate_option_append(option)?;
        }
        // `-F` means the value is a FORMAT, so what lands in the option store is
        // the expansion, not the text typed here. tmux expands first and
        // validates the result (cmd-set-option.c), so validating the raw
        // `#{...}` would refuse a legitimate assignment. The server validates
        // the expanded value in options::apply_set_option.
        if self.flag_chars.contains('F') {
            return Ok(());
        }
        crate::server::option_catalog::validate_option_value(
            option,
            &self.positionals[1..].join(" "),
        )
    }
}

pub(crate) fn parse_set_option_args<'a>(args: &[&'a str]) -> ParsedSetOptionArgs<'a> {
    let mut flag_chars = String::new();
    let mut target = None;
    let mut missing_target = false;
    let mut positionals = Vec::new();
    let mut i = 0;
    let mut parsing_flags = true;
    let mut literal_values = false;
    while i < args.len() {
        let arg = args[i];
        if parsing_flags {
            if arg == "--" {
                parsing_flags = false;
                literal_values = true;
                i += 1;
                continue;
            }
            if arg.starts_with('-') && arg.len() > 1 {
                // `-t` ends its cluster and takes the rest of the token as
                // the target (`-ts`, `-gtS`), or the next token when nothing
                // is attached, as tmux's args_parse does. The attached form
                // used to be read as more flag letters, with the NEXT token
                // (the option name) taken as the target.
                let body = &arg[1..];
                match body.find('t') {
                    Some(at) => {
                        flag_chars.push_str(&body[..=at]);
                        let attached = &body[at + 1..];
                        if !attached.is_empty() {
                            target = Some(attached);
                            i += 1;
                        } else {
                            match args.get(i + 1).copied() {
                                Some(value) => target = Some(value),
                                None => missing_target = true,
                            }
                            i += 2;
                        }
                    }
                    None => {
                        flag_chars.push_str(body);
                        i += 1;
                    }
                }
                continue;
            }
            parsing_flags = false;
        } else if !literal_values
            && (arg == "--" || arg == "-t")
            && !(positionals.len() == 1 && i + 1 == args.len())
        {
            // psmux extensions past the option name: `--` before more
            // tokens escapes them, and `<name> <value> -t <target>` puts the
            // target last. Both only fire on shapes tmux refuses as "too many
            // arguments". A lone trailing `--` or `-t` right after the name
            // is the VALUE, stored verbatim like tmux (`set @k --`, issue
            // #795).
            if arg == "--" {
                literal_values = true;
                i += 1;
                continue;
            }
            flag_chars.push('t');
            match args.get(i + 1).copied() {
                Some(value) => target = Some(value),
                None => missing_target = true,
            }
            i += 2;
            continue;
        }
        positionals.push(arg);
        i += 1;
    }
    ParsedSetOptionArgs {
        flag_chars,
        target,
        missing_target,
        positionals,
    }
}

fn invalid_set_option_flag(flag_chars: &str) -> Option<char> {
    flag_chars
        .chars()
        .find(|flag| !crate::SET_OPTION_CLI_FLAGS.contains(*flag))
}

/// Does a client whose tty is `tty` answer to the client target `spec`?
///
/// tmux's cmd_find_client: a single trailing colon is dropped, then the name,
/// the tty, and the tty without its `/dev/` prefix all match. A psmux client's
/// name IS its tty (`/dev/pts/N`), so `/dev/pts/3`, `pts/3` and `/dev/pts/3:`
/// all find the same client (issue #724).
pub fn client_spec_matches(tty: &str, spec: &str) -> bool {
    let t = spec.strip_suffix(':').unwrap_or(spec);
    !t.is_empty() && (tty == t || tty.strip_prefix("/dev/") == Some(t))
}

/// The client flags an `attach-session` argument list asks for.
///
/// tmux spells read only `-r` (cmd-attach-session.c, "c:dEf:rt:x", which sets
/// CLIENT_READONLY) or `-f read-only` (a comma list where a leading `!` turns a
/// flag off). Boolean flags cluster, so `-dr` counts; `-c`, `-f` and `-t` take
/// a value, which is skipped so a session or directory named `r` is not read
/// as the flag (issue #724).
pub fn attach_client_flags<'a>(args: impl IntoIterator<Item = &'a str>) -> ClientFlags {
    let mut flags = ClientFlags::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--" { break; }
        if !a.starts_with('-') || a.starts_with("--") || a.len() < 2 { continue; }
        let body = &a[1..];
        for (i, c) in body.char_indices() {
            match c {
                'r' => flags.read_only = true,
                'c' | 'f' | 't' => {
                    let rest = &body[i + c.len_utf8()..];
                    let value = if rest.is_empty() { it.next().unwrap_or("") } else { rest };
                    if c == 'f' {
                        flags.apply(value);
                    }
                    break;
                }
                _ => {}
            }
        }
    }
    flags
}

/// The client flags psmux acts on, from `-r` and the `-f` list of
/// `attach-session` and `new-session`.
///
/// `passthrough` is psmux's own: the client hands the host terminal the
/// active pane's output bytes instead of drawing frames, so the terminal's
/// own scrollback fills as it would in a plain shell session (see
/// `crate::passthrough`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ClientFlags {
    pub read_only: bool,
    pub passthrough: bool,
}

impl ClientFlags {
    /// Apply one `-f` value: a comma separated list in which `!name` clears
    /// a flag. Flags psmux does not act on are ignored, as before.
    pub fn apply(&mut self, value: &str) {
        for flag in value.split(',') {
            let (name, on) = match flag.strip_prefix('!') {
                Some(name) => (name, false),
                None => (flag, true),
            };
            match name {
                "read-only" => self.read_only = on,
                "passthrough" => self.passthrough = on,
                _ => {}
            }
        }
    }
}

/// Extract a flag value from args, supporting tmux short-flag CLI forms:
///   * Two-token form: `-F value`
///   * Concatenated form: `-Fvalue`
///   * Combined short-flag cluster where the value-taking flag is the last
///     char in the cluster: `-PF value` (i.e. `-P` boolean + `-F value`).
///     iTerm2 sends commands like `new-window -PF '#{window_id}'`.
pub fn extract_flag_value<'a>(args: &[&'a str], flag: &str) -> Option<String> {
    // Two-token form: -F value
    if let Some(w) = args.windows(2).find(|w| w[0] == flag) {
        return Some(w[1].to_string());
    }
    // Concatenated form: -Fvalue
    if let Some(v) = args.iter()
        .find(|a| a.starts_with(flag) && a.len() > flag.len())
        .map(|a| a[flag.len()..].to_string())
    {
        return Some(v);
    }
    // Combined-cluster form: -XYF value (flag char is last in cluster, next arg
    // is the value). Only triggers for single-char flags (e.g. "-F").
    if flag.len() == 2 && flag.starts_with('-') {
        let fc = flag.chars().nth(1).unwrap();
        for (i, a) in args.iter().enumerate() {
            if a.len() > 2
                && a.starts_with('-')
                && !a.starts_with("--")
                && a.chars().last() == Some(fc)
                && a.chars().skip(1).all(|c| c.is_ascii_alphabetic())
            {
                if let Some(next) = args.get(i + 1) {
                    return Some(next.to_string());
                }
            }
        }
    }
    None
}

/// Test whether a single-char short flag is set, accepting both standalone
/// (`-P`) and combined-cluster (`-PF`, `-lt`, etc.) forms.
pub fn has_short_flag(args: &[&str], flag_char: char) -> bool {
    for a in args {
        if a.len() < 2 || !a.starts_with('-') || a.starts_with("--") {
            continue;
        }
        // Skip args of the form -Xvalue where X is a value-taking flag — but
        // we don't know which flags take values here. Be conservative: only
        // match if all chars after '-' are ASCII alphabetic (a flag cluster).
        if !a.chars().skip(1).all(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        if a.chars().skip(1).any(|c| c == flag_char) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_target_window_name() {
        let pt = parse_target("mysession:mywindow");
        assert_eq!(pt.session, Some("mysession".to_string()));
        assert_eq!(pt.window, None);
        assert_eq!(pt.window_name, Some("mywindow".to_string()));
    }

    #[test]
    fn parse_target_window_index() {
        let pt = parse_target("mysession:2");
        assert_eq!(pt.session, Some("mysession".to_string()));
        assert_eq!(pt.window, Some(2));
        assert_eq!(pt.window_name, None);
    }

    #[test]
    fn parse_target_window_name_with_pane() {
        let pt = parse_target("mysession:mywindow.1");
        assert_eq!(pt.session, Some("mysession".to_string()));
        assert_eq!(pt.window, None);
        assert_eq!(pt.window_name, Some("mywindow".to_string()));
        assert_eq!(pt.pane, Some(1));
    }

    // A %id is legal in the PANE slot of "session:window.pane". It used to be
    // parsed with a bare parse::<usize>(), which fails on the '%', so pane came
    // back None and every caller fell through to the ACTIVE pane. The observable
    // damage was destructive rather than inert: "kill-pane -t sess:.%4" killed
    // the active pane %2, in a different window, and exited 0.
    #[test]
    fn parse_target_pane_id_in_pane_slot_current_window() {
        let pt = parse_target("mysession:.%4");
        assert_eq!(pt.session, Some("mysession".to_string()));
        assert_eq!(pt.window, None, "no window component means the current window");
        assert_eq!(pt.pane, Some(4), "%4 must reach the pane slot, not be dropped");
        assert!(pt.pane_is_id, "%4 is an id, not an index: indexes would resolve to a different pane");
    }

    #[test]
    fn parse_target_pane_id_in_pane_slot_with_window_index() {
        let pt = parse_target("mysession:1.%7");
        assert_eq!(pt.session, Some("mysession".to_string()));
        assert_eq!(pt.window, Some(1));
        assert_eq!(pt.pane, Some(7));
        assert!(pt.pane_is_id);
    }

    #[test]
    fn parse_target_pane_id_in_pane_slot_with_window_name() {
        let pt = parse_target("mysession:logs.%2");
        assert_eq!(pt.window_name, Some("logs".to_string()));
        assert_eq!(pt.pane, Some(2));
        assert!(pt.pane_is_id);
    }

    // A numeric pane index in the same slot must stay an INDEX. If this flipped
    // to pane_is_id the two syntaxes would silently mean the same thing and
    // "sess:.1" would start resolving to pane %1 instead of the second pane.
    #[test]
    fn parse_target_pane_index_is_not_an_id() {
        let pt = parse_target("mysession:.1");
        assert_eq!(pt.pane, Some(1));
        assert!(!pt.pane_is_id, "a bare index must not be treated as a %id");
    }

    #[test]
    fn parse_target_bare_window_name() {
        // :mywindow (no session)
        let pt = parse_target(":mywindow");
        assert_eq!(pt.session, None);
        assert_eq!(pt.window, None);
        assert_eq!(pt.window_name, Some("mywindow".to_string()));
    }

    #[test]
    fn parse_target_bare_window_index() {
        let pt = parse_target(":3");
        assert_eq!(pt.session, None);
        assert_eq!(pt.window, Some(3));
        assert_eq!(pt.window_name, None);
    }

    #[test]
    fn parse_target_session_only() {
        let pt = parse_target("mysession");
        assert_eq!(pt.session, Some("mysession".to_string()));
        assert_eq!(pt.window, None);
        assert_eq!(pt.window_name, None);
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_issue196_flag_equals.rs"]
mod tests_issue196_flag_equals;

#[cfg(test)]
#[path = "../tests-rs/test_issue795_literal_values.rs"]
mod tests_issue795_literal_values;

#[cfg(test)]
#[path = "../tests-rs/test_issue635_dangling_flag_value.rs"]
mod tests_issue635_dangling_flag_value;

#[cfg(test)]
#[path = "../tests-rs/test_issue497_selectwindow_id.rs"]
mod tests_issue497_selectwindow_id;

#[cfg(test)]
#[path = "../tests-rs/test_issue558_eq_prefix.rs"]
mod tests_issue558_eq_prefix;

#[cfg(test)]
#[path = "../tests-rs/test_discussion571_attached_global_args.rs"]
mod tests_discussion571_attached_global_args;

#[cfg(test)]
#[path = "../tests-rs/test_issue692_select_window_targets.rs"]
mod tests_issue692_select_window_targets;

#[cfg(test)]
#[path = "../tests-rs/test_issue734_command_queue.rs"]
mod tests_issue734_command_queue;

#[cfg(test)]
#[path = "../tests-rs/test_cli_command_list.rs"]
mod tests_cli_command_list;

#[cfg(test)]
#[path = "../tests-rs/test_issue829_client_path.rs"]
mod tests_issue829_client_path;

#[cfg(test)]
#[path = "../tests-rs/test_issue865_display_reply_error.rs"]
mod tests_issue865_display_reply_error;
