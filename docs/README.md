# psmux Documentation: tmux for Windows

psmux is a native Windows terminal multiplexer written in Rust on ConPTY. It speaks the tmux command language, reads your `.tmux.conf`, and ships `tmux.exe` and `pmux.exe` aliases, so tmux knowledge, tmux scripts, tmux themes and tmux plugins carry over to PowerShell, cmd, Git Bash, WSL and nushell without WSL, Cygwin or MSYS2. This page is the index of every guide, tutorial and reference in the `docs/` folder.

New here? Start with [Getting Started on Windows](tutorials/getting-started-windows.md), then skim [Windows Use Cases](use-cases.md). Coming from tmux? Read [Compatibility](compatibility.md) and [Cross Platform tmux Scripts](tutorials/cross-platform-tmux-scripts.md).

## Tutorials

Step by step, tested on the shipped binary, copy pasteable.

| Tutorial | What you will learn |
|----------|---------------------|
| [Getting Started on Windows](tutorials/getting-started-windows.md) | Install channels, first session, prefix key, splits and windows, detach and reattach, copy mode and the Windows clipboard, a first `~/.psmux.conf`, a Windows Terminal profile that opens straight into a session |
| [Cross Platform tmux Scripts](tutorials/cross-platform-tmux-scripts.md) | Write one bash or PowerShell automation script against the tmux command language and run it unchanged on Windows through psmux and on Linux or macOS through tmux; quoting, `--`, exit codes, format strings, a portability checklist |
| [Terminal AI Agents and Multi Pane TUIs](tutorials/terminal-agents-and-tuis.md) | Run Claude Code, Codex CLI, Gemini CLI, aider and TUIs such as neovim, lazygit, btop and yazi in panes; drive them with `send-keys`, read them with `capture-pane`, log them with `pipe-pane`, restart them with `respawn-pane` |
| [Dev Environment Layouts](tutorials/dev-environment-layouts.md) | Design multi window and multi pane project sessions, layout presets and sizing, synchronize-panes, bootstrap scripts, saving and restoring layouts |

## How psmux works

| Page | Description |
|------|-------------|
| [Architecture](architecture.md) | Client and server model, one ConPTY per pane, the VT parser and screen model, input and output paths, the warm pane pool, process priority, and why native multiplexing beats tmux inside WSL |
| [Performance](performance.md) | Measured session, split and command latencies, memory per pane, what dominates startup, and how to measure it yourself |
| [Warm Sessions](warm-sessions.md) | The pre spawned server and spare shell behind instant session and pane creation |
| [Features](features.md) | Full feature list: mouse, copy mode, layouts, popups, menus, format engine |
| [Compatibility](compatibility.md) | tmux command and option compatibility matrix |

## Using psmux

| Page | Description |
|------|-------------|
| [Windows Use Cases](use-cases.md) | Boot time services, jobs that survive logout, ops dashboards, SSH administration, AI agents in parallel, reproducible dev environments, driving interactive programs, logging scheduled jobs |
| [Key Bindings](keybindings.md) | Default keys, copy mode keys, key tables, `bind-key` customisation |
| [Configuration](configuration.md) | Config file search order, every option, `PSMUX_*` environment variables |
| [Plugins and Themes](plugins.md) | The psmux plugin ecosystem: how plugins run, tmux plugin parity, themes, writing a plugin in PowerShell |
| [Multi Shell](multi-shell.md) | pwsh, cmd, Git Bash, WSL and nushell panes side by side |
| [Pane Titles](pane-titles.md) | Pane titles, border labels, OSC sequences, why pwsh shows a path |
| [Chooser Preview](preview.md) | The live preview pane in `choose-tree` and `choose-session` |
| [Mouse Over SSH](mouse-ssh.md) | Mouse support on remote Windows servers and the Windows build requirements |
| [Scrollback Client](scrollback.md) | `-f scrollback`: the pane's own output goes to the host terminal, so its scrollback works over SSH |
| [Claude Code](claude-code.md) | Claude Code agent teams: teammates spawn into psmux panes automatically |
| [FAQ](faq.md) | Common questions: mouse and wheel behaviour, Windows builds, shells, colours |
| [Diagnostics](diagnostics.md) | Debug logs, crash logs, state files, measuring the terminal around you, what to attach to a bug report |

## Scripting and integration

| Page | Description |
|------|-------------|
| [Scripting and Automation](scripting.md) | Commands, targets, hooks, buffers, `pipe-pane`, `wait-for`, format variables |
| [Command Reference](tmux_args_reference.md) | Per command flag tables |
| [Control Mode](control-mode.md) | The `-C` and `-CC` wire protocol for IDE and plugin authors |
| [iTerm2 Control Mode](iterm2-control-mode.md) | Driving psmux from the iTerm2 tmux gateway over SSH |
| [Developer Integration](integration.md) | Driving psmux from Python (libtmux), Node.js, Go and Rust |

## For AI assistants and crawlers

The repository root carries an [llms.txt](../llms.txt) that summarises psmux and links every page above with a one line description. Every page in `docs/` opens with a short statement of what it covers, uses question shaped headings, and ends with a FAQ, so answers can be quoted directly.

## Frequently asked questions

**Q: Is psmux a port of tmux or a wrapper around WSL?**
Neither. psmux is an independent Rust implementation of the tmux command language and behaviour on top of Windows ConPTY. It does not need WSL, Cygwin or MSYS2. See [Architecture](architecture.md).

**Q: Will my existing `.tmux.conf` and tmux scripts work?**
Yes for the large majority of options, bindings and commands; the [Compatibility](compatibility.md) matrix lists what is supported and [Cross Platform tmux Scripts](tutorials/cross-platform-tmux-scripts.md) covers the Windows specific differences (paths, shell quoting, default shell).

**Q: Where do I report a bug or ask a question?**
Open an issue at https://github.com/psmux/psmux/issues and attach what [Diagnostics](diagnostics.md) asks for.
