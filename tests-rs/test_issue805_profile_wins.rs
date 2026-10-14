// Issue #805: the pwsh init block dot sources the user's profile and then
// installs `function Global:env` and `function Global:claude`, so a `claude`
// or `env` the user defined in their profile was replaced in every pane (and
// the claude wrapper cannot chain to a user function, its Get-Command filter
// skips functions on purpose, #475). Each psmux definition must now be
// skipped when the name is already a function, filter or alias.
//
// Also: CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS was forced to 1 in every pane,
// outside the claude-code-fix-tty gate and over a value the user had set.

use super::{build_psrl_init, build_psrl_init_noprofile, set_tmux_env, ENV_SHIM_PS, PROFILE_SOURCE};
use portable_pty::CommandBuilder;

const ENV_GUARD: &str = "if(-not((Test-Path Function:env) -or (Test-Path Alias:env))){ function Global:env { ";
const CLAUDE_GUARD: &str = "-not((Test-Path Function:claude) -or (Test-Path Alias:claude))){ ";

#[test]
fn init_string_guards_env_and_claude_after_profile() {
    for allow_predictions in [false, true] {
        let s = build_psrl_init(true, allow_predictions);
        let prof = s.find(PROFILE_SOURCE).expect("profile sourcing present");
        let env_at = s.find(ENV_GUARD).expect("env shim must be guarded by a user definition check");
        let claude_at = s.find(CLAUDE_GUARD).expect("claude wrapper must be guarded by a user definition check");
        assert!(prof < env_at && prof < claude_at, "guards must run after the profile defined its names");
        // The only definitions of the two names are the guarded ones.
        assert_eq!(s.matches("function Global:env").count(), 1);
        assert_eq!(s.matches("function Global:claude").count(), 1);
        assert!(s[claude_at..].contains("function Global:claude"));
    }
}

#[test]
fn noprofile_init_string_keeps_the_same_guards() {
    let s = build_psrl_init_noprofile(true);
    assert!(s.contains(ENV_GUARD));
    assert!(s.contains(CLAUDE_GUARD));
}

#[test]
fn env_shim_off_installs_neither_name() {
    let s = build_psrl_init(false, false);
    assert!(!s.contains("Global:env") && !s.contains("Global:claude"));
}

fn pwsh(script: &str) -> Option<String> {
    let attempt = |exe: &str| {
        std::process::Command::new(exe)
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script])
            .env("PSMUX_CLAUDE_TEAMMATE_MODE", "tmux")
            .output()
    };
    let out = attempt("pwsh").or_else(|_| attempt("powershell")).ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
}

/// The full init block (profile sourcing included) must still parse after the
/// guards wrapped the shim. Parse only: running it would load the developer's
/// real profile.
#[test]
fn init_string_parses_cleanly() {
    for s in [build_psrl_init(true, false), build_psrl_init(true, true), build_psrl_init_noprofile(true)] {
        let path = std::env::temp_dir().join(format!("psmux_t805_parse_{}.ps1", std::process::id()));
        std::fs::write(&path, &s).unwrap();
        let probe = format!(
            "$e=$null; $t=$null; [void][System.Management.Automation.Language.Parser]::ParseFile('{}', [ref]$t, [ref]$e); 'ERRORS=' + $e.Count",
            path.display()
        );
        let Some(out) = pwsh(&probe) else { return };
        let _ = std::fs::remove_file(&path);
        assert!(out.contains("ERRORS=0"), "init block does not parse: {out}");
    }
}

/// Runs `<profile>; <shim>` in one global scope, the way the pane init dot
/// sources the profile and then runs the shim, and reports what each name is.
fn after_shim(profile: &str) -> Option<String> {
    let probe = "foreach($n in 'claude','env'){ $c=Get-Command $n -EA 0; if($c){ \"$n=$($c.CommandType)|$($c.Definition.Trim())\" } else { \"$n=none\" } }";
    pwsh(&format!("{profile}; {ENV_SHIM_PS}; {probe}"))
}

#[test]
fn user_functions_win_over_the_shim() {
    let Some(out) = after_shim("function claude { 'USER-CLAUDE' }; function env { 'USER-ENV' }") else { return };
    assert!(out.contains("claude=Function|'USER-CLAUDE'"), "user claude function replaced:\n{out}");
    assert!(out.contains("env=Function|'USER-ENV'"), "user env function replaced:\n{out}");
}

#[test]
fn user_filters_win_over_the_shim() {
    let Some(out) = after_shim("filter claude { 'USER-CLAUDE-F' }; filter env { 'USER-ENV-F' }") else { return };
    assert!(out.contains("'USER-CLAUDE-F'"), "user claude filter replaced:\n{out}");
    assert!(out.contains("'USER-ENV-F'"), "user env filter replaced:\n{out}");
}

#[test]
fn user_aliases_win_and_no_shadow_function_is_installed() {
    let Some(out) = after_shim("Set-Alias claude Get-Date; Set-Alias env Get-Location; Remove-Item Function:claude,Function:env -EA 0") else { return };
    assert!(out.contains("claude=Alias|Get-Date"), "{out}");
    assert!(out.contains("env=Alias|Get-Location"), "{out}");
    let Some(fns) = pwsh(&format!(
        "Set-Alias claude Get-Date; Set-Alias env Get-Location; {ENV_SHIM_PS}; 'FN_CLAUDE=' + (Test-Path Function:claude); 'FN_ENV=' + (Test-Path Function:env)"
    )) else { return };
    assert!(fns.contains("FN_CLAUDE=False") && fns.contains("FN_ENV=False"), "psmux functions installed behind user aliases:\n{fns}");
}

/// #475 behaviour is kept for users who define neither name.
#[test]
fn shim_installs_both_when_the_user_has_none() {
    let Some(out) = after_shim("$null") else { return };
    assert!(out.contains("claude=Function|") && out.contains("Get-Command claude -CommandType Application,ExternalScript"), "{out}");
    assert!(out.contains("env=Function|") && out.contains("_shebang"), "{out}");
}

fn teams(builder: &CommandBuilder) -> Option<String> {
    builder.get_env("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS").map(|v| v.to_string_lossy().into_owned())
}

#[test]
fn agent_teams_defaults_to_one_under_fix_tty() {
    let mut b = CommandBuilder::new("pwsh");
    b.env_remove("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS");
    set_tmux_env(&mut b, 1, Some(1), None, "s", true, false);
    assert_eq!(teams(&b).as_deref(), Some("1"));
}

#[test]
fn agent_teams_keeps_an_inherited_user_value() {
    let mut b = CommandBuilder::new("pwsh");
    b.env("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", "0");
    set_tmux_env(&mut b, 1, Some(1), None, "s", true, false);
    assert_eq!(teams(&b).as_deref(), Some("0"), "user value replaced");
}

#[test]
fn agent_teams_not_set_with_fix_tty_off() {
    let mut b = CommandBuilder::new("pwsh");
    b.env_remove("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS");
    set_tmux_env(&mut b, 1, Some(1), None, "s", false, false);
    assert_eq!(teams(&b), None);
    assert!(b.get_env("PSMUX_CLAUDE_TEAMMATE_MODE").is_none());
}
