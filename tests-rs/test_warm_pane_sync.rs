// Unit tests for the warm_pane_sync module — the policy layer that
// decides what the warm pane needs when server state changes.
//
// These tests do NOT exercise the `apply` function (it requires a
// real PtySystem and would mutate `app.warm_pane`).  `apply` is
// covered by the E2E layer in tests/test_issue271_runtime_set_propagation.ps1
// and tests/test_warm_pane_sync_options.ps1.

use super::*;
use crate::warm_pane_sync::{for_env_change, for_option_change, for_post_config, for_resize,
    reconcile_consumed_parser, WarmPanePatch, WarmPaneSync};
use crate::types::AppState;

fn fresh_app() -> AppState {
    let mut app = AppState::new("test".to_string());
    // Strip the implicit-PSMUX_TARGET_SESSION-style entries that
    // AppState::new may seed; for_post_config skips those by name,
    // but explicit cleanup keeps the test intent visible.
    app.environment.clear();
    app
}

// ── for_option_change: the policy table ────────────────────────────

#[test]
fn option_history_limit_returns_patch() {
    let mut app = fresh_app();
    app.history_limit = 50_000;
    let sync = for_option_change("history-limit", &app);
    match sync {
        WarmPaneSync::Patch(WarmPanePatch::HistoryLimit(n)) => assert_eq!(n, 50_000),
        _ => panic!("expected Patch(HistoryLimit), got something else"),
    }
}

#[test]
fn option_default_shell_requires_respawn() {
    let app = fresh_app();
    assert!(matches!(
        for_option_change("default-shell", &app),
        WarmPaneSync::Respawn(_)
    ));
}

#[test]
fn option_allow_predictions_requires_respawn() {
    let app = fresh_app();
    assert!(matches!(
        for_option_change("allow-predictions", &app),
        WarmPaneSync::Respawn(_)
    ));
}

#[test]
fn option_default_terminal_requires_respawn() {
    let app = fresh_app();
    assert!(matches!(
        for_option_change("default-terminal", &app),
        WarmPaneSync::Respawn(_)
    ));
}

#[test]
fn option_claude_code_options_require_respawn() {
    let app = fresh_app();
    assert!(matches!(
        for_option_change("claude-code-fix-tty", &app),
        WarmPaneSync::Respawn(_)
    ));
    assert!(matches!(
        for_option_change("claude-code-force-interactive", &app),
        WarmPaneSync::Respawn(_)
    ));
}

#[test]
fn unrelated_options_are_noop() {
    let app = fresh_app();
    // Sample a bunch of options that should NOT touch the warm pane.
    for name in [
        "status-style", "mouse", "prefix", "base-index", "renumber-windows",
        "status-left", "status-right", "pane-border-style",
    ] {
        assert!(
            matches!(for_option_change(name, &app), WarmPaneSync::Noop),
            "expected Noop for '{name}', got something else"
        );
    }
}

// ── for_env_change: env vars always force respawn ──────────────────

#[test]
fn env_change_always_respawns() {
    assert!(matches!(for_env_change(), WarmPaneSync::Respawn(_)));
}

// ── for_resize: only respawns when dimensions actually changed ─────

#[test]
fn resize_to_same_size_is_noop() {
    let mut app = fresh_app();
    // Without a warm pane, `for_resize` returns Respawn defensively
    // (no warm pane means we can't compare).  Set up a fake one to
    // exercise the equal-size branch.
    let fake_term = std::sync::Arc::new(std::sync::Mutex::new(
        vt100::Parser::new(40, 120, app.history_limit),
    ));
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0, });
    let child = crate::util::StubChild::exited();
    let now = std::time::Instant::now();
    app.warm_pane.push(crate::types::WarmPane {
        master,
        writer,
        child,
        term: fake_term,
        data_version: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        cursor_shape: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0)),
        bell_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        cpr_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        color_query_pending: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        child_pid: None,
        pane_id: 0,
        rows: 40,
        cols: 120,
        output_ring: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::VecDeque::new(),
        )),
        spawned_at: now,
        ready: true,
        last_dv: 0,
        last_change: now,
        trace_settled: false,
        host_colors: None,
    });

    assert!(matches!(for_resize(&app, 40, 120), WarmPaneSync::Noop));
    assert!(matches!(for_resize(&app, 41, 120), WarmPaneSync::Respawn(_)));
    assert!(matches!(for_resize(&app, 40, 121), WarmPaneSync::Respawn(_)));
}

