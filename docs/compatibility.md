# tmux Compatibility

psmux is the most tmux-compatible terminal multiplexer on Windows.

## Overview

| Feature | Support |
|---------|---------|
| Commands | **90+** tmux commands implemented. Run `psmux list-commands` for the live list |
| Format variables | **140+** variables with full modifier support |
| Config file | Reads `~/.tmux.conf` directly, including `%if` / `%elif` / `%else` / `%endif` conditionals |
| Key bindings | `bind-key`/`unbind-key` with key tables, case-sensitive |
| Hooks | 30 event hooks (`after-new-window`, `pane-died`, `window-linked`, etc.) with `set-hook`/`show-hooks` |
| Status bar | Full format engine with conditionals, loops, and multi-line support |
| Themes | 20+ style options, 24-bit color, text attributes |
| Layouts | 5 layouts (even-h, even-v, main-h, main-v, tiled) |
| Copy mode | 53 vim keybindings, search, registers, rectangle select |
| Targets | `session:window.pane`, `session:window_name`, `%id`, `@id` syntax |
| `if-shell` / `run-shell` | ✅ Conditional config logic |
| Paste buffers | ✅ Full buffer management |
| Control mode | ✅ `-C` / `-CC` programmatic protocol |
| Popups and menus | ✅ `display-popup`, `display-menu` |
| Interactive choosers | ✅ `choose-tree`, `choose-buffer`, `choose-client` |
| Server namespaces | ✅ `-L` for isolated instances |
| Command chaining | ✅ Sequential `;` operator |
| Nesting prevention | ✅ Blocks psmux inside psmux |
| Session environment | ✅ `set-environment` / `show-environment` |

**Your existing `.tmux.conf` works.** psmux reads it automatically. Just install and go.

## Comparison

| | psmux | Windows Terminal tabs | WSL + tmux |
|---|:---:|:---:|:---:|
| Session persist (detach/reattach) | ✅ | ❌ | ⚠️ WSL only |
| Synchronized panes | ✅ | ❌ | ✅ |
| tmux keybindings | ✅ | ❌ | ✅ |
| Reads `.tmux.conf` | ✅ | ❌ | ✅ |
| tmux theme support | ✅ | ❌ | ✅ |
| Native Windows shells | ✅ | ✅ | ❌ |
| Full mouse support | ✅ | ✅ | ⚠️ Partial |
| Zero dependencies | ✅ | ✅ | ❌ (needs WSL) |
| Scriptable (90+ commands) | ✅ | ❌ | ✅ |
| Claude Code agent teams | ✅ | ❌ | ✅ |
| CJK/IME text input | ✅ | ✅ | ✅ |
| Warm session pre-spawn | ✅ | N/A | ❌ |

## Supported Commands

For the full list of supported tmux commands and arguments, see [tmux_args_reference.md](tmux_args_reference.md).

## Recent Parity Improvements

This section covers tmux features that were recently brought to full parity.

### Case-sensitive Key Bindings

Key bindings now distinguish between lowercase and uppercase letters exactly like tmux. `bind-key T` binds to `Shift+T`, while `bind-key t` binds to lowercase `t`. This is critical for plugins like PPM (`Prefix+I` to install) and psmux-sensible (`Prefix+R` to reload).

### Ctrl+Space as Prefix

`set -g prefix C-Space` now works correctly. Previously, multi-character key names like `Space` were parsed as single character fallbacks.

### Wrapped Directional Pane Navigation

Directional pane navigation (`select-pane -U/-D/-L/-R`) now wraps at layout edges, matching tmux behavior. Navigating past the rightmost pane wraps to the leftmost, and so on. Wrap is also correctly suppressed while zoomed.

### Prefix Repeat Chaining

After pressing the prefix key, successive keypresses within the `repeat-time` window (default 500ms) each trigger the bound action without needing to re-enter the prefix. This matches tmux's repeat behavior for pane navigation and resize bindings.

### Switch Client

`switch-client` is fully functional with all standard flags (`-t`, `-n`, `-p`, `-l`). Use it to programmatically switch between sessions.

### Window Name Resolution in Targets

Target syntax now resolves window names, not just indices. `send-keys -t mysession:mywindow` correctly finds the window named "mywindow" in session "mysession".

### Manual Rename Flag

`new-window -n NAME` now sets the `manual_rename` flag, preventing `automatic-rename` from overwriting the explicitly specified window name with the foreground process name.

