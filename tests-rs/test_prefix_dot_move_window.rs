// prefix + `.` — prompt for an index and move the current window there.
//
// tmux binds it by default. Measured on tmux 3.6a, `list-keys -T prefix`:
//
//     bind-key -T prefix . command-prompt -T target { move-window -t "%%" }
//
// psmux has no `{}` command blocks, so the binding below spells the same
// command the way psmux's parser reads it; parse_command_line strips the quotes
// before move-window sees the index. What is load bearing is `-T target` plus
// the `move-window -t %%` template, and those are asserted piece by piece.
//
// psmux had no `.` binding at all, so the key did nothing. Adding the line was
// not enough on its own: the client's command-prompt parser skipped an unknown
// flag WITHOUT its value, so `-T target` left `target` as the first positional
// and the template became `target move-window -t '%%'`. Enter then sent
// `target move-window -t '3'`, which is not a command.
//
// What move-window itself does with the resolved target is pinned by
// tests-rs/test_issue601_602_move_swap_window.rs (unit) and
// tests/test_issue601_602_move_swap_window.ps1 (end to end). This file pins the
// path from the key to the command line those already cover: the default
// binding, the flag parse, and the substituted line.

use super::*;

/// The default binding: tmux's command, in the quoting form psmux's parser
/// reads (tmux 3.6a prints its own as `-T target { move-window -t "%%" }`).
const TMUX_DOT: &str = "command-prompt -T target \"move-window -t '%%'\"";

/// Parse a whole `command-prompt ...` binding the way the key dispatcher does.
fn spec_of(binding: &str) -> CommandPromptSpec {
    parse_command_prompt_args(
        binding
            .strip_prefix("command-prompt")
            .expect("not a command-prompt binding")
            .trim_start(),
    )
}

fn default_for(key: &str) -> Option<&'static str> {
    crate::help::PREFIX_DEFAULTS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, cmd)| *cmd)
}

// ---------------------------------------------------------------------------
// The binding exists, and reaches the live key table
// ---------------------------------------------------------------------------

#[test]
fn prefix_dot_prompts_for_a_target_and_moves_the_window() {
    let cmd = default_for(".").expect("'.' missing from PREFIX_DEFAULTS");
    assert_eq!(cmd, TMUX_DOT);
    // The three parts that have to match tmux, whatever the quoting:
    assert!(cmd.starts_with("command-prompt "), "{}", cmd);
    assert!(cmd.contains("-T target"), "{}", cmd);
    let template = spec_of(cmd).template.expect("no template");
    assert_eq!(template.replace("'", "\""), "move-window -t \"%%\"");
}

#[test]
fn dot_is_a_bindable_key_name() {
    assert_eq!(
        crate::config::parse_key_name("."),
        Some((
            crossterm::event::KeyCode::Char('.'),
            crossterm::event::KeyModifiers::NONE
        )),
    );
}

#[test]
fn the_dot_default_lands_in_the_prefix_table() {
    // populate_default_bindings silently drops a default whose command
    // parse_command_to_action does not recognise. Prove this one survives.
    let mut app = crate::types::AppState::new("t-dot".to_string());
    crate::config::populate_default_bindings(&mut app);
    let table = app.key_tables.get("prefix").expect("no prefix table");
    let bind = table
        .iter()
        .find(|b| {
            b.key
                == (
                    crossterm::event::KeyCode::Char('.'),
                    crossterm::event::KeyModifiers::NONE,
                )
        })
        .expect("'.' did not reach the prefix key table");
    match &bind.action {
        crate::types::Action::Command(cmd) => assert_eq!(cmd, TMUX_DOT),
        other => panic!(
            "'.' became {}, not a command string",
            crate::commands::format_action(other)
        ),
    }
    assert!(!bind.repeat, "'.' is not a repeat binding in tmux");
}

// ---------------------------------------------------------------------------
// The flag parse: a value-taking flag consumes its value
// ---------------------------------------------------------------------------

#[test]
fn the_prompt_type_stays_out_of_the_template() {
    let spec = spec_of(TMUX_DOT);
    assert_eq!(spec.template.as_deref(), Some("move-window -t '%%'"));
    assert_eq!(spec.initial, "");
}

#[test]
fn a_target_flag_value_stays_out_of_the_template() {
    // -t is command-prompt's other value-taking flag.
    let spec = parse_command_prompt_args("-t %3 \"move-window -t '%%'\"");
    assert_eq!(spec.template.as_deref(), Some("move-window -t '%%'"));
}

#[test]
fn boolean_flags_do_not_eat_the_template() {
    for flags in ["-1", "-N", "-W", "-1 -N"] {
        let spec = parse_command_prompt_args(&format!("{} \"move-window -t '%%'\"", flags));
        assert_eq!(
            spec.template.as_deref(),
            Some("move-window -t '%%'"),
            "{} consumed the template",
            flags
        );
    }
}