#[test]
fn resize_with_no_warm_pane_returns_respawn() {
    // Defensive: callers should not assume warm_pane is None means
    // "do nothing"; for_resize returning Respawn lets `apply` notice
    // and possibly spawn a fresh warm pane at the new size.
    let app = fresh_app();
    assert!(matches!(for_resize(&app, 30, 80), WarmPaneSync::Respawn(_)));
}

// ── for_host_colors_change: the palette is baked into the child ─────

/// Push one spare carrying `planted` as the palette its shell was spawned with.
fn push_spare_with_palette(app: &mut AppState, planted: Option<crate::types::HostColors>) {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0 });
    let child = crate::util::StubChild::exited();
    let now = std::time::Instant::now();
    app.warm_pane.push(crate::types::WarmPane {
        master,
        writer,
        child,
        term: std::sync::Arc::new(std::sync::Mutex::new(vt100::Parser::new(40, 120, 100))),
        data_version: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        cursor_shape: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0)),
        bell_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        cpr_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        color_query_pending: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        child_pid: None,
        pane_id: 0,
        rows: 40,
        cols: 120,
        output_ring: std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
        spawned_at: now,
        ready: true,
        last_dv: 0,
        last_change: now,
        trace_settled: false,
        host_colors: planted,
    });
}

fn palette(fg: (u8, u8, u8)) -> crate::types::HostColors {
    let mut hc = crate::types::HostColors::empty();
    hc.fg = Some(fg);
    hc
}

#[test]
fn host_colors_report_retires_a_spare_that_has_no_palette() {
    // The real sequence: a server boots knowing nothing, fills its pool, then a
    // client attaches and reports the terminal's colours. Those spares will
    // hand an empty PSMUX_HOST_COLORS to whatever is transplanted into them.
    let mut app = fresh_app();
    app.host_colors = None;
    push_spare_with_palette(&mut app, None);
    app.host_colors = Some(palette((0x11, 0x22, 0x33)));
    assert!(matches!(
        crate::warm_pane_sync::for_host_colors_change(&app),
        WarmPaneSync::Respawn(_)
    ));
    app.warm_pane.kill_all();
}

#[test]
fn host_colors_report_matching_the_pool_is_noop() {
    // A second client reporting the same colours must not throw away a pool
    // that is already correct — that would cost a shell boot per attach.
    let mut app = fresh_app();
    let hc = palette((0x11, 0x22, 0x33));
    push_spare_with_palette(&mut app, Some(hc.clone()));
    app.host_colors = Some(hc);
    assert!(matches!(
        crate::warm_pane_sync::for_host_colors_change(&app),
        WarmPaneSync::Noop
    ));
    app.warm_pane.kill_all();
}

#[test]
fn host_colors_change_with_an_empty_pool_is_noop() {
    // Nothing is holding a stale palette, and the refill the loop asks for next
    // snapshots `app.host_colors` as it stands then (`pane::warm_spawn_params`),
    // so there is nothing for this to do. Unlike `for_resize`, which respawns an
    // empty pool defensively, there is no dimension here to get wrong.
    let mut app = fresh_app();
    app.host_colors = Some(palette((0x44, 0x55, 0x66)));
    assert!(matches!(
        crate::warm_pane_sync::for_host_colors_change(&app),
        WarmPaneSync::Noop
    ));
}

// ── for_post_config: priority order ────────────────────────────────

#[test]
fn post_config_warm_disabled_returns_respawn_for_kill() {
    // `apply` reads warm_enabled and degrades Respawn to a kill-only
    // when warm panes are off — this test pins the sync layer's
    // contract: it always returns Respawn here.
    let mut app = fresh_app();
    app.warm_enabled = false;
    assert!(matches!(for_post_config(&app), WarmPaneSync::Respawn(_)));
}

