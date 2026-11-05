# Multi-Shell Workflows

psmux lets you run **any combination of shells** side by side in the same session.
PowerShell, Git Bash, cmd.exe, WSL, Nushell, or any other shell or program,
each in its own pane, window, or session. Switch between them instantly.

```
 +-----------------------+-----------------------+
 |  PowerShell 7         |  Git Bash             |
 |  PS C:\project> ...   |  user@pc ~/project $  |
 |                       |                       |
 +-----------------------+-----------------------+
 |  cmd.exe              |  WSL (Ubuntu)         |
 |  C:\project>          |  user@pc:~/project$   |
 |                       |                       |
 +-----------------------+-----------------------+
  [0] pwsh*  [1] bash  [2] node  [3] python
```

## Setting Your Default Shell

Add one line to `~/.psmux.conf`:

```tmux
# Git Bash
set -g default-shell "C:/Program Files/Git/bin/bash.exe"

# Git Bash (backslashes work too)
set -g default-shell "C:\Program Files\Git\bin\bash.exe"

# Git Bash with login profile
set -g default-shell "C:/Program Files/Git/bin/bash.exe" --login

# cmd.exe
set -g default-shell cmd.exe

# PowerShell 7 (the default if nothing is set)
set -g default-shell pwsh

# Windows PowerShell 5
set -g default-shell powershell

# Nushell
set -g default-shell nu

# WSL default distro
set -g default-shell wsl
```

Bare names like `bash`, `pwsh`, `cmd`, `nu`, `wsl` are resolved via PATH.
Full paths with spaces must be wrapped in quotes. Both forward slashes and
backslashes are supported.

## Changing the Shell at Runtime

You don't need to restart psmux to switch shells. Press `Prefix + :` (default
`Ctrl+B` then `:`) to open the command prompt, then type:

```tmux
set -g default-shell "C:/Program Files/Git/bin/bash.exe"
```

Every new window and pane created after this will use the new shell.
Existing panes keep their current shell.

## Mix and Match: Different Shells in Different Panes

This is where psmux really shines. You can override the default shell for
any individual window or pane by passing the shell as a command:

### From the command prompt (`Prefix + :`)

```tmux
# Open a new Git Bash window while your default is pwsh
new-window "C:/Program Files/Git/bin/bash.exe"

# Split the current pane and run cmd.exe in the new split
split-window cmd.exe

# Split horizontally and run WSL
split-window -h wsl

# Open a new window running Python
new-window python

# Open a new window running Node.js REPL
new-window node
```

### From the CLI (PowerShell, cmd, or any terminal)

```powershell
# Create a bash window in an existing session
psmux new-window -- "C:/Program Files/Git/bin/bash.exe"

# Split with cmd.exe
psmux split-window -- cmd.exe

# Create a whole new session running WSL
psmux new-session -s linux -- wsl

# Launch a Python REPL in a split pane
psmux split-window -- python
```

### From your config file (`~/.psmux.conf`)

```tmux
# Default shell is PowerShell
set -g default-shell pwsh

# Bind keys to quickly open specific shells
bind-key B new-window "C:/Program Files/Git/bin/bash.exe"
bind-key C new-window cmd.exe
bind-key W new-window wsl
bind-key N new-window nu

# Bind keys for splitting with a specific shell
bind-key b split-window -v "C:/Program Files/Git/bin/bash.exe"
bind-key c split-window -v cmd.exe
```

Now `Prefix + B` opens a bash window, `Prefix + C` opens cmd, etc.

## Real-World Use Cases

### Web Development

Your default shell is PowerShell for project management, but you need bash
for your build tools and Node scripts:

```tmux
# ~/.psmux.conf
set -g default-shell pwsh

# Quick access to bash for npm/node
bind-key B new-window "C:/Program Files/Git/bin/bash.exe" --login
bind-key b split-window -v "C:/Program Files/Git/bin/bash.exe" --login
```

Workflow:
1. Window 0 (pwsh): `git status`, `dotnet build`, project management
2. `Prefix + B` to open Window 1 (bash): `npm run dev`
3. `Prefix + b` to split (bash): `npm test` running alongside
4. `Prefix + :` then `split-window node` for a quick Node REPL

### DevOps / Infrastructure

Mix WSL Linux tools with native Windows admin shells:

```tmux
set -g default-shell pwsh

bind-key L new-window wsl
bind-key l split-window -v wsl
```

Workflow:
1. Window 0 (pwsh): Azure/AWS CLI, Windows admin tasks
2. `Prefix + L` for Window 1 (WSL): `kubectl`, `docker`, `terraform`
3. Split both windows as needed for logs, monitoring, editors