### List Commands from Within Session

Commands like `list-panes`, `list-windows`, `list-clients`, `list-commands`, and `show-hooks` now work when run from within a psmux session (via `Prefix + :`). Output is displayed in a temporary overlay.

### Source File from Within Session

`source-file` works from within a live session via `Prefix + :`. Previously, config changes only took effect after detaching and reattaching or killing the server.

### Display Panes Overlay

`display-panes` (and `Prefix + q`) now shows pane numbers briefly and auto-dismisses after `display-panes-time` (default 1s). Type a number during the overlay to switch to that pane.

### Hook Deduplication

`set-hook -g` now replaces existing hooks on reload instead of stacking duplicates. `set-hook -gu` correctly removes hooks.

### Command Chaining with Semicolons

Multiple commands can be chained with `;` on a single line, matching tmux behavior:

```tmux
bind-key M-s split-window -h \; select-pane -L
```

### Run Shell Output

`run-shell` now displays output in the status bar, matching tmux behavior. Background mode with `-b` runs fire and forget.

### Session Server Persistence

The psmux session server now survives SSH disconnects. On reconnect, sessions are intact and `psmux attach` reattaches normally.

### Bell and Alert Support

BEL characters (`\x07`) from programs are forwarded to your host terminal for audible beep. The `bell-action` option controls when bells are forwarded and when the status bar tab gets a bell flag.

### Pane Border Labels with Truncation

`pane-border-format` labels that exceed the pane width are now truncated with ellipsis instead of overflowing or clipping mid-character.

### Pane Title Management

`select-pane -T ""` correctly clears a pane title. The default pane title is the hostname, matching tmux convention. Programs can update the pane title via OSC 0/2 escape sequences (controlled by the `allow-set-title` option). See [pane-titles.md](pane-titles.md) for details on how this interacts with PowerShell and other shells.

### `pane_current_command` and Program Titles on Windows

On Linux, tmux reads the controlling terminal's foreground process group, so a program that sets its own process title (Node's `process.title`, for instance) shows that title in `#{pane_current_command}`. Windows has no equivalent. ConPTY has no `tcgetpgrp`, `Win32_Process.Name` reports the image name, and a console process has no main window title, so a program modified title is invisible to every process enumeration API. psmux therefore reports the **executable of the pane's immediate child**, which is stable and cheap but is not a service identity (#647).

The logical name is still reachable, through a different variable. A Windows program that names itself calls `SetConsoleTitleW`, which is what `process.title` does under the hood; ConPTY turns that into an OSC title on the pane's output stream, and psmux stores it as the pane title. Turn on `allow-set-title`, which is off by default:

```powershell
psmux set-option -g allow-set-title on
psmux list-panes -t gateway -F '#{pane_id}|#{pane_current_command}|#{pane_title}'
# %1|node|openclaw-gateway
```

That is the same string tmux would put in `pane_current_command` on Linux. It is valid only while the program that set it owns the console: an interactive shell rewrites the title on every prompt, so an idle PowerShell pane reports its working directory rather than any service name.

`#{pane_start_command}` is the third piece: it records the command psmux was asked to run, so nothing the program does to itself can change it. It is empty for a pane that got the default shell, including the first pane of a plain `new-session`, so a supervisor relying on it must create its pane with an explicit command.

