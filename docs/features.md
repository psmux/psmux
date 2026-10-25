# Features

psmux brings the tmux feature set to Windows natively: sessions that outlive the terminal window, split panes and windows, full mouse support, a vim style copy mode, tmux themes and plugins, 90+ tmux compatible commands and 140+ format variables, all in one Rust binary on ConPTY with no WSL, Cygwin or MSYS2. This page is the complete list; the [Getting Started tutorial](tutorials/getting-started-windows.md) shows the essentials in ten minutes.

## Highlights

- 🦠 **Made in Rust** : opt-level 3, full LTO, single codegen unit. Maximum performance.
- 🖱️ **Full mouse support** : click panes, drag-resize borders, scroll, click tabs, select text, right-click copy
- 🎨 **tmux theme support** : 16 named colors + 256 indexed + 24-bit true color (`#RRGGBB`), 20+ style options
- 📋 **Reads your `.tmux.conf`** : drop-in config compatibility, zero learning curve
- ⚡ **Blazing fast startup** : sub-100ms session creation, near-zero overhead over shell startup
- 🔌 **90+ tmux-compatible commands** : `bind-key`, `set-option`, `if-shell`, `run-shell`, `display-popup`, `display-menu`, hooks, and more. Run `psmux list-commands` for the live list
- 🪟 **Windows-native** : ConPTY, Win32 API, works with PowerShell, cmd, bash, WSL, nushell
- 📦 **Single binary, no dependencies** : install via `cargo`, `winget`, `scoop`, or `choco`
- 🤖 **Claude Code agent teams** : first-class support for teammate pane spawning
- 🌐 **CJK/IME input** : full support for Chinese, Japanese, and Korean input methods

## Terminal Multiplexing

- Split panes horizontally (`Prefix + %`) and vertically (`Prefix + "`)
- Multiple windows with clickable status-bar tabs
- Session management: detach (`Prefix + d`) and reattach from anywhere
- 5 layouts: even-horizontal, even-vertical, main-horizontal, main-vertical, tiled
- **Floating panes** : `new-pane` (`newp`) creates a pane that floats above the tiled layout, positioned with `-X`/`-Y`, sized with `-x`/`-y`, titled with `-T`, bordered with `-B`, and movable and resizable with the mouse. See [scripting.md](scripting.md)
- **Cross-session pane transfer** : `join-pane -s othersession:1.0` and `move-pane` move a live pane between two independent session servers. The real console stays put and its input and output are tunneled, so the running program never notices. See [scripting.md](scripting.md)

## Full Mouse Support

- **Click** any pane to focus it, input goes to the right shell
- **Drag** pane borders to resize splits interactively
- **Click** status-bar tabs to switch windows
- **Scroll wheel** in any pane, scrolls that pane's output (configurable via `scroll-enter-copy-mode`)
- **Drag-select** text to copy to clipboard
- **Drag auto-scroll** : dragging a selection past the top of a pane continues it into scrollback via copy mode, and holding the pointer on the pane's first or last row keeps scrolling one line every 50ms, like tmux; psmux additionally keeps scrolling, faster, when the pointer moves past the edge. Output taller than the window can be selected in one gesture. With `scroll-enter-copy-mode off`, a selection made over a direct-scrolled view also continues past the bottom edge, back toward the live output
- **Right-click** to paste or copy selection
- **tmux-like release copy selection** : pane-clipped drag copy on left-button release with word/line multi-click (`pwsh-mouse-selection on`)
- **Disable client-side selection** : let in-pane TUI apps (opencode, lazygit, etc.) handle their own mouse selection (`mouse-selection off`)
- **VT mouse forwarding** : apps like vim, htop, and midnight commander get full mouse events
- **3-layer mouse injection** : VT protocol, VT bridge (for WSL/SSH), and native Win32 MOUSE_EVENT
- **Mouse over SSH** : works from any OS client when server runs Windows 11 build 22523+
- **Disable mouse** : `set -g mouse off` fully suppresses mouse event handling

## tmux Theme & Style Support

