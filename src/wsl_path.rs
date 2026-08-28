//! Translation of a shell-announced working directory (OSC 7 / OSC 9;9) into a
//! native Windows path.  Issue #615.
//!
//! # Why this exists
//!
//! tmux answers `#{pane_current_path}` by asking the operating system for the
//! working directory of the pane's foreground process
//! (`format.c:965 format_cb_current_path` -> `osdep_get_cwd`, which on Linux
//! reads `/proc/<tcgetpgrp(fd)>/cwd`).  psmux does the equivalent on Windows by
//! reading the PEB `RTL_USER_PROCESS_PARAMETERS.CurrentDirectory` of the pane's
//! foreground process.
//!
//! That works for every shell whose `cd` moves the *Win32* working directory:
//!
//! ```text
//!   pwsh         cd C:\Users            -> PEB cwd becomes C:\Users\
//!   cygwin bash  cd /cygdrive/c/Users   -> PEB cwd becomes C:\Users\
//!   git bash     cd /c/Users            -> PEB cwd becomes C:\Users\
//! ```
//!
//! It cannot work under `wsl`.  There the shell is a Linux process living in
//! the WSL VM; the only Win32 processes in the pane are `wsl.exe` and
//! `wslhost.exe`, and their PEB working directory is fixed at the moment they
//! were created and never moves again (measured: `cd /mnt/c/Users` inside the
//! distro leaves all of `pwsh`, `wsl`, `wsl`, `wslhost` reporting `C:\`).  The
//! Linux cwd is simply not observable from the Win32 side.
//!
//! The portable answer, and the one every Windows terminal uses, is shell
//! integration: the shell states its own directory with OSC 7
//! (`ESC ] 7 ; file://host/path ESC \`) or with ConEmu's OSC 9;9.  This module
//! turns such an announcement into something Windows can actually open, so
//! `split-window -c "#{pane_current_path}"` lands in the right place:
//!
//! ```text
//!   file://SuperFlow/mnt/c/Users  -> C:\Users
//!   /mnt/d/src                    -> D:\src
//!   file:///C:/Program%20Files    -> C:\Program Files
//!   /home/gj                      -> \\wsl.localhost\Ubuntu\home\gj
//! ```

/// Translate a cwd announced via OSC 7 or OSC 9;9 into a native Windows path.
///
/// `distro` is the WSL distribution name used to build the UNC view of a
/// Linux-only path; pass `None` (or leave it unknown) and such a path is
/// rejected rather than guessed at.
///
/// Returns `None` when the value cannot be expressed as a Windows path, in
/// which case the caller should keep whatever it had before.  Never returns an
/// empty string.
pub fn osc_cwd_to_windows(raw: &str, distro: Option<&str>) -> Option<String> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }

    // A `file:` URL carries an authority we must drop and percent escapes we
    // must undo.  A bare path is taken literally: `%` is a legal character in
    // a Windows filename, so decoding one that never went through a URL
    // encoder would corrupt it.
    let (path, was_url) = match strip_file_url(trimmed) {
        Some(p) => (percent_decode(&p), true),
        None => (trimmed.to_string(), false),
    };
    if path.is_empty() {
        return None;
    }

    // `file:///C:/Users` leaves a leading slash in front of the drive letter.
    let path = match drive_after_slash(&path) {
        Some(rest) => rest.to_string(),
        None => path,
    };

    // Already a Windows path: a drive letter, or a UNC share (including the
    // \\wsl.localhost\<distro> view of the distro's own filesystem).
    if is_drive_path(&path) {
        return Some(normalize_windows(&path));
    }
    if path.starts_with("\\\\") || (was_url && path.starts_with("//")) {
        return Some(normalize_windows(&path));
    }

    // WSL and Cygwin drive mounts map straight onto a drive letter.
    for prefix in ["/mnt/", "/cygdrive/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            if let Some(win) = mounted_drive(rest) {
                return Some(win);
            }
        }
    }

    // Any other absolute POSIX path exists only inside the distro.  Windows can
    // reach it through the \\wsl.localhost share, and nowhere else.
    if path.starts_with('/') {
        let distro = distro?;
        if distro.is_empty() {
            return None;
        }
        let body = path.trim_end_matches('/').replace('/', "\\");
        return Some(format!("\\\\wsl.localhost\\{}{}", distro, body));
    }

    None
}

/// `file://host/path` -> `/path`.  Also accepts the `file:///path` form (empty
/// authority) and a bare `file:` with no slashes.
fn strip_file_url(raw: &str) -> Option<String> {
    let rest = raw
        .strip_prefix("file://")
        .or_else(|| raw.strip_prefix("FILE://"))?;
    // Everything up to the next '/' is the authority (hostname), which says
    // which machine the path belongs to, not where it is.
    Some(match rest.find('/') {
        Some(slash) => rest[slash..].to_string(),
        None => rest.to_string(),
    })
}

