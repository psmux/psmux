# Agent Instructions

## Test Isolation

- Never run psmux tests or commands that may create sessions in the default
  namespace when working from an active psmux session.
- Some Rust tests create sessions internally and bypass the CLI `-L` option.
  Do not run `cargo test --all-targets` locally unless the process is inside a
  disposable Windows account, VM, or CI environment.
- Prefer safe targeted unit tests that do not start psmux servers, together
  with `cargo check`. Most `tests-rs/*.rs` files are modules of the binary
  (`#[cfg(test)] #[path = "../tests-rs/..."]`), so run one with
  `cargo test --bin psmux <module path filter>`, for example
  `cargo test --bin psmux choose_tree::tests_issue1065_chooser_search`,
  with `PSMUX_DATA_DIR` pointing at an empty temporary directory so a test
  that does reach a server cannot touch the user's own data directory.
  `--bin psmux` builds the tests once instead of for each of the three
  binaries (`psmux`, `pmux`, `tmux`) that share `src/main.rs`.
- Run runtime and integration checks with a unique namespace:
  `psmux -L <unique-test-namespace> ...`.
- Snapshot the default session list before and after runtime checks. Stop and
  investigate if it changes.
- Clean up only the test namespace with
  `psmux -L <unique-test-namespace> kill-server`.
- Never use bare `psmux kill-server` for test cleanup: it is scoped to the
  DEFAULT namespace (#649), so it misses your test namespace and hits whatever
  the user is running in the default one. Never use `kill-server -a` either,
  which is the deliberate every-namespace sweep.
- Run the full test suite only in CI or another disposable Windows environment
  where user sessions cannot be affected.
- `tests/run_all_tests.ps1` kills every psmux process by image name and
  deletes `~/.psmux\*.port`, `*.key`, `~/.psmux.conf` and `~/.psmuxrc`
  between suites. It refuses to start unless `PSMUX_TEST_SANDBOX=1`, which
  the Docker dev image sets. Never set it yourself on a machine with real
  sessions.
- `scripts/build.ps1` runs `psmux kill-server -a` and stops every `psmux`,
  `pmux` and `tmux` process before it installs. Do not run it while the user
  has sessions open; build with `cargo build --release` instead.
- Many single suites under `tests/` also stop psmux by image name or run a
  bare `kill-server` in the default namespace. Read a suite before running it
  and skip it on a machine with real sessions if it does either.

## Live Title Regression Tests

- Capture the attached client output without querying pane state during the
  measurement. `display-message` and `list-panes` synchronize OSC titles and can
  mask a missing refresh in the unsolicited server-push path. Query only after
  capture ends; compare parser/pane state with the host OSC output separately.
