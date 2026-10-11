//! `set-environment` / `show-environment` with tmux's two scopes (#775).
//!
//! tmux keeps a GLOBAL environment (`global_environ`, filled from the server's
//! start environment at tmux.c:418 and changed by `set-environment -g`) and a
//! per SESSION environment (`s->environ`).  A new pane gets the global one with
//! the session one copied over it (environ.c:253 `environ_for_session`), and
//! an entry can be a removal marker (`set-environment -r`, value NULL, shown as
//! `-NAME`) or hidden (`-h`, never pushed to a child, shown only by
//! `show-environment -h`).  `show-environment NAME` answers one entry, or
//! `unknown variable: NAME` at exit 1 (cmd-show-environment.c:120).
//!
//! psmux runs one server per session, so the "global" environment is this
//! server's: its process environment, which is the start environment of the
//! client that spawned it, or of the client that claimed it from the warm pool
//! (#659, `client_env`).  The session environment is `AppState::environment`
//! (set entries) plus the removal and hidden sets kept here.
//!
//! The server process environment doubles as the environment every child
//! (pane, run-shell, hook) inherits, so it always holds the MERGED view
//! `global <- session`.  To still answer `show-environment -g` after a session
//! assignment overwrote a name in the process, the global value underneath is
//! remembered in `global_under` for as long as a session entry shadows it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Global and session environment state the plain `environment` map cannot
/// express: removal markers, hidden entries, and the global values that a
/// session entry currently shadows in the process environment.
#[derive(Debug, Default, Clone)]
pub struct EnvScopes {
    /// Session `set-environment -r` markers.
    pub session_removed: BTreeSet<String>,
    /// Session `set-environment -h` entries (value kept for formats).
    pub session_hidden: BTreeMap<String, String>,
    /// Global `set-environment -g -r` markers.
    pub global_removed: BTreeSet<String>,
    /// Global hidden entries (`-g -h`, config `%hidden NAME=value`).
    pub global_hidden: BTreeMap<String, String>,
    /// Global value of every name a session entry shadows in the process
    /// environment (`None`: the global environment has no value for it).
    pub global_under: BTreeMap<String, Option<String>>,
    /// Session entries the new-session `update-environment` seed wrote, with
    /// the creating client's (start environment) values.
    pub seeded: BTreeSet<String>,
    /// Names given a global value, unset or hidden by `set-environment -g`
    /// since the server started. A global value lives in the process
    /// environment rather than in `environment`, so the post-config warm pane
    /// check (`warm_pane_sync::for_post_config`) reads this to know that a
    /// config `set-environment -g` changed what a child inherits: the early
    /// pane was spawned before the config ran and has to be respawned.
    pub global_set: BTreeSet<String>,
    /// Environment of the client whose attach is being processed (sent as
    /// `client-environ`, consumed by the attach that follows it).
    pub pending_client_environ: Option<Vec<(String, String)>>,
}

/// Windows environment names are case insensitive; tmux's are not.
fn name_eq(a: &str, b: &str) -> bool {
    if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b }
}

fn find_key<'a, I: IntoIterator<Item = &'a String>>(keys: I, name: &str) -> Option<String> {
    keys.into_iter().find(|k| name_eq(k, name)).cloned()
}

fn set_remove(set: &mut BTreeSet<String>, name: &str) {
    if let Some(k) = find_key(set.iter(), name) { set.remove(&k); }
}

fn map_remove<V>(map: &mut BTreeMap<String, V>, name: &str) -> Option<V> {
    find_key(map.keys(), name).and_then(|k| map.remove(&k))
}

fn hmap_remove(map: &mut HashMap<String, String>, name: &str) {
    let keys: Vec<String> = map.keys().filter(|k| name_eq(k, name)).cloned().collect();
    for k in keys { map.remove(&k); }
}

fn process_get(name: &str) -> Option<String> {
    std::env::var_os(name).map(|v| v.to_string_lossy().into_owned())
}

fn process_put(name: &str, value: Option<&str>) {
    match value {
        Some(v) => std::env::set_var(name, v),
        None => std::env::remove_var(name),
    }
}

/// One environment entry as tmux models it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// `None` is a removal marker (`-NAME`).
    pub value: Option<String>,
    pub hidden: bool,
}

impl EnvScopes {
    /// The value a hidden entry holds, for format and config `$NAME` lookup.
    pub fn hidden_value(&self, name: &str) -> Option<String> {
        self.session_hidden.get(name).or_else(|| self.global_hidden.get(name)).cloned()
    }

