// tmux user keys (options-table.c:734-743, tty-keys.c:533-540, key-string.c
// at tag 3.8): the key names, their stored form, and how a held sequence is
// classified. The expectations are tmux 3.8's, measured where it prints
// something (`bind -n User1001` is `unknown key: User1001`, `M-User2` binds).

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

#[test]
fn user_names_run_from_0_to_1000() {
    for n in [0u32, 1, 42, 999, 1000] {
        let name = format!("User{}", n);
        let (code, mods) = parse(&name).unwrap_or_else(|| panic!("{name} is a user key"));
        assert_eq!(mods, KeyModifiers::NONE);
        let KeyCode::Char(c) = code else { panic!("{name} is stored as a char") };
        assert!(is_user_char(c));
        assert_eq!(self::name(c).as_deref(), Some(name.as_str()));
    }
    for bad in ["User1001", "User", "User-1", "Userx", "user0", "USER0", "User 0", "User0x"] {
        assert_eq!(parse(bad), None, "{bad} is no user key");
    }
}

#[test]
fn modifiers_go_with_the_user_key() {
    let (code, mods) = parse("M-User2").unwrap();
    assert_eq!(code, KeyCode::Char(char_for(2).unwrap()));
    assert_eq!(mods, KeyModifiers::ALT);
    assert_eq!(parse("C-S-User7").unwrap().1, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
}

#[test]
fn the_stored_range_is_clear_of_the_mouse_keys() {
    for n in [0u32, 500, 1000] {
        let c = char_for(n).unwrap();
        assert!(!crate::mouse_keys::is_mouse_char(c), "User{n} overlaps the mouse keys");
    }
    let mouse = crate::mouse_keys::parse("WheelUpPane").unwrap();
    assert!(!is_user_key(&mouse));
    assert_eq!(char_for(1001), None);
}

#[test]
fn config_parsing_and_list_keys_spell_them_as_tmux_does() {
    for name in ["User0", "User1000", "M-User2"] {
        let key = crate::config::parse_key_string(name).unwrap();
        assert_eq!(crate::config::parse_key_name(name), Some(key));
        // tmux 3.8 prints User1000 as "User100" (key-string.c:331, :422);
        // psmux keeps the name that binds it.
        assert_eq!(crate::config::format_key_binding(&key), name);
    }
}

#[test]
fn a_binding_lookup_keeps_the_user_keys_modifiers() {
    let plain = crate::config::parse_key_string("User2").unwrap();
    let meta = crate::config::parse_key_string("M-User2").unwrap();
    assert_eq!(crate::config::normalize_key_for_binding(plain), plain);
    assert_eq!(crate::config::normalize_key_for_binding(meta), meta);
    assert_ne!(crate::config::normalize_key_for_binding(plain), crate::config::normalize_key_for_binding(meta));
}

#[test]
fn a_held_sequence_is_a_key_a_prefix_or_nothing() {
    let table = vec![
        (0, "\x1b[5;30012~".to_string()),
        (3, "\x1bOz".to_string()),
        (4, "\x1bOzz".to_string()),
    ];
    assert_eq!(classify_in(&table, "\x1b"), Match::Prefix);
    assert_eq!(classify_in(&table, "\x1b[5;3"), Match::Prefix);
    assert_eq!(classify_in(&table, "\x1b[5;30012~"), Match::Key(0));
    assert_eq!(classify_in(&table, "\x1bOz"), Match::KeyOrLonger(3));
    assert_eq!(classify_in(&table, "\x1bOzz"), Match::Key(4));
    assert_eq!(classify_in(&table, "\x1b[A"), Match::None);
    assert_eq!(classify_in(&table, "x"), Match::None);
    assert_eq!(classify_in(&table, ""), Match::None);
}

#[test]
fn the_higher_index_wins_a_sequence_two_elements_share() {
    // tmux adds user-keys from 0 up and tty_keys_add replaces the key of a
    // sequence already in the tree (tty-keys.c:442-448).
    let table = vec![(1, "\x1b[99~".to_string()), (7, "\x1b[99~".to_string())];
    assert_eq!(classify_in(&table, "\x1b[99~"), Match::Key(7));
}

#[test]
fn the_published_table_leaves_out_what_tmux_never_reads() {
    // Serial with any other test that publishes: there is none, and this one
    // empties the table again before it returns.
    set_sequences(&[
        (0, "\x1b[5;30012~".to_string()),
        (1001, "\x1b[98~".to_string()),
        (2, String::new()),
        // What `"\377"` in a config becomes: the byte is gone, so the element
        // would match any byte that is no UTF-8 (user_keys::matchable).
        (3, "\x1b\u{fffd}".to_string()),
    ]);
    let t = table().expect("one sequence kept");
    assert_eq!(t.as_slice(), &[(0, "\x1b[5;30012~".to_string())]);
    assert!(any());
    set_sequences(&[]);
    assert!(!any());
    assert!(table().is_none());
}

#[test]
fn the_key_event_is_the_stored_char_without_modifiers() {
    let Some(crossterm::event::Event::Key(k)) = key_event(5) else { panic!("a key event") };
    assert_eq!(k.code, KeyCode::Char(char_for(5).unwrap()));
    assert_eq!(k.modifiers, KeyModifiers::NONE);
    assert!(key_event(1001).is_none());
}

#[test]
fn an_unknown_user_name_is_diagnosed_as_tmux_reports_it() {
    // tmux 3.8: `bind -n User1001 ...` is "unknown key: User1001", and so is
    // a lower case user0 (key-string.c reads `User%u` case sensitively).
    let mut a = crate::types::AppState::new("test_user_keys".to_string());
    crate::config::parse_config_content(
        &mut a,
        "bind -n User1001 display-message x\nbind -n user0 display-message x\nbind -n User1000 display-message y\n",
    );
    let unknown: Vec<&String> = a.config_warnings.iter().filter(|w| w.contains("unknown key")).collect();
    assert_eq!(unknown.len(), 2, "{:?}", a.config_warnings);
    assert!(unknown[0].contains("User1001") && unknown[1].contains("user0"), "{unknown:?}");
    for name in ["User0", "User1000", "M-User2"] {
        assert!(crate::config::is_unmodelled_tmux_key_name(name), "{name}");
    }
    for name in ["User1001", "user0", "User", "User-1"] {
        assert!(!crate::config::is_unmodelled_tmux_key_name(name), "{name}");
    }
}

#[test]
fn a_nerd_font_glyph_is_text_where_no_user_key_can_come_from() {
    // U+F1000 was User0's stored form and is a Nerd Fonts 3 Material Design
    // icon (nf-md range U+F0001 to U+F1AF0), which the console route delivers
    // as a key event for that character after an IME commit, a paste or the
    // emoji picker. The keys now sit in Supplementary Private Use Area-B
    // (key_base), so that glyph is no user key at all.
    for v in [0xF0001u32, 0xF1000, 0xF1234, 0xF1AF0] {
        let nf = char::from_u32(v).unwrap();
        assert!(!is_user_char(nf), "U+{v:X}");
        assert!(!drop_as_unbound(nf, true, true), "U+{v:X}");
    }
    // A character that is a user key's stored form is still dropped only
    // where a user key can come from.
    let glyph = char_for(0).unwrap();
    assert_eq!(glyph as u32, crate::key_base::user_base());
    // The console route never produces a user key: the glyph is text, with or
    // without user-keys set.
    assert!(!drop_as_unbound(glyph, false, false));
    assert!(!drop_as_unbound(glyph, false, true));
    // The VT route with user-keys empty (everyone who never sets it): text.
    assert!(!drop_as_unbound(glyph, true, false));
    // Only the VT route with sequences set drops an unbound user key.
    assert!(drop_as_unbound(glyph, true, true));
    assert!(drop_as_unbound(char_for(1000).unwrap(), true, true));
    // Neighbouring characters outside the user range are never dropped.
    let base = crate::key_base::user_base();
    for v in [base - 1, base + 1001, 0xF2000] {
        assert!(!drop_as_unbound(char::from_u32(v).unwrap(), true, true), "U+{v:X}");
    }
}
