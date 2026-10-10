// Issue #775: show-environment ignored NAME and -g, never said
// `unknown variable: NAME`, printed `set-environment -r` as `NAME=`, and never
// listed the server's start environment. tmux semantics:
// cmd-show-environment.c, cmd-set-environment.c, environ.c.

use super::*;
use std::collections::HashMap;

fn set(env: &mut HashMap<String, String>, sc: &mut EnvScopes, args: &[&str]) {
    let a = parse_set_environment(args).expect("valid set-environment");
    apply_set(env, sc, &a);
}

fn show_args(env: &HashMap<String, String>, sc: &EnvScopes, args: &[&str]) -> Result<String, String> {
    show(env, sc, &parse_show_environment(args).expect("valid show-environment"))
}

/// Unique variable names so parallel tests never collide on the process env.
fn n(tag: &str) -> String {
    format!("PSMUX_T775_{}_{}", tag, std::process::id())
}

#[test]
fn set_environment_parser_matches_tmux_errors() {
    assert_eq!(parse_set_environment(&[]).unwrap_err(), "too few arguments (need at least 1)");
    assert_eq!(parse_set_environment(&["A", "b", "c"]).unwrap_err(), "too many arguments (need at most 2)");
    assert_eq!(parse_set_environment(&["A"]).unwrap_err(), "no value specified");
    assert_eq!(parse_set_environment(&["-u", "A", "x"]).unwrap_err(), "can't specify a value with -u");
    assert_eq!(parse_set_environment(&["-r", "A", "x"]).unwrap_err(), "can't specify a value with -r");
    assert_eq!(parse_set_environment(&["A=B", "x"]).unwrap_err(), "variable name contains =");
    assert_eq!(parse_set_environment(&["", "x"]).unwrap_err(), "empty variable name");
    assert_eq!(parse_set_environment(&["-q", "A", "x"]).unwrap_err(), "unknown flag -q");

    let a = parse_set_environment(&["-gr", "A"]).unwrap();
    assert!(a.global && a.remove && a.value.is_none());
    let a = parse_set_environment(&["-t", "sess", "-Fh", "A", "#{session_name}"]).unwrap();
    assert!(a.format && a.hidden && !a.global);
    assert_eq!(a.value.as_deref(), Some("#{session_name}"));
    // -t after the name (psmux CLI keeps it where the user typed it).
    let a = parse_set_environment(&["-u", "A", "-t", "sess"]).unwrap();
    assert!(a.unset && a.name == "A" && a.value.is_none());
    // A dash value after the name is a value, not a flag.
    let a = parse_set_environment(&["A", "-x"]).unwrap();
    assert_eq!(a.value.as_deref(), Some("-x"));
}

#[test]
fn show_environment_parser() {
    let a = parse_show_environment(&["-gt", "s", "NAME"]).unwrap();
    assert!(a.global);
    assert_eq!(a.name.as_deref(), Some("NAME"));
    assert_eq!(parse_show_environment(&["A", "B"]).unwrap_err(), "too many arguments (need at most 1)");
    assert!(parse_show_environment(&["-hs"]).unwrap().shell);
}

#[test]
fn named_query_removal_and_unknown_variable() {
    let _g = crate::util::lock_test_env();
    let (ses, rm, nope) = (n("SES"), n("RM"), n("NOPE"));
    let mut env = HashMap::new();
    let mut sc = EnvScopes::default();
    set(&mut env, &mut sc, &[&ses, "session-val"]);
    set(&mut env, &mut sc, &["-r", &rm]);

    assert_eq!(show_args(&env, &sc, &[&ses]).unwrap(), format!("{}=session-val\n", ses));
    assert_eq!(show_args(&env, &sc, &[&rm]).unwrap(), format!("-{}\n", rm));
    assert_eq!(show_args(&env, &sc, &[&nope]).unwrap_err(), format!("unknown variable: {}", nope));
    // A session only variable is not in the global environment.
    assert_eq!(show_args(&env, &sc, &["-g", &ses]).unwrap_err(), format!("unknown variable: {}", ses));
    // The whole session listing: sorted, removal as -NAME, nothing global.
    let listing = show_args(&env, &sc, &[]).unwrap();
    assert!(listing.contains(&format!("{}=session-val\n", ses)));
    assert!(listing.contains(&format!("-{}\n", rm)));
    assert!(!listing.contains("PATH="), "session listing must not carry the global env: {listing}");

    // Children see the merged view: the session var set, the removal absent.
    assert_eq!(std::env::var(&ses).as_deref(), Ok("session-val"));
    assert!(std::env::var(&rm).is_err());

    set(&mut env, &mut sc, &["-u", &ses]);
    set(&mut env, &mut sc, &["-u", &rm]);
    assert!(std::env::var(&ses).is_err());
    assert!(sc.global_under.is_empty());
}

