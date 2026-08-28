//! Issue #615: `#{pane_current_path}` is not updated inside `wsl`.
//!
//! The Win32 working directory of `wsl.exe` / `wslhost.exe` is frozen at the
//! moment the process is created, so the pane's PEB cwd stays wherever the user
//! was *before* typing `wsl` no matter how much the Linux shell moves around.
//! The only thing that can tell psmux the truth is the shell itself, via OSC 7
//! or OSC 9;9, and the announced value is a POSIX path that Windows cannot open
//! as-is.  These tests pin the translation.
//!
//! Filter: `cargo test issue615`

use super::osc_cwd_to_windows as tr;

const UBUNTU: Option<&str> = Some("Ubuntu");

// ---------------------------------------------------------------------------
// WSL drive mounts: the reported case.
// ---------------------------------------------------------------------------

#[test]
fn issue615_wsl_mnt_becomes_drive_letter() {
    assert_eq!(tr("/mnt/c/Users", UBUNTU).as_deref(), Some(r"C:\Users"));
    assert_eq!(tr("/mnt/d/x", UBUNTU).as_deref(), Some(r"D:\x"));
    assert_eq!(
        tr("/mnt/c/Users/godwin/src", UBUNTU).as_deref(),
        Some(r"C:\Users\godwin\src")
    );
}

