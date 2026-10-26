use super::*;

/// Run a set-option through the control-mode dispatcher with a stand-in for the
/// server thread, answering `ExpandFormat` the way `run_server` does so `-F`
/// can be exercised end to end. Returns the option name and the value that
/// actually reached the option store.
fn dispatched_assignment(args: &[&str], expansion: &str) -> (String, String) {
    let (request_tx, request_rx) = mpsc::channel();
    let (response_tx, _response_rx) = mpsc::channel();
    let expansion = expansion.to_string();
    let server = std::thread::spawn(move || {
        let mut assignment = None;
        while let Ok(request) = request_rx.recv_timeout(Duration::from_secs(10)) {
            match request {
                CtrlReq::ExpandFormat(_, resp) => {
                    let _ = resp.send(expansion.clone());
                }
                CtrlReq::SetOptionQuiet(name, value, _)
                | CtrlReq::SetOption(name, value) => {
                    assignment = Some((name, value));
                    break;
                }
                _ => {}
            }
        }
        assignment
    });
    assert!(dispatch_control_command(
        "set-option",
        args,
        &crate::server::connection::TargetedSender::new(&request_tx, None),
        response_tx,
        None,
        false,
        None,
        0, &Default::default(), false, &Default::default(),
    ));
    drop(request_tx);
    server.join().unwrap().expect("an option assignment must reach the store")
}

fn rejected_control_command(command: &str, args: &[&str]) -> String {
    let (request_tx, request_rx) = mpsc::channel();
    let (response_tx, response_rx) = mpsc::channel();
    assert!(dispatch_control_command(
        command,
        args,
        &crate::server::connection::TargetedSender::new(&request_tx, None),
        response_tx,
        None,
        false,
        None,
        0, &Default::default(), false, &Default::default(),
    ));
    let response = response_rx.recv().unwrap();
    assert!(response.starts_with("\u{0001}ERR\u{0001}"));
    assert!(matches!(
        request_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty),
    ));
    response
}

#[test]
fn combined_target_flags_consume_the_target_before_the_option_name() {
    let parsed = parse_set_option_args(&[
        "-gat",
        "target",
        "history-limit",
        "42",
    ]);
    assert_eq!(parsed.flag_chars, "gat");
    assert_eq!(parsed.target, Some("target"));
    assert_eq!(parsed.positionals, ["history-limit", "42"]);
}

#[test]
fn target_last_is_supported_and_dash_dash_preserves_literal_target_tokens() {
    let target_last = parse_set_option_args(&[
        "mouse",
        "on",
        "-t",
        "target",
    ]);
    assert_eq!(target_last.target, Some("target"));
    assert_eq!(target_last.positionals, ["mouse", "on"]);

    let literal = parse_set_option_args(&[
        "@value",
        "--",
        "-t",
        "target",
    ]);
    assert_eq!(literal.target, None);
    assert_eq!(literal.positionals, ["@value", "-t", "target"]);
}

#[test]
fn dangling_target_is_rejected_without_dispatch() {
    let parsed = parse_set_option_args(&["mouse", "off", "-t"]);
    assert!(parsed.missing_target);
    assert!(parsed.validate(false).is_err());

    let response = rejected_control_command(
        "set-option",
        &["-g", "mouse", "off", "-t"],
    );
    assert!(response.contains("target required"));
}

#[test]
fn malformed_flag_tokens_remain_flags_for_cli_rejection() {
    let parsed = parse_set_option_args(&[
        "-g1",
        "history-limit",
        "42",
    ]);
    assert_eq!(parsed.flag_chars, "g1");
    assert_eq!(parsed.target, None);
    assert_eq!(parsed.positionals, ["history-limit", "42"]);
}

#[test]
fn control_mode_rejects_malformed_flag_tokens() {
    let response = rejected_control_command(
        "set-option",
        &["-g1t", ":1", "history-limit", "42"],
    );
    assert!(response.contains("unknown flag -1"));
}

#[test]
fn missing_option_name_is_rejected_without_dispatch() {
    let response = rejected_control_command("set-option", &["-g"]);
    assert!(response.contains("too few arguments"));
}

