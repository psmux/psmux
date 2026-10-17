//! Build script for psmux.
//!
//! Captures the exact git commit the binary was built from and exposes it to
//! the crate as compile-time environment variables:
//!
//! ```text
//! PSMUX_GIT_HASH       short commit hash (e.g. `f179849`), or `unknown`
//! PSMUX_GIT_HASH_FULL  full 40-char commit hash, or `unknown`
//! PSMUX_GIT_DIRTY      `true` if the working tree had uncommitted
//!                      changes at build time, otherwise `false`
//! PSMUX_GIT_DATE       commit date (YYYY-MM-DD), or `unknown`
//! ```
//!
//! Git may be unavailable (for example when the crate is built from a
//! crates.io tarball rather than a git checkout). In that case every value
//! falls back gracefully to `unknown` / `false` so the build always succeeds.
//!
//! On Windows the script additionally embeds a Win32 resource block into the
//! executable: the application icon plus a VS_VERSIONINFO record. Without it
//! Windows has no name, description or version to show for the process, so
//! Task Manager, the taskbar, alt-tab and consent dialogs such as the one
//! 1Password raises all display a nameless generic entry (issue #620).

use std::process::Command;

fn main() {
    watch_git_inputs();

    embed_windows_resources();

    println!("cargo:rerun-if-env-changed=PSMUX_GIT_SHA");

    // `git` is the best source: it knows the short hash, the full hash, the
    // commit date and whether the tree is dirty. It is also the one source
    // that is regularly absent. `cargo install --git ...` uses libgit2, not
    // the git binary, so a machine with no git on PATH installs happily and
    // then reports `unknown commit` (#647 WIN-05); a container that refuses
    // the checkout for dubious ownership lands in the same place. Fall back
    // through every other place the revision is recorded before giving up.
    let git_short = git(&["rev-parse", "--short", "HEAD"]);
    let git_full = git(&["rev-parse", "HEAD"]);
    let fallback = if git_short.is_none() { fallback_commit() } else { None };

    let full = git_full
        .or_else(|| fallback.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let short = git_short
        .or_else(|| fallback.map(|sha| shorten(&sha)))
        .unwrap_or_else(|| "unknown".to_string());
    // Only git knows the commit date; a bare sha from a fallback carries none.
    let date = git(&["show", "-s", "--format=%cd", "--date=short", "HEAD"])
        .unwrap_or_else(|| "unknown".to_string());

    // "Dirty" means tracked, version-controlled source differs from HEAD, i.e.
    // the binary can no longer be reproduced from the named commit alone.
    // `--untracked-files=no` deliberately ignores untracked files (build tool
    // state, editor scratch, etc.) so they never produce a false "dirty" flag.
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    println!("cargo:rustc-env=PSMUX_GIT_HASH={short}");
    println!("cargo:rustc-env=PSMUX_GIT_HASH_FULL={full}");
    println!("cargo:rustc-env=PSMUX_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=PSMUX_GIT_DATE={date}");
}

/// Cargo stops scanning package inputs once a build script emits an explicit
/// rerun hint. Watch tracked files too: changing src/main.rs rebuilds the crate,
/// but would otherwise reuse the previous build script's clean/dirty value.
/// Resolve Git's metadata paths instead of assuming .git is a directory; in a
/// linked worktree HEAD/index are private and refs/packed-refs are shared.
fn watch_git_inputs() {
    if let Some(files) = git(&["ls-files", "-z"]) {
        for path in files.split('\0').filter(|path| !path.is_empty()) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    for name in ["HEAD", "index", "refs", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name]) {
            // A missing optional file would make Cargo rerun on every build.
            // Packing refs also removes a watched loose ref; unpacking one
            // changes the watched refs directory, so both transitions refresh.
            if std::path::Path::new(&path).exists() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
}

/// Embed the icon and the VS_VERSIONINFO block (Windows hosts only).
///
/// The `#[cfg(windows)]` here is the *host* platform, which is what gates the
/// `winresource` build-dependency in `Cargo.toml`. The target is checked
/// separately below so a Windows host cross-compiling to a non-Windows target
/// still produces a clean build.
#[cfg(windows)]
fn embed_windows_resources() {
    use std::path::Path;
    use winresource::{VersionInfo, WindowsResource};

    // Only PE targets carry Win32 resources. A Windows host building for,
    // say, a Linux target must skip this entirely.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let icon = Path::new("assets").join("psmux.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    let version = env!("CARGO_PKG_VERSION");

    // A build that quietly dropped the icon would reintroduce #620, so treat a
    // missing asset as a build failure rather than carrying on without it.
    assert!(
        icon.is_file(),
        "{} is missing; it is required to embed the application icon",
        icon.display()
    );

    let mut res = WindowsResource::new();
    res.set_icon(icon.to_str().expect("assets/psmux.ico path is not UTF-8"));

    res.set("ProductName", "psmux")
        .set("FileDescription", "psmux terminal multiplexer")
        .set("CompanyName", "psmux")
        .set("InternalName", "psmux")
        // All three binaries (psmux, pmux, tmux) are the same image built from
        // src/main.rs; psmux.exe is the canonical name.
        .set("OriginalFilename", "psmux.exe")
        .set("LegalCopyright", "Copyright (c) 2025 Godwin Sam Josh. MIT licensed.")
        .set("FileVersion", version)
        .set("ProductVersion", version);

    // VS_FIXEDFILEINFO wants a packed u64: major.minor.patch.build, 16 bits
    // each. Cargo versions are three components, so the build field stays 0.
    let packed = packed_version(version);
    res.set_version_info(VersionInfo::FILEVERSION, packed);
    res.set_version_info(VersionInfo::PRODUCTVERSION, packed);

    for var in ["CI", "GITHUB_ACTIONS", "PSMUX_REQUIRE_RESOURCES", "RC_PATH", "WindowsSdkVerBinPath"] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let mut result = res.compile();

    // winresource finds rc.exe through RC_PATH, a Developer Prompt's
    // WindowsSdkVerBinPath, or the KitsRoot values in the registry. When none
    // of those leads to an rc.exe it falls back to the relative path
    // `bin\x64\rc.exe`, which fails with "path not found" (os error 3, #841).
    // That happens when the SDK registration is missing or the SDK was
    // installed without its tools. Look in the other usual places before
    // giving up.
    if result.is_err()
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
        && std::env::var_os("RC_PATH").is_none()
    {
        if let Some(dir) = find_rc_dir() {
            println!("Retrying with rc.exe from '{}'", dir.display());
            res.set_toolkit_path(dir.to_str().expect("rc.exe directory is not UTF-8"));
            result = res.compile();
        }
    }

    if let Err(e) = result {
        // A release that quietly dropped the resource block would reintroduce
        // #620 (a nameless process in Task Manager and consent dialogs), so
        // CI and release builds fail loudly. A contributor building locally
        // without the Windows SDK tools still gets a working binary, only
        // without the icon and version info.
        if resources_required() {
            panic!(
                "failed to embed the Windows resource block (icon / version info): {e}\n\
                 If rc.exe could not be found, install the \"Windows 11 SDK\" component of the \
                 Visual Studio Build Tools, or point RC_PATH at rc.exe."
            );
        }
        println!(
            "cargo:warning=building without the Windows icon and version info: \
             the resource compiler failed ({e}). Install the \"Windows 11 SDK\" \
             component of the Visual Studio Build Tools, or set RC_PATH to rc.exe, \
             then run `cargo clean -p psmux` and rebuild to embed them."
        );
    }
}

/// True when a missing resource block must fail the build: on CI, and
/// whenever PSMUX_REQUIRE_RESOURCES=1 asks for it (release builds).
#[cfg(windows)]
fn resources_required() -> bool {
    let set = |name: &str| {
        std::env::var(name)
            .map(|v| {
                let v = v.trim();
                !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
            })
            .unwrap_or(false)
    };
    set("CI") || set("GITHUB_ACTIONS") || set("PSMUX_REQUIRE_RESOURCES")
}

/// Find a directory holding rc.exe outside winresource's own discovery:
/// first `PATH`, then the default Windows 10/11 SDK location, newest version
/// folder that actually contains rc.exe for this host.
#[cfg(windows)]
fn find_rc_dir() -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};

    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.join("rc.exe").is_file() {
                return Some(dir);
            }
        }
    }

    let arches: &[&str] = if cfg!(target_arch = "aarch64") {
        &["arm64", "x64", "x86"]
    } else if cfg!(target_arch = "x86_64") {
        &["x64", "x86"]
    } else {
        &["x86"]
    };
    let in_arch = |dir: &Path| {
        arches
            .iter()
            .map(|arch| dir.join(arch))
            .find(|d| d.join("rc.exe").is_file())
    };

    let root = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
    let bin = root.join("Windows Kits").join("10").join("bin");
    let mut versions: Vec<(Vec<u32>, PathBuf)> = std::fs::read_dir(&bin)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let key = name
                .split('.')
                .map(|p| p.parse::<u32>().ok())
                .collect::<Option<Vec<_>>>()?;
            Some((key, e.path()))
        })
        .collect();
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    versions
        .iter()
        .find_map(|(_, dir)| in_arch(dir))
        .or_else(|| in_arch(&bin))
}

