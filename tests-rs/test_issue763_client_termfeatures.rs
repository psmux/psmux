// #763 first slice: default reports and client selector behavior.
use super::*;
use crate::types::ClientSel;
use crate::terminal_features::TerminalFeatures;

#[test]
fn detached_query_has_no_terminal_features() {
    let app = AppState::new("work".into());
    assert!(app.client_registry.is_empty());
    assert_eq!(expand_format("[#{client_termfeatures}]", &app), "[]");
}

#[test]
fn older_and_control_clients_get_defaults() {
    for control in [false, true] {
        let mut app = AppState::new("work".into());
        assert!(app.client_registry.is_empty());
        assert!(app.register_client(3, control));
        assert_eq!(expand_format("#{client_termfeatures}", &app), "256,RGB,hyperlinks,title");
        assert_eq!(format_list_clients(&app, "#{client_termfeatures}", None), "256,RGB,hyperlinks,title\n");
    }
}

#[test]
fn disconnect_does_not_leave_features_or_client_identity() {
    let mut app = AppState::new("work".into());
    assert!(app.register_client(3, false));
    assert_eq!(expand_format("#{client_termfeatures}", &app), "256,RGB,hyperlinks,title");
    assert!(app.reap_client(3));
    assert_eq!(expand_format("#{client_name}|#{client_termfeatures}", &app), "|");
}

#[test]
fn distinct_reports_follow_the_existing_client_selector() {
    let mut app = AppState::new("work".into());
    assert!(app.register_client(3, false));
    assert!(app.register_client(5, false));
    app.client_registry.get_mut(&3).unwrap().termfeatures = TerminalFeatures::from_bits(0xf);
    app.client_registry.get_mut(&5).unwrap().termfeatures = TerminalFeatures::from_bits(0x800000000000000b);
    assert_eq!(format_list_clients(&app, "#{client_name}|#{client_termfeatures}", None),
        "/dev/pts/3|256,RGB,hyperlinks,title\n/dev/pts/5|256,RGB,title\n");
    let selected = resolve_client_sel(&app, &ClientSel::Name("pts/3:".into()));
    assert_eq!(selected, Some(3));
    assert_eq!(with_format_client(selected, || expand_format("#{client_termfeatures}", &app)),
        "256,RGB,hyperlinks,title");
    assert_eq!(expand_format("#{client_termfeatures}", &app), "256,RGB,title");
    assert!(app.reap_client(5));
    assert_eq!(expand_format("#{client_termfeatures}", &app), "256,RGB,hyperlinks,title");
}

#[test]
fn unknown_selection_preserves_existing_best_client_fallback() {
    let mut app = AppState::new("work".into());
    assert!(app.register_client(3, false));
    let missing = resolve_client_sel(&app, &ClientSel::Name("missing".into()));
    assert_eq!(missing, None);
    assert_eq!(with_format_client(missing, || expand_format("#{client_name}|#{client_termfeatures}", &app)),
        "/dev/pts/3|256,RGB,hyperlinks,title");
}