    /// Names a new child must not inherit (environ.c `environ_for_session`
    /// then `environ_push`): session removals and hidden entries, and global
    /// ones unless a session set entry overrides them.
    pub fn child_removals(&self, environment: &HashMap<String, String>) -> Vec<String> {
        let mut out: Vec<String> = self.session_removed.iter().cloned()
            .chain(self.session_hidden.keys().cloned())
            .collect();
        for k in self.global_removed.iter().chain(self.global_hidden.keys()) {
            let overridden = environment.keys().any(|e| name_eq(e, k));
            if !overridden && !out.iter().any(|o| name_eq(o, k)) {
                out.push(k.clone());
            }
        }
        out
    }

    fn shadowed(&self, name: &str) -> Option<String> {
        find_key(self.global_under.keys(), name)
    }

    /// Value a child would inherit from the global environment alone.
    fn global_child_value(&self, name: &str) -> Option<String> {
        if find_key(self.global_removed.iter(), name).is_some()
            || find_key(self.global_hidden.keys(), name).is_some()
        {
            return None;
        }
        match self.shadowed(name) {
            Some(k) => self.global_under.get(&k).cloned().flatten(),
            None => process_get(name),
        }
    }
}

/// Parsed `set-environment` (tmux args "Fhgrt:u", 1 or 2 positionals).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetEnvArgs {
    pub format: bool,
    pub hidden: bool,
    pub global: bool,
    pub remove: bool,
    pub unset: bool,
    pub name: String,
    pub value: Option<String>,
}

/// Parsed `show-environment` (tmux args "hgst:", 0 or 1 positional).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShowEnvArgs {
    pub hidden: bool,
    pub global: bool,
    pub shell: bool,
    pub name: Option<String>,
}

/// getopt style split: flag clusters before the first positional (`--` ends
/// them), `-t VALUE` skipped anywhere because the psmux client keeps the
/// routing target in the forwarded line wherever the user typed it.
fn split_args<'a>(args: &[&'a str], known: &str) -> Result<(Vec<char>, Vec<&'a str>), String> {
    let mut flags = Vec::new();
    let mut pos = Vec::new();
    let mut i = 0;
    let mut opts_done = false;
    while i < args.len() {
        let a = args[i];
        if a == "-t" {
            i += 2;
            continue;
        }
        if !opts_done && pos.is_empty() && a == "--" {
            opts_done = true;
            i += 1;
            continue;
        }
        if !opts_done && pos.is_empty() && a.len() > 1 && a.starts_with('-') {
            let body = &a[1..];
            if let Some(tpos) = body.find('t') {
                // `-gt sess`, `-tsess`
                for c in body[..tpos].chars() {
                    if !known.contains(c) { return Err(format!("unknown flag -{}", c)); }
                    flags.push(c);
                }
                if tpos + 1 == body.len() { i += 1; }
                i += 1;
                continue;
            }
            for c in body.chars() {
                if !known.contains(c) { return Err(format!("unknown flag -{}", c)); }
                flags.push(c);
            }
            i += 1;
            continue;
        }
        pos.push(a);
        i += 1;
    }
    Ok((flags, pos))
}

pub fn parse_set_environment(args: &[&str]) -> Result<SetEnvArgs, String> {
    let (flags, pos) = split_args(args, "Fhgru")?;
    if pos.is_empty() {
        return Err("too few arguments (need at least 1)".to_string());
    }
    if pos.len() > 2 {
        return Err("too many arguments (need at most 2)".to_string());
    }
    let a = SetEnvArgs {
        format: flags.contains(&'F'),
        hidden: flags.contains(&'h'),
        global: flags.contains(&'g'),
        remove: flags.contains(&'r'),
        unset: flags.contains(&'u'),
        name: pos[0].to_string(),
        value: pos.get(1).map(|s| s.to_string()),
    };
    // cmd-set-environment.c:55 onward, same order and wording.
    if a.name.is_empty() {
        return Err("empty variable name".to_string());
    }
    if a.name.contains('=') {
        return Err("variable name contains =".to_string());
    }
    if a.unset && a.value.is_some() {
        return Err("can't specify a value with -u".to_string());
    }
    if !a.unset && a.remove && a.value.is_some() {
        return Err("can't specify a value with -r".to_string());
    }
    if !a.unset && !a.remove && a.value.is_none() {
        return Err("no value specified".to_string());
    }
    Ok(a)
}

pub fn parse_show_environment(args: &[&str]) -> Result<ShowEnvArgs, String> {
    let (flags, pos) = split_args(args, "hgs")?;
    if pos.len() > 1 {
        return Err("too many arguments (need at most 1)".to_string());
    }
    Ok(ShowEnvArgs {
        hidden: flags.contains(&'h'),
        global: flags.contains(&'g'),
        shell: flags.contains(&'s'),
        name: pos.first().map(|s| s.to_string()),
    })
}

