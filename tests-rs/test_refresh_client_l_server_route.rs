// `refresh-client -l` from a hook, a display-menu item or a confirm-before
// answer runs on the in server route (execute_command_string). That route
// forwarded a bare "refresh-client" and the -l was lost: measured on 9b5c3e0e,
// the hook, the menu item and the confirm answer all ran, and the top buffer
// stayed OLDBUF. tmux runs -l from every route (cmd-refresh-client.c:237).
// Registered from src/commands.rs.

use super::*;

#[test]
fn a_free_clipboard_becomes_the_top_buffer_at_once() {
    let mut app = AppState::new("rcl".to_string());
    app.paste_add("OLDBUF".to_string());
    let forward = refresh_client_l_here(&mut app, || Ok(Some("CLIPTEXT".to_string())));
    assert!(!forward);
    assert_eq!(app.paste_top(), Some("CLIPTEXT"));
}

#[test]
fn an_empty_or_textless_clipboard_stores_nothing() {
    let mut app = AppState::new("rcl".to_string());
    app.paste_add("OLDBUF".to_string());
    assert!(!refresh_client_l_here(&mut app, || Ok(Some(String::new()))));
    assert!(!refresh_client_l_here(&mut app, || Ok(None)));
    assert_eq!(app.paste_top(), Some("OLDBUF"));
}

#[test]
fn a_busy_clipboard_is_handed_to_the_waiting_route() {
    let mut app = AppState::new("rcl".to_string());
    app.paste_add("OLDBUF".to_string());
    let forward = refresh_client_l_here(&mut app, || Err(crate::clipboard::ClipboardBusy));
    assert!(forward, "the server loop must not wait for the clipboard itself");
    assert_eq!(app.paste_top(), Some("OLDBUF"));
}

#[test]
fn the_l_flag_is_found_in_any_cluster() {
    assert!(crate::cli::has_short_flag(&["-l"], 'l'));
    assert!(crate::cli::has_short_flag(&["-t", "c", "-l"], 'l'));
    assert!(crate::cli::has_short_flag(&["-Sl"], 'l'));
    assert!(!crate::cli::has_short_flag(&[], 'l'));
    assert!(!crate::cli::has_short_flag(&["-S"], 'l'));
}
