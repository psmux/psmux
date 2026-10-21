// The warm claim carries the client's environment inline (`-E <hex>`) as well
// as in the `-e` file, so a current standby never has to open a file the
// client has just written (15 to 16 ms of on-access scanning, measured on the
// warm launch path).

use super::*;
use std::ffi::OsString;

fn block_of(pairs: &[(&str, &str)]) -> Vec<u8> {
    encode_env_block(pairs.iter().map(|(k, v)| (OsString::from(*k), OsString::from(*v))))
}

#[test]
fn hex_round_trips_an_environment_block() {
    let block = block_of(&[("PATH", r"C:\bin;C:\tools"), ("WT_SESSION", "abc-123"), ("EMPTY", "")]);
    let mut hex = String::new();
    for b in &block {
        hex.push_str(&format!("{:02x}", b));
    }
    assert_eq!(hex_decode(&hex).as_deref(), Some(block.as_slice()));
    let decoded = decode_env_block(&hex_decode(&hex).unwrap()).unwrap();
    assert!(decoded.iter().any(|(k, v)| k == "WT_SESSION" && v == "abc-123"));
}

#[test]
fn own_environment_hex_decodes_to_this_process_environment() {
    let bytes = hex_decode(&own_environment_hex()).expect("valid hex");
    let decoded = decode_env_block(&bytes).expect("a block");
    let path_now = std::env::var_os("PATH");
    let path_sent = decoded.iter().find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("PATH")).map(|(_, v)| v.clone());
    assert_eq!(path_sent, path_now);
}

#[test]
fn hex_decode_refuses_what_is_not_hex() {
    assert_eq!(hex_decode("abc"), None, "odd length");
    assert_eq!(hex_decode("zz"), None, "not a hex digit");
    assert_eq!(hex_decode(""), Some(Vec::new()));
}

#[test]
fn the_inline_block_answers_the_conpty_question_like_the_file() {
    // A real environment: a nearly empty block is refused as not sane (#659).
    let base: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(k, _)| !k.to_string_lossy().eq_ignore_ascii_case("PSMUX_CONPTY_DIR"))
        .collect();
    let mut with = base.clone();
    with.push((OsString::from("PSMUX_CONPTY_DIR"), OsString::from(r"C:\oc")));
    assert_eq!(
        client_conpty_dir_in_block(&encode_env_block(with.into_iter())),
        Some(Some(OsString::from(r"C:\oc")))
    );
    assert_eq!(client_conpty_dir_in_block(&encode_env_block(base.into_iter())), Some(None));
}

#[test]
fn claim_line_parser_takes_e_inline_without_touching_positionals() {
    let args = ["sess", r"C:\work", "-p", "normal", "-e", r"C:\d\w.claimenv.1", "-n", "win", "-E", "0a0b"];
    let parsed = crate::util::parse_claim_args_full(&args);
    assert_eq!(parsed.positionals, vec!["sess".to_string(), r"C:\work".to_string()]);
    assert_eq!(parsed.env_file.as_deref(), Some(r"C:\d\w.claimenv.1"));
    assert_eq!(parsed.window_name.as_deref(), Some("win"));
    assert_eq!(parsed.env_inline.as_deref(), Some("0a0b"));
}