/// Apply a parsed `set-environment`.  `value` is the (already `-F` expanded)
/// value.  Keeps the process environment equal to what a new child inherits.
pub fn apply_set(
    environment: &mut HashMap<String, String>,
    scopes: &mut EnvScopes,
    a: &SetEnvArgs,
) {
    let name = a.name.as_str();
    let value = a.value.clone();
    if a.global {
        if !a.remove {
            scopes.global_set.insert(name.to_string());
        }
        set_remove(&mut scopes.global_removed, name);
        map_remove(&mut scopes.global_hidden, name);
        // The value the global environment ends up with for the process.
        let new_global: Option<String> = if a.unset {
            None
        } else if a.remove {
            scopes.global_removed.insert(name.to_string());
            None
        } else if a.hidden {
            scopes.global_hidden.insert(name.to_string(), value.clone().unwrap_or_default());
            None
        } else {
            value
        };
        match scopes.shadowed(name) {
            // A session entry owns the process value; remember the global one.
            Some(k) => { scopes.global_under.insert(k, new_global); }
            None => process_put(name, new_global.as_deref()),
        }
        return;
    }

    // Session scope: remember the global value before the first session entry
    // for this name changes the process environment.
    if scopes.shadowed(name).is_none() {
        // Not shadowed yet, so the process holds the global value (a global
        // removal or hidden marker has already taken it out of the process).
        scopes.global_under.insert(name.to_string(), process_get(name));
    }
    hmap_remove(environment, name);
    set_remove(&mut scopes.session_removed, name);
    map_remove(&mut scopes.session_hidden, name);
    let desired: Option<String> = if a.unset {
        // No session entry left: the child falls back to the global value.
        let g = scopes.global_child_value(name);
        map_remove(&mut scopes.global_under, name);
        g
    } else if a.remove {
        scopes.session_removed.insert(name.to_string());
        None
    } else if a.hidden {
        scopes.session_hidden.insert(name.to_string(), value.unwrap_or_default());
        None
    } else {
        let v = value.unwrap_or_default();
        environment.insert(name.to_string(), v.clone());
        Some(v)
    };
    process_put(name, desired.as_deref());
}

/// `update-environment` patterns are fnmatch globs (environ.c:203); Windows
/// variable names compare case insensitively.
fn pattern_match(pattern: &str, name: &str) -> bool {
    if cfg!(windows) {
        crate::terminal_overrides::fnmatch(&pattern.to_uppercase(), &name.to_uppercase())
    } else {
        crate::terminal_overrides::fnmatch(pattern, name)
    }
}

fn session_set(environment: &mut HashMap<String, String>, scopes: &mut EnvScopes, name: &str, value: Option<&str>) {
    let a = SetEnvArgs {
        name: name.to_string(),
        remove: value.is_none(),
        value: value.map(|v| v.to_string()),
        ..Default::default()
    };
    apply_set(environment, scopes, &a);
}

/// tmux `environ_update` (environ.c:186): for every `update-environment`
/// pattern, copy each matching variable of `src` (the attaching or creating
/// client's environment) into the session environment, and when nothing
/// matches record the pattern itself as a removal (`-NAME`).
///
/// `skip_existing` is the new-session seed: a name the session already has an
/// entry for (`new-session -e`, a config `set-environment`) keeps it.
/// Returns the names written.
pub fn update_environment(
    environment: &mut HashMap<String, String>,
    scopes: &mut EnvScopes,
    patterns: &[String],
    src: &[(String, String)],
    skip_existing: bool,
) -> Vec<String> {
    let mut written = Vec::new();
    for pat in patterns {
        // psmux extension kept for old configs: `-NAME` drops the entry.
        if let Some(name) = pat.strip_prefix('-') {
            if !name.is_empty() && !name.contains('=') {
                apply_set(environment, scopes, &SetEnvArgs { name: name.to_string(), unset: true, ..Default::default() });
            }
            continue;
        }
        if pat.is_empty() || pat.contains('=') { continue; }
        let has_entry = |env: &HashMap<String, String>, sc: &EnvScopes, n: &str| {
            env.keys().any(|k| name_eq(k, n))
                || find_key(sc.session_removed.iter(), n).is_some()
                || find_key(sc.session_hidden.keys(), n).is_some()
        };
        let mut found = false;
        for (k, v) in src {
            if k.is_empty() || k.starts_with('=') || k.contains('=') || !pattern_match(pat, k) { continue; }
            found = true;
            if skip_existing && has_entry(environment, scopes, k) { continue; }
            session_set(environment, scopes, k, Some(v));
            written.push(k.clone());
        }
        if !found && !(skip_existing && has_entry(environment, scopes, pat)) {
            session_set(environment, scopes, pat, None);
            written.push(pat.clone());
        }
    }
    written
}

