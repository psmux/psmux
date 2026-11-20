# Agent Instructions

## Test Isolation

- Never run psmux tests or commands that may create sessions in the default
  namespace when working from an active psmux session.
- Some Rust tests create sessions internally and bypass the CLI `-L` option.
  Do not run `cargo test --all-targets` locally unless the process is inside a
  disposable Windows account, VM, or CI environment.
- Prefer safe targeted unit tests that do not start psmux servers, together
  with `cargo check`.
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

## Live Title Regression Tests

- Capture the attached client output without querying pane state during the
  measurement. `display-message` and `list-panes` synchronize OSC titles and can
  mask a missing refresh in the unsolicited server-push path. Query only after
  capture ends; compare parser/pane state with the host OSC output separately.
