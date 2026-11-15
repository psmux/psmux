// default-shell (or default-command, or a command string) naming a program
// whose path has a space, unquoted, with arguments after it:
// `C:\Program Files\PowerShell\7\pwsh.exe -NoLogo -NoProfile`. The word split
// cut the path at its first space and `C:\Program` was spawned, so no session
// started ("CreateProcessW ... The system cannot find the file specified").
// The program is now the longest run of words from the start that names an
// existing file, and the rest are its arguments.
// Registered from src/pane.rs.

use super::*;

/// A fresh folder under the temp dir whose name has a space in it, holding an
/// empty `sh.exe` (only its existence matters to the split).
fn shell_in_spaced_dir(tag: &str) -> (std::path::PathBuf, String) {
    let base = std::env::temp_dir().join(format!("psmux shell {} {}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dir = base.join("Program Files").join("Shell");
    std::fs::create_dir_all(&dir).expect("create the spaced dir");
    let exe = dir.join("sh.exe");
    std::fs::write(&exe, b"").expect("create sh.exe");
    (base, exe.to_string_lossy().into_owned())
}

#[test]
fn an_unquoted_spaced_path_with_args_splits_after_the_program() {
    let (base, exe) = shell_in_spaced_dir("args");
    let (program, args) = resolve_shell_program(&format!("{exe} -NoLogo -NoProfile"));
    assert_eq!(program, exe);
    assert_eq!(args, vec!["-NoLogo".to_string(), "-NoProfile".to_string()]);
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn the_longest_existing_prefix_wins_over_a_planted_shorter_one() {
    let (base, exe) = shell_in_spaced_dir("planted");
    // `<base>\Program` (no extension) next to `<base>\Program Files\...`: the
    // first word on its own is a file now, the case CreateProcess's unquoted
    // search gets wrong. It is still the long path that is spawned.
    let planted = base.join("Program");
    std::fs::write(&planted, b"").expect("plant Program");
    let (program, args) = resolve_shell_program(&format!("{exe} --norc"));
    assert_eq!(program, exe, "never the planted {}", planted.display());
    assert_eq!(args, vec!["--norc".to_string()]);
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn the_longest_of_several_existing_prefixes_is_the_program() {
    let (base, exe) = shell_in_spaced_dir("longest");
    // `<base>\Program Files\Shell\sh.exe x` also exists, as a file: the
    // longest prefix that is a file wins.
    let longer = format!("{exe} x");
    std::fs::write(&longer, b"").expect("create the longer file");
    let (program, args) = resolve_shell_program(&format!("{exe} x -l"));
    assert_eq!(program, longer);
    assert_eq!(args, vec!["-l".to_string()]);
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn a_spaced_path_alone_is_the_whole_program() {
    let (base, exe) = shell_in_spaced_dir("alone");
    assert_eq!(resolve_shell_program(&exe), (exe.clone(), vec![]));
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn a_quoted_spaced_path_with_args_is_unchanged() {
    let (base, exe) = shell_in_spaced_dir("quoted");
    let (program, args) = resolve_shell_program(&format!("\"{exe}\" --norc"));
    assert_eq!(program, exe);
    assert_eq!(args, vec!["--norc".to_string()]);
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn a_bare_name_with_args_is_unchanged() {
    assert_eq!(
        resolve_shell_program("pwsh -NoLogo -NoProfile"),
        ("pwsh".to_string(), vec!["-NoLogo".to_string(), "-NoProfile".to_string()])
    );
}

#[test]
fn a_spaced_path_that_does_not_exist_keeps_the_plain_split() {
    let (program, args) = resolve_shell_program(r"C:\No Such Dir 1058\sh.exe -l");
    assert_eq!(program, r"C:\No");
    assert_eq!(args.last().map(String::as_str), Some("-l"));
}

#[test]
fn the_built_command_and_the_window_name_use_the_whole_program() {
    let (base, exe) = shell_in_spaced_dir("built");
    let builder = build_default_shell(&format!("{exe} --norc"), false, false);
    let argv: Vec<String> = builder.get_argv().iter().map(|s| s.to_string_lossy().into_owned()).collect();
    assert_eq!(argv[0], exe, "argv: {argv:?}");
    assert_eq!(argv[1], "--norc", "argv: {argv:?}");
    assert_eq!(default_shell_name(None, Some(&format!("{exe} --norc"))), "sh");
    let _ = std::fs::remove_dir_all(base);
}