#[test]
fn global_lists_inherited_start_environment() {
    let _g = crate::util::lock_test_env();
    let inh = n("INH");
    std::env::set_var(&inh, "inherited-val");
    let env = HashMap::new();
    let sc = EnvScopes::default();
    assert_eq!(show_args(&env, &sc, &["-g", &inh]).unwrap(), format!("{}=inherited-val\n", inh));
    let all = show_args(&env, &sc, &["-g"]).unwrap();
    assert!(all.lines().any(|l| l.to_ascii_uppercase().starts_with("PATH=")), "-g lists PATH");
    assert!(all.contains(&format!("{}=inherited-val\n", inh)));
    std::env::remove_var(&inh);
}

#[test]
fn session_entry_shadows_global_and_restores_it() {
    let _g = crate::util::lock_test_env();
    let v = n("SHADOW");
    std::env::set_var(&v, "global-start");
    let mut env = HashMap::new();
    let mut sc = EnvScopes::default();

    set(&mut env, &mut sc, &[&v, "session"]);
    assert_eq!(std::env::var(&v).as_deref(), Ok("session"), "child gets the session value");
    assert_eq!(show_args(&env, &sc, &["-g", &v]).unwrap(), format!("{}=global-start\n", v));

    // A global change under a session entry is remembered, not pushed.
    set(&mut env, &mut sc, &["-g", &v, "global-new"]);
    assert_eq!(std::env::var(&v).as_deref(), Ok("session"));
    assert_eq!(show_args(&env, &sc, &["-g", &v]).unwrap(), format!("{}=global-new\n", v));

    // Session -r hides the global value from children.
    set(&mut env, &mut sc, &["-r", &v]);
    assert!(std::env::var(&v).is_err());
    assert_eq!(show_args(&env, &sc, &[&v]).unwrap(), format!("-{}\n", v));

    // Dropping the session entry falls back to the global value.
    set(&mut env, &mut sc, &["-u", &v]);
    assert_eq!(std::env::var(&v).as_deref(), Ok("global-new"));
    assert_eq!(show_args(&env, &sc, &[&v]).unwrap_err(), format!("unknown variable: {}", v));
    std::env::remove_var(&v);
}

#[test]
fn global_remove_unset_and_hidden() {
    let _g = crate::util::lock_test_env();
    let (r, h) = (n("GRM"), n("GHID"));
    std::env::set_var(&r, "start");
    let mut env = HashMap::new();
    let mut sc = EnvScopes::default();

    set(&mut env, &mut sc, &["-g", "-r", &r]);
    assert!(std::env::var(&r).is_err(), "a global removal never reaches a child");
    assert_eq!(show_args(&env, &sc, &["-g", &r]).unwrap(), format!("-{}\n", r));
    set(&mut env, &mut sc, &["-gu", &r]);
    assert_eq!(show_args(&env, &sc, &["-g", &r]).unwrap_err(), format!("unknown variable: {}", r));

    set(&mut env, &mut sc, &["-gh", &h, "secret"]);
    assert!(std::env::var(&h).is_err(), "hidden is not pushed to children");
    // Hidden entries print only with -h (and nothing, rc 0, without it).
    assert_eq!(show_args(&env, &sc, &["-g", &h]).unwrap(), "");
    assert_eq!(show_args(&env, &sc, &["-gh", &h]).unwrap(), format!("{}=secret\n", h));
    assert_eq!(sc.hidden_value(&h).as_deref(), Some("secret"));
    assert!(!show_args(&env, &sc, &["-g"]).unwrap().contains(&h));
}

