```
╔═══════════════════════════════════════════════════════════╗
║   ██████╗ ███████╗███╗   ███╗██╗   ██╗██╗  ██╗            ║
║   ██╔══██╗██╔════╝████╗ ████║██║   ██║╚██╗██╔╝            ║
║   ██████╔╝███████╗██╔████╔██║██║   ██║ ╚███╔╝             ║
║   ██╔═══╝ ╚════██║██║╚██╔╝██║██║   ██║ ██╔██╗             ║
║   ██║     ███████║██║ ╚═╝ ██║╚██████╔╝██╔╝ ██╗            ║
║   ╚═╝     ╚══════╝╚═╝     ╚═╝ ╚═════╝ ╚═╝  ╚═╝            ║
║     Born in PowerShell. Made in Rust. 🦀                 ║
║          Terminal Multiplexer for Windows                 ║
╚═══════════════════════════════════════════════════════════╝
```

<p align="center">
  <strong>The native Windows tmux. Born in PowerShell, made in Rust.</strong><br/>
  Full mouse support · tmux themes · tmux config · 90+ commands · blazing fast
</p>

<p align="center">
  <a href="#installation">Install</a> ·
  <a href="#usage">Usage</a> ·
  <a href="docs/tutorials/getting-started-windows.md">Get Started</a> ·
  <a href="docs/use-cases.md">Use Cases</a> ·
  <a href="docs/README.md#tutorials">Tutorials</a> ·
  <a href="docs/claude-code.md">Claude Code</a> ·
  <a href="docs/architecture.md">Architecture</a> ·
  <a href="docs/features.md">Features</a> ·
  <a href="docs/compatibility.md">Compatibility</a> ·
  <a href="docs/performance.md">Performance</a> ·
  <a href="docs/plugins.md">Plugins</a> ·
  <a href="docs/keybindings.md">Keys</a> ·
  <a href="docs/scripting.md">Scripting</a> ·
  <a href="docs/configuration.md">Config</a> ·
  <a href="docs/mouse-ssh.md">Mouse/SSH</a> ·
  <a href="docs/faq.md">FAQ</a> ·
  <a href="#related-projects">Related Projects</a>
</p>

---

# psmux

**The real tmux for Windows.** Not a port, not a wrapper, not a workaround.

psmux is a **native Windows terminal multiplexer** built from the ground up in Rust. It uses Windows ConPTY directly, speaks the tmux command language, reads your `.tmux.conf`, and supports tmux themes. All without WSL, Cygwin, or MSYS2.

> 💡 **Tip:** psmux ships with `tmux` and `pmux` aliases. Just type `tmux` and it works!

👀 On Windows 👇

![psmux in action](demo.gif)

## Installation

### Using WinGet

```powershell
winget install psmux
```

### Using Cargo

```powershell
cargo install psmux
```

This installs `psmux`, `pmux`, and `tmux` binaries to your Cargo bin directory.

### Using Scoop

```powershell
scoop bucket add psmux https://github.com/psmux/scoop-psmux
scoop install psmux
```

### Using Chocolatey

```powershell
choco install psmux
```

### From GitHub Releases

