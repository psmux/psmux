# Scrollback Client

A scrollback client hands the host terminal the active pane's own output, so
the terminal's own scrollback fills the way it does for a plain shell. This is
useful over SSH from terminals that scroll with their own scrollback, such as
Termius on a phone, where swiping back through the output is the natural way to
read it.

```powershell
psmux new -s work -f scrollback      # new session, scrollback client
psmux attach -t work -f scrollback   # attach to an existing session
```

`scrollback` is a client flag like `read-only`, so the two combine:
`attach -f read-only,scrollback`. A read-only scrollback client shows up as
read-only in `list-clients`, and the server refuses its keys. `-f passthrough`
is accepted as another name for it; it has nothing to do with the
`allow-passthrough` option.

## What it does

- On attach it prints the active pane's history and visible screen as lines,
  then places the cursor where the pane has it. Everything written while no
  client was attached lands in the terminal's scrollback.
- After that, the pane's output bytes go straight to the terminal as they
  arrive, line feeds included.
- Keys go to the pane. `prefix d` detaches; pressing the prefix twice sends
  one prefix to the pane. The prefix is the session's own if it sets one,
  the global one otherwise.
- The client size follows the terminal size, and it counts for the window
  size like any other attached client's (`window-size`, `latest` by default,
  so the session takes the size of whichever client was resized last). With
  `-f scrollback,ignore-size` it leaves the window size to the other clients;
  pane lines wider than the terminal then wrap there.
- When the active pane or window changes, the client prints the new pane's
  screen and follows its output.

## Compared with the normal client

The normal client draws frames: every cell is placed with a cursor move, so
the host terminal never sees a line scroll off its top. Through a ConPTY with
120 lines of output it sends no line feeds and nothing reaches the host's
scrollback, with or without `terminal-overrides ',*:smcup@:rmcup@'`. A
scrollback client sends the same 120 lines as line feeds and all of them
reach the scrollback.

A scrollback client shows one pane at a time and draws no status line, pane
borders or menus. Splits, copy mode and other windows are still there for any
normal client attached to the same session.

## Automatic use over SSH

To use it for every SSH login, attach with the flag when `SSH_CONNECTION` is
set, for example from a `cmd` AutoRun script:

```bat
if defined SSH_CONNECTION if not defined TMUX psmux new -A -s main -f scrollback
```
