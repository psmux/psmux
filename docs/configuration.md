# Configuration

psmux reads its config on startup from the **first file found** (in order):

1. `~/.psmux.conf`
2. `~/.psmuxrc`
3. `~/.tmux.conf`
4. `~/.config/psmux/psmux.conf`

Config syntax is **tmux-compatible**. Most `.tmux.conf` lines work as-is.

You can also specify a custom config file path with the `-f` flag:

```powershell
# Use a specific config file instead of default search
psmux -f ~/.config/psmux/custom.conf

# Use an empty config (no settings loaded)
psmux -f NUL
```

This sets the `PSMUX_CONFIG_FILE` environment variable internally, which the server checks before searching the default locations.

## Basic Config Example

Create `~/.psmux.conf`:

```tmux
# Change prefix key to Ctrl+a
set -g prefix C-a

# Enable mouse
set -g mouse on

# Window numbering base (default is 0)
set -g base-index 1

# Customize status bar
set -g status-left "[#S] "
set -g status-right "%H:%M %d-%b-%y"
set -g status-style "bg=green,fg=black"

# Cursor style: block, underline, or bar
set -g cursor-style bar
set -g cursor-blink on

# Scrollback history
set -g history-limit 5000

# Prediction dimming (disable for apps like Neovim)
set -g prediction-dimming off

# Key bindings
bind-key -T prefix h split-window -h
bind-key -T prefix v split-window -v
```

## Choosing a Shell

psmux launches **PowerShell 7 (pwsh)** by default. You can change this:

```tmux
# Use cmd.exe
set -g default-shell cmd

# Use PowerShell 5 (Windows built-in)
set -g default-shell powershell

# Use PowerShell 7 (explicit path)
set -g default-shell "C:/Program Files/PowerShell/7/pwsh.exe"

# Use Git Bash
set -g default-shell "C:/Program Files/Git/bin/bash.exe"

# Use Nushell
set -g default-shell nu

# Use Windows Subsystem for Linux (via wsl.exe)
set -g default-shell wsl
```

You can also launch a window with a specific command without changing the default:

```powershell
psmux new-window -- cmd /K echo hello
psmux new-session -s py -- python
psmux split-window -- "C:/Program Files/Git/bin/bash.exe"
```

## Quoting Option Values

A quoted option value is stored exactly as written, including runs of spaces
and any leading or trailing space. This is the same in a config file as it is
on the command line:

```bash
# Both of these store the twelve spaces
set -g status-right "left            right"
```

```bash
psmux set -g status-right "left            right"
```

Details worth knowing:

- Quotes are only stripped when they wrap the whole value. `a"b"c` stores
  `a"b"c` literally.
- Inside double quotes, `\"` and `\\` are unescaped. Inside single quotes
  nothing is unescaped, so `'a\b'` stores `a\b`.
- An unquoted value is taken as written but with trailing whitespace removed.
- A trailing `# comment` is stripped, unless the `#` is inside quotes or
  escaped, so `"#{session_name}"` and `"#[fg=red]"` are safe.
- `#{p<n>:}` still pads at render time and remains useful for padding that
  should follow the rendered width rather than a fixed number of spaces.