#[test]
fn shell_format_escapes_like_tmux() {
    let _g = crate::util::lock_test_env();
    let (a, b) = (n("SH"), n("SHRM"));
    let mut env = HashMap::new();
    let mut sc = EnvScopes::default();
    set(&mut env, &mut sc, &[&a, "x$y\"z`w\\v"]);
    set(&mut env, &mut sc, &["-r", &b]);
    assert_eq!(
        show_args(&env, &sc, &["-s", &a]).unwrap(),
        format!("{a}=\"x\\$y\\\"z\\`w\\\\v\"; export {a};\n")
    );
    assert_eq!(show_args(&env, &sc, &["-s", &b]).unwrap(), format!("unset {};\n", b));
    set(&mut env, &mut sc, &["-u", &a]);
    set(&mut env, &mut sc, &["-u", &b]);
}

// update-environment (tmux environ_update, environ.c:186): the client's
// value for every matching pattern, `-PATTERN` when nothing matches.
#[test]
fn update_environment_copies_client_values_and_marks_missing() {
    let _g = crate::util::lock_test_env();
    let (a, miss) = (n("UE"), n("UEMISS"));
    let glob = format!("PSMUX_T775_UEG_*_{}", std::process::id());
    let globbed = format!("PSMUX_T775_UEG_x_{}", std::process::id());
    let mut env = HashMap::new();
    let mut sc = EnvScopes::default();
    let patterns = vec![a.clone(), miss.clone(), glob.clone()];

    let src = vec![(a.clone(), "two".to_string()), (globbed.clone(), "g".to_string())];
    let written = update_environment(&mut env, &mut sc, &patterns, &src, false);
    assert_eq!(written.len(), 3);
    assert_eq!(show_args(&env, &sc, &[&a]).unwrap(), format!("{}=two\n", a));
    assert_eq!(show_args(&env, &sc, &[&miss]).unwrap(), format!("-{}\n", miss));
    assert_eq!(show_args(&env, &sc, &[&globbed]).unwrap(), format!("{}=g\n", globbed));
    assert_eq!(std::env::var(&a).as_deref(), Ok("two"), "children get the client's value");

    // A later attach from a client without A records the removal.
    update_environment(&mut env, &mut sc, &patterns, &[], false);
    assert_eq!(show_args(&env, &sc, &[&a]).unwrap(), format!("-{}\n", a));
    assert!(std::env::var(&a).is_err());
    assert_eq!(show_args(&env, &sc, &[&glob]).unwrap(), format!("-{}\n", glob));

    for name in [&a, &miss, &globbed, &glob] {
        apply_set(&mut env, &mut sc, &parse_set_environment(&["-u", name]).unwrap());
    }
}

#[test]
fn new_session_seed_keeps_explicit_entries_and_is_replaced_on_claim() {
    let _g = crate::util::lock_test_env();
    let (a, b) = (n("SEEDA"), n("SEEDB"));
    std::env::set_var(&a, "start-a");
    std::env::remove_var(&b);
    let mut env = HashMap::new();
    let mut sc = EnvScopes::default();
    // `new-session -e B=explicit` wins over the seed.
    env.insert(b.clone(), "explicit".to_string());
    let patterns = vec![a.clone(), b.clone()];
    seed_session_from_start_env(&mut env, &mut sc, &patterns);
    assert_eq!(show_args(&env, &sc, &[&a]).unwrap(), format!("{}=start-a\n", a));
    assert_eq!(show_args(&env, &sc, &[&b]).unwrap(), format!("{}=explicit\n", b));
    assert!(sc.seeded.contains(&a) && !sc.seeded.contains(&b));

    // A warm claim adopts another environment and re-seeds.
    std::env::remove_var(&a);
    sc.global_under.remove(&a);
    seed_session_from_start_env(&mut env, &mut sc, &patterns);
    assert_eq!(show_args(&env, &sc, &[&a]).unwrap(), format!("-{}\n", a));
    for name in [&a, &b] {
        apply_set(&mut env, &mut sc, &parse_set_environment(&["-u", name]).unwrap());
    }
}