/// The global environment as `update-environment` source at new-session: on a
/// cold server it is the creating client's environment, on a claimed warm
/// server the adopted one (#659).
pub fn global_source(scopes: &EnvScopes) -> Vec<(String, String)> {
    global_entries(scopes)
        .into_values()
        .filter(|e| !e.hidden)
        .filter_map(|e| e.value.map(|v| (e.name, v)))
        .collect()
}

/// New-session half of tmux's update-environment (cmd-new-session.c:
/// `environ_update(global_s_options, c->environ, env)`), applied after the
/// config so its option value is final.  Remembers what it seeded so a warm
/// claim can replace it and the post-config warm-pane check knows those
/// values already reached the early shell.
pub fn seed_session_from_start_env(environment: &mut HashMap<String, String>, scopes: &mut EnvScopes, patterns: &[String]) {
    // A claim replaces the previous seed (it came from the standby's spawner).
    let old: Vec<String> = std::mem::take(&mut scopes.seeded).into_iter().collect();
    for name in old {
        apply_set(environment, scopes, &SetEnvArgs { name, unset: true, ..Default::default() });
    }
    let src = global_source(scopes);
    let written = update_environment(environment, scopes, patterns, &src, true);
    scopes.seeded.extend(written);
}

/// The session environment, sorted like tmux's RB tree.
pub fn session_entries(environment: &HashMap<String, String>, scopes: &EnvScopes) -> BTreeMap<String, Entry> {
    let mut m = BTreeMap::new();
    for (k, v) in environment {
        m.insert(k.clone(), Entry { name: k.clone(), value: Some(v.clone()), hidden: false });
    }
    for k in &scopes.session_removed {
        m.insert(k.clone(), Entry { name: k.clone(), value: None, hidden: false });
    }
    for (k, v) in &scopes.session_hidden {
        m.insert(k.clone(), Entry { name: k.clone(), value: Some(v.clone()), hidden: true });
    }
    m
}

/// The global environment: this server's process environment with every
/// shadowed name put back to its global value, plus markers.
pub fn global_entries(scopes: &EnvScopes) -> BTreeMap<String, Entry> {
    let mut m = BTreeMap::new();
    for (k, v) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        // `=C:` style per drive cwd entries are not variables.
        if k.is_empty() || k.starts_with('=') { continue; }
        if scopes.shadowed(&k).is_some() { continue; }
        let v = v.to_string_lossy().into_owned();
        m.insert(k.clone(), Entry { name: k, value: Some(v), hidden: false });
    }
    for (k, u) in &scopes.global_under {
        if let Some(v) = u {
            m.insert(k.clone(), Entry { name: k.clone(), value: Some(v.clone()), hidden: false });
        }
    }
    for k in &scopes.global_removed {
        m.insert(k.clone(), Entry { name: k.clone(), value: None, hidden: false });
    }
    for (k, v) in &scopes.global_hidden {
        m.insert(k.clone(), Entry { name: k.clone(), value: Some(v.clone()), hidden: true });
    }
    m
}

fn lookup(entries: &BTreeMap<String, Entry>, name: &str) -> Option<Entry> {
    entries.get(name).cloned().or_else(|| {
        entries.values().find(|e| name_eq(&e.name, name)).cloned()
    })
}

/// cmd-show-environment.c:52 `cmd_show_environment_escape`.
fn shell_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 2);
    for c in value.chars() {
        if matches!(c, '$' | '`' | '"' | '\\') { out.push('\\'); }
        out.push(c);
    }
    out
}

/// cmd-show-environment.c:70 `cmd_show_environment_print`.
fn print_entry(out: &mut String, a: &ShowEnvArgs, e: &Entry) {
    if a.hidden != e.hidden { return; }
    match (&e.value, a.shell) {
        (Some(v), false) => out.push_str(&format!("{}={}\n", e.name, v)),
        (None, false) => out.push_str(&format!("-{}\n", e.name)),
        (Some(v), true) => out.push_str(&format!("{}=\"{}\"; export {};\n", e.name, shell_escape(v), e.name)),
        (None, true) => out.push_str(&format!("unset {};\n", e.name)),
    }
}

/// Answer a parsed `show-environment`: `Ok(text)` (possibly empty) or
/// `Err("unknown variable: NAME")`.
pub fn show(environment: &HashMap<String, String>, scopes: &EnvScopes, a: &ShowEnvArgs) -> Result<String, String> {
    let entries = if a.global { global_entries(scopes) } else { session_entries(environment, scopes) };
    let mut out = String::new();
    match &a.name {
        Some(name) => {
            let e = lookup(&entries, name).ok_or_else(|| format!("unknown variable: {}", name))?;
            print_entry(&mut out, a, &e);
        }
        None => {
            for e in entries.values() { print_entry(&mut out, a, e); }
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "../tests-rs/test_issue775_show_environment.rs"]
mod tests_issue775_show_environment;
