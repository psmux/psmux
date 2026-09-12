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

/// Normalize `-x=VALUE` short-flag forms into `["-x", "VALUE"]`.
///
/// tmux accepts both `-t VALUE` (space) and `-t=VALUE` (equals) for
/// single-character flags.  psmux's parsers only handled the space form.
/// This function expands the equals form so every downstream comparison
/// (`arg == "-t"`, `args.windows(2)`, etc.) works without changes.
///
/// Rules:
///   - Only tokens starting with a single `-` (not `--`) are split.
///   - The flag letter must be ASCII alphabetic (`-t=foo` yes, `-1=bar` no).
///   - Long flags (`--name=value`) pass through unchanged.
///   - Positional tokens without a leading `-` pass through unchanged.
///   - Bare `-` and degenerate `-=` pass through unchanged.
pub fn normalize_flag_equals(args: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        // Must start with exactly one dash, followed by a single ASCII letter,
        // then `=`, then at least one character of value.
        if arg.len() >= 4
            && arg.starts_with('-')
            && !arg.starts_with("--")
        {
            let bytes = arg.as_bytes();
            if bytes[1].is_ascii_alphabetic() && bytes[2] == b'=' {
                out.push(format!("-{}", bytes[1] as char));
                out.push(arg[3..].to_string());
                continue;
            }
        }
        out.push(arg);
    }
    out
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
/// value-taking flag letter as that flag's argument, for EVERY command.
/// psmux's exact-match command parsers ignored such tokens, and the global
/// routing fallback then picked the most recent session — reproduced:
/// `kill-session -tvictimA` killed victimB.  `-t` is value-taking in every
/// tmux and psmux command, so it is the one letter that can be rewritten
/// generically without a per-command flag table (`-L`, `-T`, `-s`, ... are
/// boolean in some commands and value-taking in others).
///
/// Rules:
///   - Only tokens after the first non-flag token (the subcommand) are
///     candidates; the global region is `normalize_attached_global_args`'s
///     job and is left alone here.
///   - Rewriting stops at a literal `--`: everything after it is payload
///     (send-keys literals, direct-exec argv), exactly like tmux.
///   - Only `-t<value>` is split.  A bare `-t` (detached form) and every
///     other flag pass through unchanged.
///   - tmux parses a token like `-tfoo` as a target in every command, so
///     this rewrite is parity even where the token was meant as text; the
///     tmux-blessed way to pass literal `-t...` text is after `--`.
pub fn normalize_attached_target_flag(args: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut seen_subcommand = false;
    let mut seen_dashdash = false;
    for (idx, arg) in args.into_iter().enumerate() {
        if idx == 0 {
            out.push(arg); // binary name
            continue;
        }
        if arg == "--" {
            seen_dashdash = true;
            out.push(arg);
            continue;
        }
        if !seen_subcommand {
            if !arg.starts_with('-') || arg == "-" {
                seen_subcommand = true;
            }
            out.push(arg);
            continue;
        }
        if !seen_dashdash && arg.len() > 2 && arg.starts_with("-t") && !arg.starts_with("--") {
            out.push("-t".to_string());
            out.push(arg[2..].to_string());
            continue;
        }
        out.push(arg);
    }
    out
}

pub fn deferred_command_start<S: AsRef<str>>(command: &str, args: &[S]) -> Option<usize> {
    let value_options: &[&str] = match command {
        "bind-key" | "bind" => &["-T"],
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

/// Same as [`normalize_flag_equals`] but operates on `Vec<&str>`, returning
/// owned strings (needed where the caller already has borrowed slices).
pub fn normalize_flag_equals_borrowed(args: &[&str]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        if arg.len() >= 4
            && arg.starts_with('-')
            && !arg.starts_with("--")
        {
            let bytes = arg.as_bytes();
            if bytes[1].is_ascii_alphabetic() && bytes[2] == b'=' {
                out.push(format!("-{}", bytes[1] as char));
                out.push(arg[3..].to_string());
                continue;
            }
        }
        out.push(arg.to_string());
    }
    out
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
    kill-server             Kill all sessions and the server
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
    escape-time         Int  Escape delay in ms (default: 500)
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
    word-separators     Str  Copy-mode word delimiters (default: " -_@")
    prediction-dimming  Bool Dim predictive text (default: on)
    cursor-style        Str  Cursor shape: block, underline, bar
    cursor-blink        Bool Cursor blinking (default: off)
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
    PSMUX_CURSOR_STYLE       Cursor style (block, underline, bar)
    PSMUX_CURSOR_BLINK       Cursor blinking (1/0)
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
  kill-server               - Kill the psmux server
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
                flag_chars.push_str(&arg[1..]);
                if arg.contains('t') {
                    match args.get(i + 1).copied() {
                        Some(value) => target = Some(value),
                        None => missing_target = true,
                    }
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            parsing_flags = false;
        } else if !literal_values && arg == "--" {
            literal_values = true;
            i += 1;
            continue;
        } else if !literal_values && arg == "-t" {
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
