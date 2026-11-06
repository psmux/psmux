use super::*;

const INPUTS: &[&[&str]] = &[
    &["-t", "B:", "@probe", "v"],
    &["-t", "A:", "-t", "B:", "@probe", "v"],
    &["@probe", "v", "-t", "B:"],
    &["-gqt", "B:", "@probe", "v"],
    &["-t", "B:", "@probe", ";"],
    &["-t", "B:", "@probe", "\\;"],
    &["-t", "B:", "@probe", "\\\\;"],
    &["-t", "B:", "@probe", "a\\\\;"],
    &["-t", "B:", "@probe", "\\\\\\;"],
    &["-t", "B:", "@probe", "a b"],
    &["-t", "B:", "@probe", "--"],
    &["-t", "B:", "@probe", "-t"],
    &["-t", "B:", "--", "@probe", "-t"],
    &["-t", "B:", "@probe", ""],
];

fn flags_without_target(flag_chars: &str) -> String {
    flag_chars.chars().filter(|flag| *flag != 't').collect()
}

#[test]
fn forwarded_set_line_reads_back_as_the_same_set() {
    for args in INPUTS {
        let original = parse_set_option_args(args);
        let target = original.target.expect("input has a target");
        let line = forward_set_line("set", &original, target);
        let words = parse_command_line(&line);
        assert_eq!(
            words.first().map(String::as_str),
            Some("set"),
            "line {:?}",
            line
        );
        let rest: Vec<&str> = words[1..].iter().map(String::as_str).collect();
        let again = parse_set_option_args(&rest);
        assert_eq!(again.target, original.target, "line {:?}", line);
        assert_eq!(
            flags_without_target(&again.flag_chars),
            flags_without_target(&original.flag_chars),
            "line {:?}",
            line
        );
        assert_eq!(again.positionals, original.positionals, "line {:?}", line);
    }
}

#[test]
fn forwarded_set_line_stays_one_command() {
    for args in INPUTS {
        let original = parse_set_option_args(args);
        let target = original.target.expect("input has a target");
        let line = forward_set_line("set", &original, target);
        assert_eq!(
            crate::config::split_chained_commands_pub(&line),
            [line.as_str()],
            "line {:?}",
            line
        );
    }
}

#[test]
fn forwarded_set_words_come_back_from_the_quote() {
    for args in INPUTS {
        let original = parse_set_option_args(args);
        let target = original.target.expect("input has a target");
        for word in std::iter::once(target).chain(original.positionals.iter().copied()) {
            assert_eq!(
                parse_command_line(&crate::util::quote_arg_for_handler(word)),
                [word],
                "word {:?}",
                word
            );
        }
    }
}