#[test]
fn post_config_custom_default_shell_respawns() {
    let mut app = fresh_app();
    app.default_shell = "C:\\Program Files\\PowerShell\\7\\pwsh.exe".to_string();
    assert!(matches!(for_post_config(&app), WarmPaneSync::Respawn(_)));
}

#[test]
fn post_config_env_vars_respawn() {
    let mut app = fresh_app();
    app.environment.insert("MY_VAR".to_string(), "hello".to_string());
    assert!(matches!(for_post_config(&app), WarmPaneSync::Respawn(_)));
}

#[test]
fn post_config_predictions_respawn() {
    let mut app = fresh_app();
    app.allow_predictions = true;
    assert!(matches!(for_post_config(&app), WarmPaneSync::Respawn(_)));
}

#[test]
fn post_config_history_limit_only_returns_patch() {
    let mut app = fresh_app();
    app.history_limit = 100_000;
    match for_post_config(&app) {
        WarmPaneSync::Patch(WarmPanePatch::HistoryLimit(n)) => assert_eq!(n, 100_000),
        _ => panic!("expected Patch when only history-limit differs from default"),
    }
}

#[test]
fn post_config_default_state_is_noop() {
    let app = fresh_app();
    assert!(matches!(for_post_config(&app), WarmPaneSync::Noop));
}

#[test]
fn post_config_skips_implicit_psmux_env() {
    // PSMUX_TARGET_SESSION / TMUX / TMUX_PANE are server-internal and
    // must not trigger a respawn — they are set on every spawn anyway.
    let mut app = fresh_app();
    app.environment.insert("PSMUX_TARGET_SESSION".to_string(), "test".to_string());
    app.environment.insert("TMUX".to_string(), "1".to_string());
    app.environment.insert("TMUX_PANE".to_string(), "%0".to_string());
    assert!(matches!(for_post_config(&app), WarmPaneSync::Noop));
}

#[test]
fn post_config_priority_respawn_beats_patch() {
    // If both env and history-limit changed, respawn wins because a
    // fresh spawn pulls in current history-limit too.
    let mut app = fresh_app();
    app.environment.insert("FOO".to_string(), "bar".to_string());
    app.history_limit = 100_000;
    assert!(matches!(for_post_config(&app), WarmPaneSync::Respawn(_)));
}

// ── reconcile_consumed_parser: the consume-time safety net ────────

#[test]
fn reconcile_consumed_parser_grows_cap_when_stale() {
    let mut p = vt100::Parser::new(4, 20, 2000);
    let mut app = fresh_app();
    app.history_limit = 100_000;
    reconcile_consumed_parser(&mut p, &app);
    assert_eq!(p.screen().scrollback_len(), 100_000);
}

#[test]
fn reconcile_consumed_parser_is_noop_when_already_synced() {
    let mut p = vt100::Parser::new(4, 20, 50_000);
    let mut app = fresh_app();
    app.history_limit = 50_000;
    reconcile_consumed_parser(&mut p, &app);
    assert_eq!(p.screen().scrollback_len(), 50_000);
}

#[test]
fn reconcile_consumed_parser_shrinks_when_limit_lowered() {
    let mut p = vt100::Parser::new(2, 10, 2000);
    let mut data = String::new();
    for i in 0..30 { data.push_str(&format!("L{i}\r\n")); }
    p.process(data.as_bytes());
    assert!(p.screen().scrollback_filled() > 5);

    let mut app = fresh_app();
    app.history_limit = 5;
    reconcile_consumed_parser(&mut p, &app);
    assert_eq!(p.screen().scrollback_len(), 5);
    assert!(p.screen().scrollback_filled() <= 5);
}

#[test]
fn reconcile_consumed_parser_propagates_alt_screen_flag() {
    // The same helper also reconciles the allow_alternate_screen flag
    // (#88).  When app config disables alt-screen handling, a fresh
    // or transplanted parser must pick that up at consume time.
    let mut p = vt100::Parser::new(4, 20, 2000);
    assert!(p.screen().allow_alternate_screen());
    let mut app = fresh_app();
    app.allow_alternate_screen = false;
    reconcile_consumed_parser(&mut p, &app);
    assert!(!p.screen().allow_alternate_screen());
}