#[test]
fn issue615_wsl_mnt_drive_root_keeps_its_separator() {
    // `C:` alone means "the current directory on C:", which is not what the
    // shell said.  The root must stay `C:\`.
    assert_eq!(tr("/mnt/c", UBUNTU).as_deref(), Some(r"C:\"));
    assert_eq!(tr("/mnt/c/", UBUNTU).as_deref(), Some(r"C:\"));
}

#[test]
fn issue615_wsl_mnt_uppercases_the_drive_letter() {
    assert_eq!(tr("/mnt/e/tmp", UBUNTU).as_deref(), Some(r"E:\tmp"));
}

#[test]
fn issue615_full_osc7_url_from_a_wsl_shell() {
    // What `printf '\033]7;file://%s%s\033\\' "$HOSTNAME" "$PWD"` emits.
    assert_eq!(
        tr("file://SuperFlow/mnt/c/Users", UBUNTU).as_deref(),
        Some(r"C:\Users")
    );
    // Empty authority is legal and means "this machine".
    assert_eq!(tr("file:///mnt/c/Users", UBUNTU).as_deref(), Some(r"C:\Users"));
}

// ---------------------------------------------------------------------------
// Linux-only paths: reachable from Windows only through the distro's UNC view.
// ---------------------------------------------------------------------------

#[test]
fn issue615_linux_only_path_uses_the_distro_unc_share() {
    assert_eq!(
        tr("/home/u", UBUNTU).as_deref(),
        Some(r"\\wsl.localhost\Ubuntu\home\u")
    );
    assert_eq!(
        tr("file://SuperFlow/home/gj", UBUNTU).as_deref(),
        Some(r"\\wsl.localhost\Ubuntu\home\gj")
    );
    assert_eq!(tr("/", UBUNTU).as_deref(), Some(r"\\wsl.localhost\Ubuntu"));
}

#[test]
fn issue615_linux_only_path_without_a_distro_is_refused_not_guessed() {
    // Better to leave `#{pane_current_path}` at its last known good value than
    // to hand `split-window -c` a path that cannot be opened.
    assert_eq!(tr("/home/u", None), None);
    assert_eq!(tr("/home/u", Some("")), None);
}

#[test]
fn issue615_mnt_wsl_is_a_directory_not_a_drive() {
    // A single letter after /mnt is a drive; anything longer is a real Linux
    // directory (`/mnt/wsl`, `/mnt/wslg`, `/mnt/data`).
    assert_eq!(
        tr("/mnt/wsl/foo", UBUNTU).as_deref(),
        Some(r"\\wsl.localhost\Ubuntu\mnt\wsl\foo")
    );
    assert_eq!(
        tr("/mnt/data", UBUNTU).as_deref(),
        Some(r"\\wsl.localhost\Ubuntu\mnt\data")
    );
}

// ---------------------------------------------------------------------------
// Windows-side emitters must survive untouched.
// ---------------------------------------------------------------------------

#[test]
fn issue615_windows_file_url_from_pwsh_shell_integration() {
    assert_eq!(
        tr("file:///C:/Users", UBUNTU).as_deref(),
        Some(r"C:\Users")
    );
    assert_eq!(
        tr("file:///C:/Windows/Temp", None).as_deref(),
        Some(r"C:\Windows\Temp")
    );
    assert_eq!(tr("file:///C:/", None).as_deref(), Some(r"C:\"));
}

#[test]
fn issue615_bare_windows_path_from_osc_9_9() {
    // ConEmu / Windows Terminal OSC 9;9 sends a plain path, sometimes quoted.
    assert_eq!(tr(r"C:\Windows", None).as_deref(), Some(r"C:\Windows"));
    assert_eq!(tr("C:/Windows", None).as_deref(), Some(r"C:\Windows"));
    assert_eq!(tr("\"C:\\Windows\"", None).as_deref(), Some(r"C:\Windows"));
    assert_eq!(tr(r"C:\", None).as_deref(), Some(r"C:\"));
}

#[test]
fn issue615_unc_paths_pass_through() {
    assert_eq!(
        tr(r"\\server\share\dir", None).as_deref(),
        Some(r"\\server\share\dir")
    );
    assert_eq!(
        tr(r"\\wsl.localhost\Ubuntu\home\gj", None).as_deref(),
        Some(r"\\wsl.localhost\Ubuntu\home\gj")
    );
}

// ---------------------------------------------------------------------------
// Escaping.
// ---------------------------------------------------------------------------

#[test]
fn issue615_percent_escapes_are_decoded_in_a_url() {
    assert_eq!(
        tr("file:///C:/Program%20Files", None).as_deref(),
        Some(r"C:\Program Files")
    );
    assert_eq!(
        tr("file://host/mnt/c/Program%20Files/Git", UBUNTU).as_deref(),
        Some(r"C:\Program Files\Git")
    );
    // UTF-8 percent escapes reassemble into one character.
    assert_eq!(
        tr("file:///C:/caf%C3%A9", None).as_deref(),
        Some("C:\\café")
    );
}

#[test]
fn issue615_percent_is_literal_in_a_bare_path() {
    // `%` is a legal Windows filename character.  A bare OSC 9;9 payload never
    // went through a URL encoder, so decoding it would corrupt the name.
    assert_eq!(
        tr(r"C:\tmp\100%20", None).as_deref(),
        Some(r"C:\tmp\100%20")
    );
}

#[test]
fn issue615_malformed_escapes_are_left_alone() {
    assert_eq!(
        tr("file:///C:/a%zzb", None).as_deref(),
        Some(r"C:\a%zzb")
    );
    assert_eq!(tr("file:///C:/a%", None).as_deref(), Some(r"C:\a%"));
}

// ---------------------------------------------------------------------------
// Rejections.  Returning `None` makes the caller keep the last known path,
// which is what tmux does when `osdep_get_cwd` fails (format.c:965).
// ---------------------------------------------------------------------------

#[test]
fn issue615_unusable_payloads_are_refused() {
    assert_eq!(tr("", UBUNTU), None);
    assert_eq!(tr("   ", UBUNTU), None);
    assert_eq!(tr("relative/path", UBUNTU), None);
    assert_eq!(tr("file://", UBUNTU), None);
}

#[test]
fn issue615_trailing_separator_is_trimmed_except_at_a_drive_root() {
    assert_eq!(tr("/mnt/c/Users/", UBUNTU).as_deref(), Some(r"C:\Users"));
    assert_eq!(tr(r"C:\Users\", None).as_deref(), Some(r"C:\Users"));
    assert_eq!(tr(r"C:\", None).as_deref(), Some(r"C:\"));
    assert_eq!(
        tr("/home/gj/", UBUNTU).as_deref(),
        Some(r"\\wsl.localhost\Ubuntu\home\gj")
    );
}

#[test]
fn issue615_translated_paths_are_absolute_and_openable_in_shape() {
    // Whatever comes back must be something `split-window -c` can hand to
    // CreateProcess: a drive-rooted path or a UNC path, never a POSIX one.
    for raw in [
        "/mnt/c/Users",
        "file://h/mnt/c/Users",
        "/home/gj",
        "file:///C:/Windows",
        r"C:\Windows",
    ] {
        let got = tr(raw, UBUNTU).expect(raw);
        assert!(
            got.starts_with(r"\\") || is_drive_rooted(&got),
            "{raw} translated to {got}, which Windows cannot open"
        );
        assert!(!got.contains('/'), "{raw} translated to {got}, still POSIX");
    }
}

fn is_drive_rooted(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}
