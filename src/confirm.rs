//! `confirm-before` as tmux has it (cmd-confirm-before.c), in one place for
//! every route: a prefix binding answered in the attached client, the
//! server's own prompt, the command prompt and the config file.
//!
//! tmux's flags are `-b`, `-c confirm-key`, `-p prompt`, `-t target-client`
//! and `-y`. The prompt takes a single key, and only the confirm key (`y`
//! unless `-c` names another) runs the command; Enter runs it only with `-y`
//! (`s[0] != confirm_key && (s[0] != '\r' || !default_yes)`,
//! cmd-confirm-before.c:134). Every other key answers no.
//!
//! Two things go beyond tmux, both for keyboards whose letters are not Latin
//! (issue #616: under a Russian layout the Y key types `н`):
//!
//! * `-c` takes any one printable character, not only ASCII 32 to 126, so
//!   `bind x confirm -c н kill-pane` works.
//! * A key whose character is not ASCII also confirms when the keyboard
//!   layout puts it on the same physical key as the confirm key, so the Y key
//!   says yes under any layout. ASCII keys are compared as they are, so a
//!   Latin layout (QWERTZ included, where Y and Z swap) behaves exactly as
//!   tmux does.
//!
//! The confirm key matches in either case (`Y` as well as `y`), as psmux has
//! always done, so Caps Lock does not turn a yes into a no.

/// The flags of one `confirm-before`, with the command it guards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfirmSpec {
    /// `-p`, verbatim (formats are expanded by whoever shows it).
    pub(crate) prompt: Option<String>,
    /// `-c`, or `y`.
    pub(crate) key: char,
    /// `-y`: Enter answers yes.
    pub(crate) default_yes: bool,
    /// `-b`: do not wait. psmux never blocks the caller on the answer, so the
    /// flag is accepted and has nothing more to do.
    pub(crate) background: bool,
    /// `-t`, the target client.
    pub(crate) target: Option<String>,
}

impl Default for ConfirmSpec {
    fn default() -> Self {
        ConfirmSpec { prompt: None, key: 'y', default_yes: false, background: false, target: None }
    }
}

/// Read `confirm-before`'s flags from its arguments (the command name not
/// included) as tmux's getopt string `bc:p:t:y` reads them, flags clustered
/// or not (`-yb`, `-cн`, `-p prompt`). Returns the flags and the index of the
/// first argument of the guarded command, which is `args.len()` when there
/// is none.
pub(crate) fn parse_confirm_args<S: AsRef<str>>(args: &[S]) -> Result<(ConfirmSpec, usize), String> {
    let mut spec = ConfirmSpec::default();
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
        let flags: Vec<char> = token.chars().skip(1).collect();
        let mut j = 0;
        while j < flags.len() {
            let flag = flags[j];
            match flag {
                'b' => spec.background = true,
                'y' => spec.default_yes = true,
                'c' | 'p' | 't' => {
                    // A value flag takes the rest of the cluster, or else
                    // the next argument.
                    let rest: String = flags[j + 1..].iter().collect();
                    let value = if !rest.is_empty() {
                        rest
                    } else {
                        i += 1;
                        match args.get(i) {
                            Some(v) => v.as_ref().to_string(),
                            None => return Err(format!("-{} expects an argument", flag)),
                        }
                    };
                    match flag {
                        'c' => spec.key = confirm_key_from(&value)?,
                        'p' => spec.prompt = Some(value),
                        _ => spec.target = Some(value),
                    }
                    j = flags.len();
                    continue;
                }
                other => return Err(format!("unknown flag -{}", other)),
            }
            j += 1;
        }
        i += 1;
    }
    Ok((spec, i))
}

/// `-c`'s value: exactly one printable character (tmux: "invalid confirm
/// key" otherwise, cmd-confirm-before.c:89-97).
fn confirm_key_from(value: &str) -> Result<char, String> {
    let mut chars = value.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_control() => Ok(c),
        _ => Err("invalid confirm key".to_string()),
    }
}

/// The command line that runs on yes, from the arguments after the flags.
/// tmux takes one argument and parses it as a command string
/// (`args_make_commands_now`), so `confirm-before "kill-session -t a"` runs
/// that line as written. psmux also takes the command as several words
/// (`confirm-before kill-pane -t 1`), which `requote` joins back into a line
/// that parses to the same words.
pub(crate) fn command_line<S: AsRef<str>>(args: &[S], requote: impl Fn(&[&str]) -> String) -> String {
    let words: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();
    match words.as_slice() {
        [one] => one.to_string(),
        many => requote(many),
    }
}

/// The text the prompt shows. With `-p` it is the prompt as given; without
/// it, tmux's `Confirm '<command name>'? (<key>/n)`, which names only the
/// first command's name (cmd-confirm-before.c:104-108).
pub(crate) fn prompt_text(spec: &ConfirmSpec, command: &str) -> String {
    match &spec.prompt {
        Some(p) => p.clone(),
        None => {
            let words = crate::commands::parse_command_line(command);
            let name = words.first().and_then(|w| w.split_whitespace().next()).unwrap_or("");
            format!("Confirm '{}'? ({}/n)", name, spec.key)
        }
    }
}

/// A confirmation the attached client put up for one of its own bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingConfirm {
    /// What runs on yes, quoted again so it parses to the same words.
    pub(crate) command: String,
    /// What the box shows, before format expansion.
    pub(crate) prompt: String,
    pub(crate) key: char,
    pub(crate) default_yes: bool,
}

