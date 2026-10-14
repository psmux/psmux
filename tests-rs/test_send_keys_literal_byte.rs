// `send-keys -H` is a byte channel: every operand is one hexadecimal byte value
// that reaches the pty verbatim (tmux flags those keys KEYC_LITERAL and
// input_key.c writes them as single bytes). psmux accepted the flag but never
// implemented it, so the operands fell through to the key-name lookup and were
// echoed into the pane as literal text -- typing `echo` produced "65 63 68 6f".
//
// These tests pin the two pure functions the fix rests on:
//   * decode_send_command  -- per-command operand decoding
//   * coalesce_send_commands -- merging consecutive sub-commands on one line
//
// They deliberately start no server (see AGENTS.md).

use super::*;

fn decode(line: &str) -> Option<(String, Vec<u8>)> {
    decode_send_command(line)
}

#[test]
fn literal_byte_operands_decode_to_raw_bytes() {
    // The repro: "echo" as bare hex bytes.
    let (target, bytes) = decode("send-keys -H -t %1 65 63 68 6f").expect("must decode");
    assert_eq!(target, "%1");
    assert_eq!(bytes, b"echo");
}

#[test]
fn literal_byte_carries_utf8_sequences_unchanged() {
    // 中 = E4 B8 AD. A byte channel must not re-encode these.
    let (_, bytes) = decode("send-keys -H -t %1 e4 b8 ad").expect("must decode");
    assert_eq!(bytes, vec![0xe4, 0xb8, 0xad]);
    assert_eq!(String::from_utf8(bytes).unwrap(), "中");
}

#[test]
fn literal_byte_accepts_bytes_that_are_not_valid_utf8() {
    // A caller may split a multi-byte character across chunked send-keys calls,
    // so individual commands are not required to be valid UTF-8.
    let (_, bytes) = decode("send-keys -H -t %1 41 00 ff 42").expect("must decode");
    assert_eq!(bytes, vec![0x41, 0x00, 0xff, 0x42]);
}

#[test]
fn hex_codepoint_operands_stay_codepoints() {
    // `0xNN` is the iTerm2 encoding for typed characters and means a codepoint,
    // so it is UTF-8 encoded. This is the opposite of -H and must stay that way.
    let (_, bytes) = decode("send -t %1 0x4e2d").expect("must decode");
    assert_eq!(bytes, vec![0xe4, 0xb8, 0xad], "0x4e2d is 中, encoded as UTF-8");

    // Latin-1 range: a codepoint, not a byte.
    let (_, bytes) = decode("send -t %1 0xe9").expect("must decode");
    assert_eq!(bytes, vec![0xc3, 0xa9], "0xe9 is é, encoded as UTF-8");
}

#[test]
fn malformed_literal_byte_operand_refuses_to_coalesce() {
    // Returning None leaves the command to the send-keys handler rather than
    // silently dropping or mangling it here.
    assert!(decode("send-keys -H -t %1 zz").is_none());
}

#[test]
fn coalescing_merges_mixed_encodings_into_one_write() {
    // iTerm2 splits an arrow key into three differently-encoded sub-commands.
    // They must merge into a single pty write, otherwise PSReadLine times out
    // between the ESC and the "[A" and prints them as literal characters.
    let parts = vec![
        "send -H -t %1 1b".to_string(),
        "send -t %1 0x5b".to_string(),
        "send -lt %1 A".to_string(),
    ];
    let merged = coalesce_send_commands(parts);
    assert_eq!(merged.len(), 3, "one write plus a reply placeholder per merged command (#799)");
    assert_eq!(merged[0], "send -H -t %1 1b 5b 41");
    assert_eq!(merged[1], format!("{}send -t %1 0x5b", COALESCED_SEND_REPLY));
    assert_eq!(merged[2], format!("{}send -lt %1 A", COALESCED_SEND_REPLY));
}

#[test]
fn coalesced_carrier_is_byte_exact_for_multibyte_input() {
    // The carrier used to be `send -l '<latin-1>'`, which mapped each byte
    // through `as char` and encoded already-UTF-8 bytes a second time: 中
    // arrived at the pane as "ä¸­". The -H carrier has no such round trip.
    let merged = coalesce_send_commands(vec!["send -t %1 0x4e2d".to_string()]);
    assert_eq!(merged, vec!["send -H -t %1 e4 b8 ad"]);

    // And the carrier re-decodes to exactly the bytes we started with.
    let (_, bytes) = decode(&merged[0]).expect("carrier must decode");
    assert_eq!(String::from_utf8(bytes).unwrap(), "中");
}

#[test]
fn coalescing_keeps_distinct_targets_apart() {
    let merged = coalesce_send_commands(vec![
        "send -H -t %1 41".to_string(),
        "send -H -t %2 42".to_string(),
    ]);
    assert_eq!(merged.len(), 2, "different panes must not be merged");
    assert_eq!(merged[0], "send -H -t %1 41");
    assert_eq!(merged[1], "send -H -t %2 42");
}

#[test]
fn non_send_commands_pass_through_untouched() {
    let parts = vec![
        "send -H -t %1 41".to_string(),
        "refresh-client -C 80,25".to_string(),
        "send -H -t %1 42".to_string(),
    ];
    let merged = coalesce_send_commands(parts);
    assert_eq!(
        merged,
        vec!["send -H -t %1 41", "refresh-client -C 80,25", "send -H -t %1 42"],
        "an unrelated command between two sends must break the run and survive verbatim"
    );
}
