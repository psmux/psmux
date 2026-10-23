# Passthrough Client

A passthrough client hands the host terminal the active pane's own output, so
the terminal's own scrollback fills the way it does for a plain shell. This is
useful over SSH from terminals that scroll with their own scrollback, such as
Termius on a phone, where swiping back through the output is the natural way to
read it.

```powershell
psmux new -s work -f passthrough      # new session, passthrough client
psmux attach -t work -f passthrough   # attach to an existing session
```

`passthrough` is a client flag like `read-only`, so the two combine:
`attach -f read-only,passthrough`.

## What it does

- On attach it prints the active pane's history and visible screen as lines,
  then places the cursor where the pane has it. Everything written while no
  client was attached lands in the terminal's scrollback.
- After that, the pane's output bytes go straight to the terminal as they
  arrive, line feeds included.
- Keys go to the pane. `prefix d` detaches; pressing the prefix twice sends
  one prefix to the pane.
- The client size follows the terminal size.
- When the active pane or window changes, the client prints the new pane's
  screen and follows its output.

## Compared with the normal client

The normal client draws frames: every cell is placed with a cursor move, so
the host terminal never sees a line scroll off its top. Through a ConPTY with
120 lines of output it sends no line feeds and nothing reaches the host's
scrollback, with or without `terminal-overrides ',*:smcup@:rmcup@'`. A
passthrough client sends the same 120 lines as line feeds and all of them
reach the scrollback.

A passthrough client shows one pane at a time and draws no status line, pane
borders or menus. Splits, copy mode and other windows are still there for any
normal client attached to the same session.

## Automatic use over SSH

To use it for every SSH login, attach with the flag when `SSH_CONNECTION` is
set, for example from a `cmd` AutoRun script:

```bat
if defined SSH_CONNECTION if not defined TMUX psmux new -A -s main -f passthrough
```