- **20+ customizable style options** : status bar, pane borders, messages, copy-mode highlights, popups, menus, window backgrounds. See [configuration.md](configuration.md) for the full table
- **Full color spectrum** : 16 named colors, 256 indexed (`colour0` to `colour255`), 24-bit true color (`#RRGGBB`)
- **Text attributes** : bold, dim, italic, underline, blink, reverse, strikethrough, and more
- **Styled underscores** : double, curly (undercurl), dotted and dashed underlines from `ESC[4:2m` through `ESC[4:5m`, plus the SGR 58 underline colour, are parsed and passed on to the outer terminal
- **Status bar** : fully customizable left/right content with format variables
- **Window tab styling** : separate styles for active, inactive, activity, bell, and last-used tabs
- **Pane border line styles** : `set -g pane-border-lines double` picks the glyph set (`single`, `double`, `heavy`, `simple`, `number`, `spaces`, `none`), and a post-pass upgrades straight runs to proper junction glyphs where borders meet. See [configuration.md](configuration.md)
- Compatible with existing tmux theme configs

## Copy Mode (Vim Keybindings)

- **53 vi-style key bindings** : motions, selections, search, text objects
- Visual, line, and **rectangle selection** modes (`v`, `V`, `Ctrl+v`)
- `/` and `?` search with `n`/`N` navigation, across the whole scrollback (an off screen match scrolls the view to it)
- Copy mode is a property of the pane, as in tmux: new panes start live, and more than one pane can be in copy mode at once
- **The view is a snapshot, as in tmux**: entering copy mode freezes the pane's grid, so output that keeps arriving cannot push the view down or evict the lines under the cursor. Every pane in copy mode holds its own snapshot for as long as it is in copy mode, focused or not, the way a tmux pane keeps the mode it was put into. The live screen keeps running behind it, `capture-pane` (plain, `-S`/`-E`, `-e`) still answers with the live screen, and leaving copy mode reveals everything that arrived. `#{history_size}` keeps reporting the live grid while copy mode is active, as tmux does, so a status line pairing it with `#{scroll_position}` describes the live pane rather than the frozen view. The snapshot shares the pane's retained history instead of copying it, so opening copy mode on a 50000 line scrollback costs under a megabyte rather than the whole history
- `f`/`F`/`t`/`T` character find, `%` bracket matching, `{`/`}` paragraph jump
- Named registers (`"a` through `"z`), count prefixes, word/WORD variants
- Mouse drag-select copies to Windows clipboard on release and returns to the live view (tmux parity); dragging at the pane's first/last row auto-scrolls through scrollback
- **Line numbers** : `set -g copy-mode-line-numbers relative` adds a left gutter, with `off`, `default`, `absolute`, `relative`, and `hybrid` modes and independent styling through `copy-mode-line-number-style` and `copy-mode-current-line-number-style`. See [configuration.md](configuration.md)

See [keybindings.md](keybindings.md) for the full copy mode key reference.

## Format Engine

- **140+ tmux-compatible format variables** across sessions, windows, panes, cursor, client, and server
- Conditionals (`#{?cond,true,false}`), comparisons (`#{==:a,b}`, `#{!=:a,b}`), boolean logic (`#{||:}`, `#{&&:}`)
- Regex substitution (`#{s/pat/rep/:var}`), string manipulation
- Loop iteration (`#{W:fmt}`, `#{P:fmt}`, `#{S:fmt}`) over windows, panes, sessions
- Truncation, padding, basename, dirname, strftime, shell quoting
- Inline style directives: `#[list]`, `#[fill]`, `#[align=left|centre|right]`, `#[range=...]`

## Scripting & Automation

