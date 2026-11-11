// The `servers` ownership markers must not outlive their servers. A server
// that exited while something still held a handle to it (a zombie) still
// answers with its original creation time, and a torn marker (all zeros after
// a crash mid write) parses to nothing: neither was ever pruned, and one
// machine collected 92 dead markers in four days.
use super::*;
use std::collections::HashMap;
use std::time::Duration;

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("psmux_marker_{}_{}_{}", tag, std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(dir.join("servers")).unwrap();
    dir
}

fn write_marker(dir: &Path, name: &str, body: &[u8]) {
    std::fs::write(dir.join("servers").join(name), body).unwrap();
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir.join("servers")).unwrap()
        .flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn a_zombie_server_marker_is_pruned_and_a_live_one_kept() {
    let dir = scratch("zombie");
    write_marker(&dir, "100", b"100:5000");
    write_marker(&dir, "200", b"200:7000");
    let owned = read_owned_server_pids(&dir);
    assert_eq!(owned.len(), 2);
    // 100 has exited but its object is still open: creation time still answers.
    let running = |pid: u32| pid == 200;
    let creation = |pid: u32| match pid { 100 => Some(5000), 200 => Some(7000), _ => None };
    prune_stale_server_markers_with(&dir, &owned, running, creation, Duration::from_secs(60));
    assert_eq!(names(&dir), vec!["200".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_torn_marker_is_pruned_once_old_enough_but_not_while_fresh() {
    let dir = scratch("torn");
    write_marker(&dir, "300", &[0u8; 24]);
    let owned = read_owned_server_pids(&dir);
    assert!(owned.is_empty(), "a torn marker names no process");
    // Fresh: may be a write in progress, keep it.
    prune_stale_server_markers_with(&dir, &owned, |_| true, |_| None, Duration::from_secs(60));
    assert_eq!(names(&dir), vec!["300".to_string()]);
    // Old enough: torn for good, delete it.
    prune_stale_server_markers_with(&dir, &owned, |_| true, |_| None, Duration::ZERO);
    assert!(names(&dir).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reused_pid_is_pruned_and_an_unverifiable_live_one_kept() {
    let dir = scratch("reuse");
    write_marker(&dir, "400", b"400:1111");
    write_marker(&dir, "500", b"500");
    let owned = read_owned_server_pids(&dir);
    prune_stale_server_markers_with(&dir, &owned, |_| true,
        |pid| match pid { 400 => Some(2222), 500 => Some(9), _ => None }, Duration::from_secs(60));
    assert_eq!(names(&dir), vec!["500".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}
