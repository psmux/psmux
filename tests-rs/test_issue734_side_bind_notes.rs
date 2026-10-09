// Issue #734 side finding 3: `bind-key -N 'note' -T mytbl x display-message
// hi` bound nothing. No bind-key parser knew `-N` (tmux cmd-bind-key.c,
// "nrN:T:"): the TCP parser stopped at `-N` and took it as the KEY, and the
// config parser skipped `-N` as a flag and took the note as the key (`unknown
// key: note`). psmux also had nowhere to keep a note, so `list-keys -N`
// (cmd-list-keys.c, notes_only) could only print every binding.
//
// A binding now carries its note, and `list-keys -N` prints tmux's notes
// template: `#{key_prefix} #{p|W:key_string} #{?key_note,#{key_note},
// #{key_command}}`, bindings without a note left out unless `-a`.

use super::*;

fn mock_app() -> AppState {
    let mut app = AppState::new("t734notes".to_string());
    app.control_port = None;
    app
}

fn bind_in<'a>(app: &'a AppState, table: &str, key: &str) -> Option<&'a crate::types::Bind> {
    let k = normalize_key_for_binding(parse_key_name(key)?);
    app.key_tables.get(table)?.iter().find(|b| b.key == k)
}

#[test]
fn a_noted_binding_lands_in_its_table_with_its_note() {
    let mut app = mock_app();
    parse_config_line(&mut app, "bind-key -N 'note' -T mytbl x display-message hi");
    let b = bind_in(&app, "mytbl", "x").expect("x bound in mytbl");
    assert_eq!(b.note.as_deref(), Some("note"));
    assert_eq!(crate::commands::format_action(&b.action), "display-message hi");
    assert!(bind_in(&app, "prefix", "note").is_none(), "the note is not a key");
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
}

#[test]
fn a_note_of_several_words_and_the_flag_after_the_table() {
    let mut app = mock_app();
    parse_config_line(&mut app, "bind -T mytbl -N \"split it, wide\" y split-window -h");
    let b = bind_in(&app, "mytbl", "y").expect("y bound");
    assert_eq!(b.note.as_deref(), Some("split it, wide"));
    assert_eq!(crate::commands::format_action(&b.action), "split-window -h");
}

#[test]
fn a_clustered_value_flag_and_root_table() {
    let mut app = mock_app();
    parse_config_line(&mut app, "bind -nrN 'repeatable root' F7 next-window");
    let b = bind_in(&app, "root", "F7").expect("F7 bound in root");
    assert!(b.repeat);
    assert_eq!(b.note.as_deref(), Some("repeatable root"));
}

#[test]
fn rebinding_without_a_note_drops_the_note() {
    let mut app = mock_app();
    parse_config_line(&mut app, "bind -N old -T mytbl x display-message a");
    parse_config_line(&mut app, "bind -T mytbl x display-message b");
    assert_eq!(bind_in(&app, "mytbl", "x").unwrap().note, None);
}

#[test]
fn a_bare_quote_key_is_still_a_key() {
    let mut app = mock_app();
    parse_config_line(&mut app, "bind '\"' split-window -v");
    assert!(bind_in(&app, "prefix", "\"").is_some());
}

#[test]
fn list_keys_notes_output() {
    let mut app = mock_app();
    parse_config_line(&mut app, "bind -N 'note' -T mytbl x display-message hi");
    parse_config_line(&mut app, "bind -T mytbl y display-message yo");
    parse_config_line(&mut app, "bind -N 'long key' -T mytbl F12 display-message z");
    let prefix = format_key_binding(&app.prefix_key);

    // Only noted bindings, keys padded to the widest listed key.
    assert_eq!(
        list_key_notes(&app, Some("mytbl"), false, None),
        format!("{p} x   note\n{p} F12 long key\n", p = prefix)
    );
    // -a lists every binding, the command where there is no note.
    let all = list_key_notes(&app, Some("mytbl"), true, None);
    assert!(all.contains(&format!("{} y   display-message yo\n", prefix)), "{all:?}");
    // A key filter keeps one key.
    assert_eq!(
        list_key_notes(&app, Some("mytbl"), false, Some("x")),
        format!("{} x note\n", prefix)
    );
    // No -T: the prefix and root tables, not mytbl.
    assert!(!list_key_notes(&app, None, false, None).contains("note"));
    parse_config_line(&mut app, "bind -N 'in prefix' z display-message p");
    assert_eq!(list_key_notes(&app, None, false, None), format!("{} z in prefix\n", prefix));
}

#[test]
fn the_plain_listing_still_shows_noted_bindings() {
    let mut app = mock_app();
    parse_config_line(&mut app, "bind -N 'note' -T mytbl x display-message hi");
    let entries = list_keys_entries(&app);
    assert!(entries
        .iter()
        .any(|(t, k, c, _)| t == "mytbl" && k == "x" && c == "display-message hi"));
}
