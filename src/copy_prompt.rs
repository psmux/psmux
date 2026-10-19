//! `command-prompt` opened from a copy mode key table.
//!
//! tmux writes most of its own copy mode keys as prompts: `:` is
//! `command-prompt -p'(goto line)' { send -X goto-line -- '%%' }`, `/` and `?`
//! search through one, and `f`, `F`, `t`, `T` read their character through
//! `command-prompt -1` (key-bindings.c:582 to :704). The prompt sits on the
//! status line while the pane STAYS in copy mode, and the command it builds runs
//! against that pane, still in copy mode (cmd-command-prompt.c:186 opens it on
//! the client without touching the pane's mode; :238 builds the command from
//! the template once the prompt is accepted).
//!
//! psmux keeps copy mode and its prompts in the server's single `Mode` slot,
//! and the server's `command-prompt` arm used to overwrite that slot with
//! `Mode::CommandPrompt`, which no client draws. The pane left copy mode, no
//! prompt appeared, and whatever was typed next went to the shell. A prompt
//! opened while the pane is in copy mode is therefore its own copy mode state,
//! drawn the way the built in search and goto line prompts are: as a sticky
//! status message.

use crate::types::{AppState, Mode};

/// One open copy mode prompt and everything needed to finish it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyCommandPrompt {
    /// `(label, initial input)` for each prompt `-p a,b` asked for, in order.
    pub prompts: Vec<(String, String)>,
    /// Index into `prompts` of the one on screen.
    pub current: usize,
    /// What the earlier prompts were answered with, `%1`, `%2`, ...
    pub answers: Vec<String>,
    /// Text typed into the prompt on screen.
    pub input: String,
    /// The command to run, with `%%` / `%1` still in it. `None` for a bare
    /// `command-prompt`, where the typed text is itself the command.
    pub template: Option<String>,
    /// `-1`: one key answers the prompt.
    pub single: bool,
    /// `-N`: digits only; any other key accepts and then acts as itself.
    pub numeric: bool,
    /// `-k`: the NAME of the next key answers the prompt.
    pub key: bool,
    /// `-e`: backspace on an empty prompt cancels it.
    pub bspace_exit: bool,
    /// `-T search`: Up and Down walk the copy mode search history and Enter
    /// adds to it. tmux keeps one history per prompt type
    /// (status_prompt_up_history, status.c:1761), and its own `/` and `?` are
    /// `command-prompt -T search` (key-bindings.c:597, :610), so this prompt
    /// shares the history psmux's built in search prompt keeps.
    pub search_history: bool,
}

impl CopyCommandPrompt {
    /// What the status line reads: the prompt, then what has been typed.
    pub fn status_text(&self) -> String {
        let label = self.prompts.get(self.current).map(|p| p.0.as_str()).unwrap_or("");
        format!("{}{}", label, self.input)
    }
}

/// A token of a `command-prompt` argument string: its value with quotes
/// removed, and the byte offset in the raw string where it starts.
fn tokens_with_offsets(s: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start: Option<usize> = None;
    let mut in_single = false;
    let mut in_double = false;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if in_single {
            if c == '\'' { in_single = false; } else { cur.push(c); }
            continue;
        }
        if in_double {
            if c == '\\' {
                if let Some(&(_, n)) = it.peek() {
                    if n == '"' || n == '\\' { cur.push(n); it.next(); continue; }
                }
                cur.push(c);
            } else if c == '"' {
                in_double = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        if c.is_whitespace() {
            if let Some(st) = start.take() {
                out.push((std::mem::take(&mut cur), st));
            }
            continue;
        }
        if start.is_none() { start = Some(i); }
        match c {
            '\'' => in_single = true,
            '"' => in_double = true,
            _ => cur.push(c),
        }
    }
    if let Some(st) = start {
        out.push((cur, st));
    }
    out
}

/// The flags and template of `command-prompt <args>`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PromptArgs {
    pub prompts: Option<String>,
    pub inputs: Option<String>,
    pub template: Option<String>,
    pub single: bool,
    pub numeric: bool,
    pub key: bool,
    pub bspace_exit: bool,
    pub literal: bool,
    /// `-T`: the prompt type.
    pub prompt_type: Option<String>,
}