To identify a service, combine a controller chosen window or pane name addressed by stable id, `#{pane_start_command}`, pane liveness (`#{pane_dead}` and `#{pane_pid}`), and a health check outside psmux such as a port probe. Treat `#{pane_title}` as a further signal once `allow-set-title` is on, and `#{pane_current_command}` as a coarse filter only. See [integration.md](integration.md#identifying-the-program-running-in-a-pane) for the full recipe.

### Multi-line Status Bar

`set -g status 2` enables a multi-line status bar with `status-format[0]` and `status-format[1]` fully rendering style directives like `#[fg=red]`, `#[align=left]`, and `#[fill=blue]`. Mouse clicks are hit tested on every status row, not only the first, and `#[range=window|N]` regions on any row switch to window index `N` (#593), so a window list placed on the second line is clickable.

### Status Bar Style Directives

The following inline style directives are now rendered correctly in status-format lines:

- `#[list]` for the window list region
- `#[fill=colour]` for background fill
- `#[align=left|centre|right]` for text alignment
- `#[range=...]` for click regions

### Format Variable Expansion in Bindings

The `-F` flag on `bind-key` now properly expands format variables, enabling plugins like smart-splits.nvim to query pane dimensions.

### Set Environment

`set-environment` and `show-environment` follow tmux: `-g` global vs session scope, `-u`, `-r` removal markers (`-NAME`), `-h` hidden entries, `-F`, `show-environment NAME` with `unknown variable: NAME` at exit 1, and `-s` shell output. The global environment is the session server's start environment, since psmux runs one server per session. Variables are inherited by new panes at the process level (no shell commands echoed). The `new-session -e VAR=val` flag also sets session environment correctly.

### Unbind All Keys

`unbind-key -a` correctly removes all key bindings across all key tables. You can also target specific tables: `unbind-key -a -T prefix`, `unbind-key -a -T root`, `unbind-key -a -T copy-mode`.

### Client Prefix Format Variable

The `#{client_prefix}` format variable is correctly set when the prefix key is pressed. This enables status bar indicators like:

```tmux
set -g status-right "#{?client_prefix,#[bg=red] PREFIX ,}"
```

### Window Zoomed Flag

The `#{window_zoomed_flag}` format variable is correctly maintained during zoom/unzoom operations.

### Capture Pane

`capture-pane -p` correctly outputs pane content to stdout, enabling scripts and integrations (including Claude Code agent team coordination) to read pane state.

### Split Window Percentage

`split-window -p <percent>` correctly creates splits at the specified percentage instead of defaulting to 50/50.

### Split Window Working Directory

`split-window -c "#{pane_current_path}"` correctly resolves the format variable and opens the new pane in the current pane's working directory.

### End of Options and Dash-leading Values

`set-option` stops parsing flags at the option name, as tmux's getopt does, so a value that begins with a dash is stored rather than eaten: `set @k -u` writes the literal `-u` instead of silently deleting `@k` (#583). `send-keys` and `set-option` both honour `--` as an end of options marker (#562, #583), so `send-keys -l -- -rf` types `-rf`.

### Direct argv Execution after `--`

`new-window`, `split-window` and `new-session` treat a multi token command after `--` as an argv and execute it directly, exactly like tmux's `execvp` (#582). A single token stays a shell command string. Scripts that used `-- program arg arg` to bypass the shell on Linux now behave the same way on Windows, with no PowerShell profile in the path and `#{pane_current_command}` reporting the program.

### Window Target Resolver

`move-window` and `swap-window` honour `-s` and resolve `+N`, `-N`, `!`, `^`, `$`, `@id`, `{last}` style symbols, indexes and window names through a single tmux parity resolver (#602), and attached clients see the new window list immediately instead of a stale one (#601).

### Title-only `select-pane`

`select-pane -T` and `select-pane -P` set the pane title or style without moving the active window or pane, matching tmux (#592). Only a direction flag or `-l` changes focus.

### Pane Input Off (`select-pane -d` / `-e`)

`select-pane -d` stops input from reaching a pane's program and `select-pane -e` lets it through again, tmux's `PANE_INPUTOFF`; `#{pane_input_off}` reports which. Neither selects the pane. While input is off, typed keys, `send-keys` in every form, `paste-buffer` and forwarded mouse events are dropped and `synchronize-panes` leaves the pane out, while copy mode, `send-keys -X` and psmux's answers to the program's terminal queries keep working. The flag survives `respawn-pane`. See [tmux_args_reference.md](tmux_args_reference.md) for the details. Earlier releases accepted both flags and ignored them.

### Bare Command Routing

A command with no `-t` reaches the session with the most recent activity, the same choice tmux's `cmd_find_best_session` makes, rather than whichever session was attached last according to a stale file (#603). A bare `attach` with nothing to attach to prints `no sessions` (#605).

### Singular Option Aliases

`show-option` and `show-window-option` are accepted alongside the plural forms (#586).

### Hexadecimal and Base64 Transport

`send-keys -H 68 69` writes single bytes to the pane, byte exact. `send-paste` takes a base64 payload at the CLI, so multi line text with quotes and semicolons is delivered as one bracketed paste and can never be misread as a second command.

### UTF-8 and CJK Support

Multi-byte UTF-8 characters (box-drawing, emoji, CJK text) render correctly in panes. Pasting CJK text no longer crashes the session. Japanese and Korean IME input is handled with minimal latency (the paste-detection heuristic was tuned to avoid misidentifying rapid IME bursts).

Emoji clusters take the columns tmux gives them (#749). A ZWJ sequence such as a family, an emoji with a skin tone modifier, a flag spelled with two regional indicators, and a character followed by the variation selector U+FE0F each occupy one cell two columns wide, so `U+1F468 ZWJ U+1F469 ZWJ U+1F467 ZWJ U+1F466` moves the cursor two columns, not eight. The rules are tmux's `utf8_should_combine` (utf8-combined.c) and `screen_write_combine` (screen-write.c), including tmux's 32 byte limit on one cell, and command prompts count columns with the same rule. Windows Terminal, VS Code, JetBrains, WezTerm, Alacritty, ConEmu and conhost draw these sequences as one glyph too; mintty is the known outlier and draws the parts separately.

### Window-scoped Options

Every window carries its own option table, so a window-scoped write reaches one window and nothing else (#648, from the #647 WIN-01 report):

```text
psmux new-session -d -s s -n zero
psmux new-window  -d -t "s:" -n one
psmux set-option -w -t "s:zero" remain-on-exit on

psmux show-options -w -v -t "s:zero" remain-on-exit   # on
psmux show-options -w -v -t "s:one"  remain-on-exit   # off  (inherited)
psmux show-options -g -v remain-on-exit               # off  (untouched)
```

The chain is pane, then window, then global, the way tmux resolves an option. `-wg` writes the global window table, `-w -u` removes one window's entry so it inherits again, and `-p` still outranks `-w` on the pane that has it. `show-options -w -A` marks every inherited entry with tmux's `*`:

```text
psmux show-options -w -A -t "s:zero"   # remain-on-exit on
psmux show-options -w -A -t "s:one"    # remain-on-exit* off
```

Which store a write lands in follows the option NAME, as in tmux, so `setw status-left ...` still writes the session option. `@name` user options stay in the one session-wide map psmux reads them from everywhere, so set those with `-g` (or `-wg`).

Before #648 psmux kept a single ordinary option store and `-w` selected the same map as `-g`. The visible symptom was a reporting one (`zero=on one=on global=on`), but the pane reaper read that one flag for every window, so ordinary panes in an untargeted window stopped closing after `exit`. `-t` also dropped the window NAME and acted on the active window; it now resolves names, indices, `@id` and tmux's symbolic window spellings on the CLI, TCP, command-prompt and config-file routes alike.

Two window options keep a dedicated field alongside the table because other code reads them directly: `automatic-rename` still reports `off` for a window born with `-n NAME` (#266) and `window-size` still drives `resize-window`. Both stay in step with the table on a write and on a `-u`.

### Value-only `show-options -v`

`-v` prints the value and nothing else in every scope, so a script can compare stdout with `on` or `off` directly (#647):

```text
psmux set-option -p -t %0 remain-on-exit on
psmux show-options -p -v -t %0 remain-on-exit    # on
psmux show-options -pv  -t %0 remain-on-exit     # on
psmux show-options -p   -t %0 remain-on-exit     # remain-on-exit on
```

A named query answers for that one option only. As in tmux, an option that is not set in the queried scope's own store prints nothing and exits 0; add `-A` to fall back to the inherited value, which is then marked with `*` when the name is printed. A bare `show-options -p` still lists the whole pane store.

### Control Characters in Names and Messages

Window and session names are sanitized when they are set, the way tmux does it in `clean_name`, so a name can never carry a raw control byte and `#{window_name}` is always safe to drop into a tab separated or newline separated record (#647):

```text
psmux rename-window "w<TAB>x"   # stored and printed as w\tx
psmux rename-window "w<ESC>y"   # stored and printed as w\033y
```

`display-message -p` encodes its result the same way tmux does: `ESC` becomes `\033`, `CR` becomes `\r`, `BEL` becomes `\a`, any other control byte becomes a three digit octal escape, and valid UTF-8 passes through untouched. A tab and a newline are deliberately left alone, so a multi line `#{...}` result still prints as multiple lines.

Two things follow tmux by not changing. `list-panes`, `list-windows` and `list-sessions` hand back the `-F` string byte for byte, so a literal tab you put in your own format stays a tab (tmux stopped visually encoding command output in 3.6). `capture-pane`, `show-buffer`, `save-buffer` and control mode are never touched, because they carry pane bytes rather than metadata.

One deliberate deviation: a backslash is never doubled. tmux stores `C:\src` as `C:\\src`, which on Windows would corrupt every path shaped window name.

### Git Revision in `psmux -V`

`psmux -V` reports the commit the binary was built from. `git` is the preferred source, and when it is unavailable the build falls back, in order, to `.cargo_vcs_info.json` (present in every crates.io tarball), to the cargo git checkout directory name (`cargo install --git` checks a revision out into a directory named after its short hash), and finally to a `PSMUX_GIT_SHA` environment variable that release tooling can set (#647):

```text
psmux 3.3.8 (4d12898 2026-09-12)          # built in a git checkout
psmux 3.3.8 (4d12898 2026-09-12, dirty)   # built from a modified tree
psmux 3.3.8 (4d12898)                     # commit known, date not
psmux 3.3.8 (unknown commit)              # no revision recorded anywhere
```

`cargo install --git` uses libgit2 and never needs the `git` binary, so a machine without git on PATH used to install happily and then report `unknown commit`. The commit date is only ever available from git, so a fallback build prints the hash without a date.

### Per Pane Colour Palette (OSC 4 and OSC 104)

Each pane keeps its own 256 entry colour palette, exactly as tmux keeps one on every `window_pane` (#685). A pane child sets an entry with OSC 4 and clears it with OSC 104, and psmux substitutes the palette's RGB for an indexed colour at render time, which is where tmux does the same substitution in `tty_check_fg`, `tty_check_bg` and `tty_check_us`:

```text
ESC]4;4;rgb:00/00/80 ESC\      index 4 becomes #000080 in THIS pane
ESC]4;4;#000080 ESC\           the # form, also accepted
ESC]4;4;rgb:0000/0000/8080 ESC\   one to four hex digits per channel
ESC]4;4;rgb:00/00/80;6;rgb:00/80/80 ESC\   several pairs in one sequence
ESC]104;4 ESC\                 put index 4 back
ESC]104 ESC\                   put every index back
ESC]4;4;? ESC\                 answered from this pane's entry, then from the host terminal
```

This matters most for Windows console applications. A pane child on ConPTY announces its console colour table as OSC 4 and then paints with indexed SGR rather than true colour, so Far Manager's panels are `ESC[48;5;4m` against an OSC 4 that says index 4 is `#000080`. Before #685 psmux dropped the palette and re rendered the cell as a plain `ESC[44m`, which Windows Terminal painted with its own Campbell `#0037DA`. Far inside psmux now paints `#000080`, `#008080` and `#00FFFF` exactly as Far outside psmux does.

Three deliberate choices:

- **The palette is per pane and is never forwarded to the outer terminal.** Two panes with different palettes would fight over one terminal, which is precisely why tmux resolves it per pane instead.
- **A pane that never sets a palette is unchanged, byte for byte.** The substitution costs one null check per frame for such a pane, and nothing travels on the wire that did not travel before.
- **A respawn keeps the palette, RIS clears it.** tmux keeps the palette on the pane and clears it only on RIS (`ESC c`), `ESC]104` and `send-keys -R`, so a `respawn-pane` or `respawn-window` keeps it, along with the pane's history (#708). The respawn resets everything else the way tmux's `screen_reinit` does: the visible rows are cleared, the cursor goes home, the modes are reset and the alternate screen is left.

One platform limit, measured on Windows 11 build 26200: conhost swallows a **bare** `ESC]104 ESC\` on the ConPTY output path, so a pane child running under ConPTY cannot reach the clear-everything form. `ESC]104;4 ESC\` arrives normally.

## Behavioral Differences from tmux

A few commands intentionally behave differently from upstream tmux. These are deliberate choices, not bugs.

### `kill-server` with Multiple Sockets

In upstream tmux, each `-L <name>` socket is a fully separate server, and `kill-server` only ever affects the socket it was invoked on. psmux matches that: **a bare `kill-server` ends the default namespace and leaves every `-L` namespace running**, and `-L <name> kill-server` ends that one namespace only.

```text
psmux kill-server            # ends the DEFAULT namespace only; -L namespaces keep running
psmux -L work kill-server    # ends the "work" namespace only
psmux kill-server -a         # psmux extension: ends EVERY namespace in the data dir
```

`-a` (long form `--all`) is the psmux-only "stop everything" switch, for when you have lost track of background servers and want a clean slate. tmux has no equivalent, because on Unix each socket is a separate process tree you can see in `ps`.

Exit codes follow tmux: when there is nothing to kill in the scope, `kill-server` prints `no server running on <socket>` and exits 1, so `psmux kill-server || true` behaves the way scripts written against tmux expect. `-a` is a sweep rather than a request to one server, so it exits 0 even when it finds nothing.

**Changed after psmux 3.3.8 (#649).** Up to that release a bare `kill-server` tore down every socket at once, which meant one command in one shell could end somebody else's `-L` sessions in the same data dir. If you relied on that, use `kill-server -a`.

An attached client's `kill-server` (typed at the command prompt, or bound to a key) follows the same rule: it ends every server on the client's own socket, which is what tmux's single-process server does implicitly.

### Background Processes When a Pane Exits

On Unix, tmux leans on the kernel: closing a pane's terminal sends SIGHUP to its foreground process group, and anything deliberately detached (`nohup`, daemons, most GUI apps) survives. Windows has no SIGHUP and no pty process groups, so psmux walks the pane's process tree instead. By default, when a pane's shell exits on its own, psmux terminates the background children that shell left behind; without the sweep they leak invisibly along with a `conhost.exe` each, which in bulk can exhaust the desktop heap.

To get tmux-style survival for intentionally backgrounded processes, opt out with the `@kill-descendants` user option:

```tmux
set -g @kill-descendants off
```

Explicit `kill-pane`, `kill-window`, and `kill-session` always terminate the pane's full process tree regardless of this option. See the Dead Panes section in [configuration.md](configuration.md) for details.

## Format Variables

psmux supports 140+ format variables with full modifier support, including:

- Session/window/pane variables (`#S`, `#W`, `#P`, `#{pane_current_path}`, etc.)
- Style and color modifiers
- Conditional expressions (`#{?condition,true,false}`)
- Comparison operators (`#{==:a,b}`, `#{!=:a,b}`, `#{<:a,b}`)
- Logical operators (`#{||:a,b}`, `#{&&:a,b}`)
- Regex substitution (`#{s/pat/rep/:var}`)
- String operations: basename (`#{b:}`), dirname (`#{d:}`), lowercase (`#{l:}`), shell quote (`#{q:}`)
- Truncation and padding (`#{=N:var}`, `#{pN:var}`)
- Loop iteration over windows (`#{W:fmt}`), panes (`#{P:fmt}`), and sessions (`#{S:fmt}`)

## Named Paste Buffers

psmux supports named paste buffers, matching tmux behavior:

```powershell
# Set a named buffer
psmux set-buffer -b mybuf "hello world"

# Show a named buffer
psmux show-buffer -b mybuf

# Delete a named buffer
psmux delete-buffer -b mybuf

# Paste from a named buffer
psmux paste-buffer -b mybuf
```

Named buffers are separate from the default (anonymous) buffer stack. They persist for the lifetime of the session and can be used for inter-pane data exchange in scripts and automation workflows.

## Developer Integration: Using psmux as a tmux Drop-in on Windows

psmux implements the same CLI protocol as tmux. Any tool, library, or script that drives tmux via
subprocess commands will work on psmux with minimal or zero changes:

- The command syntax, flags, and output formats are the same, and psmux installs a `tmux.exe` alias
  so scripts that call `tmux` by name find it on the PATH without any code change.
- Stable IDs use the same scheme: `$N` for a session, `@N` for a window, `%N` for a pane. They are
  monotonically increasing and never reused during a server's lifetime.
- Control mode (`-C` / `-CC`) uses the same wire protocol, with `%begin`/`%end` framing and async
  notifications.
- libtmux works against psmux, with one Windows encoding caveat.

The one thing that is genuinely different on Windows is text encoding. psmux emits UTF-8, but the
default Windows console code page is often cp1252, so a Python caller that does not pass
`encoding="utf-8"` will garble non-ASCII output. Set `PYTHONUTF8=1` or pass the encoding explicitly.
This is what causes libtmux to return empty session lists on Windows.

The full developer guide lives in [integration.md](integration.md), which covers subprocess examples
in Python, PowerShell, Node.js, Go, and Rust, libtmux setup and its encoding fix, the cross-platform
project pattern, targeting syntax, psmux extension commands such as `dump-state`, environment
variable propagation, hooks, `wait-for` synchronization, and troubleshooting. Start there rather than
here for anything integration related.

For control mode specifically, see [control-mode.md](control-mode.md), and for the iTerm2 tmux
gateway see [iterm2-control-mode.md](iterm2-control-mode.md).
