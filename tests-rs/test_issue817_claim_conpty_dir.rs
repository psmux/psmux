//! #817: a warm standby chose its console host (inbox conhost or the
//! OpenConsole `PSMUX_CONPTY_DIR` names) when it opened its first pane, and
//! that choice cannot change for the life of the process.  A claim whose
//! client asks for a different host must therefore be refused, so the client
//! cold spawns a server that makes the choice from the client's environment.

use super::*;

fn os(s: &str) -> OsString {
    OsString::from(s)
}

fn block(extra: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    let mut v: Vec<(OsString, OsString)> = [
        ("PATH", r"C:\Windows;C:\psmux"),
        ("SYSTEMROOT", r"C:\Windows"),
        ("USERPROFILE", r"C:\Users\x"),
        ("COMSPEC", r"C:\Windows\system32\cmd.exe"),
    ]
    .iter()
    .map(|(k, v)| (os(k), os(v)))
    .collect();
    v.extend(extra.iter().map(|(k, v)| (os(k), os(v))));
    v
}

#[test]
fn conpty_dir_read_from_block_any_casing() {
    assert_eq!(conpty_dir_in(&block(&[])), None);
    assert_eq!(
        conpty_dir_in(&block(&[("psmux_conpty_dir", r"D:\occ")])),
        Some(os(r"D:\occ"))
    );
    // Empty is unset, as the loader treats it.
    assert_eq!(conpty_dir_in(&block(&[("PSMUX_CONPTY_DIR", "")])), None);
}

#[test]
fn same_conpty_dir_unset_matches_only_unset() {
    assert!(same_conpty_dir(None, None));
    assert!(!same_conpty_dir(None, Some(OsStr::new(r"D:\occ"))));
    assert!(!same_conpty_dir(Some(OsStr::new(r"D:\occ")), None));
}

#[test]
fn same_conpty_dir_ignores_case_separator_and_trailing_slash() {
    let a = OsStr::new(r"C:\Tools\OpenConsole");
    assert!(same_conpty_dir(Some(a), Some(OsStr::new(r"c:\tools\openconsole\"))));
    assert!(same_conpty_dir(Some(a), Some(OsStr::new("C:/Tools/OpenConsole/"))));
    assert!(!same_conpty_dir(Some(a), Some(OsStr::new(r"C:\Tools\OpenConsole2"))));
}

#[test]
fn client_conpty_dir_reads_without_consuming() {
    let dir = std::env::temp_dir().join(format!("psmux-i817-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("claimenv");
    let p = path.to_str().unwrap();

    std::fs::write(&path, encode_env_block(block(&[("PSMUX_CONPTY_DIR", r"D:\occ")]))).unwrap();
    assert_eq!(client_conpty_dir(p), Some(Some(os(r"D:\occ"))));
    // Adoption reads the same file afterwards, so the check must leave it.
    assert!(path.exists());

    std::fs::write(&path, encode_env_block(block(&[]))).unwrap();
    assert_eq!(client_conpty_dir(p), Some(None));

    // A payload adoption would ignore cannot refuse a claim either.
    std::fs::write(&path, encode_env_block(vec![(os("A"), os("b"))])).unwrap();
    assert_eq!(client_conpty_dir(p), None);
    let _ = std::fs::remove_file(&path);
    assert_eq!(client_conpty_dir(p), None);
    let _ = std::fs::remove_dir_all(&dir);
}