impl PendingConfirm {
    /// A built in question with tmux's defaults: `y` confirms, Enter does
    /// not.
    pub(crate) fn plain(command: &str, prompt: &str) -> Self {
        PendingConfirm { command: command.to_string(), prompt: prompt.to_string(), key: 'y', default_yes: false }
    }

    /// Read a `confirm-before ...` (or `confirm ...`) command line, the
    /// command name included. A line whose flags do not parse (`-c ab`) is
    /// refused, as tmux refuses it.
    pub(crate) fn from_line(line: &str) -> Result<Self, String> {
        let words = crate::commands::parse_command_line(line);
        let args = words.get(1..).unwrap_or(&[]);
        let (spec, start) = parse_confirm_args(args)?;
        let command = command_line(&args[start..], |words| {
            words.iter().map(|a| crate::util::quote_arg_if_needed(a)).collect::<Vec<_>>().join(" ")
        });
        Ok(PendingConfirm {
            prompt: prompt_text(&spec, &command),
            command,
            key: spec.key,
            default_yes: spec.default_yes,
        })
    }
}

/// One key as the prompt sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfirmInput {
    Char(char),
    Enter,
    /// Escape, a function key, an arrow: every one of them answers no.
    Other,
}

impl ConfirmInput {
    /// Whether a key answers the prompt at all. A modifier on its own (with
    /// the keyboard enhancement flags on, Shift arrives before the letter it
    /// shifts) is half of a key, not an answer.
    pub(crate) fn answers(code: crossterm::event::KeyCode) -> bool {
        use crossterm::event::KeyCode;
        !matches!(code, KeyCode::Modifier(_) | KeyCode::Null | KeyCode::CapsLock | KeyCode::NumLock | KeyCode::ScrollLock)
    }

    pub(crate) fn from_key_code(code: crossterm::event::KeyCode) -> Self {
        use crossterm::event::KeyCode;
        match code {
            KeyCode::Char(c) => ConfirmInput::Char(c),
            KeyCode::Enter => ConfirmInput::Enter,
            _ => ConfirmInput::Other,
        }
    }
}

/// Whether a key answers the prompt with yes. Every key answers it; this
/// says which way.
pub(crate) fn is_yes(key: char, default_yes: bool, input: ConfirmInput) -> bool {
    is_yes_with(key, default_yes, input, layout_virtual_key)
}

/// `is_yes` with the keyboard layout lookup passed in, so the rule can be
/// tested without the layouts of the machine it runs on.
pub(crate) fn is_yes_with(
    key: char,
    default_yes: bool,
    input: ConfirmInput,
    virtual_key: impl Fn(char) -> Option<u16>,
) -> bool {
    match input {
        ConfirmInput::Enter => default_yes,
        ConfirmInput::Other => false,
        ConfirmInput::Char(c) => {
            if chars_match(c, key) {
                return true;
            }
            // A Latin key is what it says it is; only a key outside ASCII
            // may stand for the confirm key's physical key. Windows names
            // the letter and digit keys by their Latin character (VK_A is
            // 0x41, VK_0 is 0x30) in every layout.
            !c.is_ascii()
                && key.is_ascii_alphanumeric()
                && virtual_key(c) == Some(key.to_ascii_uppercase() as u16)
        }
    }
}

fn chars_match(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

/// The virtual key the keyboard layout in use types `typed` with. The layout
/// is the foreground window's, which is the terminal the key was typed in;
/// when it has no key for `typed`, the other installed layouts are tried,
/// and the first that has one decides.
#[cfg(windows)]
fn layout_virtual_key(typed: char) -> Option<u16> {
    #[link(name = "user32")]
    extern "system" {
        fn GetForegroundWindow() -> isize;
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        fn GetKeyboardLayout(thread: u32) -> isize;
        fn GetKeyboardLayoutList(n: i32, list: *mut isize) -> i32;
        fn VkKeyScanExW(ch: u16, layout: isize) -> i16;
    }

    let mut units = [0u16; 2];
    let encoded = typed.encode_utf16(&mut units);
    if encoded.len() != 1 {
        return None;
    }
    let unit = encoded[0];
    // The low byte is the virtual key, the high byte the shift state;
    // -1 means the layout has no key for the character.
    let vk_in = |layout: isize| -> Option<u16> {
        let r = unsafe { VkKeyScanExW(unit, layout) };
        (r != -1).then_some((r as u16) & 0xFF)
    };
    let active = unsafe {
        let hwnd = GetForegroundWindow();
        let thread = if hwnd == 0 { 0 } else { GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) };
        GetKeyboardLayout(thread)
    };
    if active != 0 {
        if let Some(vk) = vk_in(active) {
            return Some(vk);
        }
    }
    let count = unsafe { GetKeyboardLayoutList(0, std::ptr::null_mut()) };
    if count <= 0 {
        return None;
    }
    let mut layouts = vec![0isize; count as usize];
    let got = unsafe { GetKeyboardLayoutList(count, layouts.as_mut_ptr()) };
    layouts.truncate(got.max(0) as usize);
    layouts.into_iter().find_map(vk_in)
}

#[cfg(not(windows))]
fn layout_virtual_key(_typed: char) -> Option<u16> {
    None
}

#[cfg(test)]
#[path = "../tests-rs/test_issue616_confirm_keys.rs"]
mod tests_issue616_confirm_keys;
