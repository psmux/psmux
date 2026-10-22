# psmux Plugins and Themes: the tmux plugin ecosystem on Windows

psmux plugins are ports of the most popular tmux plugins, reimplemented in PowerShell so they run natively on Windows without bash, cygwin, or WSL. A psmux plugin works exactly the way a tmux plugin works: a script that calls the multiplexer's own CLI (`set-option`, `bind-key`, `set-hook`, `run-shell`) to configure it. The commands are identical to tmux; only the scripting language around them changed from bash to PowerShell. tmux themes that are plain option lines work in psmux verbatim.

**Key facts**

- Plugin manager: **ppm** (the tmux `tpm` equivalent), declared with `set -g @plugin '...'` and loaded with a single `run` line, installed with `Prefix + I`.
- Plugins live in `~/.psmux/plugins/<name>/` (or `$XDG_CONFIG_HOME/psmux/plugins/` as a fallback).
- Entry point: `<name>/<name>.ps1` (PowerShell) or `<name>/plugin.conf` (pure tmux option lines).
- psmux reads `@plugin` declarations itself during config parsing and sources the plugin's `plugin.conf` synchronously, so themes apply before the first frame (no flash).
- A tmux plugin's `.tmux` entry script on a `run` line runs with Git Bash when Git for Windows is installed; without it, only the script's plain `tmux` lines are applied.
- Browse plugins: [psmux-plugins](https://github.com/psmux/psmux-plugins). Install with a TUI: [Tmux Plugin Panel](https://github.com/psmux/Tmux-Plugin-Panel).

Related: [Configuration](configuration.md) · [Key Bindings](keybindings.md) · [Scripting](scripting.md) · [Dev environment layouts](tutorials/dev-environment-layouts.md)

## Plugin Repository

**Browse available plugins and themes:** [**psmux-plugins**](https://github.com/psmux/psmux-plugins)

**Install & manage plugins with a TUI:** [**Tmux Plugin Panel**](https://github.com/psmux/Tmux-Plugin-Panel), a terminal UI for browsing, installing, updating, and removing plugins and themes.

## Available Plugins

| Plugin | Description |
|--------|-------------|
| [ppm](https://github.com/psmux/psmux-plugins/tree/main/ppm) | Plugin manager (like tpm) |
| [psmux-sensible](https://github.com/psmux/psmux-plugins/tree/main/psmux-sensible) | Sensible defaults for psmux |
| [psmux-yank](https://github.com/psmux/psmux-plugins/tree/main/_trash/psmux-yank) | Windows clipboard integration (retired: psmux copies to the Windows clipboard natively, see below) |
| [psmux-resurrect](https://github.com/psmux/psmux-plugins/tree/main/psmux-resurrect) | Save/restore sessions |
| [psmux-continuum](https://github.com/psmux/psmux-plugins/tree/main/psmux-continuum) | Auto save/restore sessions (works with resurrect) |
| [psmux-pain-control](https://github.com/psmux/psmux-plugins/tree/main/psmux-pain-control) | Better pane navigation |
| [psmux-vim-navigator](https://github.com/psmux/psmux-plugins/tree/main/psmux-vim-navigator) | Prefix-less `Ctrl-h/j/k/l` pane navigation that passes through to vim, nvim and fzf |
| [psmux-prefix-highlight](https://github.com/psmux/psmux-plugins/tree/main/psmux-prefix-highlight) | Prefix key indicator |
| [psmux-battery](https://github.com/psmux/psmux-plugins/tree/main/psmux-battery) | Battery status in status bar |
| [psmux-cpu](https://github.com/psmux/psmux-plugins/tree/main/psmux-cpu) | CPU usage in status bar |
| [psmux-net-speed](https://github.com/psmux/psmux-plugins/tree/main/psmux-net-speed) | Network speed in status bar |
| [psmux-git-status](https://github.com/psmux/psmux-plugins/tree/main/psmux-git-status) | Git branch and status in status bar |
| [psmux-sidebar](https://github.com/psmux/psmux-plugins/tree/main/psmux-sidebar) | File tree sidebar |
| [psmux-logging](https://github.com/psmux/psmux-plugins/tree/main/psmux-logging) | Log pane output to files |

## Themes

| Theme | Description |
|-------|-------------|
| [Catppuccin](https://github.com/psmux/psmux-plugins/tree/main/psmux-theme-catppuccin) | Soothing pastel theme (Latte, Frappe, Macchiato, Mocha) |
| [Dracula](https://github.com/psmux/psmux-plugins/tree/main/psmux-theme-dracula) | Dark theme with vibrant colors |
| [Nord](https://github.com/psmux/psmux-plugins/tree/main/psmux-theme-nord) | Arctic, north bluish color palette |
| [Tokyo Night](https://github.com/psmux/psmux-plugins/tree/main/psmux-theme-tokyonight) | Clean dark theme inspired by Tokyo at night |
| [Gruvbox](https://github.com/psmux/psmux-plugins/tree/main/psmux-theme-gruvbox) | Retro groove color scheme |
| [Everforest](https://github.com/psmux/psmux-plugins/tree/main/psmux-theme-everforest) | Comfortable green based color scheme |
| [Kanagawa](https://github.com/psmux/psmux-plugins/tree/main/psmux-theme-kanagawa) | Dark theme inspired by Katsushika Hokusai |
| [One Dark](https://github.com/psmux/psmux-plugins/tree/main/psmux-theme-onedark) | Atom One Dark inspired theme |
| [Rose Pine](https://github.com/psmux/psmux-plugins/tree/main/psmux-theme-rosepine) | Soho vibes for the terminal |
| [Warm Burnout](https://github.com/psmux/psmux-plugins/tree/main/psmux-theme-warm-burnout) | Warm, low contrast theme (dark and light) |

## Quick Start

```powershell
# Install the plugin manager
git clone https://github.com/psmux/psmux-plugins.git "$env:TEMP\psmux-plugins"
Copy-Item "$env:TEMP\psmux-plugins\ppm" "$env:USERPROFILE\.psmux\plugins\ppm" -Recurse
Remove-Item "$env:TEMP\psmux-plugins" -Recurse -Force
```

Then add to your `~/.psmux.conf`:

```tmux
set -g @plugin 'psmux-plugins/ppm'
set -g @plugin 'psmux-plugins/psmux-sensible'
run '~/.psmux/plugins/ppm/ppm.ps1'
```

Press `Prefix + I` inside psmux to install the declared plugins. `Prefix + U` updates them, `Prefix + M` removes plugins that are no longer declared.

## How do psmux plugins work?

The mechanism is the tmux one. A tmux plugin is a bash script that runs `tmux set-option ...` and `tmux bind-key ...`. A psmux plugin is a PowerShell script that runs `psmux set-option ...` and `psmux bind-key ...`. Everything after the binary name is the same text.

```
tmux plugin (bash):         tmux  set-option -g mouse on
psmux plugin (PowerShell):  psmux set-option -g mouse on
```

The pieces of psmux that a plugin relies on, and where each lives in the source:

| Mechanism | What psmux does | Source |
|-----------|-----------------|--------|
| `set -g @plugin 'owner/name'` | The config parser stores the value as a user option, then looks for `~/.psmux/plugins/<name>/plugin.conf` and sources it on the spot. If there is no `plugin.conf` it looks for `<name>/<name>.ps1`. A script that only sets options and binds keys with literal text is applied as config lines; any other script is queued to run whole. With ppm declared (`set -g @plugin 'psmux-plugins/ppm'`) psmux does neither and leaves the script to ppm, as tpm alone loads plugins in tmux. XDG paths under `$XDG_CONFIG_HOME/psmux/plugins/` are tried as well. | `src/config.rs` (`@plugin` branch in the `set` handler, `parse_ps1_plugin_script`) |
| `run '~/.psmux/plugins/ppm/ppm.ps1'` in the config | A config `run`/`run-shell` line without `-b` runs the command once the session's window exists and holds the lines after it until the command ends, as tmux does; the command's own `psmux` calls are answered while it runs. With `-b` it is started and the next line runs. `~` expands to your profile, and a `~/.psmux/plugins/` path is redirected to the XDG directory when only that one exists. The child gets `PSMUX_TARGET_SESSION` so its `psmux` calls reach the server that is starting up. | `src/config.rs` (`run_shell_job_kind`, `parse_run_shell`), `src/server/connection.rs` (the `run-shell` arm), `src/util.rs` (`expand_run_shell_path`) |
| Queued `.ps1` plugin scripts | After the config is loaded the server runs each queued script with `pwsh -NoProfile -ExecutionPolicy Bypass -File <script>` (falls back to `powershell.exe` when pwsh is missing) and services the scripts' `set`/`show-options` requests for up to 5 seconds before the UI starts. Until the session's window exists, only option, key binding and `source-file` calls are answered: any other call (`set-buffer`, `display-message -p`) does nothing or reads empty. A warm standby runs none; the session that claims it starts them once it has answered the client, without waiting. A script gets `TMUX` and `PSMUX_TARGET_SESSION` for its session and psmux's directory on `PATH`, as a `run-shell` does. A `source-file` a script runs then applies the file's options at once; its commands run once the session's window exists, after the config's own. A `@plugin` line parsed later, after the config's first command or by a runtime `source-file`, starts its script at once, without waiting for it. | `src/server/mod.rs` (`pending_plugin_scripts`) |
| `run-shell` from a key binding, hook, or the CLI | The command is format expanded (`#{pane_current_path}` and friends) first. A `.ps1` path runs as `pwsh -NoProfile -ExecutionPolicy Bypass -File <path>`, which is what makes paths with spaces safe. A `pwsh`, `powershell`, or `cmd` prefix is honoured as written. Anything else is wrapped in `pwsh -NoProfile -Command` (or `powershell.exe`, then `cmd /c`, whichever exists). Without `-b` the output is shown in a popup; with `-b` the command is fire and forget, and a spawn failure is reported in the status line instead of being swallowed. | `src/commands.rs` (`build_run_shell_command`, `resolve_run_shell`, the `run-shell` arm) |
| `set -g @name value` and `show -gv @name` | Any option whose name starts with `@` is a user option. Plugins use them for configuration (`@resurrect-dir`) and for output (`@cpu_display`). | `src/config.rs`, `src/format.rs` |
| `#{@name}` in a format | User options expand inside status line formats, so a plugin can publish a value with `set -g @cpu_display '42%'` and a theme can place it with `#{@cpu_display}`. | `src/format.rs` |
| `#(command)` in the status line | Supported. In the status bar it is expanded asynchronously and refreshed every `status-interval` seconds (default 15, floor 1 second), so a slow command never stalls rendering. In one-shot contexts such as `display-message -p '#(hostname)'` it runs synchronously and returns real output. | `src/format.rs` (`run_shell_command`) |
| `if-shell 'cmd' 'then' 'else'` and `if-shell -F '#{...}'` | The condition runs in the same resolved shell as `run-shell` (so it is PowerShell syntax on Windows: `if-shell 'Test-Path C:\tools' ...`). `-F` evaluates a format instead of a process. | `src/commands.rs` (`if-shell` arm) |
| `set-hook -g <hook> '<command>'` | Global hooks fire from the server loop. The hooks that really fire are the table in [scripting.md](scripting.md#available-hook-events), which is the canonical list: `after-new-window`, `after-split-window`, `after-select-window`, `before-select-window`, `after-select-pane`, `after-select-layout`, `after-kill-pane`, `after-resize-pane`, `after-rename-window`, `after-rename-session`, `after-swap-pane`, `after-swap-window`, `after-rotate-window`, `after-break-pane`, `after-join-pane`, `after-respawn-pane`, `client-attached`, `client-detached`, `client-resized`, `client-session-changed`, `session-created`, `session-closed`, `pane-died`, `pane-exited`, `pane-focus-in`, `pane-focus-out`, `pane-mode-changed`, `pane-set-clipboard`, `window-linked`, `window-unlinked`, `window-closed`, `alert-activity`, `alert-silence`, `alert-bell`. `client-attached` and `session-created` fire once at server start so plugins that hook them initialise on the first attach; `session-created` fires ONLY then, so a plugin has to set it from a config file. `after-new-session`, `after-copy-mode`, `after-set-option`, `after-bind-key`, `after-unbind-key` and `after-source` are accepted by `set-hook` and never fire (issue #691). | `src/help.rs` (`hooks_lines`), `src/server/mod.rs` (the per request hook slot) |
| `display-message` | Plugins use `display-message` for toasts and `display-message -p` to read a format from a script; ppm uses `display-message -d <ms>` for its progress messages. | `src/commands.rs` |

Two things in that table are worth repeating because they differ from tmux in a helpful way:

1. **Themes apply synchronously.** tmux's tpm sources plugins asynchronously, so a themed status line can appear a moment after the default one. psmux sources a plugin's `plugin.conf` while it parses the line that declares it, so the first frame is already themed.
2. **Every plugin process is told which server to talk to.** `run-shell`, hooks, and queued plugin scripts all receive `TMUX` and `PSMUX_TARGET_SESSION`, so a plugin running under `-L` namespaces or a preview server never configures the wrong instance.

## Where do plugins live and how does ppm load them?

```
~/.psmux/plugins/
  ppm/                      # plugin manager
    ppm.ps1                 # entry point, loaded by the `run` line in your config
    plugin.conf             # the same three bindings as static config lines
    scripts/
      install_plugins.ps1   # Prefix + I
      update_plugins.ps1    # Prefix + U
      clean_plugins.ps1     # Prefix + M
  psmux-sensible/
    psmux-sensible.ps1
  psmux-resurrect/
    psmux-resurrect.ps1
    plugin.conf
    scripts/save.ps1, restore.ps1
  psmux-theme-nord/
    plugin.conf             # pure option lines, sourced by psmux itself
    psmux-theme-nord.ps1
```

What `ppm.ps1` does when the config's `run` line executes it:

1. Creates `~/.psmux/plugins` if it is missing.
2. Binds `Prefix + I`, `Prefix + U`, `Prefix + M` to its install, update and clean scripts (`run-shell 'pwsh -NoProfile -File "...install_plugins.ps1"'`).
3. Reads the declared plugins with `psmux show-options -g` (every `@plugin` value) and, as a fallback, by scanning `~/.psmux.conf`, `~/.psmuxrc`, `~/.tmux.conf`, or `~/.config/psmux/psmux.conf` for `set -g @plugin` lines.
4. For every declared plugin that is installed, runs its entry point: `<name>.ps1`, then `<name without psmux- prefix>.ps1`, then `plugin.ps1`, then `init.ps1`. A plugin that ships a `plugin.conf` is skipped here because psmux already sourced it at parse time, which keeps your own settings placed after the `@plugin` line from being overridden. psmux runs no entry script itself when ppm is declared, so each runs once.

`Prefix + I` resolves each `@plugin` spec and clones it:

| Spec | What ppm clones |
|------|-----------------|
| `psmux-plugins/psmux-sensible` | The `psmux/psmux-plugins` monorepo, extracting the `psmux-sensible` folder |
| `owner/repo` | `https://github.com/owner/repo.git` |
| `owner/repo/subdir` | `owner/repo`, extracting `subdir` |
| `https://...` or `git@...` | That URL directly |
| `psmux-sensible` (bare name) | Treated as `psmux-plugins/psmux-sensible` |
| any of the above with `#branch` | The same, with `git clone --branch <branch>` |

Git runs with the credential helper and terminal prompts disabled, so a misspelt spec fails with a clean message instead of an interactive sign-in window.

Note that `PSMUX_DATA_DIR` (which relocates psmux's registry and state files, see [Configuration](configuration.md)) does not move the plugin directory. Plugin paths are derived from `USERPROFILE` and `XDG_CONFIG_HOME` only.

## What does each plugin do on Windows?

**psmux-sensible** sets defaults with `set -go` (only if unset) so it never overrides your config: `escape-time 50` (tmux-sensible uses 0; 50 ms is safer on Windows so escape sequences are not split), `history-limit 50000`, `mouse on`, `mode-keys vi`, `focus-events on`, `display-time 2000`, `status-interval 5`, `base-index 1`, `pane-base-index 1`, `renumber-windows on`, `automatic-rename on`. It binds `Prefix + R` to reload the config, `Prefix + |` and `Prefix + -` to split, and `Shift+Left/Right` to switch windows without a prefix.

**psmux-resurrect** binds `Prefix + Ctrl-s` (save) and `Prefix + Ctrl-r` (restore). A save records every session, window and pane layout with exact geometry, the working directory of every pane, the active pane and window, zoom state, pane titles, and the running command. Restore recreates them. Options mirror tmux-resurrect: `@resurrect-dir` (default `~/.psmux/resurrect`), `@resurrect-capture-pane-contents 'on'`, `@resurrect-processes 'ssh python node'` (or `'false'` to restore no processes, `':all:'` to restore all). Saves rotate (the latest 20 are kept) and a save whose environment is unchanged is skipped. `@resurrect-overwrite 'on'` kills and recreates a session that already exists instead of skipping it.

**psmux-continuum** runs resurrect saves automatically every `@continuum-save-interval` minutes (default 15, `0` disables), restores the last environment at server start when `@continuum-restore 'on'` is set, and can register psmux to start at boot with `@continuum-boot 'on'`. Same option names as tmux-continuum.

**psmux-pain-control** adds `Prefix + h/j/k/l` navigation, `Prefix + Alt-h/j/k/l` resizing, and `|`, `\`, `-`, `_` splits. **psmux-vim-navigator** is the vim-tmux-navigator port: `Ctrl-h/j/k/l` without a prefix switch panes, or pass through when the pane's foreground program is vim, nvim, or fzf (psmux exposes the foreground process as `#{pane_current_command}`, which is what the plugin checks).

**Status line plugins** (psmux-cpu, psmux-battery, psmux-net-speed, psmux-git-status) gather data with `Get-CimInstance Win32_Processor`, `Win32_Battery`, `Get-NetAdapterStatistics` and `git` instead of `/proc`, `sysfs` or `acpi`, then publish it as user options. You place the values with format variables, for example:

```tmux
set -g status-right '#{@cpu_display} #{@ram_display} | %H:%M'
```

**psmux-prefix-highlight** reads the current `status-right`, prepends `#{?client_prefix,...}`, `#{?pane_in_mode,...}` and `#{?synchronize-panes,...}` segments, and writes it back. Same option names as tmux-prefix-highlight.

**psmux-logging** binds `Prefix + Alt-o` (toggle logging via `pipe-pane`), `Prefix + Alt-p` (screen capture), `Prefix + Alt-i` (full history capture), `Prefix + Alt-c` (clear history). **psmux-sidebar** toggles a directory tree pane with `Prefix + Tab`.

**psmux-yank** is retired (it lives in the repo's `_trash` folder). psmux writes to the Windows clipboard itself (`src/clipboard.rs`, called from copy mode): a mouse drag copies on release, and a copy mode yank copies the selection, with OSC 52 for the outer terminal when `set-clipboard` is not `off`. The plugin only wrapped `Set-Clipboard`, which the core no longer needs. Keep the `@plugin` line out of new configs.

## tmux plugin to psmux plugin parity

The configuration lines are the same. The differences are all things that only exist on one operating system.

| tmux plugin | psmux plugin | What differs on Windows |
|-------------|--------------|--------------------------|
| tpm | ppm | bash `tpm` script became `ppm.ps1`; clones with git the same way; `Prefix + I / U / M` are identical (tpm uses `alt-u` for clean, ppm uses `M`) |
| tmux-sensible | psmux-sensible | `escape-time` defaults to 50 instead of 0; `Prefix + R` reloads `~/.psmux.conf` |
| tmux-yank | (built in) | `xclip`/`pbcopy`/`wl-copy` became the native Windows clipboard; no plugin needed |
| tmux-resurrect | psmux-resurrect | save/restore scripts are `.ps1`; paths are Windows paths; same `@resurrect-*` options |
| tmux-continuum | psmux-continuum | interval timer runs from PowerShell; same `@continuum-*` options |
| tmux-pain-control | psmux-pain-control | identical bindings |
| vim-tmux-navigator | psmux-vim-navigator | process detection uses `#{pane_current_command}` (psmux reports the foreground console process) instead of `ps` |
| tmux-prefix-highlight | psmux-prefix-highlight | identical format variables |
| tmux-battery | psmux-battery | `acpi` / `pmset` became `Get-CimInstance Win32_Battery` |
| tmux-cpu | psmux-cpu | `/proc/stat` and `top` became `Get-CimInstance Win32_Processor` / `Win32_OperatingSystem`; values are published as `#{@cpu_display}` and `#{@ram_display}` |
| tmux-net-speed | psmux-net-speed | `/proc/net/dev` became `Get-NetAdapter` / `Get-NetAdapterStatistics` |
| tmux-logging | psmux-logging | `pipe-pane` sink is a PowerShell command instead of `cat >>` |
| tmux-sidebar | psmux-sidebar | uses `tree` when present, `Get-ChildItem` otherwise |
| catppuccin, dracula, nord, tokyonight, gruvbox, everforest, kanagawa, onedark, rosepine | psmux-theme-* | option lines are identical; the theme ships a `plugin.conf` that psmux sources natively |

For plugin authors, the [Plugin Developer Guide](https://github.com/psmux/psmux-plugins/blob/main/PLUGIN_DEVELOPER_GUIDE.md) has a bash to PowerShell translation table (`show-option -gqv` to `show-options -g -v`, `xclip` to `Set-Clipboard`, `pgrep` to `Get-Process`, `cat /proc/stat` to `Get-CimInstance Win32_Processor`, and so on).

## Can I run an unmodified tmux plugin under psmux?

Name the plugin's `.tmux` entry script on a `run` line, as tpm runs it:

```tmux
# ~/.psmux.conf
run '~/.tmux/plugins/tmux-sensible/sensible.tmux'
```

**With Git for Windows**, psmux runs the script with Git Bash, as tmux runs it with `/bin/sh`. Git Bash is the `bin\bash.exe` of the Git install that `git.exe` on `PATH` belongs to, else of `%ProgramFiles%\Git`, `%LOCALAPPDATA%\Programs\Git` or scoop's `git`. A `bash` that is only on `PATH` is not used: on Windows that is often WSL's, whose `tmux` is a Linux tmux. The script's `tmux ...` calls reach this session, because psmux's `tmux.exe` is first on the script's `PATH` and `TMUX` names the session. As in tmux, a `run` line without `-b` holds the lines after it until the script exits. `run-shell '~/.tmux/plugins/x/x.tmux'` from the CLI or a key binding runs the script the same way.

Verified with tmux-sensible, tmux-yank, tmux-prefix-highlight, tmux-resurrect, catppuccin, nord and dracula: sensible's options and keys apply and its macOS-only `default-command` does not, yank copies with `clip.exe`, and the themes apply. Whether the rest of a plugin works depends on what its helper scripts call. `sed`, `awk`, `grep`, `date` and `cut` exist in Git Bash; `xclip`, `pbcopy`, `acpi`, `/proc` and `sysfs` do not. A plugin that only shells out to `tmux` and coreutils runs; a plugin that reads Linux system files needs the PowerShell port.

**Without Git for Windows**, psmux reads the script and applies only what bash would surely do the same way: a `tmux ...` line at the top level of the script whose words are literal once the script's directory (`"$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"`) and its literal assignments are put in. A line inside a function, an `if` or a loop, or with any other `$`, is not applied, because applied without its condition or its value it does the wrong thing. When a script has such lines, a config warning says that it needs Git Bash. Companion `*_tmux.conf` files in the script's folder are sourced too. A theme whose script only sources its own files, such as catppuccin's, is applied; most other plugins do nothing but warn. The script is read in place, so the lines after it do not wait.

## Do tmux themes work without changes?

Yes, when the theme is option lines. Most themes are. Paste them into `~/.psmux.conf` or keep them in a file and `source-file` it. Verified verbatim with the Nord palette:

```tmux
set -g status-style "bg=#3b4252,fg=#d8dee9"
set -g status-left "#[bg=#81a1c1,fg=#2e3440,bold] #S "
set -g status-right "#[fg=#8fbcbb] %H:%M "
set -g pane-active-border-style "fg=#88c0d0"
set -g window-status-current-format "#[bg=#88c0d0,fg=#2e3440,bold] #I #W "
```

```powershell
psmux -f .\theme.conf new-session -d -s themed
psmux show-options -g -v status-style          # bg=#3b4252,fg=#d8dee9
psmux show-options -g -v pane-active-border-style   # fg=#88c0d0
```

`#[fg=...]`, `#[bg=...]`, `bold`, `#{?client_prefix,...}`, `#I`, `#W`, `#S`, `%H:%M` and the other strftime fields all render. See [Configuration](configuration.md) for the full list of style options and [Scripting](scripting.md) for format variables.

## How do I write my own psmux plugin?

A plugin is a folder with a PowerShell entry point named after the folder. This one adds a user option, a prefix-less key, and a status segment. It was tested from a path containing a space.

```powershell
# ~/.psmux/plugins/hello/hello.ps1
$PSMUX = (Get-Command psmux).Source

# A user option other config can read with #{@hello_greeting}
& $PSMUX set -g '@hello_greeting' 'hi from my plugin' 2>&1 | Out-Null

# A binding (no prefix) that shows the option
& $PSMUX bind-key -n M-h display-message '#{@hello_greeting}' 2>&1 | Out-Null

# Prepend a status segment, once
$right = (& $PSMUX show-options -g -v status-right 2>&1 | Out-String).Trim()
if ($right -notmatch 'hello') {
    & $PSMUX set -g status-right "#[fg=green][hello] $right" 2>&1 | Out-Null
}
```

Load it from your config:

```tmux
run '~/.psmux/plugins/hello/hello.ps1'
```

or, to test against a running server without editing the config:

```powershell
psmux run-shell 'C:/Users/you/.psmux/plugins/hello/hello.ps1'
psmux show -gv '@hello_greeting'                 # hi from my plugin
psmux show-options -g -v status-right            # #[fg=green][hello] ...
psmux list-keys | Select-String 'M-h'            # bind-key -T root M-h display-message #{@hello_greeting}
psmux display-message -p 'greeting=#{@hello_greeting}'
```

Rules that keep a plugin robust:

- **Quote option names that start with `@` in PowerShell.** `@name` is PowerShell's splatting syntax; unquoted, `psmux set -g @hello_greeting 'x'` reaches psmux as `set -g x` and fails with `set-option: empty value`. Write `'@hello_greeting'`. This is the single most common plugin bug.
- Use `set -go` for defaults you want the user to be able to override from their config.
- Find the binary with `Get-Command psmux, pmux, tmux` rather than hard coding a path; the Windows installers put it in different places.
- Bind helper scripts with `run-shell 'pwsh -NoProfile -File "<path>"'` or give `run-shell` the bare `.ps1` path; both are safe for paths with spaces. Use forward slashes in paths you embed in config strings.
- Reactions to events go through `set-hook -g`. Verified: a config line `set-hook -g after-new-window "run-shell -b '<dir>/hook.ps1'"` ran the script on `new-window`, and the script saw `PSMUX_TARGET_SESSION` set to the session name.
- If a plugin needs a long running collector (cpu, net speed), start it once from the entry point and publish results with `set -g '@x_display'`; the status line picks the value up on its next `status-interval` refresh.
- Ship a `plugin.conf` with your static `set`/`bind` lines when you can. psmux sources it synchronously at the `@plugin` line, so there is no startup flash and no dependency on ppm's load order.

The [Plugin Developer Guide](https://github.com/psmux/psmux-plugins/blob/main/PLUGIN_DEVELOPER_GUIDE.md) covers porting an existing tmux plugin step by step, and the psmux-plugins repo has a `tests/` folder with the pattern used to test resurrect end to end.

## Tmux Plugin Panel (TUI installer)

[Tmux Plugin Panel](https://github.com/psmux/Tmux-Plugin-Panel) (`tmuxpanel`) is a Rust TUI that browses, installs, updates, removes, and previews plugins and themes for both psmux and tmux, working from a registry file rather than hand edited config. Install with `cargo install tmuxpanel`, `choco install tmuxpanel`, `scoop install tmuxpanel`, or `winget install marlocarlo.tmuxpanel`.

## Troubleshooting

**A plugin script does nothing and there is no error.** With ppm declared, psmux leaves every plugin without a `plugin.conf` to ppm, and the config is missing the `run` line that loads ppm. Without ppm, psmux runs the plugin's `<name>.ps1` itself, so check that the script is at `~/.psmux/plugins/<name>/<name>.ps1`.

**`set-option: empty value for '...'` when a plugin sets an `@option`.** Unquoted `@name` in PowerShell (see the rules above). Quote it.

**Execution policy.** psmux runs `.ps1` files with `pwsh -NoProfile -ExecutionPolicy Bypass -File`, so a restrictive policy does not block plugin scripts started by psmux. If you run a plugin script by hand from a PowerShell prompt and get an execution policy error, `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` fixes that prompt only.

**`Prefix + I` does nothing.** ppm was not loaded, so the binding does not exist. Check with `psmux list-keys | Select-String ' I '`. The `run '~/.psmux/plugins/ppm/ppm.ps1'` line must be at the bottom of the config and `ppm.ps1` must exist at that path (`~/.config/psmux/plugins/ppm/` is used automatically when the classic path does not exist).

**Path with spaces.** Give `run-shell` the `.ps1` path directly, or wrap the whole command as `run-shell 'pwsh -NoProfile -File "C:/Program Files/x/plugin.ps1"'`. Both forms were verified. What fails is an unquoted path inside a `-Command` string.

**A theme flashes the default status line first.** The theme has no `plugin.conf` and relies on ppm to run its `.ps1`. Themes in the psmux-plugins repo ship a `plugin.conf`, which psmux applies synchronously; a third party theme can be converted by copying its `set` lines into a `plugin.conf` beside the script.

**The plugin configured the wrong server.** Only happens when a script ignores `PSMUX_TARGET_SESSION` and hard codes `-L` or `-t`. Let the environment variable route the calls.

**`run-shell` output pops up over the pane.** That is tmux behaviour for a foreground `run-shell` with output. Use `run-shell -b` for scripts that should stay silent, or send their output to `Out-Null`.

**A key bound to a slow `run-shell`.** The prefix indicator clears as soon as the key is pressed and the client keeps drawing and reading keys while the script runs; the output popup appears when it finishes. As in tmux, the rest of the binding waits for it: in `bind C-s run-shell 'save.ps1' \; display-message saved` the message shows only after `save.ps1` exits. One difference: tmux holds keys typed during a foreground `run-shell` until it ends, while psmux delivers them to the pane straight away.

## FAQ

**Do psmux plugins require bash, cygwin, or WSL?**
No. Plugins from psmux-plugins are PowerShell. PowerShell 7 (`pwsh`) is preferred and Windows PowerShell 5.1 is used when pwsh is not installed.

**Can I keep my `~/.tmux.conf` plugin section?**
The `set -g @plugin` lines are read from `~/.tmux.conf` too. Change the tpm `run` line to ppm's `run '~/.psmux/plugins/ppm/ppm.ps1'` and swap `tmux-plugins/tmux-sensible` for `psmux-plugins/psmux-sensible` and so on. Theme lines stay as they are.

**Where are user options stored?**
In the server's option table for the lifetime of the server. `psmux show-options -g` lists them along with the built in options; `psmux show -gv '@name'` prints one value.

**How often does `#(command)` in the status line run?**
Every `status-interval` seconds (default 15). Set `set -g status-interval 5` for status plugins that should refresh faster. psmux never runs the command more often than once per second.

**Which hooks can a plugin use?**
Run `psmux help` and look at the hooks section, or see the table above. `client-attached` and `session-created` fire at server start, so they are the right place for one time initialisation.

**Does a plugin installed on Linux tmux work on psmux and vice versa?**
The declarations do. A bash plugin runs on psmux only through Git Bash and only if it avoids Linux specific tools; a PowerShell plugin runs on Linux tmux only if pwsh is installed there and the plugin avoids Windows specific cmdlets. Themes and option based plugins are portable in both directions.
