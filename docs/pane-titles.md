# Pane Titles and OSC Escape Sequences

## How Pane Titles Work

Every pane in psmux has a **title**. By default this is the system hostname, matching tmux convention. You can see it in the status bar, pane border labels, and format variables like `#{pane_title}` and `#T`.

Programs running inside a pane can change its title by sending **OSC (Operating System Command) escape sequences**:

| Sequence | Name | Effect |
|----------|------|--------|
| `ESC ] 0 ; <title> BEL` | OSC 0 | Sets both the window icon name and the pane title |
| `ESC ] 2 ; <title> BEL` | OSC 2 | Sets the pane title |
| `ESC [ 22 ; 0 t` | XTPUSHTITLE | Saves the title (`22 ; 2 t` too) |
| `ESC [ 23 ; 0 t` | XTPOPTITLE | Restores the title saved last (`23 ; 2 t` too) |

When psmux receives one of these from a child process, it updates `pane_title` so that format variables, status bar, and border labels all reflect the new value.

As in tmux, the title is everything after the first `;`, so `ESC ] 2 ; a;b BEL` sets `a;b`. Up to ten titles are saved, and the oldest goes when an eleventh is saved. Restoring a title saved before any was set gives the pane its first title, the host name, back. RIS and `send-keys -R` keep the title and the saved ones. The inbox console host passes these two sequences on at once but holds a title for its next screen update, so a save, a title and a restore written in one go reach psmux in another order. Written apart, as a program saves at its start and restores at its end, they work on both console hosts.

Two more OSC sequences are understood, and they never touch the title. They tell psmux where the shell is:

| Sequence | Name | Effect |
|----------|------|--------|
| `ESC ] 7 ; file://host/path ESC \` | OSC 7 | Announces the shell's working directory. Feeds `#{pane_current_path}` for panes whose directory Windows cannot see (WSL, SSH) |
| `ESC ] 9 ; 9 ; path ESC \` | OSC 9;9 | ConEmu's form of the same announcement |

A POSIX path in either form is translated to a native Windows one (`/mnt/c/Users` to `C:\Users`, `/home/you` to `\\wsl.localhost\<distro>\home\you`). PowerShell, cmd, Git Bash and Cygwin panes do not need it, since their `cd` moves the Win32 working directory and psmux reads that directly. See the WSL section of [multi-shell.md](multi-shell.md#wsl-panes).

## PowerShell and OSC Titles

Here is the important part for Windows users: **PowerShell 7 sends OSC 0 automatically on every single prompt**. It sets the terminal title to the current working directory (e.g. `C:\Users\you\Projects\myapp`).

This means that if your status bar format references `#{pane_title}` or `#T`, you will see a truncated file path instead of the hostname. For example, with the tmux default status right format `"#{=21:pane_title}" %H:%M %d-%b-%y`, you would see something like:

```
"C:\Program Files\Powe" 19:39 17-Apr-26
```

instead of:

```
"DESKTOP-ABC1234" 19:39 17-Apr-26
```

This is not a bug. It is the expected behavior: PowerShell tells the terminal "my title is this path" and psmux faithfully applies it. On Linux, bash and zsh do not send OSC title sequences by default, so tmux users on Linux almost always see the hostname in that position.

**The default psmux experience avoids this issue, but not because of the format string.** psmux's default `status-right` does reference `"#{=21:pane_title}"`, exactly as tmux's does. What protects you is that **`allow-set-title` defaults to `off`**, so PowerShell's OSC title sequence is ignored, `pane_title` keeps its fallback value of the machine hostname, and the status bar shows the hostname. You will only encounter the path in your status bar if you set `allow-set-title on` in your config, or if you use a tmux config or theme that enables it and references `#{pane_title}` or `#T` in the status bar.

> **Note:** `show-options -g status-right` reports the live value, and resetting `status-right` from `customize-mode` restores exactly that same value.

## Options That Control This Behavior

### `allow-set-title` (default: `off`)

Controls whether programs can update the pane title via OSC 0/2 sequences. When `off` (the default), OSC title sequences are ignored and `pane_title` stays at the hostname or whatever was set via `select-pane -T`. Set to `on` if you want programs to dynamically update the pane title.

```tmux
# Allow programs to change pane titles via OSC sequences
set -g allow-set-title on
```

### `select-pane -T` (title lock)

Setting a pane title manually with `select-pane -T` **locks** that title. After locking, OSC sequences from child processes will not overwrite it. This lets you label specific panes permanently.

```tmux
# Lock a pane's title
select-pane -T "build output"

# Clear the lock (title reverts to hostname, OSC sequences can update it again)
select-pane -T ""
```

