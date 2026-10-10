// Issue #773 - a pane's base environment is the server's inherited process
// environment; the registry environment only fills keys the process lacks.
//
// Before the fix get_base_env() laid HKLM Session Manager\Environment and
// HKCU\Environment OVER std::env::vars_os(), so the launching shell's PATH,
// TEMP, GOPATH, OS (anything the registry also defines) were replaced by the
// registry values in every pane. tmux passes the server's start environment
// through unchanged.

#![cfg(windows)]

use super::*;

fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<OsString, EnvEntry> {
    pairs
        .iter()
        .map(|(k, v)| {
            (
                EnvEntry::map_key((*k).into()),
                EnvEntry {
                    is_from_base_env: true,
                    preferred_key: (*k).into(),
                    value: (*v).into(),
                },
            )
        })
        .collect()
}

fn reg(pairs: &[(&str, &str)]) -> Vec<(String, OsString)> {
    pairs.iter().map(|(k, v)| (k.to_string(), (*v).into())).collect()
}

fn get(env: &BTreeMap<OsString, EnvEntry>, k: &str) -> Option<String> {
    env.get(&EnvEntry::map_key(k.into()))
        .map(|e| e.value.to_string_lossy().into_owned())
}

#[test]
fn inherited_values_win_over_registry() {
    let mut env = env_of(&[
        ("Path", r"C:\path-marker;C:\Windows"),
        ("TEMP", r"C:\temp-marker"),
        ("OS", "os-marker"),
        ("GOPATH", r"C:\go-marker"),
    ]);
    fill_from_registry(
        &mut env,
        reg(&[("Path", r"C:\Windows;C:\Windows\System32"), ("OS", "Windows_NT"), ("TEMP", r"C:\Windows\TEMP")]),
        reg(&[("Path", r"C:\Users\u\bin"), ("TEMP", r"C:\Users\u\Temp"), ("GOPATH", r"C:\Users\u\go")]),
    );
    assert_eq!(get(&env, "PATH").as_deref(), Some(r"C:\path-marker;C:\Windows"));
    assert_eq!(get(&env, "TEMP").as_deref(), Some(r"C:\temp-marker"));
    assert_eq!(get(&env, "OS").as_deref(), Some("os-marker"));
    assert_eq!(get(&env, "GOPATH").as_deref(), Some(r"C:\go-marker"));
}

#[test]
fn missing_keys_are_filled_and_path_merges_machine_then_user() {
    let mut env = env_of(&[("BH_MARK", "inherited")]);
    fill_from_registry(
        &mut env,
        reg(&[("Path", r"C:\Windows"), ("OS", "Windows_NT"), ("USERNAME", "SYSTEM")]),
        reg(&[("PATH", r"C:\Users\u\bin"), ("TEMP", r"C:\Users\u\Temp")]),
    );
    assert_eq!(get(&env, "BH_MARK").as_deref(), Some("inherited"));
    assert_eq!(get(&env, "PATH").as_deref(), Some(r"C:\Windows;C:\Users\u\bin"));
    assert_eq!(get(&env, "OS").as_deref(), Some("Windows_NT"));
    assert_eq!(get(&env, "TEMP").as_deref(), Some(r"C:\Users\u\Temp"));
    // The machine key's USERNAME is SYSTEM; it is never used.
    assert_eq!(get(&env, "USERNAME"), None);
}

#[test]
fn key_match_is_case_insensitive() {
    let mut env = env_of(&[("temp", r"C:\temp-marker")]);
    fill_from_registry(&mut env, reg(&[("TEMP", r"C:\Windows\TEMP")]), reg(&[("Temp", r"C:\u")]));
    assert_eq!(get(&env, "TEMP").as_deref(), Some(r"C:\temp-marker"));
    assert_eq!(env.len(), 1);
}

#[test]
fn user_value_overrides_machine_value_only_when_both_filled() {
    let mut env = env_of(&[]);
    fill_from_registry(&mut env, reg(&[("TMP", r"C:\Windows\TEMP")]), reg(&[("TMP", r"C:\Users\u\Temp")]));
    assert_eq!(get(&env, "TMP").as_deref(), Some(r"C:\Users\u\Temp"));
}

/// End to end on the real registry: every variable this process has reaches
/// the base environment with exactly the value it has here. Before the fix
/// the test process's PATH (cargo prepends its own directories) and any
/// other registry-defined variable came back as the registry value.
#[test]
fn base_env_preserves_every_inherited_variable() {
    let base = get_base_env();
    for (k, v) in std::env::vars_os() {
        let entry = base
            .get(&EnvEntry::map_key(k.clone()))
            .unwrap_or_else(|| panic!("{:?} missing from base env", k));
        assert_eq!(entry.value, v, "{:?} was replaced in the base env", k);
    }
}
