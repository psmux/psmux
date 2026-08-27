# FAQ

**Q: Is psmux cross-platform?**
A: No. psmux is built exclusively for Windows using the Windows ConPTY API. For Linux/macOS, use tmux. psmux is the Windows counterpart.

**Q: Does psmux work with Windows Terminal?**
A: Yes! psmux works great with Windows Terminal, PowerShell, cmd.exe, ConEmu, and other Windows terminal emulators.

**Q: Why use psmux instead of Windows Terminal tabs?**
A: psmux offers session persistence (detach/reattach), synchronized input to multiple panes, full tmux command scripting, hooks, format engine, and tmux-compatible keybindings. Windows Terminal tabs can't do any of that.

**Q: Can I use my existing `.tmux.conf`?**
A: Yes! psmux reads `~/.tmux.conf` automatically. Most tmux config options, key bindings, and style settings work as-is.

**Q: Can I use tmux themes?**
A: Yes. psmux supports 20+ style options with 24-bit true color, 256 indexed colors, and text attributes (bold, italic, dim, etc.). Most tmux theme configs are compatible. See the Style Options table in [configuration.md](configuration.md) for the full set.

**Q: Can I use tmux commands with psmux?**
A: Yes! psmux includes a `tmux` alias. Commands like `tmux new-session`, `tmux attach`, `tmux ls`, `tmux split-window` all work. 90+ commands in total. Run `psmux list-commands` to see exactly what your build accepts.

**Q: How fast is psmux?**
A: Session creation takes < 100ms. New windows/panes add < 80ms overhead. The bottleneck is your shell's startup time, not psmux. Compiled with opt-level 3 and full LTO.

**Q: Does psmux support mouse?**
A: Full mouse support: click to focus panes, drag to resize borders, scroll wheel, click status-bar tabs, drag-select text (including tmux-like copy-on-release with `pwsh-mouse-selection on`), and right-click copy/paste paths. Plus VT mouse forwarding for TUI apps like vim, htop, and midnight commander.

