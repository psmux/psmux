// Issue #888: Alt+' never reached the pane from an attached client.
//
// The client forwards a key as one wire line, `send-key <name>`, and the
// server tokenizes that line with the quote aware command parser.  Written
// raw, `send-key M-'` opened a single quote at the `'`, so the server read the
// key as `M-` and wrote nothing.  `M-"` lost its key the same way, and so did
// `C-'` and `C-M-'`.  The CLI route quoted its argv and always worked.
use super::*;

/// What the server's line handler does with one wire line: split it into
/// sub-commands, then tokenize the first one.
fn server_reads(line: &str) -> Vec<String> {
    let line = line.trim_end_matches('\n');
    let subs = crate::config::split_chained_commands_pub(line.trim());
    assert_eq!(subs.len(), 1, "[{line}] split into {subs:?}");
    crate::commands::parse_command_line(&subs[0])
}

#[test]
fn raw_line_is_what_lost_the_key() {
    // The root cause, pinned: these are the lines the client used to write.
    assert_eq!(crate::commands::parse_command_line("send-key M-'"), vec!["send-key", "M-"]);
    assert_eq!(crate::commands::parse_command_line("send-key M-\""), vec!["send-key", "M-"]);
}

#[test]
fn quote_keys_survive_the_wire() {
    for k in ["M-'", "M-\"", "C-'", "C-M-'", "C-M-\"", "C-S-'", "C-\""] {
        assert_eq!(server_reads(&send_key_line(k)), vec!["send-key", k], "key {k}");
    }
}

#[test]
fn backslash_semicolon_hash_and_brace_keys_survive_the_wire() {
    for k in ["M-\\", "C-\\", "C-M-\\", "M-;", "C-;", "M-#", "M-{", "M-}", "M-`", "M-$", "M-~", "M-|"] {
        assert_eq!(server_reads(&send_key_line(k)), vec!["send-key", k], "key {k}");
    }
}

#[test]
fn every_printable_ascii_key_survives_every_modifier_prefix() {
    for b in 0x21u8..=0x7e {
        let c = b as char;
        for prefix in ["M-", "C-", "C-M-", "C-S-"] {
            let k = format!("{prefix}{c}");
            assert_eq!(server_reads(&send_key_line(&k)), vec!["send-key".to_string(), k.clone()], "key {k}");
        }
    }
}

#[test]
fn plain_names_stay_unquoted() {
    // Nothing that worked before changes shape on the wire.
    assert_eq!(send_key_line("M-a"), "send-key M-a\n");
    assert_eq!(send_key_line("C-Space"), "send-key C-Space\n");
    assert_eq!(send_key_line("M-;"), "send-key M-;\n");
    assert_eq!(send_key_line("M-\\"), "send-key M-\\\n");
}