Download the latest `.zip` from [GitHub Releases](https://github.com/psmux/psmux/releases) and add to your PATH.

### From Source

```powershell
git clone https://github.com/psmux/psmux.git
cd psmux
cargo build --release
```

Built binaries:

```text
target\release\psmux.exe
target\release\pmux.exe
target\release\tmux.exe
```

### Docker (build environment)

A ready-made Windows container with Rust + MSVC + SSH for building psmux:

```powershell
cd docker
docker build -t psmux-dev .
docker run -d --name psmux-dev -p 127.0.0.1:2222:22 -e ADMIN_PASSWORD=YourPass123! psmux-dev
ssh ContainerAdministrator@localhost -p 2222
```

See [docker/README.md](docker/README.md) for full details.

### Requirements

- Windows 10 or Windows 11
- **PowerShell 7+** (recommended) or cmd.exe
  - Download PowerShell: `winget install --id Microsoft.PowerShell`
  - Or visit: https://aka.ms/powershell

## Why psmux?

If you've used tmux on Linux/macOS and wished you had something like it on Windows, **this is it**. Split panes, multiple windows, session persistence, full mouse support, tmux themes, 90+ tmux-compatible commands (run `psmux list-commands` for the live list), 140+ format variables, 53 vim copy-mode keys. Your existing `.tmux.conf` works. Full details: **[docs/features.md](docs/features.md)** · **[docs/compatibility.md](docs/compatibility.md)**

## Usage

Use `psmux`, `pmux`, or `tmux`. They're identical:

```powershell
psmux                        # Start a new session
psmux new-session -s work    # Named session
psmux ls                     # List sessions
psmux attach -t work         # Attach to a session
psmux list-commands          # List every command psmux accepts
psmux --help                 # Show help
```

## Claude Code Agent Teams

psmux has first-class support for Claude Code agent teams. When Claude Code runs inside a psmux session, teammate agents automatically spawn in separate tmux panes instead of running in-process.

```powershell
psmux new-session -s work    # Start a psmux session
claude                       # Run Claude Code, agent teams just work
```

No extra configuration needed. Full guide: **[docs/claude-code.md](docs/claude-code.md)**

## Documentation

The full index with one line summaries of every page lives in **[docs/README.md](docs/README.md)**. AI assistants and crawlers can start from **[llms.txt](llms.txt)**.

### Tutorials

| Tutorial | What you will learn |
|----------|---------------------|
| **[Getting Started on Windows](docs/tutorials/getting-started-windows.md)** | Install, first session, prefix key, splits, detach and reattach, first config, Windows Terminal profile |
| **[Cross Platform tmux Scripts](docs/tutorials/cross-platform-tmux-scripts.md)** | Write one tmux automation script that runs unchanged on Windows (psmux), Linux and macOS (tmux) |
| **[Terminal AI Agents and TUIs](docs/tutorials/terminal-agents-and-tuis.md)** | Run Claude Code, Codex CLI, Gemini CLI, neovim, lazygit and other TUIs in scripted multi pane layouts |
| **[Dev Environment Layouts](docs/tutorials/dev-environment-layouts.md)** | Multi window and multi pane project sessions, layout presets, bootstrap scripts, save and restore |

### Guides and reference

| Topic | Description |
|-------|-------------|
| **[Windows Use Cases](docs/use-cases.md)** | Practical Windows scenarios: startup services, AI agents, dashboards, remote admin |
| **[Architecture](docs/architecture.md)** | How psmux multiplexes natively: client/server, one ConPTY per pane, input and output paths, warm pool |
| **[Features](docs/features.md)** | Full feature list: mouse, copy mode, layouts, format engine |
| **[Compatibility](docs/compatibility.md)** | tmux command/config compatibility matrix |
| **[Performance](docs/performance.md)** | Benchmarks and optimization details |
| **[Key Bindings](docs/keybindings.md)** | Default keys and customization |
| **[Scripting](docs/scripting.md)** | Commands, hooks, targets, pipe-pane |
| **[Command Reference](docs/tmux_args_reference.md)** | Per-command flag tables |
| **[Configuration](docs/configuration.md)** | Config files, options, environment variables |
| **[Multi-Shell](docs/multi-shell.md)** | Running pwsh, cmd, Git Bash, WSL, and nushell side by side |
| **[Pane Titles](docs/pane-titles.md)** | Pane titles, border labels, and why pwsh shows a path |
| **[Chooser Preview](docs/preview.md)** | Live preview pane in choose-tree and choose-session |
| **[Warm Sessions](docs/warm-sessions.md)** | The pre-spawned server behind instant session creation |
| **[Diagnostics](docs/diagnostics.md)** | Debug logs, crash logs, state files, what to attach to a bug report |
| **[Plugins & Themes](docs/plugins.md)** | Plugin ecosystem: Catppuccin, Dracula, Nord, and more |
| **[Mouse Over SSH](docs/mouse-ssh.md)** | SSH mouse support and Windows version requirements |
| **[Control Mode](docs/control-mode.md)** | The `-C` / `-CC` wire protocol for IDE and plugin authors |
| **[iTerm2 Control Mode](docs/iterm2-control-mode.md)** | Driving psmux from the iTerm2 tmux gateway over SSH |
| **[Developer Integration](docs/integration.md)** | Driving psmux from Python, Node.js, Go, Rust, and libtmux |
| **[Claude Code](docs/claude-code.md)** | Agent teams integration guide |
| **[FAQ](docs/faq.md)** | Common questions and answers |

## Related Projects

<table>
  <tr>
    <td align="center" width="50%">
      <a href="https://github.com/psmux/pstop">
        <img src="https://raw.githubusercontent.com/psmux/pstop/master/pstop-demo.gif" width="400" alt="pstop demo" /><br/>
        <b>pstop</b>
      </a><br/>
      <sub>htop for Windows: real-time system monitor with per-core CPU bars, tree view, 7 color schemes</sub><br/>
      <code>cargo install pstop</code>
    </td>
    <td align="center" width="50%">
      <a href="https://github.com/psmux/psnet">
        <img src="https://raw.githubusercontent.com/psmux/psnet/master/image.png" width="400" alt="psnet screenshot" /><br/>
        <b>psnet</b>
      </a><br/>
      <sub>Real-time TUI network monitor: live speed graphs, connections, traffic log, packet sniffer</sub><br/>
      <code>cargo install psnet</code>
    </td>
  </tr>
  <tr>
    <td align="center" width="50%">
      <a href="https://github.com/psmux/Tmux-Plugin-Panel">
        <img src="https://raw.githubusercontent.com/psmux/Tmux-Plugin-Panel/master/screenshot.png" width="400" alt="Tmux Plugin Panel screenshot" /><br/>
        <b>Tmux Plugin Panel</b>
      </a><br/>
      <sub>TUI plugin & theme manager for tmux and psmux: browse, install, update from your terminal</sub><br/>
      <code>cargo install tmuxpanel</code>
    </td>
    <td align="center" width="50%">
      <a href="https://github.com/psmux/omp-manager">
        <img src="https://raw.githubusercontent.com/psmux/omp-manager/master/screenshot.png" width="400" alt="OMP Manager screenshot" /><br/>
        <b>OMP Manager</b>
      </a><br/>
      <sub>Oh My Posh setup wizard: browse 100+ themes, install fonts, configure shells automatically</sub><br/>
      <code>cargo install omp-manager</code>
    </td>
  </tr>
  <tr>
    <td align="center" width="50%">
      <a href="https://github.com/psmux/DeskVNC">
        <img src="https://psmux.github.io/media/DeskVNC/agent.jpg" width="400" alt="DeskVNC screenshot" /><br/>
        <b>DeskVNC</b>
      </a><br/>
      <sub>VNC, RDP and SSH client for Windows, macOS and Linux, with an MCP server so AI agents can drive remote desktops</sub><br/>
      <code>github.com/psmux/DeskVNC/releases</code>
    </td>
    <td align="center" width="50%">
      <a href="https://github.com/psmux/GodwinMix">
        <img src="https://psmux.github.io/media/GodwinMix/live-switching.jpg" width="400" alt="GodwinMix web UI" /><br/>
        <b>GodwinMix</b>
      </a><br/>
      <sub>Live video mixer in Rust: headless core, web UI, desktop app, MCP for agents</sub><br/>
      <code>docker compose up</code>
    </td>
  </tr>
  <tr>
    <td align="center" width="50%">
      <a href="https://github.com/psmux/TerminalMap">
        <img src="https://psmux.github.io/media/TerminalMap/zoom-berlin.jpg" width="400" alt="TerminalMap rendering Berlin" /><br/>
        <b>TerminalMap</b>
      </a><br/>
      <sub>OpenStreetMap vector tiles rendered in your terminal, as an app and a Rust library</sub><br/>
      <code>cargo install terminalmap</code>
    </td>
    <td align="center" width="50%">
      <a href="https://github.com/psmux/vigil">
        <img src="https://psmux.github.io/media/vigil/vigil-views.jpg" width="400" alt="vigil dashboard" /><br/>
        <b>vigil</b>
      </a><br/>
      <sub>Security dashboard for Linux servers: attack radar, every listening port with its risk, a 0 to 100 security score</sub><br/>
      <code>build from source</code>
    </td>
  </tr>
</table>

Every project, with live demos: **[psmux.github.io](https://psmux.github.io)**

## License

MIT

## Contributing

Contributions welcome: bug reports, PRs, docs, and test scripts via [GitHub Issues](https://github.com/psmux/psmux/issues).

Filing a bug? See [docs/diagnostics.md](docs/diagnostics.md) for the logs and state files to attach.

If psmux helps your Windows workflow, consider giving it a ⭐ on GitHub!

## psmux in Numbers

[![psmux live statistics: stars, forks, contributors, companies, universities and countries](https://psmux.pages.dev/stats-card.svg)](https://psmux.pages.dev)

---

<p align="center">
  Made with ❤️ for PowerShell using Rust 🦀
</p>
