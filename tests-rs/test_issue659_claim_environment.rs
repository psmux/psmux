//! #659: a claimed warm server must adopt the claiming client's environment.
//!
//! The pure parts of that adoption live in `src/client_env.rs` so they can be
//! exercised without mutating this process's environment (which the whole test
//! binary shares).

use super::*;

fn os(s: &str) -> OsString {
    OsString::from(s)
}

fn pairs(v: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    v.iter().map(|(k, val)| (os(k), os(val))).collect()
}

fn sane(extra: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    let mut v = pairs(&[
        ("PATH", r"C:\Windows;C:\psmux"),
        ("SYSTEMROOT", r"C:\Windows"),
        ("USERPROFILE", r"C:\Users\x"),
        ("COMSPEC", r"C:\Windows\system32\cmd.exe"),
    ]);
    v.extend(pairs(extra));
    v
}

// ─── block encoding ─────────────────────────────────────────────────────────

#[test]
fn env_block_round_trips() {
    let vars = pairs(&[("PATH", r"C:\Windows"), ("FOO", "bar baz")]);
    let decoded = decode_env_block(&encode_env_block(vars.clone())).expect("decodes");
    assert_eq!(decoded, vars);
}

#[test]
fn env_block_round_trips_values_with_equals_and_semicolons() {
    let vars = pairs(&[
        ("PATH", r"C:\a;C:\b;C:\c"),
        ("CONNECTION", "a=b;c=d"),
        ("EMPTY", ""),
    ]);
    let decoded = decode_env_block(&encode_env_block(vars.clone())).expect("decodes");
    assert_eq!(decoded, vars);
}

#[test]
fn env_block_drops_the_cmd_drive_variables_that_poison_a_block() {
    // "=C:" style entries are what makes CreateProcessW reject a whole block
    // with error 87 (see the pty crate's environment_block).
    let vars = pairs(&[("=C:", r"C:\somewhere"), ("PATH", r"C:\Windows")]);
    let decoded = decode_env_block(&encode_env_block(vars)).expect("decodes");
    assert_eq!(decoded, pairs(&[("PATH", r"C:\Windows")]));
}

#[test]
fn env_block_rejects_a_truncated_payload() {
    assert!(decode_env_block(&[0x41]).is_none());
}

#[test]
fn env_block_of_nothing_decodes_to_nothing() {
    assert_eq!(decode_env_block(&encode_env_block(vec![])), Some(vec![]));
}

// ─── sanity gate ────────────────────────────────────────────────────────────

#[test]
fn a_full_environment_is_sane() {
    assert!(is_sane(&sane(&[])));
}

#[test]
fn an_environment_without_path_is_not_sane() {
    let no_path: Vec<(OsString, OsString)> = sane(&[])
        .into_iter()
        .filter(|(k, _)| k != "PATH")
        .collect();
    assert!(!is_sane(&no_path));
}

#[test]
fn an_environment_with_an_empty_path_is_not_sane() {
    let mut v = sane(&[]);
    v[0].1 = os("");
    assert!(!is_sane(&v));
}

#[test]
fn a_nearly_empty_environment_is_not_sane() {
    assert!(!is_sane(&pairs(&[("PATH", r"C:\Windows")])));
}

// ─── the adoption plan ──────────────────────────────────────────────────────

/// The reported failure: the standby was born without the psmux directory on
/// PATH, the claiming client has it.  After the claim the server must have the
/// client's PATH, because that is what a cold spawned server would have had.
#[test]
fn poisoned_server_path_is_replaced_by_the_claiming_client_path() {
    let server = sane(&[]);
    let mut client = sane(&[]);
    client[0].1 = os(r"C:\Windows;C:\Users\x\.cargo\bin");

    let plan = plan_adoption(&server, &client);
    assert!(plan
        .set
        .iter()
        .any(|(k, v)| k == "PATH" && v == r"C:\Windows;C:\Users\x\.cargo\bin"));
    assert!(plan.remove.is_empty());
}

#[test]
fn a_variable_the_client_does_not_have_is_dropped() {
    let server = sane(&[("SEED_ONLY", "from the shell that spawned the standby")]);
    let client = sane(&[]);
    let plan = plan_adoption(&server, &client);
    assert_eq!(plan.remove, vec![os("SEED_ONLY")]);
}

#[test]
fn a_variable_only_the_client_has_is_added() {
    let server = sane(&[]);
    let client = sane(&[("CLIENT_ONLY", "1")]);
    let plan = plan_adoption(&server, &client);
    assert!(plan.set.iter().any(|(k, v)| k == "CLIENT_ONLY" && v == "1"));
}

#[test]
fn an_identical_environment_needs_no_work() {
    let plan = plan_adoption(&sane(&[]), &sane(&[]));
    assert!(plan.is_empty(), "unexpected plan: {plan:?}");
}

#[test]
fn variable_names_are_matched_case_insensitively() {
    let server = sane(&[("Path_Extra", "keep")]);
    let client = sane(&[("PATH_EXTRA", "keep")]);
    let plan = plan_adoption(&server, &client);
    assert!(
        plan.is_empty(),
        "PATH_EXTRA and Path_Extra are one variable on Windows: {plan:?}"
    );
}

