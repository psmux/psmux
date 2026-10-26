// A control client made read only by `refresh-client -f read-only`: what it
// may still run. Registered from src/server/connection.rs.
use super::*;

#[test]
fn a_read_only_control_client_still_reads_panes() {
    let may = readonly_control_client_may_run;
    assert!(may("capture-pane", &["-p", "-e", "-S", "-"]));
    assert!(may("capturep", &[]));
    assert!(may("display-message", &["-p", "#{pane_id}"]));
    assert!(may("show-options", &["-Av", "prefix"]));
    assert!(may("refresh-client", &["-C", "80x24"]));
    assert!(may("refresh-client", &["-f", "read-only"]));
    assert!(may("detach-client", &[]));
}

#[test]
fn a_read_only_control_client_changes_nothing() {
    let may = readonly_control_client_may_run;
    assert!(!may("send-keys", &["-H", "61"]));
    assert!(!may("display-message", &["hello"]), "a message on the status line");
    assert!(!may("refresh-client", &["-l"]), "fills a paste buffer");
    assert!(!may("kill-session", &[]));
    assert!(!may("new-window", &[]));
    assert!(!may("if-shell", &["true", "send-keys x"]));
}