#[test]
fn targeted_invalid_assignment_is_rejected_without_dispatch() {
    let response = rejected_control_command(
        "set-option",
        &["-gt", ":1", "history-limit"],
    );
    assert!(response.contains("empty value"));
}

#[test]
fn multi_token_integer_is_rejected_without_dispatch() {
    let response = rejected_control_command(
        "set-option",
        &["-g", "history-limit", "42", "junk"],
    );
    assert!(response.contains("42 junk"));
}

#[test]
fn negative_integer_is_rejected_without_dispatch() {
    let response = rejected_control_command(
        "set-option",
        &["-g", "history-limit", "-1"],
    );
    assert!(response.contains("-1"));
}

#[test]
fn numeric_options_use_catalog_destination_ranges() {
    use crate::server::option_catalog::{NumberKind, OptionType, OPTION_CATALOG};

    let cases = [
        ("escape-time", NumberKind::U64, u64::MAX as u128),
        ("history-limit", NumberKind::Usize, usize::MAX as u128),
        ("base-index", NumberKind::Index, isize::MAX as u128),
        ("pane-base-index", NumberKind::Index, isize::MAX as u128),
    ];

    for (name, expected_kind, maximum) in cases {
        let definition = OPTION_CATALOG
            .iter()
            .find(|definition| definition.name == name)
            .unwrap();
        assert_eq!(definition.option_type, OptionType::Number(expected_kind));
        let maximum = maximum.to_string();
        assert!(
            parse_set_option_args(&["-g", name, &maximum])
                .validate(false)
                .is_ok(),
            "{name} should accept {maximum}",
        );

        let too_large = (maximum.parse::<u128>().unwrap() + 1).to_string();
        let rejection = parse_set_option_args(&["-g", name, &too_large])
            .validate(false)
            .expect_err(&format!("{name} should reject {too_large}"));
        assert!(
            rejection.contains("value is too large"),
            "{name} must report an out of range value the way tmux does, got {rejection}",
        );
    }
}

/// tmux types main-pane-width and main-pane-height OPTIONS_TABLE_STRING and
/// documents a percentage form (options-table.c:1474), so validating them as a
/// number rejected input real tmux accepts.
#[test]
fn main_pane_dimensions_are_strings_that_accept_percentages() {
    use crate::server::option_catalog::{OptionType, OPTION_CATALOG};

    for name in ["main-pane-width", "main-pane-height"] {
        let definition = OPTION_CATALOG
            .iter()
            .find(|definition| definition.name == name)
            .unwrap();
        assert_eq!(definition.option_type, OptionType::String);
        for value in ["0", "80", "100000", "10%"] {
            assert_eq!(
                parse_set_option_args(&["-g", name, value]).validate(false),
                Ok(()),
                "{name} should accept {value}",
            );
        }
    }

    // tmux keeps the value as written (a cell count or a percentage) and
    // resolves it when the layout is applied, so the percent has to survive.
    use crate::server::options::parse_main_pane_size;
    assert_eq!(parse_main_pane_size("50%").as_deref(), Some("50%"));
    assert_eq!(parse_main_pane_size("50").as_deref(), Some("50"));
    assert_eq!(parse_main_pane_size(" 10% ").as_deref(), Some("10%"));
    assert_eq!(parse_main_pane_size("wide"), None);
}

/// An integer that simply falls outside the destination range is reported the
/// way tmux reports it (strtonum errstr, options.c:1295), not as "expected a
/// number", which only fits a value that is not an integer at all.
#[test]
fn out_of_range_integers_are_reported_as_out_of_range() {
    let too_small = parse_set_option_args(&["-g", "base-index", "-1"])
        .validate(false)
        .expect_err("base-index must reject -1");
    assert!(
        too_small.contains("value is too small: -1"),
        "got {too_small}",
    );

    let not_a_number = parse_set_option_args(&["-g", "base-index", "1x"])
        .validate(false)
        .expect_err("base-index must reject 1x");
    assert!(
        not_a_number.contains("expected a number"),
        "got {not_a_number}",
    );

    let repeat_low = parse_set_option_args(&["-g", "repeat-time", "-5"])
        .validate(false)
        .expect_err("repeat-time must reject -5");
    assert!(repeat_low.contains("value is too small: -5"), "got {repeat_low}");

    let repeat_high = parse_set_option_args(&["-g", "repeat-time", "2000001"])
        .validate(false)
        .expect_err("repeat-time must reject 2000001");
    assert!(
        repeat_high.contains("value is too large: 2000001"),
        "got {repeat_high}",
    );
}