- **90+ tmux-compatible commands** : everything you need for automation. Run `psmux list-commands` for the live list
- `send-keys`, `capture-pane`, `pipe-pane` for CI/CD and DevOps workflows
- `display-popup` for floating popup windows with custom commands
- `display-menu` for interactive context menus
- `choose-tree` for interactive session/window/pane selection
- `choose-buffer` and `choose-client` for interactive buffer and client picking
- `if-shell` and `run-shell` for conditional config logic
- **30 event hooks** : `after-new-window`, `after-split-window`, `client-attached`, `pane-died`, `pane-focus-in`, `window-linked`, and more. Note that `set-hook` accepts any name without validating it, so a typo silently never fires
- **Config file conditionals** : `%if`, `%elif`, `%else`, `%endif` blocks (they nest), plus `%hidden NAME=value`. The condition is a format string, so `%if "#{==:#{@theme},dark}"` works. See [configuration.md](configuration.md)
- **User defined command aliases** : `set -s command-alias[100] 'bigsplit=split-window -p 70'` creates your own command name, in tmux's array form. It resolves from the CLI, key bindings, the command prompt, hooks and control mode. See [configuration.md](configuration.md)
- **Positional pane targets** : `select-pane -t '{top-right}'`, plus `{top-left}`, `{bottom-left}`, `{bottom-right}`, `{top}`, `{bottom}`, `{left}`, `{right}`. See [scripting.md](scripting.md)
- **Prompt history** : `show-prompt-history` (`showphist`) and `clear-prompt-history` (`clearphist`) inspect and reset what the command prompt remembers. See [scripting.md](scripting.md)
- **Session groups** : `set -g session-group <name>` or `psmux server -g <name>` links sessions, surfaced by `#{session_group}`, `#{session_group_size}`, and `#{session_grouped}`. See [scripting.md](scripting.md)
- Paste buffers, named registers, `display-message` with format variables
- Server namespaces via `-L` for running isolated psmux instances
- Command chaining with `;` for multi-step bindings
- `switch-client` for programmatic session switching
- `break-pane` and `join-pane` for pane reorganization
- `wait-for` with lock/signal/unlock for cross-pane synchronization
- `confirm-before` for user confirmation dialogs

See [scripting.md](scripting.md) for full command reference and examples, and
[tmux_args_reference.md](tmux_args_reference.md) for per-command flags.

## Session Persistence

