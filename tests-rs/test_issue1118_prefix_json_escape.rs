// Issue #1118: `set -g prefix "C-\\"` put an unescaped `C-\` into the client
// state JSON (`"prefix":"C-\"`), which ended the string early, so the client
// could not parse a single frame: blank screen, and Ctrl+\ went to the pane
// instead of being read as the prefix. These build the same fragment the
// server's frame uses and parse it back the way the client does.
use super::prefix_json_strings;
use crossterm::event::{KeyCode, KeyModifiers};

fn round_trip(
    prefix: Option<(KeyCode, KeyModifiers)>,
    prefix2: Option<(KeyCode, KeyModifiers)>,
) -> (String, String) {
    let (p, p2) = prefix_json_strings(&prefix, prefix2.as_ref());
    let frame = format!("{{\"prefix\":\"{}\",\"prefix2\":\"{}\"}}", p, p2);
    let v: serde_json::Value = serde_json::from_str(&frame)
        .unwrap_or_else(|e| panic!("frame does not parse: {e}: {frame}"));
    (
        v["prefix"].as_str().unwrap().to_string(),
        v["prefix2"].as_str().unwrap().to_string(),
    )
}

#[test]
fn ctrl_backslash_prefix_survives_the_frame() {
    let (p, p2) = round_trip(Some((KeyCode::Char('\\'), KeyModifiers::CONTROL)), None);
    assert_eq!(p, "C-\\");
    assert_eq!(p2, "");
}

#[test]
fn ctrl_backslash_prefix2_survives_the_frame() {
    let (p, p2) = round_trip(
        Some((KeyCode::Char('b'), KeyModifiers::CONTROL)),
        Some((KeyCode::Char('\\'), KeyModifiers::CONTROL)),
    );
    assert_eq!(p, "C-b");
    assert_eq!(p2, "C-\\");
}

#[test]
fn double_quote_prefix_survives_the_frame() {
    let (p, _) = round_trip(Some((KeyCode::Char('"'), KeyModifiers::NONE)), None);
    assert_eq!(p, "\"");
}

#[test]
fn no_prefix_is_sent_as_none() {
    let (p, p2) = round_trip(None, None);
    assert_eq!(p, "None");
    assert_eq!(p2, "");
}

#[test]
fn plain_prefix_is_unchanged() {
    let (p, p2) = round_trip(Some((KeyCode::Char('a'), KeyModifiers::CONTROL)), None);
    assert_eq!(p, "C-a");
    assert_eq!(p2, "");
}