### Cross-Platform Testing

Test your scripts in every shell without leaving your session:

```tmux
bind-key F1 new-window pwsh
bind-key F2 new-window "C:/Program Files/Git/bin/bash.exe"
bind-key F3 new-window cmd.exe
bind-key F4 new-window wsl
```

### Dedicated Tool Windows

Run long-running tools in their own shells:

```tmux
# Quick launch for common tools
bind-key P new-window python
bind-key J new-window node
bind-key S new-window "C:/Program Files/Git/bin/bash.exe" -c "ssh myserver"
```

## Multiple Sessions with Different Defaults

You can also create entirely separate sessions, each with its own default shell:

```powershell
# Session for PowerShell work
psmux new-session -d -s work

# Session for Linux/bash work
psmux new-session -d -s linux -- wsl

# Session for a specific project using bash
psmux new-session -d -s webapp -- "C:/Program Files/Git/bin/bash.exe" --login
```

Switch between sessions with `Prefix + s` (session picker) or `Prefix + (` / `)`.

## Supported Shells

psmux works with any program that reads from stdin and writes to stdout.
Here are common shells and how to configure them:

| Shell | Config Value | Notes |
|-------|-------------|-------|
| PowerShell 7 | `pwsh` | Default. Fastest startup with psmux optimizations |
| Windows PowerShell 5 | `powershell` | Built into Windows |
| Git Bash | `"C:/Program Files/Git/bin/bash.exe"` | Quotes required (path has spaces) |
| Git Bash (login) | `"C:/Program Files/Git/bin/bash.exe" --login` | Loads `.bash_profile` |
| cmd.exe | `cmd` or `cmd.exe` | Classic Windows command prompt |
| WSL | `wsl` | Launches your default WSL distro |
| WSL (specific distro) | `wsl -d Ubuntu` | Specify a distro by name |
| Nushell | `nu` | Modern structured-data shell |
| Fish | `"C:/path/to/fish.exe"` | If installed via MSYS2/Cygwin |
| Python REPL | `python` | Not a shell, but works great in a pane |
| Node.js REPL | `node` | Same, useful for quick JS testing |

## WSL Panes

A pane running `wsl` is a Windows process (`wsl.exe`, plus `wslhost.exe`) bridging to a Linux
shell inside the WSL VM. Three things follow from that, and psmux handles each of them.

### `#{pane_current_path}` inside WSL needs shell integration

