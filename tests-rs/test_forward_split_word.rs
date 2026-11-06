use super::*;

const TABLE: &[(&str, &str)] = &[
    (";", "\";\""),
    ("\\;", "\"\\\\;\""),
    ("\\\\;", "\"\\\\\\\\;\""),
    ("a\\\\;", "\"a\\\\\\\\;\""),
    ("\\\\\\;", "\\\\\\;"),
    ("x;", "\"x;\""),
    ("M-;", "M-;"),
    ("a;b", "a;b"),
    ("x", "x"),
    ("a b", "\"a b\""),
    ("\\e[99~", "\\e[99~"),
    ("a\rb", "\"a\\rb\""),
];

fn forwarded_line(word: &str) -> (Vec<String>, String) {
    let words: Vec<String> = ["set", "-t", "B:", "@probe", word, "set", "@other", "v"]
        .iter()
        .map(|part| part.to_string())
        .collect();
    let line = words
        .iter()
        .map(|part| quote_arg_for_handler(part))
        .collect::<Vec<_>>()
        .join(" ");
    (words, line)
}

#[test]
fn forwarded_word_is_quoted_as_the_table_says() {
    for (word, expected) in TABLE {
        assert_eq!(quote_arg_for_handler(word), *expected, "word {:?}", word);
    }
}

#[test]
fn forwarded_line_stays_one_command_with_the_same_words() {
    for (word, _) in TABLE {
        let (words, line) = forwarded_line(word);
        assert_eq!(
            crate::config::split_chained_commands_pub(&line),
            [line.as_str()],
            "word {:?}",
            word
        );
        let parsed = crate::commands::parse_command_line(&line);
        if word.contains('\r') {
            let mut expected = words.clone();
            expected[4] = word.replace('\r', "\\r");
            assert_eq!(parsed, expected, "word {:?}", word);
        } else {
            assert_eq!(parsed, words, "word {:?}", word);
        }
    }
}

#[test]
fn forwarded_words_the_splitter_keeps_are_unchanged() {
    let words = [
        "set", "-t", "B:", "@probe", "a b", "x;", "M-;", "a;b", "\\\\\\;", "\\e[99~",
    ];
    for word in words {
        assert_eq!(
            quote_arg_for_handler(word),
            quote_arg_if_needed(word),
            "word {:?}",
            word
        );
    }
}