### `allow-rename` (default: `on`)

Controls whether programs can rename the **window** (not the pane) via escape sequences. This is separate from `allow-set-title` which controls the pane title.

## Controlling PowerShell's Title Behavior

By default, `allow-set-title` is `off`, so PowerShell's OSC title sequences are ignored and you will see the hostname. If you enable `allow-set-title on` for dynamic titles but want to stop PowerShell specifically from overwriting the title with the CWD, you have several options:

### Option 1: Disable pwsh's window title in your PowerShell profile

Add this to your `$PROFILE`:

```powershell
# Prevent PowerShell from setting the terminal title
$PSStyle.WindowTitle = ''
```

Or for older PowerShell versions:

```powershell
function prompt {
    # Your custom prompt here, without setting WindowTitle
    "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) "
}
```

### Option 2: Turn off OSC title propagation globally (this is the default)

```tmux
# In ~/.psmux.conf (this is already the default, shown here for reference)
set -g allow-set-title off
```

This keeps `pane_title` at the hostname (or whatever you set with `select-pane -T`). No program can change it via escape sequences.

### Option 3: Use `#H` instead of `#{pane_title}` in your status bar

If your theme or config uses `#{pane_title}` in the status bar and you want the hostname there instead, replace it:

```tmux
# Before (shows CWD from pwsh):
set -g status-right '"#{=21:pane_title}" %H:%M %d-%b-%y'

# After (always shows hostname):
set -g status-right '"#H" %H:%M %d-%b-%y'
```

`#H` always resolves to the system hostname regardless of OSC sequences.

### Option 4: Lock specific pane titles

```tmux
# Set and lock a title on a pane
select-pane -T "my server"

# Now OSC sequences from pwsh won't overwrite it
```

## Where `pane_title` Appears

The pane title is exposed through several format variables and locations:

| Variable | Description |
|----------|-------------|
| `#{pane_title}` | Full pane title |
| `#T` | Alias for the active pane's title |
| `#{=21:pane_title}` | Pane title truncated to 21 characters |

These can appear in:

- **`status-right`** and **`status-left`**: the status bar at the bottom (or top) of the screen
- **`pane-border-format`**: labels on pane borders (when `pane-border-status` is `top` or `bottom`)
- **`window-status-format`** and **`window-status-current-format`**: window tab labels
- **`display-message -p`**: programmatic queries
- **`list-panes -F`** and **`list-windows -F`**: scripting and automation

## How Different Shells Behave

| Shell | Sends OSC title? | Content |
|-------|-------------------|---------|
| PowerShell 7 (pwsh) | Yes, on every prompt | Current working directory |
| PowerShell 5 | No | N/A |
| cmd.exe | No | N/A |
| Git Bash | Depends on config | Usually `user@host:path` |
| WSL bash | Depends on config | Usually `user@host:path` |
| Nushell | No | N/A |

## Interaction With Other Features

### Automatic Window Rename

`automatic-rename` (default: `on`) renames the **window** based on the foreground process. This is separate from the pane title. A window can be named "pwsh" while the pane title shows the hostname or CWD.

### Pane Border Labels

Enabling `pane-border-status top` (or `bottom`) on its own is enough to show pane titles on the border. When `pane-border-format` is not set, psmux falls back to the tmux default of `#{pane_index} "#{pane_title}"`, so this alone shows a labelled border:

```tmux
set -g pane-border-status top
```

Set `pane-border-format` only when you want to customize the label. With `#{pane_title}` it updates live as OSC titles change, which is useful for showing what directory each pane is working in:

```tmux
set -g pane-border-status top
set -g pane-border-format " #{pane_index}: #{pane_title} "
```

### Tmux Themes

Many tmux themes (Catppuccin, Dracula, Tokyo Night, etc.) use `#{pane_title}` in their status bar formats. On Windows with PowerShell, this will show the CWD instead of the hostname. Check your theme's configuration for options to customize which variables appear in the status bar, or use `allow-set-title off` as described above.

## Quick Reference

| Goal | Config |
|------|--------|
| Keep hostname in status bar (default) | `allow-set-title` is already `off` by default |
| Let programs set titles dynamically (not the default) | `set -g allow-set-title on` |
| Always show hostname (regardless of config) | Use `#H` instead of `#{pane_title}` |
| Stop pwsh from setting title | Add `$PSStyle.WindowTitle = ''` to `$PROFILE` |
| Lock a specific pane's title | `select-pane -T "my title"` |
| Show CWD in pane borders (useful!) | `set -g pane-border-format " #{pane_index}: #{pane_title} "` |