#[test]
fn index_options_reject_negative_and_malformed_values() {
    for name in ["base-index", "pane-base-index"] {
        for value in ["-1", "1x"] {
            assert!(
                parse_set_option_args(&["-g", name, value])
                    .validate(false)
                    .is_err(),
                "{name} should reject {value}",
            );
        }
    }
}

#[test]
fn validation_only_numeric_options_preserve_cli_compatibility() {
    use crate::server::option_catalog::{OPTION_CATALOG, VALIDATION_ONLY_OPTIONS};

    for name in ["message-limit", "history-file-limit"] {
        assert!(
            OPTION_CATALOG
                .iter()
                .all(|definition| definition.name != name),
            "{name} is validation-only and must not appear in option listings",
        );
        assert!(
            VALIDATION_ONLY_OPTIONS
                .iter()
                .any(|definition| definition.name == name),
            "{name} must have explicit validation metadata",
        );
        assert!(
            parse_set_option_args(&["-g", name, "-1"])
                .validate(false)
                .is_ok(),
            "{name} historically accepts signed integers",
        );
        assert!(
            parse_set_option_args(&["-g", name, "invalid"])
                .validate(false)
                .is_err(),
            "{name} should reject malformed integers",
        );
    }
}

/// tmux's set-option accepts `-F` (its flag table is "aFgoqst:uUw") and expands
/// the value as a format before storing it. psmux's config-file parser always
/// did this; the CLI and TCP paths rejected the flag outright because format
/// expansion needs the server's AppState. The value is now expanded on the
/// server thread, so `-F` works on every route.
#[test]
fn dash_f_is_an_accepted_flag_on_every_route() {
    assert!(
        crate::SET_OPTION_CLI_FLAGS.contains('F'),
        "set-option must accept tmux's -F",
    );
    assert_eq!(
        parse_set_option_args(&["-gF", "status-right", "#{session_name}"]).validate(false),
        Ok(()),
    );
}

/// A `-F` value is a FORMAT, so it is validated after expansion, not before.
/// Validating the raw `#{...}` would refuse a legitimate assignment to a
/// choice option whose expansion is perfectly valid.
#[test]
fn dash_f_defers_value_validation_to_the_expansion() {
    assert_eq!(
        parse_set_option_args(&["-gF", "pane-border-indicators", "#{@wanted}"])
            .validate(false),
        Ok(()),
    );
    assert!(
        parse_set_option_args(&["-g", "pane-border-indicators", "#{@wanted}"])
            .validate(false)
            .is_err(),
        "without -F the literal text is the value and must still be validated",
    );
}

#[test]
fn dash_f_stores_the_expanded_value() {
    let (name, value) = dispatched_assignment(
        &["-gF", "status-right", "#{session_name}"],
        "expanded-session",
    );
    assert_eq!(name, "status-right");
    assert_eq!(value, "expanded-session");
}

#[test]
fn without_dash_f_the_value_is_stored_verbatim() {
    let (name, value) = dispatched_assignment(
        &["-g", "status-right", "#{session_name}"],
        "expanded-session",
    );
    assert_eq!(name, "status-right");
    assert_eq!(
        value, "#{session_name}",
        "no -F means no expansion: the literal format text is the value",
    );
}

/// `-F` on text that contains no format is the identity, and it must not turn
/// into an empty or mangled value.
#[test]
fn dash_f_without_format_text_is_the_identity() {
    let (name, value) = dispatched_assignment(&["-gF", "status-right", "plain"], "plain");
    assert_eq!(name, "status-right");
    assert_eq!(value, "plain");
}