/// Parse everything after the command name.
///
/// Flags follow tmux's `args_parse_flags` (arguments.c:207): they end at the
/// first token that is not one or at `--`, several can share a token, and a
/// value can be glued to its flag (`-p'(goto line)'`, `-pindex`). Which flags
/// take a value comes from the one flag table in cli.rs.
///
/// The template keeps its quoting, because it is parsed as a command only
/// after `%%` has been replaced: tmux's `{ ... }` form gives the text between
/// the braces, a single quoted argument gives its unquoted value, and anything
/// else is the rest of the line as written.
pub fn parse_args(args: &str) -> PromptArgs {
    let toks = tokens_with_offsets(args);
    let mut pa = PromptArgs::default();
    let mut i = 0;
    while i < toks.len() {
        let raw_start = toks[i].1;
        let token = toks[i].0.clone();
        // A token that starts with a quote is never a flag, even when its
        // value begins with `-`.
        let quoted = matches!(args[raw_start..].chars().next(), Some('\'') | Some('"'));
        if token == "--" && !quoted {
            i += 1;
            break;
        }
        let Some(rest) = token.strip_prefix('-').filter(|r| !r.is_empty() && !quoted) else {
            break;
        };
        for (at, flag) in rest.char_indices() {
            if !crate::cli::flag_takes_value("command-prompt", flag) {
                match flag {
                    '1' => pa.single = true,
                    'N' => pa.numeric = true,
                    'k' => pa.key = true,
                    'e' => pa.bspace_exit = true,
                    'l' => pa.literal = true,
                    _ => {}
                }
                continue;
            }
            let glued = &rest[at + flag.len_utf8()..];
            let value = if !glued.is_empty() {
                Some(glued.to_string())
            } else {
                i += 1;
                toks.get(i).map(|t| t.0.clone())
            };
            match flag {
                'p' => pa.prompts = value,
                'I' => pa.inputs = value,
                'T' => pa.prompt_type = value,
                _ => {}
            }
            break;
        }
        i += 1;
    }
    if i < toks.len() {
        let raw = args[toks[i].1..].trim();
        let template = if raw.starts_with('{') && raw.ends_with('}') && raw.len() >= 2 {
            raw[1..raw.len() - 1].trim().to_string()
        } else if i + 1 == toks.len() {
            toks[i].0.clone()
        } else {
            raw.to_string()
        };
        pa.template = Some(template);
    }
    pa
}

/// tmux's `cmd_template_replace` (cmd.c:849): put `s` where the template says.
///
/// `%idx` (and `%idx%`) is replaced every time it appears; `%%` (and `%%%`) only
/// the first time, as in tmux. `%%%` / `%idx%` are the double quoted spellings:
/// tmux escapes the typed text for a double quoted string there, and psmux,
/// whose parser only unescapes `\"` and `\\` inside double quotes, escapes those
/// two.
///
/// Plain `%%` / `%idx` go in as typed, as tmux's do (cmd.c:841, `quoted` is
/// only set by the third `%`), so the command parser sees exactly what tmux's
/// parser sees: `rename-window "%%"` keeps a typed `'`, an unquoted `%%` splits
/// on a typed space. One deliberate difference: where the placeholder sits
/// inside single quotes in the template, a typed `'` is written `'"'"'` so it
/// survives instead of ending the argument (tmux drops it). That escape applies
/// only inside single quotes; written unconditionally it turned `it's` into
/// `it"'s` for an unquoted or double quoted `%%`.
pub fn template_replace(template: &str, s: &str, idx: usize) -> String {
    replace_placeholder(template, s, idx, true)
}

/// [`template_replace`] exactly as tmux does it, without the copy mode
/// prompt's single quote escape: a typed `'` inside `'%%'` ends the quoted
/// argument as it does in tmux. The client's command prompt uses this (#789
/// follow up), so a prefix binding and `psmux command-prompt` give the names
/// tmux gives.
pub fn template_replace_tmux(template: &str, s: &str, idx: usize) -> String {
    replace_placeholder(template, s, idx, false)
}