- psmux session servers survive SSH disconnects and terminal crashes
- Detach with `Prefix + d`, reconnect with `psmux attach` from any terminal
- Warm sessions (`set -g warm on`, default) pre-spawn background servers for instant session creation
- Use [psmux-resurrect](https://github.com/psmux/psmux-plugins/tree/main/psmux-resurrect) to save/restore sessions across reboots
- Use [psmux-continuum](https://github.com/psmux/psmux-plugins/tree/main/psmux-continuum) for automatic periodic save/restore

## Session Switching

- **Prefix + s** opens an interactive session/window/pane tree chooser
- **Prefix + (** and **Prefix + )** cycle through sessions
- `switch-client -t sessionname` switches to a named session
- `switch-client -l` returns to the last (most recently used) session
- Create multiple sessions with `new-session -s name` and switch freely between them

## Display Panes Overlay

- **Prefix + q** shows numbered overlays on all panes for quick selection
- Press a number key to jump to that pane instantly
- Numbers respect `pane-base-index` (e.g., starts from 1 if configured)
- Overlay auto-dismisses after `display-panes-time` milliseconds (default: 1000ms)
- Only single-digit pane numbers (0 through 9) can be selected by keypress

## Nesting Prevention

- psmux automatically detects when running inside an existing session and blocks the launch, so you never end up with a psmux inside a psmux
- This matches tmux behavior, where nesting requires explicitly unsetting `$TMUX`
- To create a new session from inside psmux, use the command prompt (`Prefix + :`) and type `new-session`
- Set `PSMUX_ALLOW_NESTING=1` in the pane's environment when you genuinely want to nest

## Dead Pane Handling

- `set -g remain-on-exit on` keeps panes visible after their process exits
- Dead panes display their final output for inspection
- `respawn-pane` restarts the shell or a new command in a dead pane
- Useful for monitoring long-running processes that may crash

## Command Prompt

- **Prefix + :** opens a command prompt at the bottom of the screen
- Full cursor movement (arrow keys, Home, End) within the command line
- Command history (Up/Down arrows recall previous commands)
- Any psmux/tmux command can be typed and executed interactively
- Supports `source-file`, `set-option`, `split-window`, `list-commands`, and every other psmux command
- Prompt history is persistent and inspectable with `show-prompt-history`

## Claude Code Agent Teams

- First-class support for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) teammate pane spawning
- Automatically sets `TMUX`, `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS`, and teammate mode
- Each agent gets its own visible pane with full terminal output
- No extra configuration needed: start psmux, run `claude`, and ask it to create a team

See [claude-code.md](claude-code.md) for detailed setup and troubleshooting.

## CJK and IME Input

- Full support for Chinese, Japanese, and Korean character input
- IME composition handled with minimal latency (paste-detection heuristic tuned for rapid IME bursts)
- Korean IME input correctly handled without bracketed paste sequence injection
- CJK text pasting works reliably for any length
- UTF-8 multi-byte characters (box-drawing, emoji, CJK) render correctly in ConPTY panes
- Characters outside the Basic Multilingual Plane (emoji, JIS X 0213 kanji such as `𠮟`) arrive whole
  whether pasted, committed by the IME or picked from the Win+. emoji panel, in panes and in psmux's
  own prompts, on every host. The console hands such a character over as one key record per UTF-16
  half; psmux pairs the halves itself because crossterm 0.29 drops them (#742)

## Interactive Choosers

- `choose-tree` (`Prefix + w`): browse and select sessions, windows, and panes interactively, with optional [live preview pane](preview.md) (`p` to toggle, `set -g choose-tree-preview on` to default on)
- `choose-session` (`Prefix + s`): browse sessions only, same live preview support
- `choose-buffer` (`Prefix + =`): pick from paste buffers with preview
- `choose-client`: view connected clients
- `customize-mode`: interactive options editor
- **Digit-jump** (all pickers): type a number and press `Enter` to jump directly to that row (1-based). A `go to N` indicator appears at the bottom; `Backspace` edits the number, `Esc` cancels. Every row is numbered so the mapping is visible at a glance. See [keybindings.md](keybindings.md#picker-navigation-choose-session-choose-tree-choose-buffer-list-keys-customize) for the full key reference.

## Multi-Shell Support

- **PowerShell 7** (default), PowerShell 5, cmd.exe
- **Git Bash**, WSL, nushell, and any Windows executable
- Sets `TERM=xterm-256color`, `COLORTERM=truecolor` automatically
- Sets `TMUX` and `TMUX_PANE` env vars for tmux-aware tool compatibility

See [configuration.md](configuration.md) for `default-shell` and other options.

## Named Paste Buffers

- `set-buffer -b <name> "text"` to create a named buffer
- `show-buffer -b <name>` to read it back
- `paste-buffer -b <name>` to paste into the active pane
- `delete-buffer -b <name>` to remove it
- Named buffers are separate from the anonymous buffer stack
- Useful for structured data exchange between automation steps

## Diagnostics

- **Debug logging subsystem** : eleven independent loggers, all off by default, each writing a timestamped and capped log covering one subsystem (client rendering, styles, input, server, session registry, mouse, SSH input, latency, popups, warm pool, auth handshake). Turn one on with a single environment variable, for example `$env:PSMUX_INPUT_DEBUG = "1"`
- **Always on failure records** : `server-startup.log` captures the real reason a server could not start, including the Windows error and the exact path it tried to spawn, `config-warnings.log` captures config lines that were silently ignored, and `crash.log` captures a panic with a full backtrace
- **Predictable state layout** : every runtime file lives under `%USERPROFILE%\.psmux\`, so a bug report can include exactly what is on disk

See [diagnostics.md](diagnostics.md) for every log file, what each records, and what to attach to a bug report.

## Developer Integration and tmux API Compatibility

psmux is designed as a drop-in replacement for tmux on Windows at the API level:

- **Same CLI protocol**: 90+ tmux commands with identical flags, arguments, and output formats. Run `psmux list-commands` for the live list
- **Same stable IDs**: `$N` (session), `@N` (window), `%N` (pane) targeting works identically
- **Same control mode**: `-C`/`-CC` wire protocol with `%begin`/`%end` framing and async notifications
- **Same format engine**: 140+ format variables, conditionals, loops, regex, string ops
- **Same config**: Reads `~/.tmux.conf` directly
- **libtmux compatible**: The libtmux Python library works with psmux (see note on Windows encoding)
- **tmux.exe alias**: psmux installs a `tmux.exe` alias so existing scripts find it on the PATH

For a full developer integration guide with examples in Python, PowerShell, Node.js, Go, and Rust, see [integration.md](integration.md).

For the tmux command and feature compatibility matrix, see [compatibility.md](compatibility.md).