#[test]
fn a_value_glued_to_its_flag_satisfies_it() {
    // tmux's args_parse_flags: a template letter with a character after it is
    // already satisfied, so the next token is not its value.
    let spec = parse_command_prompt_args("-pindex \"move-window -t '%%'\"");
    assert_eq!(spec.label.as_deref(), Some("index"));
    assert_eq!(spec.template.as_deref(), Some("move-window -t '%%'"));
}

#[test]
fn double_dash_ends_the_flags() {
    let spec = parse_command_prompt_args("-- -N");
    assert_eq!(spec.template.as_deref(), Some("-N"));
}

// ---------------------------------------------------------------------------
// The heading: with no -p, tmux names the command the prompt will run
// ---------------------------------------------------------------------------

#[test]
fn the_prompt_names_the_command_it_will_run() {
    assert_eq!(spec_of(TMUX_DOT).label.as_deref(), Some("(move-window)"));
}

#[test]
fn an_explicit_prompt_wins_over_the_derived_one() {
    let spec = parse_command_prompt_args("-p index \"move-window -t '%%'\"");
    assert_eq!(spec.label.as_deref(), Some("index"));
}

#[test]
fn the_heading_stops_at_a_space_or_a_comma() {
    // tmux: strcspn(template, " ,").
    assert_eq!(
        parse_command_prompt_args("'rename-window \"%%\"'").label.as_deref(),
        Some("(rename-window)"),
    );
    assert_eq!(
        parse_command_prompt_args("'new-window,split-window'").label.as_deref(),
        Some("(new-window)"),
    );
}

#[test]
fn a_bare_command_prompt_has_no_heading() {
    // prefix + `:` has no template, so there is no command to name.
    assert_eq!(parse_command_prompt_args("").label, None);
}

#[test]
fn an_initial_value_and_a_prompt_still_parse() {
    // The shape a user's config uses for a rename binding, unchanged.
    let spec = parse_command_prompt_args("-p 'new name:' -I '#W' 'rename-window \"%%\"'");
    assert_eq!(spec.initial, "#W");
    assert_eq!(spec.label.as_deref(), Some("new name:"));
    assert_eq!(spec.template.as_deref(), Some("rename-window \"%%\""));
}

#[test]
fn a_bare_command_prompt_has_no_template() {
    // prefix + `:` — whatever is typed is the whole command.
    assert_eq!(default_for(":"), Some("command-prompt"));
    assert_eq!(parse_command_prompt_args(""), CommandPromptSpec::default());
}

#[test]
fn several_positionals_join_into_one_template() {
    let spec = parse_command_prompt_args("-T target move-window -t %%");
    assert_eq!(spec.template.as_deref(), Some("move-window -t %%"));
}

// ---------------------------------------------------------------------------
// What Enter sends
// ---------------------------------------------------------------------------

#[test]
fn a_typed_index_reaches_move_window_as_one_argument() {
    let template = spec_of(TMUX_DOT).template.expect("no template");
    let line = template.replace("%%", "3");
    assert_eq!(line, "move-window -t '3'");
    // The server tokenizes with parse_command_line, which strips the quotes
    // tmux's binding puts around the index.
    assert_eq!(
        crate::commands::parse_command_line(&line),
        vec![
            "move-window".to_string(),
            "-t".to_string(),
            "3".to_string()
        ],
    );
}

#[test]
fn a_symbolic_target_survives_the_substitution() {
    // resolve_window_spec accepts +N / {end} / a name; the prompt must not
    // mangle them on the way.
    let template = spec_of(TMUX_DOT).template.expect("no template");
    for typed in ["+1", "-1", "{end}", "$"] {
        assert_eq!(
            crate::commands::parse_command_line(&template.replace("%%", typed)),
            vec![
                "move-window".to_string(),
                "-t".to_string(),
                typed.to_string()
            ],
            "typed {}",
            typed
        );
    }
}

#[test]
fn the_generated_line_passes_the_flag_validator() {
    // Every command ingress runs validate_command_line_flags (#635) before
    // dispatch; a line it rejects never reaches move-window.
    let template = spec_of(TMUX_DOT).template.expect("no template");
    let tokens = crate::commands::parse_command_line(&template.replace("%%", "3"));
    assert_eq!(crate::cli::validate_command_line_flags(&tokens), Ok(()));
}

#[test]
fn the_flag_table_is_what_the_parser_asks() {
    // The parser reads ARGS_TEMPLATES rather than keeping its own list, so pin
    // the answers it depends on.
    assert!(crate::cli::flag_takes_value("command-prompt", 'T'));
    assert!(crate::cli::flag_takes_value("command-prompt", 't'));
    assert!(crate::cli::flag_takes_value("command-prompt", 'I'));
    assert!(crate::cli::flag_takes_value("command-prompt", 'p'));
    assert!(!crate::cli::flag_takes_value("command-prompt", 'N'));
    assert!(!crate::cli::flag_takes_value("command-prompt", '1'));
    // 'W' is not in tmux's template at all, so it is not ours to police.
    assert!(!crate::cli::flag_takes_value("command-prompt", 'W'));
}