/// [`final_command`] with [`template_replace_tmux`].
pub fn final_command_tmux(template: Option<&str>, answers: &[String]) -> String {
    let mut cmd = template.unwrap_or("%1").to_string();
    for (n, a) in answers.iter().enumerate() {
        cmd = template_replace_tmux(&cmd, a, n + 1);
    }
    cmd
}

fn replace_placeholder(template: &str, s: &str, idx: usize, escape_single: bool) -> String {
    #[derive(PartialEq)]
    enum Q { None, Single, Double }
    if !template.contains('%') {
        return template.to_string();
    }
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::with_capacity(template.len() + s.len());
    let mut replaced = false;
    // Where the template itself is, quote wise, so a placeholder knows
    // whether it sits inside single quotes. Only the template's own characters
    // move this; the typed text never does.
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        i += 1;
        if ch != '%' {
            if ch == '\\' && in_double {
                out.push(ch);
                if let Some(&escaped) = chars.get(i) {
                    out.push(escaped);
                    i += 1;
                }
                continue;
            }
            if ch == '\'' && !in_double {
                in_single = !in_single;
            } else if ch == '"' && !in_single {
                in_double = !in_double;
            }
            out.push(ch);
            continue;
        }
        let next = chars.get(i).copied();
        let plain = if in_single && escape_single { Q::Single } else { Q::None };
        let quote;
        if next.map_or(false, |d| ('1'..='9').contains(&d) && (d as usize - '0' as usize) == idx) {
            i += 1;
            quote = if chars.get(i) == Some(&'%') { i += 1; Q::Double } else { plain };
        } else if next == Some('%') && !replaced {
            replaced = true;
            i += 1;
            quote = if chars.get(i) == Some(&'%') { i += 1; Q::Double } else { plain };
        } else {
            out.push('%');
            continue;
        }
        for c in s.chars() {
            if quote == Q::Single && c == '\'' {
                out.push_str("'\"'\"'");
                continue;
            }
            if quote == Q::Double && (c == '"' || c == '\\') {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out
}

/// Build the prompt `command-prompt <args>` asks for, with `-I` expanded the
/// way prompt.c:184 expands it.
pub fn build(app: &AppState, args: &str) -> CopyCommandPrompt {
    let pa = parse_args(args);
    // With no -p the prompt names the command it will run (cmd-command-prompt.c
    // :113 to :121): `(name) `, or a bare `:` when there is no template.
    let (labels, space) = match (&pa.prompts, &pa.template) {
        (Some(p), _) => (p.clone(), true),
        (None, Some(t)) => {
            let head = t.split(|c: char| c == ' ' || c == ',').next().unwrap_or("");
            (format!("({})", head), true)
        }
        (None, None) => (":".to_string(), false),
    };
    let mut inputs = pa.inputs.as_deref().map(|s| s.split(',').map(str::to_string).collect::<Vec<_>>());
    let label_list: Vec<String> = if pa.literal {
        vec![labels]
    } else {
        labels.split(',').map(str::to_string).collect()
    };
    let mut prompts = Vec::new();
    for (n, l) in label_list.into_iter().enumerate() {
        let label = if space { format!("{} ", l) } else { l };
        let input = if pa.literal {
            pa.inputs.clone().unwrap_or_default()
        } else {
            inputs.as_mut().and_then(|v| v.get(n).cloned()).unwrap_or_default()
        };
        let input = if input.contains('#') { crate::format::expand_format(&input, app) } else { input };
        prompts.push((label, input));
    }
    let input = prompts.first().map(|p| p.1.clone()).unwrap_or_default();
    CopyCommandPrompt {
        prompts,
        current: 0,
        answers: Vec::new(),
        input,
        template: pa.template,
        single: pa.single,
        numeric: !pa.single && pa.numeric,
        key: !pa.single && !pa.numeric && pa.key,
        bspace_exit: pa.bspace_exit,
        search_history: pa.prompt_type.as_deref() == Some("search"),
    }
}

/// The command a finished prompt runs: every answer put into the template in
/// turn (args_make_commands, arguments.c:838), or the typed text itself for a
/// bare `command-prompt` (its default template is `%1`).
pub fn final_command(template: Option<&str>, answers: &[String]) -> String {
    let mut cmd = template.unwrap_or("%1").to_string();
    for (n, a) in answers.iter().enumerate() {
        cmd = template_replace(&cmd, a, n + 1);
    }
    cmd
}

/// Open a prompt on a pane that is in copy mode. A prompt that is already
/// open wins, as `tc->prompt != NULL` makes tmux ignore the second one
/// (cmd-command-prompt.c:100).
pub fn open(app: &mut AppState, args: &str) {
    if !matches!(app.mode, Mode::CopyMode) {
        return;
    }
    let prompt = build(app, args);
    // A pending count stays for the command the prompt runs (`3f` then `x`
    // jumps to the third x): input::run_copy_mode_command_line spends it.
    app.mode = Mode::CopyCommandPrompt(Box::new(prompt));
    // The cursor starts at the end, after any -I text (status_prompt_set sets
    // prompt_index to the length of the input, status.c:720), and Up starts
    // from the newest history entry (prompt_hindex is cleared, :727). Nothing
    // is cut yet for C-y (status_prompt_clear frees prompt_saved, :764).
    app.copy_prompt_back = 0;
    app.copy_search_history_pos = None;
    app.prompt_saved = None;
    refresh(app);
}

/// Redraw the prompt's status line.
pub fn refresh(app: &mut AppState) {
    if let Mode::CopyCommandPrompt(ref p) = app.mode {
        app.status_message = Some((p.status_text(), std::time::Instant::now(), Some(0)));
    }
}

/// A key typed while a copy mode prompt is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKey<'a> {
    Char(char),
    Enter,
    Escape,
    Backspace,
    /// Any other key, by its tmux name (`Up`, `C-u`, ...).
    Named(&'a str),
}

/// What became of a key fed to the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fed {
    /// The prompt used the key.
    Consumed,
    /// The prompt closed and the key must now be handled as a copy mode key
    /// (`-N` and a key that is not a digit, prompt.c:1242).
    Reprocess,
}

/// Leave the prompt for plain copy mode, taking its status line with it.
fn close(app: &mut AppState) {
    app.mode = Mode::CopyMode;
    app.status_message = None;
    app.copy_prompt_back = 0;
    app.copy_search_history_pos = None;
}

/// The name a `-k` prompt answers with: tmux's `key_string_lookup_key`
/// (key-string.c:327, called at status.c:1439). Keys reach the prompt under
/// the client's send-key names (`left`, `pageup`, `insert`, `f1`, `btab`, a
/// space as ' '), which are the same keys under other spellings, so each is
/// read as a key and written back the way tmux writes it: the modifiers as
/// `C-`, `M-`, `S-` in that order, then the first name key-string.c's table
/// gives the key (`Left`, `PPage`, `NPage`, `IC`, `DC`, `F1`, `BTab`, `Space`,
/// `Tab`, `Enter`, `Escape`, `BSpace`). A name psmux cannot read as a key is
/// passed on unchanged.
pub fn key_string(name: &str) -> String {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        // One typed character is that key. parse_key_name trims its input,
        // which would read a space as no key at all.
        return crate::config::format_key_binding(&(
            crossterm::event::KeyCode::Char(c),
            crossterm::event::KeyModifiers::NONE,
        ));
    }
    match crate::config::parse_key_name(name) {
        Some(key) => crate::config::format_key_binding(&key),
        None => name.to_string(),
    }
}