/// `/C:/Users` -> `C:/Users`.  Windows `file:` URLs put the drive letter in the
/// path component, so the URL always has one slash too many.
fn drive_after_slash(path: &str) -> Option<&str> {
    let rest = path.strip_prefix('/')?;
    if is_drive_path(rest) { Some(rest) } else { None }
}

/// Does `p` start with a `X:` drive specification?
fn is_drive_path(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 2
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b.len() == 2 || b[2] == b'/' || b[2] == b'\\')
}

/// `c/Users` (the tail of `/mnt/c/Users`) -> `C:\Users`.
///
/// The mount segment must be a single letter: `/mnt/wsl/...` and
/// `/mnt/wslg/...` are real Linux directories, not drives, and must fall
/// through to the UNC translation.
fn mounted_drive(rest: &str) -> Option<String> {
    let mut parts = rest.splitn(2, '/');
    let letter = parts.next()?;
    let mut chars = letter.chars();
    let c = chars.next()?;
    if chars.next().is_some() || !c.is_ascii_alphabetic() {
        return None;
    }
    let tail = parts.next().unwrap_or("");
    let drive = c.to_ascii_uppercase();
    if tail.is_empty() {
        return Some(format!("{drive}:\\"));
    }
    Some(format!("{}:\\{}", drive, tail.trim_end_matches('/').replace('/', "\\")))
}

/// Forward slashes to backslashes, and drop a trailing separator except on a
/// bare drive root (`C:\` must keep it or it means "the cwd on C:").
fn normalize_windows(p: &str) -> String {
    let s = p.replace('/', "\\");
    if s.len() > 3 && s.ends_with('\\') && !s.ends_with(":\\") {
        s.trim_end_matches('\\').to_string()
    } else {
        s
    }
}

/// Undo `%XX` escapes.  Invalid escapes are passed through untouched, which is
/// what a terminal should do with a payload it cannot make sense of.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hi = (b[i + 1] as char).to_digit(16);
            let lo = (b[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// Default WSL distribution
// ---------------------------------------------------------------------------

/// Name of the default WSL distribution, or `None` when WSL is not installed.
///
/// Read once from `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss` and
/// cached for the life of the server: `#{pane_current_path}` is evaluated on
/// every status-line repaint and must not touch the registry, let alone spawn
/// `wsl.exe`, on that path.
pub fn default_distro() -> Option<&'static str> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHE.get_or_init(read_default_distro).as_deref()
}

#[cfg(windows)]
fn read_default_distro() -> Option<String> {
    const HKEY_CURRENT_USER: isize = -2147483647; // 0x80000001
    const KEY_READ: u32 = 0x2_0019;
    const ERROR_SUCCESS: i32 = 0;
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(key: isize, sub: *const u16, opts: u32, sam: u32, out: *mut isize) -> i32;
        fn RegCloseKey(key: isize) -> i32;
        fn RegGetValueW(
            key: isize,
            sub: *const u16,
            value: *const u16,
            flags: u32,
            ty: *mut u32,
            data: *mut u16,
            cb: *mut u32,
        ) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn get_sz(key: isize, sub: Option<&str>, value: &str) -> Option<String> {
        let sub_w = sub.map(wide);
        let val_w = wide(value);
        let mut buf = [0u16; 512];
        let mut cb = (buf.len() * 2) as u32;
        let rc = RegGetValueW(
            key,
            sub_w.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            val_w.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            &mut cb,
        );
        if rc != ERROR_SUCCESS {
            return None;
        }
        let chars = (cb as usize / 2).min(buf.len());
        let s: String = String::from_utf16_lossy(&buf[..chars]);
        let s = s.trim_end_matches('\0').to_string();
        if s.is_empty() { None } else { Some(s) }
    }

    unsafe {
        let mut lxss: isize = 0;
        let sub = wide(r"Software\Microsoft\Windows\CurrentVersion\Lxss");
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_READ, &mut lxss) != ERROR_SUCCESS {
            return None;
        }
        let guid = get_sz(lxss, None, "DefaultDistribution");
        let name = guid.and_then(|g| get_sz(lxss, Some(&g), "DistributionName"));
        RegCloseKey(lxss);
        name
    }
}

#[cfg(not(windows))]
fn read_default_distro() -> Option<String> {
    None
}

#[cfg(test)]
#[path = "../tests-rs/test_issue615_wsl_pane_path.rs"]
mod issue615_wsl_pane_path_tests;