## All Set Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `prefix` | Key | `C-b` | Prefix key |
| `prefix2` | Key | `none` | Secondary prefix key (optional) |
| `base-index` | Int | `0` | First window number |
| `pane-base-index` | Int | `0` | First pane number |
| `escape-time` | Int | `500` | Escape delay (ms) |
| `repeat-time` | Int | `500` | How long a key bound with `bind-key -r` keeps repeating without the prefix, in ms. Range `0` to `2000000`; `0` disables repeat |
| `history-limit` | Int | `2000` | Scrollback lines per pane |
| `priority` | Str | `above-normal` | Scheduling class for psmux's own processes: `normal`, `above-normal`, `high`. Pane shells and the programs you run are never raised. See [Process Priority](#process-priority) |
| `display-time` | Int | `750` | Message display time (ms) |
| `display-panes-time` | Int | `1000` | Pane overlay time (ms) |
| `status-interval` | Int | `15` | Status refresh (seconds) |
| `mouse` | Bool | `on` | Mouse support |
| `mouse-selection` | Bool | `on` | psmux's client-side drag selection. Set `off` to let in-pane TUI apps (opencode, nvim, etc.) handle their own mouse selection without psmux drawing on top |
| `mouse-selection-force` | Bool | `off` | Keep psmux drag selection active in apps that request mouse tracking. Plain clicks are replayed to the app; drags are copied by psmux |
| `scroll-enter-copy-mode` | Bool | `on` | Enter copy mode on mouse scroll (set `off` to disable) |
| `pwsh-mouse-selection` | Bool | `off` | tmux-like release-copy selection with word/line multi-click and pane-clipped extraction |
| `paste-detection` | Bool | `on` | Detect Ctrl+V paste from console host and send as bracketed paste (set `off` to let Ctrl+V reach child apps like neovim) |
| `choose-tree-preview` | Bool | `off` | Open `choose-session` / `choose-tree` pickers with the live preview pane already visible (saves pressing `p`). See [preview.md](preview.md) |
| `bold-is-bright` | Bool | `on` | Restore standard SGR codes for the 16 basic colors so the outer terminal renders `bold` as bright (matches a bare shell). Set `off` to keep explicit 256-indexed low colors byte-accurate. See [Bold Is Bright](#bold-is-bright-color-rendering) |
| `status` | Bool/Int | `on` | Show status bar (number = line count) |
| `status-position` | Str | `bottom` | `top` or `bottom` |
| `status-justify` | Str | `left` | `left`, `centre`, `right`, `absolute-centre` |
| `status-left-length` | Int | `10` | Max width of status-left |
| `status-right-length` | Int | `40` | Max width of status-right |
| `focus-events` | Bool | `off` | Pass focus events to apps |
| `alternate-screen` | Bool | `on` | Honour the DEC 47 / 1049 alternate screen. Set `off` so full screen program output lands in the scrollback instead of being discarded on exit. See [Alternate Screen](#alternate-screen) |
| `mode-keys` | Str | `emacs` | `vi` or `emacs` |
| `status-keys` | Str | | Editing style at the command prompt. Only the literal `vi` is checked; any other value (including unset) keeps emacs style editing |
| `copy-mode-line-numbers` | Str | `off` | Line number gutter in copy mode: `off`, `default`, `absolute`, `relative`, `hybrid`. See [Copy Mode Line Numbers](#copy-mode-line-numbers) |
| `wrap-search` | Bool | `on` | Wrap copy-mode searches around the ends of the scrollback. Set `off` to stop at the first or last match |
| `renumber-windows` | Bool | `off` | Auto-renumber windows on close |
| `automatic-rename` | Bool | `on` | Rename windows from foreground process |
| `monitor-activity` | Bool | `off` | Flag windows with new output |
| `monitor-silence` | Int | `0` | Seconds before silence flag (0=off) |
| `visual-activity` | Bool | `off` | Visual indicator for activity |
| `synchronize-panes` | Bool | `off` | Send input to all panes |
| `remain-on-exit` | Bool | `off` | Keep panes after process exits |
| `@kill-descendants` | Bool | `on` | Terminate a self-exited pane shell's background children (psmux extension) |
| `@mouse-force` | Bool | `off` | Pane scoped only (`set-option -p -t %N @mouse-force on`). Forward the wheel into this pane even though its application registered for the mouse through neither channel psmux can see. psmux normally forwards only to a pane that asked, either by sending a mouse DECSET or by holding `ENABLE_MOUSE_INPUT` on its console, and it keeps that authorization for as long as the process that earned it is alive, so a `node` child entering raw mode can no longer take it away. Some applications never register through either channel on Windows: conhost can swallow their DECSET before psmux parses it, and libuv raw mode leaves their console without the mouse bit. Set this for that pane. Leave it off for panes running programs that do not read the mouse, which receive the report as literal keystrokes (psmux extension) |
| `aggressive-resize` | Bool | `off` | Resize to smallest client |
| `window-size` | Str | `latest` | `largest`, `smallest`, `manual`, `latest` |
| `destroy-unattached` | Bool | `off` | Exit server when no clients attached |
| `exit-empty` | Bool | `on` | Exit server when all windows closed |
| `set-titles` | Bool | `off` | Update terminal title |
| `set-titles-string` | Str | | Terminal title format |
| `default-shell` | Str | `pwsh` | Shell to launch |
| `default-command` | Str | | Alias for default-shell |
| `word-separators` | Str | `" -_@"` | Copy-mode word delimiters |
| `activity-action` | Str | `other` | Action on window activity: `any`, `none`, `current`, `other` |
| `silence-action` | Str | `other` | Action on window silence: `any`, `none`, `current`, `other` |
| `bell-action` | Str | `any` | Bell action: controls audible bell forwarding and status bar flag (`any`, `none`, `current`, `other`) |
| `visual-bell` | Bool | `off` | Visual bell indicator |
| `allow-passthrough` | Str | `off` | Allow terminal passthrough sequences (`on`/`off`/`all`) |
| `allow-rename` | Bool | `on` | Allow programs to set window title via escape sequences |
| `allow-set-title` | Bool | `off` | Allow programs to set pane title via OSC 0/2 escape sequences (see [pane-titles.md](pane-titles.md)) |
| `allow-predictions` | Bool | `off` | Preserve PSReadLine prediction settings (see below) |
| `default-terminal` | Str | | Terminal type string (sets `TERM` env var in panes) |
| `update-environment` | Str | *(tmux defaults)* | Space-separated list of env vars to refresh on client attach |
| `warm` | Bool | `on` | Pre-spawn shells for instant window/pane creation (see [warm-sessions.md](warm-sessions.md)) |
| `copy-command` | Str | | Shell command for clipboard pipe |
| `set-clipboard` | Str | `on` | Clipboard interaction (`on`/`off`/`external`) |
| `main-pane-width` | Int | `0` | Main pane width in main-vertical layout |
| `main-pane-height` | Int | `0` | Main pane height in main-horizontal layout |
| `session-group` | Str | | Name of the session group this session joins. `none` or an empty value clears it. See [Session Groups](#session-groups) |
| `command-alias` | Str | | Define your own command alias as `alias=expansion`. See [Command Aliases](#command-aliases) |

### Style Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `status-left` | Str | `[#S] ` | Left status bar content |
| `status-right` | Str | `#{?window_bigger,[#{window_offset_x}#,#{window_offset_y}] ,}"#{=21:pane_title}" %H:%M %d-%b-%y` | Right status bar content. This is **not** empty by default: out of the box it renders the pane title followed by the time and date |
| `status-style` | Str | `bg=green,fg=black` | Status bar style |
| `status-left-style` | Str | | Left status style |
| `status-right-style` | Str | | Right status style |
| `status-bg` | Str | | Legacy convenience setter. Rewrites only the `bg=` part of `status-style` and leaves the rest intact |
| `status-fg` | Str | | Legacy convenience setter. Rewrites only the `fg=` part of `status-style` and leaves the rest intact |
| `message-style` | Str | `bg=yellow,fg=black` | Message style |
| `message-command-style` | Str | `bg=black,fg=yellow` | Command prompt style |
| `mode-style` | Str | `bg=yellow,fg=black` | Copy-mode highlight |
| `pane-border-style` | Str | | Inactive border style |
| `pane-active-border-style` | Str | `fg=green` | Active border style |
| `pane-border-hover-style` | Str | `fg=yellow` | Border style while the mouse hovers a draggable pane border |
| `pane-border-lines` | Str | `single` | Border glyph set: `single`, `double`, `heavy`, `simple`, `number`, `spaces`, `none`. See [Pane Border Lines](#pane-border-lines) |
| `pane-border-format` | Str | | Pane border format string (e.g. `#{pane_index}: #{pane_title}`) |
| `pane-border-status` | Str | | Pane border status position (`top`/`bottom`/`off`) |
| `copy-mode-line-number-style` | Str | `fg=brightblack` | Style of the copy-mode line number gutter |
| `copy-mode-current-line-number-style` | Str | `fg=yellow,bold` | Style of the line number on the copy-mode cursor row |
| `window-style` | Str | | Style applied to the contents of every pane |
| `window-active-style` | Str | | Style applied to the contents of the active pane |
| `popup-border-style` | Str | `fg=yellow` | Border style of `display-popup` overlays |
| `popup-border-lines` | Str | `single` | Popup border glyph set: `single`, `double`, `heavy`, `rounded` |
| `popup-style` | Str | | Accepted and stored, but never read. See [Accepted but Not Functional](#accepted-but-not-functional) |
| `clock-mode-colour` | Str | | Colour of the `clock-mode` digits |
| `clock-mode-style` | Str | | Accepted and stored, but never read. See [Accepted but Not Functional](#accepted-but-not-functional) |
| `window-status-format` | Str | `#I:#W#{?window_flags,#{window_flags}, }` | Inactive tab format. Equivalent to tmux's `#I:#W#F`, written out so a flagless window still reserves one space |
| `window-status-current-format` | Str | `#I:#W#{?window_flags,#{window_flags}, }` | Active tab format. Same shape as `window-status-format` |
| `window-status-separator` | Str | `" "` | Tab separator |
| `window-status-style` | Str | | Inactive tab style |
| `window-status-current-style` | Str | | Active tab style |
| `window-status-activity-style` | Str | `reverse` | Activity tab style |
| `window-status-bell-style` | Str | `reverse` | Bell tab style |
| `window-status-last-style` | Str | | Last-active tab style |

### Multi-line Status Bar (`status-format[]`)

psmux supports a multi-line status bar using the `status-format[]` array. Set the `status` option to a number to control how many lines the status bar displays:

```tmux
# Enable a 2-line status bar
set -g status 2

# Configure each line (0-indexed)
set -g status-format[0] "#[align=left]#S #[align=right]%H:%M"
set -g status-format[1] "#[align=left]#{W:#[range=window|#{window_index}]#I:#W #[norange],#[range=window|#{window_index}]#I:#W* #[norange]}"
```

The first line (`status-format[0]`) replaces the default status bar content. Additional lines stack below (or above, depending on `status-position`).

**Clickable window tabs.** In a custom `status-format[N]` line, tab click targets come from `#[range=window|N]`/`#[norange]` markers and nothing else: a window list written without them renders correctly but cannot be clicked, exactly as in tmux (tmux's shipped default `status-format[0]` carries these markers, which is why its window list is clickable out of the box). `N` is the window index itself, so `#[range=window|#{window_index}]` is the correct idiom at any `base-index`. Inside `#{W:inactive,current}` the comma separates the two loop arguments, so any other comma must be escaped as `#,` and combined styles are easier written as `#[fg=green]#[bold]` than `#[fg=green,bold]`. Ranges are honored on every status line, and clicks are hit-tested on every status row.

### Pane Border Labels

Show pane information on the border between panes:

```tmux
# Enable pane border labels at the top of each pane
set -g pane-border-status top

# Customize what the label shows
set -g pane-border-format " #{pane_index}: #{pane_title} [#{pane_current_command}] "

# Disable pane border labels
set -g pane-border-status off
```

Use `select-pane -T "title"` to set a pane title that appears in the border label. Clear a title with `select-pane -T ""`. The default pane title is the hostname, matching tmux convention.

> **Note:** PowerShell 7 automatically sets the pane title to the current working directory on every prompt via OSC escape sequences. If you see a file path in your pane border labels instead of the hostname, see [pane-titles.md](pane-titles.md) for details and options to control this.

### Pane Border Lines

`pane-border-lines` picks the glyph set psmux draws pane borders with. After the borders are drawn, psmux runs a junction pass that upgrades straight runs into proper corner and tee glyphs, so `double` and `heavy` borders join cleanly instead of showing mismatched crossings.

```tmux
# Default: thin single lines
set -g pane-border-lines single

# Double lines
set -g pane-border-lines double

# Thick lines
set -g pane-border-lines heavy

# ASCII only, for fonts without box drawing glyphs
set -g pane-border-lines simple

# Blank borders (the gap stays, the glyphs do not)
set -g pane-border-lines spaces

# No border glyphs at all
set -g pane-border-lines none
```

`number` is accepted for tmux compatibility and renders the same as `single`. Any unrecognised value also falls back to `single`.

### Copy Mode Line Numbers

Copy mode can draw a line number gutter down the left edge. It is off by default:

```tmux
# Absolute line numbers counted from the top of the scrollback
set -g copy-mode-line-numbers absolute

# Distance from the cursor row, vi style
set -g copy-mode-line-numbers relative

# Cursor row shows its absolute number, every other row shows the relative distance
set -g copy-mode-line-numbers hybrid

# Distance from the current scroll offset
set -g copy-mode-line-numbers default

# Turn the gutter off again
set -g copy-mode-line-numbers off
```

The gutter has two independent styles:

```tmux
# Every line number except the cursor row
set -g copy-mode-line-number-style "fg=brightblack"

# The line number on the cursor row
set -g copy-mode-current-line-number-style "fg=yellow,bold"
```

### Popup and Window Styling

`display-popup` overlays and the pane contents themselves can be styled separately from the borders around them:

```tmux
# Popup border colour and glyph set
set -g popup-border-style "fg=cyan"
set -g popup-border-lines rounded

# Background and foreground for every pane's contents
set -g window-style "fg=colour247,bg=colour236"

# ... and a brighter pair for the active pane, so focus is obvious
set -g window-active-style "fg=colour250,bg=black"

# Colour of the clock-mode digits (Prefix + t)
set -g clock-mode-colour cyan
```

`popup-border-lines` accepts `single` (the default), `double`, `heavy` and `rounded`. The values `none` and `simple` are accepted but render as plain single lines, because a popup always draws a border.

> **Note:** `popup-style` and `clock-mode-style` are accepted and stored but never read. See [Accepted but Not Functional](#accepted-but-not-functional).

### Bell

When a program inside a pane emits BEL (`\x07`), psmux forwards the bell character to your host terminal so you hear the audible beep. The `bell-action` option controls when this happens and when the status bar tab gets a bell flag (`!`):

```tmux
# Forward bell from any window (default)
set -g bell-action any

# Forward bell only from the active window
set -g bell-action current

# Forward bell only from non-active windows
set -g bell-action other

# Mute bell completely (no sound, no status bar flag)
set -g bell-action none
```

The `window-status-bell-style` option controls how the tab looks when flagged:

```tmux
set -g window-status-bell-style "fg=red,bold"
```

PowerShell example to test:

```powershell
# These should all produce an audible beep inside psmux:
Write-Host "`a"
[Console]::Beep()
[char]7
```

### Mouse Configuration

Mouse support is enabled by default. You can customize how the mouse interacts with psmux:

```tmux
# Disable mouse entirely (no click, scroll, or drag)
set -g mouse off

# Disable entering copy mode on mouse scroll
set -g scroll-enter-copy-mode off

# Enable tmux-like release-copy selection with pane clipping
# Double-click selects a word, triple-click selects a line
set -g pwsh-mouse-selection on
```

When `pwsh-mouse-selection` is `on`, releasing a left-drag copies the selected text immediately and clears the transient highlight. Right-click copy and `Ctrl+Shift+C` still work as explicit copy actions.

When `scroll-enter-copy-mode` is `off`, scrolling in a pane does not enter copy mode and instead passes scroll events directly to the running application. A drag selection made over such a scrolled-back view still converts to a copy-mode selection when it reaches the pane's top or bottom row, and keeps auto-scrolling in that direction, so text taller than the window can be selected in one gesture (see [features.md](features.md)).

#### Disabling psmux's drag selection (`mouse-selection`)

Some TUI applications render their own internal layouts (multiple columns, sidebars, panels) inside a single psmux pane. Examples include `opencode`, `lazygit`, `nvim` with split windows, and similar dashboards.

psmux's own client-side drag selection does not know about those internal layouts, so a left-click drag inside such an app draws a selection rectangle that crosses the app's internal columns instead of respecting them.

If you would rather have the application handle mouse selection itself, disable psmux's drag selection:

```tmux
# Let the app inside the pane handle its own mouse selection.
# psmux will no longer render its drag-selection rectangle.
set -g mouse-selection off
```

What still works when `mouse-selection` is `off`:

- Click on a pane to focus it
- Click on a window tab in the status bar to switch to it
- Mouse wheel scrolling and scroll-into-copy-mode
- Pane border drag-to-resize
- Mouse events being forwarded to applications that request mouse tracking (DECSET 1000/1002/1003), so `opencode`, `htop`, `nvim`, `claude`, etc. continue to receive their clicks and drags

What changes when `mouse-selection` is `off`:

- psmux no longer draws its own selection rectangle on left-click drag
- Right-click clipboard copy via psmux's selection is no longer triggered (selection never starts)
- The `pwsh-mouse-selection` word/line multi-click and release-copy behavior is suppressed too while `mouse-selection off` is in effect

To restore the default behaviour:

```tmux
set -g mouse-selection on
```

You can also toggle this at runtime without restarting:

```
psmux set-option -g mouse-selection off
psmux set-option -g mouse-selection on
```

This option is independent of `mouse` (which controls whether mouse events are received at all) and `pwsh-mouse-selection` (which only affects the style of the drag selection when it is active).

### Paste Detection (Ctrl+V Passthrough)

On Windows, the console host intercepts Ctrl+V, reads the clipboard, and injects the content as character events. psmux detects this pattern and reassembles it into a single bracketed paste for child applications. This is the `paste-detection` option and it is enabled by default.

If you use TUI applications like **neovim** or **vim** where Ctrl+V has a different meaning (visual block mode), the paste detection will intercept the keypress before it reaches the application. To let Ctrl+V pass through to the child app:

```tmux
# Disable paste detection so Ctrl+V reaches child apps
set -g paste-detection off
```

With paste detection off, you can still paste using:

* **Ctrl+Shift+V** (Windows Terminal default paste shortcut)
* **Right click** (paste in most terminals)
* **Prefix + ]** (psmux paste from buffer)
* **`psmux send-keys C-v`** from another terminal

> **Note:** `unbind-key -n C-v` alone is not sufficient to stop Ctrl+V interception because the paste detection operates outside the key binding system. You must use `set -g paste-detection off`.

### Live Preview in Choosers

`choose-session` (prefix + s) and `choose-tree` (prefix + w) include a live preview pane that mirrors the selected session or window in real time. By default it is hidden and you press `p` to toggle it. To make it visible by default:

```tmux
# Open all choosers with the preview pane already visible
set -g choose-tree-preview on
```

You can still press `p` inside the chooser to hide it for the current session. The setting is read once when the chooser opens, so changes to the option take effect immediately on the next open. See [preview.md](preview.md) for the full feature documentation.

### Bold Is Bright (color rendering)

Many terminals, including Windows Terminal, render bold text in one of the 16 basic ANSI colors as the brighter variant of that color. This is the common "bold is bright" behavior, and a bare shell gets it because it emits the standard SGR codes (`ESC[32m` for green, brightened by `ESC[1m`).

psmux renders its screen through ratatui and crossterm, and crossterm serializes all 16 basic colors as the 256-indexed form (`ESC[38;5;N`) instead of the standard `30`-`37` codes. Windows Terminal only applies "bold is bright" to the standard codes, not the 256-indexed form, so colored bold text like PowerShell's `$PSStyle` output looked muted with a heavier font ([#425](https://github.com/psmux/psmux/issues/425)). psmux rewrites those basic-color sequences back to the standard codes so bold renders bright, exactly matching a bare shell. This is on by default.

```tmux
# Default: basic colors get "bold is bright" (matches a bare shell)
set -g bold-is-bright on

# Opt out: pass crossterm output through untouched
set -g bold-is-bright off
```

There is one tradeoff. crossterm collapses a basic color (`ESC[32m`) and an explicit 256-indexed low color (`ESC[38;5;2m`) into the identical bytes, so the rewrite cannot tell them apart and brightens both. If a program you use deliberately emits the 256-indexed colors 0 through 15 and you need them to stay exactly as sent, set `bold-is-bright off`. With it off, both basic and explicit 256-indexed low colors are byte-accurate, and you give up "bold is bright" on the basic colors. This is the inherent limitation of crossterm's lossy encoding; real tmux does not have it because it never collapses the two forms.

The option applies from config, at runtime, and reports through `show-options` and `#{bold-is-bright}`:

```powershell
psmux set-option -g bold-is-bright off
psmux show-options -g bold-is-bright
psmux display-message -p '#{bold-is-bright}'
```

### Alternate Screen

Full screen programs (`nvim`, `less`, `htop`) switch the terminal to the alternate screen with DEC private mode 47 or 1049, draw over it, and switch back on exit. That is why your scrollback is untouched after you quit them. psmux honours this by default.

Setting `alternate-screen off` makes psmux ignore the switch, so everything those programs draw goes into the normal buffer and stays in the scrollback after they exit:

```tmux
# Default: full screen apps get their own screen and leave no trace
set -g alternate-screen on

# Keep full screen output in the scrollback instead
set -g alternate-screen off
```

The flag lives in each pane's terminal parser, and psmux patches live panes and the warm pane when you change it, so the new value applies immediately without restarting anything.

### Session Groups

A session group is a name shared by several sessions. psmux exposes the membership through format variables so a status bar or a script can tell grouped sessions apart:

```tmux
# Join this session to the "work" group
set -g session-group work

# Leave the group again
set -g session-group none
```

The group can also be named when the server is spawned:

```powershell
psmux server -g work
```

Grouped state is readable from any format string:

```powershell
psmux display-message -p '#{session_group} #{session_grouped} #{session_group_size}'
```

### Command Aliases

`command-alias` defines your own short name for a command:

```tmux
# "dev" now means "split-window -h"
set -g command-alias 'dev=split-window -h'

bind-key D dev
```

> **Note:** the alias is resolved by the server, so it works from a key binding, a config line, a hook, `run-shell` and the command prompt. It is **not** resolved by the command line front end, so `psmux dev` typed at a shell still fails with "unknown command". Wrap the CLI form in the real command name instead.

### Command Chaining

psmux supports tmux-style command chaining with the `;` operator. Multiple commands on a single line are executed sequentially:

```tmux
# Split and move focus in one binding
bind-key M-s split-window -h \; select-pane -L

# Create a development layout
bind-key D split-window -v -p 30 \; split-window -h \; select-pane -t 0
```

In config files, escape the semicolon with `\;` so it is not treated as a comment delimiter.

### Case-Sensitive Key Bindings

psmux distinguishes between lowercase and uppercase letters in key bindings, matching tmux behavior:

```tmux
# These are two different bindings:
bind-key t clock-mode           # Prefix + t (lowercase)
bind-key T choose-tree          # Prefix + Shift+T (uppercase)

# Uppercase bindings for plugin managers
bind-key I run-shell '~/.psmux/plugins/ppm/scripts/install_plugins.ps1'
bind-key U run-shell '~/.psmux/plugins/ppm/scripts/update_plugins.ps1'
```

### Ctrl+Space as Prefix

Multi-character key names like `Space`, `Enter`, `Tab`, and `Escape` are fully supported in prefix configuration:

```tmux
set -g prefix C-Space
unbind-key C-b
bind-key C-Space send-prefix
```

### psmux Extensions (Windows-specific)

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `prediction-dimming` | Bool | `off` | Dim predictive/speculative text |
| `cursor-style` | Str | `bar` | Cursor shape: `block`, `underline`, or `bar` |
| `cursor-blink` | Bool | `on` | Cursor blinking |
| `env-shim` | Bool | `on` | Inject Unix-compatible `env` function in PowerShell panes |
| `claude-code-fix-tty` | Bool | `on` | Patch Node.js process.stdout.isTTY for Claude Code |
| `claude-code-force-interactive` | Bool | `on` | Set CLAUDE_CODE_FORCE_INTERACTIVE=1 in panes |
| `@heal-crashed-panes` | Bool | `off` | Respawn a pane whose shell exits within a short grace window of being spawned, instead of closing the window. Only needed where PowerShell's non-PSReadLine fallback reader dies on its first ConPTY read |

`bold-is-bright` and `paste-detection` are psmux extensions too, but they are defined once in
[All Set Options](#all-set-options) rather than repeated here. Their behaviour is explained in
[Bold Is Bright](#bold-is-bright-color-rendering) and
[Paste Detection](#paste-detection-ctrlv-passthrough).

### Process Priority

Windows gives its foreground scheduling boost to whichever process owns the foreground window. The
psmux server owns no window at all, and the attach client draws inside a console window that the
terminal host owns, so neither of them ever receives that boost. On an idle or lightly loaded
machine this costs nothing. On a heavily oversubscribed one it means your keystrokes queue behind
every compute job on the box, and typing starts to lag.

`priority` sets the scheduling class of psmux's **own** processes, and nothing else:

```tmux
# Default: a small boost, enough to stay ahead of background compute
set -g priority above-normal

# Turn the boost off entirely
set -g priority normal

# For a machine you deliberately oversubscribe. Use sparingly
set -g priority high
```

Any other value is refused: `set -g priority realtime` exits nonzero with a message and leaves the
class where it was. Realtime is not offered at all, because a process at that class outranks most of
the kernel's own threads.

**What it applies to, exactly:**

| | |
|---|---|
| The server process | Immediately, the moment the option is set, and again at every server startup |
| The attach client | At client startup, from `PSMUX_PRIORITY` or from a `priority` line in your config file |
| A warm standby server | At its own startup, so a session that claims one gets the configured class |
| Pane shells and your programs | **Never.** A Windows child created with no explicit class flag gets `NORMAL_PRIORITY_CLASS` regardless of its parent, so nothing you run inside a pane is raised |

Two things it does not do. A `set -g priority` issued in one session does not reach a warm standby
that is already parked in the pool, and it does not retroactively change a client that is already
attached; both pick the value up the next time they start. And an already running client keeps its
class for its whole life, so use `PSMUX_PRIORITY` or a config file line if you want every client to
agree.

`PSMUX_PRIORITY` overrides the option, for both processes. That ordering is deliberate: it is the
escape hatch that lets you climb out of a configured value from the shell you start psmux in,
without editing a file. `show-options -g priority` always reports what the process is **actually**
running at, so if the environment won you will see the environment's value there.

```powershell
# This shell only
$env:PSMUX_PRIORITY = "normal"
psmux
```

Setting the class can be refused by a restricted token or by a job object that caps it. psmux treats
that as nothing to report: it carries on at whatever class it already had rather than failing to
start.

For reference, tmux has no equivalent option. It never sets a scheduling priority anywhere in its
source, because the Linux scheduler already favours a process that spends its life blocked on a
read. This is a Windows specific extension rather than a parity feature.

### Style Value Grammar

Every `*-style` option and every inline `#[...]` block in a format string uses the same comma separated grammar:

```tmux
set -g status-style "fg=white,bg=colour236,bold"
set -g status-left "#[fg=green,bold]#S#[default] "
```

**Colours.** `default` and `terminal` (both mean the terminal's own default), `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, the bright variants (`brightblack` through `brightwhite`, also spelled `bright-black`), `colour0` to `colour255` (the American spelling `color0` works too), `#RRGGBB`, `idx:N`, and `rgb:R,G,B`.

**Attributes.** `bold`, `dim`, `italics` (also `italic`), `underscore` (also `underline`), `double-underscore`, `curly-underscore`, `dotted-underscore`, `dashed-underscore`, `blink`, `reverse`, `hidden`, `strikethrough`.

**Underscore colour.** `us=colour` sets the colour of the underline on its own, separate from `fg=`:

```tmux
set -g message-style "curly-underscore,us=red"
```

The styled underscores and `us=` need a terminal that understands the SGR 4 subparameter forms (`4:2` through `4:5`) and SGR 58. Windows Terminal, WezTerm and kitty all do; a terminal that does not falls back to a plain underline.

**Negations.** Each attribute has a matching keyword that removes it again: `nobold`, `nodim`, `noitalics`, `nounderscore` (also `nounderline`, and it clears the styled underscores too), `noblink`, `noreverse`, `nohidden`, `nostrikethrough`. These matter inside a format string, where a later `#[...]` block builds on the style already in effect rather than starting from nothing:

```tmux
# Bold for the session name, then drop bold only, keeping the colour
set -g status-left "#[fg=cyan,bold]#S#[nobold] #{pane_current_command}"
```

**Reset.** `default` or `none` returns to the base style of whatever is being drawn.

**Style stack.** `push-default` saves the style in effect, and `pop-default` restores the most recently saved one. Use them to make a temporary change without having to spell out how to undo it:

```tmux
# Save the bar style, highlight the window list, then restore exactly what was there
set -g status-left "#[push-default]#[fg=black,bg=yellow] #S #[pop-default] ready"
```

A `pop-default` with nothing on the stack falls back to the base style, so an unbalanced format degrades instead of breaking.

`fill`, `align=...`, `list=...`, `nolist`, `range=...` and `norange` are accepted for tmux compatibility and are handled by the status bar layout code rather than by the style parser.

## Configuration File Conditionals

Config files support tmux's `%if` directives, so one file can serve several machines. Conditions are ordinary format strings, which means anything you can write inside `#{...}` can drive a branch. A condition is true when it expands to something that is neither empty nor `0`.

```tmux
# A hidden variable. Later config lines can reference it as $THEME or ${THEME},
# and format strings (including %if conditions) can read it as #{THEME}.
%hidden THEME=dark
%hidden ACCENT=cyan

%if "#{==:#{THEME},dark}"
  set -g status-style "bg=colour236,fg=colour250"
  set -g pane-active-border-style "fg=$ACCENT"
%elif "#{==:#{THEME},light}"
  set -g status-style "bg=colour253,fg=black"
  set -g pane-active-border-style "fg=blue"
%else
  set -g status-style "bg=green,fg=black"
%endif

# Conditions can read your own user options, and blocks nest
set -g @work-machine yes

%if "#{==:#{@work-machine},yes}"
  set -g status-right "#{@work-machine} %H:%M"
  %if "#{mouse}"
    set -g pwsh-mouse-selection on
  %endif
%endif
```

The directives:

| Directive | Effect |
|---|---|
| `%if <condition>` | Open a block. Lines inside it run only when the condition is true |
| `%elif <condition>` | Try another condition, but only if no earlier branch in this block matched |
| `%else` | Run when no earlier branch matched |
| `%endif` | Close the innermost block |
| `%hidden NAME=value` | Define `NAME` for the rest of the config, and for panes spawned from this session |

Notes:

- Blocks nest. An inner `%if` inside a branch that was skipped is skipped as a whole, so you never get a partly applied inner block.
- The condition may be quoted with single or double quotes; the quotes are stripped before it is expanded.
- Only the first matching branch runs. Once one has, later `%elif` and `%else` branches are skipped.
- `%hidden` assignments inside a skipped branch are not applied.
- The two ways to read a `%hidden` variable are not interchangeable. `$NAME` and `${NAME}` are substituted only on ordinary config lines, so they do **not** work inside a `%if` condition. Use the format form `#{NAME}` there, which resolves through the same session environment.
- `%hidden` writes into the session environment, so the name is also visible to `show-environment` and is inherited by new panes. It is not a private compile-time constant.

## Accepted but Not Functional

These options parse cleanly, survive `show-options`, and do absolutely nothing. They exist so that an
imported `.tmux.conf` loads without errors. They are listed here so you do not spend time debugging a
setting that was never wired up.

| Option | Status |
|---|---|
| `terminal-overrides` | An explicit no-op. The config file path parses it and throws the value away; the runtime `set-option` path keeps it in the user options map. Neither is ever read, because terminfo overrides have no meaning on Windows, where psmux talks to ConPTY rather than to a terminfo database. Use `default-terminal` to control the `TERM` value panes see |
| `lock-after-time` | Accepted and stored. Session locking is not implemented, so the timer never runs. `lock-client`, `lock-server` and `lock-session` exist as commands but nothing locks on a timer |
| `lock-command` | Accepted and stored. Never read, for the same reason |
| `popup-style` | Accepted and stored. Popup borders are styled by `popup-border-style`; the popup body itself is not styled yet |
| `clock-mode-style` | Accepted and stored. Use `clock-mode-colour`, which is read |

Setting any of these produces no warning, because the values are stored verbatim in the user options
map instead of being validated against a known option. For most of them `show-options` will read the
value back to you unchanged, and that is the trap: a value that round-trips is not evidence that it
took effect.

## Environment Variables

Most of psmux is configured with options, not environment variables. The variables below exist either
because the setting has to be known before a config file is read, or because it is a per shell escape
hatch you want for one invocation rather than forever.

### Startup and session selection

| Variable | Effect |
|---|---|
| `PSMUX_CONFIG_FILE` | Replaces the config file search entirely. Set for you by `psmux -f <file>`. A leading `~` is expanded |
| `PSMUX_DEFAULT_SESSION` | Session name used when nothing else determines one |
| `PSMUX_SESSION_NAME` | Target session for a bare `psmux` or a control mode invocation. psmux also sets this itself when it spawns |
| `PSMUX_TARGET_SESSION` | Session that CLI commands address when you give no `-t`. Exported into panes, which is how a command run inside a pane knows where it is |
| `PSMUX_TARGET_FULL` | The full `session:window.pane` target string for CLI commands. Set by the global `-t` parse |
| `PSMUX_ALLOW_NESTING` | Set to `1` to permit running psmux inside a psmux pane. Without it the nesting guard refuses, because a nested client would fight the outer one for the console |
| `PSMUX_REMOTE_ATTACH` | Marks the invocation as a remote attach, which skips the bare invocation session bootstrap |
| `PSMUX_ACTIVE` | Set to `1` on a client process to mark that it owns the console. This is what the nesting guard reads |
| `PSMUX_SWITCH_TO` | Handshake variable carrying the session name across a `switch-client` |
| `PSMUX_NO_WARM` | Set to `1` to disable warm pane and warm server pre-spawning. Equivalent to `set -g warm off`. See [warm-sessions.md](warm-sessions.md) |
| `PSMUX_PRIORITY` | Scheduling class for the psmux server and client processes: `normal`, `above-normal` or `high`. Overrides the `priority` option, so it is the per shell way out of a configured value. An unrecognised value is reported and ignored. See [Process Priority](#process-priority) |

### Appearance and rendering

| Variable | Effect |
|---|---|
| `PSMUX_CURSOR_STYLE` | Cursor shape: `bar`, `block`, `underline`, or `default`. Normally set for you by `set -g cursor-style` |
| `PSMUX_CURSOR_BLINK` | Cursor blink. `0` disables it. Normally set for you by `set -g cursor-blink` |
| `PSMUX_DIM_PREDICTIONS` | Dim PSReadLine prediction text for this shell only. The option form is `prediction-dimming` |
| `PSMUX_HOST_COLORS` | Supplies the host terminal's palette so psmux can answer OSC 4, 10 and 11 colour queries. psmux normally queries the host itself; set this when the host misreports or when the query cannot run |

### Windows and transport escape hatches

| Variable | Effect |
|---|---|
| `PSMUX_NO_PASSTHROUGH` | Set to `1` to disable the experimental ConPTY passthrough flag on Windows build 22621 and newer. Use this if pane creation fails with `ERROR_INVALID_PARAMETER` |
| `PSMUX_PIPE_VT` | Forces pipe mode VT handling for Cygwin and MSYS style PTYs. `1` forces it on, `0` forces it off. Left unset, psmux detects the pipe itself |
| `PSMUX_BARE_ENV` | Spawn panes with a bare environment instead of inheriting yours. Useful when a broken inherited variable stops shells from starting |
| `PSMUX_FORCE_MOUSE` | Overrides the ConPTY mouse safety gate. On Windows builds below 22523 psmux refuses to enable mouse reporting, because on Windows 10 era conhost the first click could fast fail the console host and take the pane down with it. Some later builds under that threshold, Windows Server 2022 (20348) among them, handle mouse perfectly well but still need psmux to write the enable sequence itself. Set to `1` there to get the mouse back. Set to `0` to force it off on a newer build whose console host misbehaves. Accepts `1`, `on`, `true`, `yes` and their negatives. If your session dies the moment you click, unset it |
| `PSMUX_FORCE_WHEEL` | Set to `1` to forward the wheel into every pane on this server regardless of whether its application registered for the mouse. This is the server wide last resort; prefer the pane scoped `set-option -p -t %N @mouse-force on`, which is the scope the decision belongs at. psmux normally forwards only to a pane that asked, and it now keeps that authorization for as long as the process that earned it is alive, so a `node` child entering raw mode no longer takes it away. Reach for this only when an application registers through neither channel psmux can see and the wheel is dead in the pane from its first instant. Leave it unset if you run programs that do not read the mouse: they receive the report as literal keystrokes, which is what the gate exists to prevent. Accepts `1`, `on`, `true`, `yes`; anything else keeps the gate |

### Set inside panes by psmux

These are exported into every pane, so scripts can detect that they are running under psmux and where:

| Variable | Effect |
|---|---|
| `TMUX` | Socket path and server info, for tmux compatibility. This is what most tools check |
| `TMUX_PANE` | Current pane id (`%0`, `%1`, and so on) |
| `PSMUX_SESSION` | Current session name |

Setting a variable for one shell, or for good:

```powershell
# This shell only
$env:PSMUX_NO_WARM = "1"
psmux

# Every future shell
setx PSMUX_NO_WARM 1
```

> **Note:** psmux also has a set of `PSMUX_*_DEBUG` logging variables. They are documented in
> [diagnostics.md](diagnostics.md) rather than here, because they are for reporting a problem rather
> than for configuring psmux.

## Managing Environment Variables

Use `set-environment` to set env vars that are inherited by newly created panes:

```powershell
# Set a global env var (inherited by all new panes)
psmux set-environment -g EDITOR vim

# Set a session-scoped env var
psmux set-environment MY_VAR value

# Unset an env var
psmux set-environment -gu MY_VAR

# Show all environment variables
psmux show-environment
psmux show-environment -g
```

Environment variables set this way are injected at the process level when new panes spawn, so they are completely invisible (no commands echoed in the shell).

## PSReadLine Predictions (Intellisense / Autocompletion)

By default, psmux disables PSReadLine inline predictions (the grayed-out autocompletion/intellisense suggestions that appear as you type) to avoid additional unexpected bugs caused by the interaction between predictions and ConPTY. This means `PredictionSource` defaults to `None` inside psmux, even if your profile sets it to `HistoryAndPlugin` ([#150](https://github.com/psmux/psmux/issues/150)).

If enough people test predictions and the community supports enabling them by default, this will be changed in a future release.

To preserve your prediction/autocompletion settings, enable `allow-predictions`:

```tmux
set -g allow-predictions on
```

With this enabled:
- If your profile sets `PredictionSource`, psmux respects your choice
- If your profile does not set it, psmux restores the system default (typically `HistoryAndPlugin`)

## Prediction Dimming

Prediction dimming is off by default. If you want psmux to dim predictive/speculative text (e.g. shell autosuggestions), you can enable it in `~/.psmux.conf`:

```tmux
set -g prediction-dimming on
```

You can also enable it for the current shell only:

```powershell
$env:PSMUX_DIM_PREDICTIONS = "1"
psmux
```

To make it persistent for new shells:

```powershell
setx PSMUX_DIM_PREDICTIONS 1
```

## Reloading Configuration at Runtime

You can reload your config file without restarting psmux. From the command prompt (`Prefix + :`), run:

```tmux
source-file ~/.psmux.conf
```

Or from outside psmux:

```powershell
psmux source-file ~/.psmux.conf
```

This re-executes every line in the config file, applying any changes to options, key bindings, hooks, and styles immediately.

## Window and Pane Numbering

By default, windows and panes are numbered starting from 0. You can change the starting index for both:

```tmux
# Start window numbering at 1
set -g base-index 1

# Start pane numbering at 1
set -g pane-base-index 1
```

The `pane-base-index` setting affects:

- **Display Panes overlay** (`Prefix + q`): The numbers shown on each pane start from your configured base index
- **Pane targets**: When referencing panes by number (e.g. `select-pane -t 1`), numbering follows your base index
- **Format variables**: `#{pane_index}` reflects the base index setting
- **Status bar and border labels**: Pane numbers in format strings use the configured base

A common setup for both windows and panes to start at 1:

```tmux
set -g base-index 1
set -g pane-base-index 1
```

## Display Panes Overlay

Press `Prefix + q` to show numbered overlays on each pane. While the overlay is visible, press any displayed number key to jump to that pane. The overlay auto-dismisses after `display-panes-time` milliseconds (default: 1000ms).

```tmux
# Show pane numbers for 3 seconds
set -g display-panes-time 3000
```

The numbers shown respect your `pane-base-index` setting. For example, with `pane-base-index 1`, three panes show as 1, 2, 3 instead of 0, 1, 2.

You can also trigger this overlay from the command line:

```powershell
psmux display-panes
```

## Split Window Options

When splitting panes, you can control the size and starting directory of the new pane:

```tmux
# Split vertically, new pane takes 30% of the space
split-window -v -p 30

# Split horizontally, new pane takes 70% of the space
split-window -h -p 70

# Split and start in a specific directory
split-window -v -c "C:\Projects\myapp"

# Split and start in the current pane's directory
split-window -h -c "#{pane_current_path}"

# Split and run a specific command
split-window -v -- python
```

These flags also work when creating new windows:

```tmux
# New window with a specific name
new-window -n "logs"

# New window in a specific directory
new-window -c "C:\Projects"

# New window running a specific command with a name
new-window -n "build" -- cargo build --watch
```

When you set a window name with `-n`, the `automatic-rename` flag is turned off for that window so psmux does not overwrite your chosen name with the foreground process name. To re-enable automatic renaming for that window:

```tmux
set-option -w automatic-rename on
```

## Detach and Exit Policies

Control what happens when clients disconnect or all windows close:

```tmux
# Exit the server when no clients are attached (default: off)
set -g destroy-unattached on

# Exit the server when the last window/session closes (default: on)
set -g exit-empty on
```

With `destroy-unattached on`, the server process terminates as soon as the last client detaches. This is useful for single-use sessions.

With `exit-empty off`, the server stays alive even after all sessions are closed, allowing new sessions to be created without restarting.

## Dead Panes and Respawn

When a process inside a pane exits, the pane normally closes. To keep the pane visible after its process exits:

```tmux
set -g remain-on-exit on
```

A pane with a dead process shows its last output and can be respawned:

```powershell
# Restart the default shell in the pane
psmux respawn-pane

# Kill any remaining process and restart
psmux respawn-pane -k

# Respawn in a different directory
psmux respawn-pane -c "C:\Projects"

# Respawn with a specific command
psmux respawn-pane -- python app.py
```

This is useful for monitoring: if a long-running process crashes, you can see its final output and restart it without losing the pane layout.

### Background Processes and `@kill-descendants`

On Unix, tmux relies on the kernel's SIGHUP delivery when a pane's terminal closes, so a process that was deliberately detached (for example with `nohup`) survives its pane. Windows has no SIGHUP and no pty process groups, so psmux instead walks the pane's process tree. By default, when a pane's shell exits on its own, psmux terminates any child processes the shell left behind (for example something launched with `Start-Process`), because otherwise those processes and their `conhost.exe` hosts accumulate invisibly and can exhaust the desktop heap.

If you intentionally launch background processes from a pane and want them to outlive the shell, opt out (psmux extension):

```tmux
# Let background children survive when their pane's shell exits on its own
set -g @kill-descendants off
```

Notes:

- This only affects panes whose shell exits on its own. Explicit `kill-pane`, `kill-window`, and `kill-session` always terminate the pane's full process tree.
- With `remain-on-exit on` the pane is kept instead of pruned, so no sweep happens either way until the pane is actually closed.
- Recognized off values: `off`, `0`, `false`, `no`. Anything else, including unset, keeps the sweep enabled.

## Session Environment Variables

You can set environment variables at the session or global level that get inherited by all new panes:

```powershell
# Set a global env var (all new panes in all sessions inherit this)
psmux set-environment -g EDITOR vim

# Set a session-scoped env var
psmux set-environment MY_VAR value

# Unset a global env var
psmux set-environment -gu MY_VAR

# View all environment variables
psmux show-environment
psmux show-environment -g
```

You can also pass environment variables when creating a new session:

```powershell
# Create a session with custom environment
psmux new-session -s work -e "PROJECT=myapp" -e "ENV=production"
```

## Status Bar Time Updates

The status bar supports time format variables that update in real time:

```tmux
# Show current time in the status bar (updates every second)
set -g status-right "%H:%M:%S %d-%b-%y"

# Common time format variables:
#   %H   Hour (24-hour, 00-23)
#   %I   Hour (12-hour, 01-12)
#   %M   Minute (00-59)
#   %S   Second (00-59)
#   %p   AM/PM
#   %r   Full time in 12-hour format (e.g. 02:30:45 PM)
#   %R   Hour:Minute in 24-hour format (e.g. 14:30)
#   %d   Day of month (01-31)
#   %b   Abbreviated month name (Jan, Feb, ...)
#   %Y   Full year (2025)
#   %a   Abbreviated weekday (Mon, Tue, ...)
```

Time variables refresh based on the `status-interval` option (default: 15 seconds). For second-level precision, reduce the interval:

```tmux
# Update status bar every second (for live clock)
set -g status-interval 1
```

## PSReadLine ListView

psmux supports PSReadLine's ListView prediction style, which shows a dropdown list of suggestions:

```powershell
# In your PowerShell profile ($PROFILE)
Set-PSReadLineOption -PredictionSource HistoryAndPlugin
Set-PSReadLineOption -PredictionViewStyle ListView
```

For this to work inside psmux, enable `allow-predictions` in your psmux config:

```tmux
set -g allow-predictions on
```

Without `allow-predictions on`, psmux resets PSReadLine's prediction settings during initialization, which disables ListView mode.
