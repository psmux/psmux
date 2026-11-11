// switch-client from outside a client acts on the most recently active client
// (cmd_find_best_client, cmd-find.c at tag 3.8). psmux has a server per
// session, so the CLI picks the session whose server holds that client from
// each server's `list-clients -F '#{client_activity}'` reply.
use super::best_client_session;

fn l(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(s, r)| (s.to_string(), r.to_string())).collect()
}

#[test]
fn a_session_made_after_the_attach_has_no_client_and_is_not_picked() {
    // The routing fallback points at the newest session `b`, which has no
    // client; the client is on `a`.
    let listings = l(&[("ns__a", "1762870000\n"), ("ns__b", "")]);
    assert_eq!(best_client_session(&listings, Some("ns__b")).as_deref(), Some("ns__a"));
}

#[test]
fn the_most_recently_active_client_wins() {
    let listings = l(&[("ns__a", "1762870000\n"), ("ns__b", "1762870100\n1762869000\n"), ("ns__c", "1762870050\n")]);
    assert_eq!(best_client_session(&listings, None).as_deref(), Some("ns__b"));
}

#[test]
fn a_tie_keeps_the_routed_session() {
    let listings = l(&[("ns__a", "1762870000\n"), ("ns__b", "1762870000\n")]);
    assert_eq!(best_client_session(&listings, Some("ns__b")).as_deref(), Some("ns__b"));
    assert_eq!(best_client_session(&listings, Some("ns__a")).as_deref(), Some("ns__a"));
    assert_eq!(best_client_session(&listings, None).as_deref(), Some("ns__a"));
}

#[test]
fn no_client_anywhere_keeps_the_routing() {
    let listings = l(&[("ns__a", ""), ("ns__b", "\n")]);
    assert_eq!(best_client_session(&listings, Some("ns__b")), None);
    assert_eq!(best_client_session(&[], None), None);
}

#[test]
fn a_reply_that_is_not_a_number_is_no_client() {
    let listings = l(&[("ns__a", "ERROR: something\n"), ("ns__b", "1762870000\r\n")]);
    assert_eq!(best_client_session(&listings, Some("ns__a")).as_deref(), Some("ns__b"));
}