/// The server's own identity and state root must survive a claim: the client
/// may be running inside ANOTHER psmux session and carry its values.
#[test]
fn server_owned_variables_are_never_taken_from_the_client() {
    let server = sane(&[
        ("PSMUX_TARGET_SESSION", "ns__standby"),
        ("PSMUX_DATA_DIR", r"C:\roots\a"),
    ]);
    let client = sane(&[
        ("PSMUX_TARGET_SESSION", "outer__session"),
        ("PSMUX_DATA_DIR", r"C:\roots\b"),
    ]);
    let plan = plan_adoption(&server, &client);
    assert!(plan.is_empty(), "unexpected plan: {plan:?}");
}

/// Same, the other way round: a client that has neither must not strip them
/// off the server.
#[test]
fn server_owned_variables_are_never_removed_by_a_client_without_them() {
    let server = sane(&[
        ("PSMUX_TARGET_SESSION", "ns__standby"),
        ("PSMUX_DATA_DIR", r"C:\roots\a"),
    ]);
    let plan = plan_adoption(&server, &sane(&[]));
    assert!(plan.is_empty(), "unexpected plan: {plan:?}");
}

#[test]
fn is_server_owned_is_case_insensitive() {
    assert!(is_server_owned(&os("psmux_data_dir")));
    assert!(is_server_owned(&os("PSMUX_TARGET_SESSION")));
    assert!(!is_server_owned(&os("PSMUX_SESSION")));
    assert!(!is_server_owned(&os("PATH")));
}

/// A corrupt or truncated payload must leave the standby exactly as it was:
/// a stale environment is survivable, one without PATH is not.
#[test]
fn an_insane_payload_produces_no_plan() {
    let server = sane(&[("SEED_ONLY", "x")]);
    let plan = plan_adoption(&server, &pairs(&[("FOO", "bar")]));
    assert!(plan.is_empty(), "unexpected plan: {plan:?}");
}

/// cmd.exe plants per drive working directories as `=C:`, they are inherited
/// by everything it starts, and `std::env::vars_os` really hands them back
/// (measured: `["=C:", "=ExitCode"]`). A leading `=` is legal for set_var, but
/// carrying these is pointless and deleting the server's own is worse, so the
/// plan drops them. The shapes that would PANIC set_var outright (an interior
/// `=`, an empty name, a NUL value) go through the same filter.
#[test]
fn the_cmd_drive_variables_never_reach_the_environment_api() {
    let mut server = sane(&[]);
    server.push((os("=C:"), os(r"C:\some\dir")));
    server.push((os("=D:"), os(r"D:\other")));
    let mut client = sane(&[]);
    client.push((os("=C:"), os(r"C:\elsewhere")));

    let plan = plan_adoption(&server, &client);
    assert!(
        plan.remove.iter().all(|k| is_settable(k)),
        "unsettable name in remove: {plan:?}"
    );
    assert!(
        plan.set.iter().all(|(k, _)| is_settable(k)),
        "unsettable name in set: {plan:?}"
    );
}

#[test]
fn is_settable_rejects_what_set_var_cannot_take() {
    assert!(is_settable(&os("PATH")));
    assert!(!is_settable(&os("")));
    assert!(!is_settable(&os("=C:")));
    assert!(!is_settable(&os("A=B")));
}

/// A value with an interior NUL panics set_var as well, and the payload is
/// read off disk rather than trusted.
#[test]
fn a_value_with_an_interior_nul_is_skipped() {
    let server = sane(&[]);
    let mut client = sane(&[]);
    client.push((os("BAD"), OsString::from("a\u{0}b")));
    let plan = plan_adoption(&server, &client);
    assert!(
        !plan.set.iter().any(|(k, _)| k == "BAD"),
        "unexpected plan: {plan:?}"
    );
}

#[test]
fn duplicate_incoming_names_are_applied_once() {
    let server = sane(&[]);
    let mut client = sane(&[("DUP", "first")]);
    client.push((os("dup"), os("second")));
    let plan = plan_adoption(&server, &client);
    assert_eq!(
        plan.set.iter().filter(|(k, _)| norm_is_dup(k)).count(),
        1,
        "unexpected plan: {plan:?}"
    );
}

fn norm_is_dup(k: &OsString) -> bool {
    k.to_string_lossy().eq_ignore_ascii_case("dup")
}

/// #773: a claim that changes PATH respawns the standby's window 0 pane, so
/// the server needs to know whether the plan touched PATH, in any casing.
#[test]
fn issue773_plan_touches_path_case_insensitively() {
    let current = sane(&[]);
    let same = plan_adoption(&current, &sane(&[]));
    assert!(!same.touches("PATH"));

    let mut changed = sane(&[]);
    changed[0] = (os("Path"), os(r"C:\path-marker;C:\Windows;C:\psmux"));
    let plan = plan_adoption(&current, &changed);
    assert!(plan.touches("PATH"));
    assert!(plan.touches("path"));
    assert!(!plan.touches("TEMP"));

    let other = plan_adoption(&current, &sane(&[("TEMP", r"C:\t")]));
    assert!(!other.touches("PATH"));
    assert!(other.touches("temp"));
}