#[cfg(not(windows))]
fn embed_windows_resources() {}

/// Pack `major.minor.patch` into the u64 layout VS_FIXEDFILEINFO expects.
/// Non-numeric or missing components fall back to 0.
#[cfg(windows)]
fn packed_version(version: &str) -> u64 {
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|p| p.parse::<u64>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);
    (major << 48) | (minor << 32) | (patch << 16)
}

/// Find the commit this source tree came from without asking git.
///
/// Tried in order:
///
///   1. `.cargo_vcs_info.json`, which `cargo package` writes into every
///      crates.io tarball as `{"git": {"sha1": "..."}}`.
///   2. The cargo git checkout directory name. `cargo install --git URL` checks
///      the revision out into
///      `~/.cargo/git/checkouts/<name>-<hash>/<short sha>/`, so the package
///      root's own directory name IS the short hash. The marker file
///      `.cargo-ok` that cargo drops beside the sources confirms this is such
///      a checkout and not a directory that merely happens to be named in hex.
///   3. `PSMUX_GIT_SHA`, so release tooling that builds from an exported tree
///      can inject the revision it published.
fn fallback_commit() -> Option<String> {
    let root = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let root = std::path::Path::new(&root);

    if let Some(sha) = vcs_info_sha(&root.join(".cargo_vcs_info.json")) {
        return Some(sha);
    }
    if root.join(".cargo-ok").is_file() {
        if let Some(name) = root.file_name().and_then(|n| n.to_str()) {
            if is_hex_sha(name) {
                return Some(name.to_string());
            }
        }
    }
    std::env::var("PSMUX_GIT_SHA")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| is_hex_sha(s))
}

/// Pull `git.sha1` out of a `.cargo_vcs_info.json` without a JSON dependency.
/// The file cargo writes is small and machine generated, so finding the key and
/// reading the quoted hex value after it is enough, and a malformed file simply
/// yields `None` rather than failing the build.
fn vcs_info_sha(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let after = text.split("\"sha1\"").nth(1)?;
    let after = after.split_once(':')?.1;
    let start = after.find('"')? + 1;
    let rest = &after[start..];
    let end = rest.find('"')?;
    let sha = &rest[..end];
    if is_hex_sha(sha) { Some(sha.to_string()) } else { None }
}

fn is_hex_sha(s: &str) -> bool {
    s.len() >= 7 && s.len() <= 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Cargo names its checkout directories with a 7 character short hash, and a
/// tarball records the full 40. Report both the same way git would.
fn shorten(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// Run `git <args>` and return trimmed stdout on success, or `None` if git is
/// missing or the command failed.
fn git(args: &[&str]) -> Option<String> {
    // `status` normally refreshes the index as a side effect. Since the index
    // is a watched input, that would cause an otherwise idle build to rerun
    // forever. Build metadata queries must leave Git's files unchanged.
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