**Q: The scroll wheel does nothing inside some full screen program. Why?**
A: Because that program never asked for the mouse, and psmux follows tmux here. tmux only writes a mouse report to a pane whose application has enabled a mouse mode (`input_key_mouse` in `input-keys.c` returns immediately otherwise), and its default `WheelUpPane` binding treats the alternate screen only as a reason NOT to fall through to copy mode. So over a full screen program with no mouse support the wheel is a no-op in tmux, and now in psmux too. Before psmux 3.3.9 psmux forwarded the report anyway, and a program that does not parse mouse reports read the bytes as keystrokes: htop opened its `Search:` prompt and typed the report into it, and codex lost its transcript ([#598](https://github.com/psmux/psmux/issues/598)). Turn the program's own mouse support on (`:set mouse=a` in vim and neovim, `--mouse` for `less`, the mouse setting in htop) and the wheel starts working again. Over a plain shell prompt the wheel still enters copy mode and scrolls psmux's own scrollback, unchanged.

**Q: The mouse works in some programs but not in Claude Code on Windows 10. Why?**
A: On Windows builds below 22523 (Windows 10 19041 and 19045, Windows Server 2019 and Server 2022 build 20348) conhost's inbound VT parser does not pass an SGR mouse report to the pane's child. psmux writes that report into the pane, and on those builds it dies inside conhost, so a program that reads the mouse as VT bytes on stdin never sees it. That covers node based TUIs such as Claude Code and anything reading raw stdin. Programs that read the mouse as console INPUT_RECORDs do get it, because psmux also injects a Win32 `MOUSE_EVENT` record straight into the pane's console input buffer, which skips that parser entirely. That covers crossterm and ratatui apps, Bubble Tea and other Go TUIs, PSReadLine, and native Windows TUIs, and since psmux 3.3.9 it covers clicks, releases and drags on those builds and not only the wheel. On 22523 and above both kinds of program work. This was measured on real 19045 hardware in [#597](https://github.com/psmux/psmux/issues/597): a crossterm app received every wheel notch while a node child reading stdin received zero bytes. There is no psmux level fix for the VT half, the data is lost inside conhost. Upgrading to a build of 22523 or later is the only way to get the mouse into a VT reading program.

**Q: What shells does psmux support?**
A: PowerShell 7 (default), PowerShell 5, cmd.exe, Git Bash, WSL, nushell, and any Windows executable. Change with `set -g default-shell <shell>`.

**Q: Is it stable for daily use?**
A: Yes. psmux is stress-tested with 15+ rapid windows, 18+ concurrent panes, 5 concurrent sessions, kill+recreate cycles, and sustained load, all with zero hangs or resource leaks.

**Q: PSReadLine predictions / intellisense / autocompletion (inline history suggestions) are disabled inside psmux. How do I enable them?**
A: Add `set -g allow-predictions on` to your `~/.psmux.conf`. This tells psmux to preserve your `PredictionSource` setting after initialization. If your profile sets `PredictionSource` explicitly, psmux respects that. If not, psmux restores the system default (typically `HistoryAndPlugin`). See the [PSReadLine Predictions](configuration.md#psreadline-predictions-intellisense--autocompletion) section in the configuration docs for details.

**Q: How do I use a custom config file?**
A: Use the `-f` flag: `psmux -f /path/to/config.conf`. This loads the specified file instead of the default search order.

**Q: How do I disable warm (pre-spawned) sessions?**
A: Add `set -g warm off` to your config, or set `$env:PSMUX_NO_WARM = "1"`. See [warm-sessions.md](warm-sessions.md) for details.

**Q: Can I set environment variables for panes?**
A: Yes. Use `psmux set-environment -g VARNAME value` to set env vars inherited by all new panes. Use `-gu` to unset. See [configuration.md](configuration.md) for details.

**Q: How do I mute the audible bell inside psmux?**
A: Add `set -g bell-action none` to your `~/.psmux.conf`. This silences both the audible beep and the status bar bell flag. To keep the visual flag but mute the sound, this is not currently split into separate controls. See the [Bell](configuration.md#bell) section in the configuration docs.

**Q: Does psmux work with Claude Code agent teams?**
A: Yes, first-class support. Start psmux, run `claude` inside a pane, and ask Claude to create a team. psmux automatically sets the required environment variables and injects `--teammate-mode tmux`. Each teammate agent gets its own visible pane. See [claude-code.md](claude-code.md) for details.

**Q: Do CJK characters (Chinese/Japanese/Korean) and IME input work?**
A: Yes. CJK character input, IME composition, and pasting CJK text all work correctly. The paste detection heuristic is tuned to avoid misidentifying rapid IME bursts as clipboard pastes, keeping IME input latency minimal.

**Q: Can I save and restore sessions across reboots?**
A: Yes, using the [psmux-resurrect](https://github.com/psmux/psmux-plugins/tree/main/psmux-resurrect) plugin. For automatic periodic save/restore, pair it with [psmux-continuum](https://github.com/psmux/psmux-plugins/tree/main/psmux-continuum). See [plugins.md](plugins.md) for setup.

**Q: Do sessions survive SSH disconnects?**
A: Yes. The psmux session server persists even when your SSH connection drops. After reconnecting, run `psmux attach` to reattach to your sessions.

**Q: How do I reload my config without restarting psmux?**
A: Press `Prefix + :` to open the command prompt, then type `source-file ~/.psmux.conf`. You can also run `psmux source-file ~/.psmux.conf` from another terminal. Either way, all options, key bindings, and styles are re-applied immediately. To make it a one-key action, bind it: `bind-key R source-file ~/.psmux.conf \; display-message "Config reloaded"`. If a reloaded line seems to have been ignored, check `~/.psmux/config-warnings.log`, described in [diagnostics.md](diagnostics.md).

**Q: How do I run commands from inside a psmux session?**
A: Press `Prefix + :` to open the command prompt. Type any command (e.g. `split-window -h`, `new-window -n logs`, `set -g status-style "bg=blue"`). You can also run `list-commands` from the prompt to see all available commands.

**Q: How do I switch between sessions?**
A: Press `Prefix + s` to open the interactive session chooser. Use arrow keys to navigate and Enter to select. You can also use `Prefix + (` and `Prefix + )` to cycle through sessions, or `switch-client -t sessionname` from the command prompt.

**Q: How do I split a pane with a specific size?**
A: Use the `-p` flag with a percentage: `split-window -v -p 30` gives the new pane 30% of the space. This works with both `-v` (vertical) and `-h` (horizontal) splits.

**Q: How do I open a new pane in the same directory?**
A: Use `split-window -c "#{pane_current_path}"`. You can bind this in your config for convenience: `bind-key '"' split-window -v -c "#{pane_current_path}"`.

**Q: How do I prevent psmux from nesting inside itself?**
A: psmux automatically detects when it is already running inside a psmux session and prevents accidental nesting. If you try to start `psmux` inside an existing session, it will warn you instead of creating a nested instance. To explicitly create a new session from within psmux, use the command prompt (`Prefix + :`) and type `new-session`.

**Q: How do I keep a pane open after its process exits?**
A: Add `set -g remain-on-exit on` to your config. When a process exits, the pane stays visible with its last output. Use `respawn-pane` (or `respawn-pane -k`) to restart the process in that pane.

**Q: How do I make pane numbers start from 1 instead of 0?**
A: Add `set -g pane-base-index 1` to your config. This affects the `Prefix + q` display panes overlay and pane target numbering. For windows, use `set -g base-index 1`.

**Q: How do I set a window name that does not get overwritten?**
A: Use the `-n` flag when creating: `new-window -n "myname"`. This automatically disables `automatic-rename` for that window. If you renamed a window with `Prefix + ,` and it keeps getting overwritten, add `set -g automatic-rename off` to your config or set it per-window with `set -w automatic-rename off`.

**Q: How do I use PSReadLine ListView (dropdown suggestions) inside psmux?**
A: First, add `set -g allow-predictions on` to your `~/.psmux.conf`. Then in your PowerShell profile, set `Set-PSReadLineOption -PredictionViewStyle ListView`. Without `allow-predictions on`, psmux resets PSReadLine settings during initialization.

**Q: How do I get a live updating clock in my status bar?**
A: Use time format variables like `%H:%M:%S` in your status-right: `set -g status-right "%H:%M:%S %d-%b-%y"`. Then set `set -g status-interval 1` to refresh every second.

**Q: What is the difference between psmux, pmux, and tmux executables?**
A: They are all the same binary. psmux installs as `psmux.exe` with `pmux.exe` and `tmux.exe` as aliases. Use whichever name you prefer. The `tmux` alias lets existing tmux scripts and muscle memory work without changes.

**Q: Can I prevent psmux from entering copy mode on mouse scroll?**
A: Yes. Add `set -g scroll-enter-copy-mode off` to your config. Scroll events will be passed directly to the running application instead of entering copy mode.

**Q: Ctrl+V is intercepted by psmux even after unbinding. How do I let Ctrl+V reach neovim/vim?**
A: psmux has a Windows paste detection system that intercepts Ctrl+V at the client input layer, outside of the key binding system. `unbind-key -n C-v` alone will not stop it. Add `set -g paste-detection off` to your `~/.psmux.conf`. This forwards Ctrl+V to the child application so neovim can use it for visual block mode. You can still paste using Ctrl+Shift+V, right click, or Prefix + ]. See [configuration.md](configuration.md#paste-detection-ctrlv-passthrough) for details.

**Q: How do I chain multiple commands in a key binding?**
A: Use `\;` to separate commands: `bind-key M-s split-window -h \; select-pane -L`. The semicolon must be escaped in config files.

**Q: Can I run psmux inside psmux (nested sessions)?**
A: No. psmux prevents nesting to avoid UI confusion. This matches tmux behavior. If you need to connect to a remote psmux, use SSH from within a psmux pane to reach the remote session.

**Q: How do I use Ctrl+Space as my prefix key?**
A: Add to your config: `set -g prefix C-Space` followed by `unbind-key C-b` and `bind-key C-Space send-prefix`.

**Q: Why does `Prefix + I` not work for plugin install?**
A: Make sure you are pressing `Shift+I` (uppercase). Key bindings are case-sensitive: `I` and `i` are distinct bindings.

**Q: Does psmux work with Neovim/Vim?**
A: Yes. Ctrl+[, Shift+Tab, mouse events, and truecolor rendering all work correctly inside psmux panes. Set `set -g default-terminal "xterm-256color"` for best compatibility.

**Q: Why does my status bar show a file path instead of the hostname?**
A: PowerShell 7 automatically sets the terminal title to the current working directory on every prompt. If your config has `set -g allow-set-title on` and your status bar format uses `#{pane_title}` or `#T`, you will see that path. By default, `allow-set-title` is `off` in psmux so this does not happen. If you enabled it and want to revert, remove the `allow-set-title on` line from your config, replace `#{pane_title}` with `#H` in your status bar format, or add `$PSStyle.WindowTitle = ''` to your PowerShell profile. See [pane-titles.md](pane-titles.md) for full details.

**Q: Can I run multiple isolated psmux servers?**
A: Yes, use the `-L` flag for server namespaces: `psmux -L work new-session -s dev`. Each namespace gets its own server, sessions, and discovery files.

**Q: How many tmux commands does psmux support?**
A: 90+ tmux-compatible commands, covering session management, window and pane control, copy mode, popups and menus, interactive choosers, hooks, environment variables, pipe-pane, wait-for synchronization, and more. Rather than trusting a number in a doc, run `psmux list-commands`, which prints exactly what your build accepts. See [tmux_args_reference.md](tmux_args_reference.md) for per-command flags.

**Q: psmux is misbehaving. Where are the logs?**
A: Under `%USERPROFILE%\.psmux\`. Three files are written without you doing anything: `server-startup.log` (why a server failed to start, including the real Windows error and the path it tried to spawn), `config-warnings.log` (config lines that were silently ignored), and `crash.log` (a panic plus backtrace). Eleven more detailed loggers can be switched on one environment variable at a time, for example `$env:PSMUX_INPUT_DEBUG = "1"`. See [diagnostics.md](diagnostics.md) for the full list and for what to attach to a bug report.

---

## Developer Integration FAQ

**Q: Can I use psmux as a drop-in replacement for tmux in my project?**
A: Yes. psmux implements the same CLI protocol, commands, flags, and output formats as tmux. It also installs a `tmux.exe` alias, so scripts calling `tmux` will find psmux on the PATH without any code changes.

**Q: Does libtmux work with psmux?**
A: Yes. libtmux (the Python tmux API library) works with psmux because psmux implements the same commands and output formats. On Windows, you need to ensure UTF-8 encoding is used (set `PYTHONUTF8=1` or patch libtmux's `common.py` to add `encoding="utf-8"` to the Popen call). See [integration.md](integration.md) for details.

**Q: Why does libtmux return empty sessions on Windows?**
A: libtmux uses a Unicode separator character (U+241E) internally to parse format output. On Windows, Python defaults to cp1252 encoding which garbles this character. Set `$env:PYTHONUTF8 = "1"` before running your script, or patch libtmux to use `encoding="utf-8"`. This is an upstream libtmux issue, not psmux-specific.

**Q: Does psmux support control mode for IDE integrations?**
A: Yes. `psmux -CC` enters control mode with the same wire protocol as tmux (command/response framing with `%begin`/`%end`, async notifications for window/session/pane events, output escaping). See [control-mode.md](control-mode.md) for the full protocol reference.

**Q: What is `dump-state` and when should I use it?**
A: `dump-state` is a psmux extension command (not in tmux) that returns the entire session state as a JSON blob, including windows, panes, options, sizes, and screen content. Use it when building rich UIs or debugging integrations.

**Q: Do named paste buffers work?**
A: Yes. `set-buffer -b <name> "text"`, `show-buffer -b <name>`, `paste-buffer -b <name>`, and `delete-buffer -b <name>` all work. Named buffers are useful for structured data exchange between automation steps.

**Q: How do I handle encoding when reading psmux output in Python?**
A: Always specify `encoding="utf-8"` in `subprocess.Popen()` or `subprocess.run()` calls on Windows. Alternatively, set the `PYTHONUTF8=1` environment variable globally. psmux outputs UTF-8, but Python defaults to cp1252 on Windows.

**Q: Can I target windows by their stable ID (`@N`) instead of index?**
A: Yes. `psmux select-window -t @2` targets the window with stable ID 2 (not display index 2). Stable IDs are assigned when windows are created and never change during the server's lifetime.

**Q: What environment variables does psmux set?**
A: `TMUX` (session info), `TMUX_PANE` (pane ID like `%0`), `TERM=xterm-256color`, and `COLORTERM=truecolor`. Tools that check `$TMUX` to detect tmux will correctly detect psmux.

**Q: Where is the full developer integration guide?**
A: See [integration.md](integration.md) for examples in Python, PowerShell, Node.js, Go, and Rust, plus cross-platform project patterns, libtmux usage, control mode integration, and troubleshooting.