/// Feed one key to the open prompt.
pub fn feed(app: &mut AppState, key: PromptKey) -> Fed {
    let Mode::CopyCommandPrompt(ref mut p) = app.mode else {
        return Fed::Reprocess;
    };
    if p.key {
        let name = match key {
            PromptKey::Char(c) => key_string(&c.to_string()),
            PromptKey::Enter => "Enter".to_string(),
            PromptKey::Escape => "Escape".to_string(),
            PromptKey::Backspace => "BSpace".to_string(),
            PromptKey::Named(n) => key_string(n),
        };
        p.input = name;
        accept(app);
        return Fed::Consumed;
    }
    if p.numeric {
        // Every key that is not a digit, Enter and Escape included, answers
        // the prompt and is then handled as the key it is (prompt.c:1242 to
        // :1249 returns PROMPT_KEY_NOT_HANDLED after firing the callback).
        if let PromptKey::Char(c) = key {
            if c.is_ascii_digit() {
                p.input.push(c);
                refresh(app);
                return Fed::Consumed;
            }
        }
        accept(app);
        return Fed::Reprocess;
    }
    // Everything else is line editing, through the editor the other prompts
    // use, so the cursor moves and keys like C-a, C-k and C-t work here too.
    // tmux runs this prompt's keys through the same switch as any other
    // prompt's (process_key, status.c:1483). Only -k, -N and -1 leave
    // before it (status.c:1438 to :1470).
    match key {
        PromptKey::Char(c) => {
            if p.single {
                p.input.push(c);
                accept(app);
            } else {
                crate::copy_mode::prompt_insert(&mut p.input, app.copy_prompt_back, c.encode_utf8(&mut [0; 4]));
                refresh(app);
            }
        }
        PromptKey::Enter => {
            // Enter adds a non empty line to the type's history before the
            // command runs (status.c:1664 to :1666).
            if p.search_history {
                let typed = p.input.clone();
                crate::copy_mode::remember_search(app, &typed);
            }
            accept(app);
        }
        PromptKey::Escape | PromptKey::Named("C-c") | PromptKey::Named("C-g") => close(app),
        PromptKey::Backspace => {
            // tmux closes a -e prompt on BSpace only when nothing is typed,
            // wherever the cursor is (status.c:1520).
            if p.input.is_empty() && p.bspace_exit {
                close(app);
            } else {
                crate::copy_mode::prompt_edit(&mut p.input, &mut app.copy_prompt_back,
                    &mut app.prompt_saved, &app.word_separators, "bspace", || None);
                refresh(app);
            }
        }
        // A -1 prompt takes only a key that answers it. The prompt is still
        // redrawn: the server clears the status line before it hands a key
        // on, so a key that changes nothing would otherwise take the prompt
        // off the screen while it stays open.
        PromptKey::Named(_) if p.single => refresh(app),
        // Up and Down (and C-p, C-n) recall history. Only a -T search prompt
        // has one here; for any other type they change nothing. Every key
        // here redraws the prompt, for the same reason as above.
        PromptKey::Named(name) => {
            let buffers = &app.paste_buffers;
            let up = match crate::copy_mode::prompt_edit(&mut p.input, &mut app.copy_prompt_back,
                &mut app.prompt_saved, &app.word_separators, name, || buffers.first().map(|b| b.data.clone())) {
                crate::copy_mode::PromptEdit::HistoryUp => Some(true),
                crate::copy_mode::PromptEdit::HistoryDown => Some(false),
                _ => None,
            };
            if let (Some(up), true) = (up, p.search_history) {
                let mut pos = app.copy_search_history_pos;
                if let Some(text) = crate::copy_mode::search_history_step(&app.copy_search_history, &mut pos, up) {
                    // A recalled line puts the cursor at its end (status.c:1633).
                    p.input = text;
                    app.copy_prompt_back = 0;
                }
                app.copy_search_history_pos = pos;
            }
            refresh(app);
        }
    }
    Fed::Consumed
}

/// The prompt on screen was answered: move to the next one, or close and run
/// the command (cmd_command_prompt_callback, cmd-command-prompt.c:212).
fn accept(app: &mut AppState) {
    let Mode::CopyCommandPrompt(ref mut p) = app.mode else { return };
    let typed = std::mem::take(&mut p.input);
    p.answers.push(typed);
    p.current += 1;
    if p.current < p.prompts.len() {
        p.input = p.prompts[p.current].1.clone();
        // The next prompt also starts with the cursor at its end and its
        // history from the newest entry (status_prompt_update, status.c:791
        // and :794).
        app.copy_prompt_back = 0;
        app.copy_search_history_pos = None;
        refresh(app);
        return;
    }
    let cmd = final_command(p.template.as_deref(), &p.answers);
    close(app);
    for sub in crate::config::split_chained_commands_pub(&cmd) {
        crate::input::run_copy_mode_command_line(app, &sub);
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_copy_mode_command_prompt_binding.rs"]
mod tests;