psmux answers `#{pane_current_path}` the way tmux does, by asking the operating system for the
working directory of the pane's foreground process. That works for PowerShell, `cmd`, Cygwin bash
and Git Bash, whose `cd` also moves the Win32 working directory. It cannot work for WSL: the shell
you type at is a Linux process, and the only Windows processes in the pane keep the directory they
were started in forever. Nothing on the Windows side can see where the Linux shell went, so without
help `split-window -c "#{pane_current_path}"` opens where you were *before* you typed `wsl`
([#615](https://github.com/psmux/psmux/issues/615)).

The fix is the same one tmux and Windows Terminal rely on: let the shell announce its own
directory with OSC 7. Add this to `~/.bashrc` inside the distro:

```bash
PROMPT_COMMAND='printf "\033]7;file://%s%s\033\\" "$HOSTNAME" "$PWD"'
```

For zsh use `precmd() { printf "\033]7;file://%s%s\033\\" "$HOST" "$PWD"; }`. When a bridge such
as `wsl.exe` or `ssh.exe` is in the pane's process tree, the announced path outranks what Windows
reports, and psmux translates it into a path Windows can open: `/mnt/c/Users` becomes `C:\Users`,
and a Linux only directory such as `/home/you` becomes `\\wsl.localhost\<distro>\home\you`. ConEmu's
`OSC 9;9` form (`printf "\033]9;9;%s\033\\" "$PWD"`) is accepted too. Without the announcement
nothing breaks, the variable simply keeps the last directory psmux could observe.

The same mechanism works for an `ssh` pane: a remote shell that emits OSC 7 gives
`#{pane_current_path}` a real answer where the local process walk cannot.

### Ctrl+C right after launching `wsl`

Pressing Ctrl+C inside a WSL pane sends the interrupt to the Linux foreground program, as it does
in tmux. Earlier psmux builds had a window of about 1.5 seconds after typing `wsl` (while the VM
was still booting) in which a Ctrl+C aborted the whole WSL launch and dropped you back at the
PowerShell prompt. That window is closed ([#579](https://github.com/psmux/psmux/issues/579)):
while a bridge process is starting, psmux never broadcasts a console Ctrl+C event into the pane,
and it strips `ENABLE_PROCESSED_INPUT` before writing the raw `0x03` byte so conhost cannot turn
it into one either.

### Pressing Ctrl+C repeatedly in Git Bash

Ctrl+C in a Git Bash pane interrupts the program in that pane and nothing else, however fast or
often you press it, as in tmux. Earlier psmux builds could lose the whole server to a burst of
Ctrl+C presses in Git Bash, most easily with the stock Git for Windows prompt (its
`PROMPT_COMMAND` runs native programs): every session vanished at once and the server exited with
`0xC000013A`. Git Bash panes keep the console's Ctrl+C processing on, so conhost turns each press
into a console Ctrl+C event, and when one arrived while psmux was briefly attached to that console
to deliver input, or just as it left, the server took it as its own. psmux now ignores console
Ctrl+C and Ctrl+Break events while it is attached to a pane's console and after it has left
([#989](https://github.com/psmux/psmux/issues/989)).

### Mouse in WSL programs

Clicks, drags and the wheel reach a mouse aware program inside WSL (for example `nvim` with
`set mouse=a`) the same way they reach a native one. A pointer merely moving over the pane is not
reported unless the program asked for motion tracking (DECSET 1003), so hovering costs nothing
([#604](https://github.com/psmux/psmux/issues/604)). The wheel is forwarded only to a program that
has enabled a mouse mode; over a program that never asked, the notch is a no-op rather than a burst
of literal keystrokes. See [faq.md](faq.md) and [mouse-ssh.md](mouse-ssh.md).

## Start Directories and Warm Panes

`new-window -c <dir>` and `split-window -c <dir>` normally hand the directory to the new process at
creation time. When the new pane is served from the warm pool (a pre-spawned copy of your
`default-shell`, see [warm-sessions.md](warm-sessions.md)), that shell is already running, so psmux
instead types a `cd` line into it and clears the screen. The line is written in the syntax of the
shell that is actually running, chosen from the effective `default-shell`
([#600](https://github.com/psmux/psmux/issues/600)). The choice is made on the **basename** of that
setting, so every spelling of one shell behaves the same: a bare name or a full path, forward or
backslashes, with or without `.exe`, in any letter case, quoted, and with arguments after it
([#672](https://github.com/psmux/psmux/issues/672)):

| `default-shell` | Injected line |
|---|---|
| `pwsh`, `powershell` | `cd '<dir>'; [System.IO.Directory]::SetCurrentDirectory(...); cls` (the second call keeps the Win32 working directory in step so `#{pane_current_path}` follows) |
| `cmd` | `cd /d "<dir>" & cls` |
| `bash`, `zsh`, `sh`, `fish`, `dash`, `ksh`, `tcsh`, `csh`, `ash`, `busybox` | `cd '<dir>'; clear` with the path written with forward slashes |
| `nu`, `nushell` | `cd '<dir>'; clear` with the path as given (a backslash is an ordinary character in nu single quotes); a path containing an apostrophe switches to double quotes (`cd "<dir>"`, every backslash doubled) because a nushell single-quoted string has no quote escape at all |
| anything else | the PowerShell form |

`git-bash.exe` (Git Bash's GUI launcher, which psmux runs as the console `bash.exe` beside it)
counts as bash. `wsl` falls into the last row; a `default-shell` of
`C:\Windows\System32\bash.exe` is WSL's bash and gets the bash row, because the shell on the other
side of it is a POSIX shell. If your `default-shell` falls into the last row and the injected line
errors, turn warm panes off with `set -g warm off` so every pane starts cold in the requested
directory instead.

## Tips

- **Paths with spaces** must be wrapped in double quotes: `"C:/Program Files/..."`
- **Forward slashes and backslashes** both work: `C:/Program Files` and `C:\Program Files` are equivalent
- **Bare names** like `bash`, `pwsh`, `cmd`, `nu` are resolved via your system PATH
- **Extra arguments** go after the path: `"C:/Program Files/Git/bin/bash.exe" --login`
- **Changing default-shell at runtime** only affects new panes/windows. Existing ones keep their shell
- **Each pane is independent**. Closing a bash pane does not affect your pwsh panes
- **Environment variables** (`TMUX`, `PSMUX_SESSION`, `TERM`) are set correctly in all shell types
- **Pane titles differ by shell**: PowerShell 7 sets the pane title to the CWD on every prompt, while cmd.exe and nushell do not. See [pane-titles.md](pane-titles.md) for how this affects your status bar and how to control it
