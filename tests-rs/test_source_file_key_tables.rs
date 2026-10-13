// source-file and the key tables: tmux never resets them on source-file;
// psmux resets only when the file is the server's startup config (a reload,
// kept for #195), and `unbind-key -a` with no table clears the prefix table
// only (tmux cmd-unbind-key.c).

use super::*;

fn app() -> crate::types::AppState {
    let mut app = crate::types::AppState::new("test_session".to_string());
    populate_default_bindings(&mut app);
    app
}

fn has(app: &crate::types::AppState, table: &str, key: &str) -> bool {
    let k = normalize_key_for_binding(parse_key_name(key).unwrap());
    app.key_tables.get(table).map_or(false, |t| t.iter().any(|b| b.key == k))
        || list_keys_entries(app).iter().any(|(t, n, _, _)| t == table && n == key)
}

fn temp_conf(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("psmux_sfkt_{}_{}", std::process::id(), name));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("main.conf");
    std::fs::write(&p, "bind x display x\n").unwrap();
    p
}

#[test]
fn same_config_path_ignores_case_and_slashes_on_windows() {
    let p = temp_conf("same");
    let s = p.to_string_lossy().to_string();
    assert!(same_config_path(&s, &s));
    #[cfg(windows)]
    {
        assert!(same_config_path(&s.to_uppercase(), &s));
        assert!(same_config_path(&s.replace('\\', "/"), &s));
    }
    let other = p.with_file_name("other.conf");
    std::fs::write(&other, "").unwrap();
    assert!(!same_config_path(&other.to_string_lossy(), &s));
    // A path that does not exist compares by its spelling.
    assert!(!same_config_path(r"C:\no\such\a.conf", &s));
}

#[test]
fn only_the_startup_config_is_a_reload() {
    let p = temp_conf("startup");
    let s = p.to_string_lossy().to_string();
    let mut app = app();
    assert!(!is_startup_config(&app, &s), "no startup config recorded");
    app.startup_config_file = Some(s.clone());
    assert!(is_startup_config(&app, &s));
    assert!(is_startup_config(&app, &format!("\"{}\"", s)));
    assert!(is_startup_config(&app, &format!("-F {}", s)));
    #[cfg(windows)]
    assert!(is_startup_config(&app, &s.to_lowercase().replace('\\', "/")));
    let other = p.with_file_name("plugin.conf");
    std::fs::write(&other, "").unwrap();
    assert!(!is_startup_config(&app, &other.to_string_lossy()));
}

#[test]
fn unbind_all_without_a_table_is_the_prefix_table() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n F2 display root\nbind -T copy-mode-vi M-q send -X cancel\n");
    assert!(has(&app, "prefix", "c"));
    parse_config_content(&mut app, "unbind-key -a\n");
    assert!(!has(&app, "prefix", "c"), "prefix defaults gone");
    assert!(has(&app, "root", "F2"), "root keeps its bindings (tmux)");
    assert!(has(&app, "copy-mode-vi", "M-q"), "copy-mode-vi keeps its bindings");
    assert!(has(&app, "root", "WheelUpPane"), "built-in root mouse default kept");
}

#[test]
fn unbind_all_in_a_named_table() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n F2 display root\n");
    parse_config_content(&mut app, "unbind-key -a -T root\n");
    assert!(!has(&app, "root", "F2"));
    assert!(!has(&app, "root", "WheelUpPane"));
    assert!(has(&app, "prefix", "c"), "prefix untouched");
    parse_config_content(&mut app, "unbind-key -a -T copy-mode\n");
    assert!(!has(&app, "copy-mode", "WheelUpPane"));
    assert!(has(&app, "copy-mode-vi", "WheelUpPane"), "copy-mode-vi untouched");
    parse_config_content(&mut app, "bind -n F3 display x\nunbind-key -a -n\n");
    assert!(!has(&app, "root", "F3"), "-n is root");
}
