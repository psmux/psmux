# Developer Integration Guide

This guide is for developers who want to build tools, scripts, IDE extensions, or automation pipelines that use psmux on Windows, especially if you already have tmux integrations on Linux/macOS.

## Why psmux for Developers

psmux implements the same CLI protocol and command set as tmux. If your project already integrates with tmux via subprocess calls, control mode, or libraries like libtmux, you can run on Windows with minimal or zero code changes.

Key points:

- **Same binary name**: psmux installs `tmux.exe` as an alias. Existing scripts that call `tmux` will find psmux on the PATH.
- **Same commands**: 83 tmux commands with the same flags, arguments, and output formats.
- **Same IDs**: `$N` (session), `@N` (window), `%N` (pane) stable IDs follow the tmux scheme.
- **Same control mode**: `-C`/`-CC` wire protocol with `%begin`/`%end` framing, notifications, and output escaping.
- **Same config**: Reads `~/.tmux.conf` directly. Your config, key bindings, and themes transfer as-is.
- **Same format engine**: 140+ format variables with conditionals, loops, regex, and string operations.

## Installation

```powershell
# Cargo (recommended for developers)
cargo install --git https://github.com/psmux/psmux

# Scoop
scoop bucket add extras
scoop install psmux

# Winget
winget install psmux

# Chocolatey
choco install psmux
```

After installation, `psmux`, `pmux`, and `tmux` are all available as commands. Use whichever fits your project.

## Quick Start: Subprocess Integration

The simplest integration pattern. Works with any language that can spawn processes.

### Python

```python
import subprocess
import platform

def mux_cmd(args, encoding="utf-8"):
    """Run a tmux/psmux command and return stdout."""
    kwargs = {"capture_output": True, "text": True}
    if platform.system() == "Windows":
        kwargs["encoding"] = encoding
    result = subprocess.run(["tmux"] + args, **kwargs)
    if result.returncode != 0:
        raise RuntimeError(f"tmux command failed: {result.stderr}")
    return result.stdout.strip()

# Create a session
mux_cmd(["new-session", "-d", "-s", "dev", "-x", "120", "-y", "30"])

# Send a command
mux_cmd(["send-keys", "-t", "dev", "echo hello", "Enter"])

# Read pane output
content = mux_cmd(["capture-pane", "-t", "dev", "-p"])
print(content)

# Query format variables
pane_path = mux_cmd(["display-message", "-t", "dev", "-p", "#{pane_current_path}"])

# List all sessions
sessions = mux_cmd(["list-sessions", "-F", "#{session_name}"])

# Clean up
mux_cmd(["kill-session", "-t", "dev"])
```

### PowerShell

```powershell
function Invoke-Mux {
    param([string[]]$Args)
    $result = & tmux @Args 2>&1
    if ($LASTEXITCODE -ne 0) { throw "tmux command failed: $result" }
    return $result
}

# Create and interact with a session
Invoke-Mux new-session -d -s dev -x 120 -y 30
Invoke-Mux send-keys -t dev "Get-Process | Select -First 5" Enter
Start-Sleep -Seconds 1
$content = Invoke-Mux capture-pane -t dev -p
Write-Host $content
Invoke-Mux kill-session -t dev
```

### Node.js

```javascript
const { execFileSync } = require("child_process");

function muxCmd(args) {
  return execFileSync("tmux", args, { encoding: "utf-8" }).trim();
}

// Create a session
muxCmd(["new-session", "-d", "-s", "dev", "-x", "120", "-y", "30"]);

// Send keys
muxCmd(["send-keys", "-t", "dev", "echo hello", "Enter"]);

// Capture output
const content = muxCmd(["capture-pane", "-t", "dev", "-p"]);
console.log(content);

// Clean up
muxCmd(["kill-session", "-t", "dev"]);
```

### Go

```go
package main

import (
    "fmt"
    "os/exec"
    "strings"
)

func muxCmd(args ...string) (string, error) {
    out, err := exec.Command("tmux", args...).Output()
    return strings.TrimSpace(string(out)), err
}

func main() {
    muxCmd("new-session", "-d", "-s", "dev", "-x", "120", "-y", "30")
    muxCmd("send-keys", "-t", "dev", "echo hello", "Enter")
    content, _ := muxCmd("capture-pane", "-t", "dev", "-p")
    fmt.Println(content)
    muxCmd("kill-session", "-t", "dev")
}
```

### Rust

```rust
use std::process::Command;

fn mux_cmd(args: &[&str]) -> String {
    let output = Command::new("tmux")
        .args(args)
        .output()
        .expect("failed to run tmux/psmux");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn main() {
    mux_cmd(&["new-session", "-d", "-s", "dev", "-x", "120", "-y", "30"]);
    mux_cmd(&["send-keys", "-t", "dev", "echo hello", "Enter"]);
    let content = mux_cmd(&["capture-pane", "-t", "dev", "-p"]);
    println!("{}", content);
    mux_cmd(&["kill-session", "-t", "dev"]);
}
```

## libtmux Integration

[libtmux](https://github.com/tmux-python/libtmux) is the most popular Python library for controlling tmux programmatically. psmux is compatible with libtmux because it implements the same commands and output formats.

### Setup

```powershell
pip install libtmux
```

### Basic Usage

```python
import libtmux

# Connect to the psmux server
server = libtmux.Server(socket_name="default")

# List sessions
for session in server.sessions:
    print(f"{session.name} ({session.id}): {len(session.windows)} windows")

# Work with a session
session = server.sessions[0]

# Create a window
window = session.new_window(window_name="build")

# Access panes
pane = window.panes[0]

# Send commands
pane.send_keys("cargo build")

# Capture output
lines = pane.capture_pane()
for line in lines:
    print(line)

# Kill the window
window.kill()
```

### Windows Encoding Fix

libtmux uses the Unicode character U+241E (SYMBOL FOR RECORD SEPARATOR) internally to split format fields when querying tmux. On Linux, this works transparently because both tmux and Python use UTF-8.

On Windows, Python's `subprocess.Popen(text=True)` defaults to cp1252 encoding, which garbles the 3-byte UTF-8 sequence for U+241E. This causes `server.sessions` and similar queries to return empty results or parse errors.

**Option 1**: Set `PYTHONUTF8=1` before running your script:

```powershell
$env:PYTHONUTF8 = "1"
python my_script.py
```

**Option 2**: Patch libtmux locally. In your installed libtmux package, edit `common.py` and add `encoding="utf-8"` to the `Popen` call in the `tmux_cmd.__init__` method:

```python
subprocess.Popen(
    cmd, stdout=PIPE, stderr=PIPE, text=True,
    encoding="utf-8", errors="backslashreplace"
)
```

This is an upstream libtmux issue (not psmux-specific). The library should specify encoding explicitly for cross-platform compatibility.

### libtmux API Coverage

The following libtmux operations are verified working with psmux:

| Operation | Status | Notes |
|-----------|--------|-------|
| `Server(socket_name="default")` | Works | Connects to the running psmux server |
| `server.sessions` | Works | Returns all sessions (needs encoding fix on Windows) |
| `session.id` (`$N`) | Works | Returns the stable session ID |
| `session.windows` | Works | Lists all windows in the session |
| `session.new_window()` | Works | Creates a new window |
| `window.id` (`@N`) | Works | Returns the stable window ID |
| `window.panes` | Works | Lists all panes in the window |
| `pane.id` (`%N`) | Works | Returns the stable pane ID |
| `pane.send_keys()` | Works | Sends keystrokes to the pane |
| `pane.capture_pane()` | Works | Captures visible pane content |
| `window.kill()` | Works | Destroys the window |
| `session.kill()` | Works | Destroys the session |
| `server.has_session()` | Works | Checks if a session exists |
| Custom format queries (`-F`) | Works | All 140+ format variables supported |

## Control Mode Integration

For persistent, event-driven integration (IDE plugins, session managers, monitoring tools), use control mode. See [control-mode.md](control-mode.md) for the full protocol reference.

### Quick Example

```python
import subprocess
import threading

proc = subprocess.Popen(
    ["psmux", "-CC"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    encoding="utf-8",
)

def reader():
    for line in proc.stdout:
        line = line.rstrip("\n")
        if line.startswith("%output"):
            _, pane_id, *data = line.split(" ", 2)
            print(f"[{pane_id}] {data[0] if data else ''}")
        elif line.startswith("%window-add"):
            print(f"Window created: {line}")
        elif line.startswith("%session-changed"):
            print(f"Session changed: {line}")

t = threading.Thread(target=reader, daemon=True)
t.start()

# Send commands
proc.stdin.write("list-windows\n")
proc.stdin.flush()

proc.stdin.write("new-window -n monitor\n")
proc.stdin.flush()

proc.stdin.write('send-keys "Get-Process" Enter\n')
proc.stdin.flush()
```

### psmux Extension Commands

In addition to the 83 standard tmux commands, psmux provides extra commands useful for rich integrations:

| Command | Description |
|---------|-------------|
| `dump-state` | Full session state as JSON (windows, panes, options, screen content) |
| `dump-layout` | Pane layout tree structure |
| `list-tree` | Hierarchical session/window/pane tree |
| `send-text <text>` | Send raw text to active pane (no key name parsing) |
| `send-paste <text>` | Send text as a bracketed paste sequence |
| `claim-session` | Claim a warm (pre-spawned) session for instant startup |
| `set-pane-title <title>` | Set pane title directly |
| `toggle-sync` | Toggle synchronized input for all panes in a window |
| `zoom-pane` | Toggle zoom on the active pane |
| `new-pane` (`newp`) | Create a floating pane above the tiled layout. With `-P` it prints the new pane id |

`new-pane` is also a CLI command, so a tool can create an overlay pane and get its id back in one
call:

```powershell
psmux new-pane -d -P -X 10 -Y 5 -x 60 -y 20 -T "agent log"
# %4
```

A floating pane is not part of the window's layout tree, so it does not appear in `list-panes`
output and `select-layout` leaves it alone. See
[scripting.md, "new-pane (floating panes)"](scripting.md#new-pane-floating-panes).

### Text-input route signal (`#{pane_last_text_input}`)

A read-only format variable: **milliseconds since printable text last reached
this pane via the interactive input route** (empty until the first one).

```powershell
psmux display-message -t dev -p '#{pane_last_text_input}'   # e.g. "740", or "" if none yet
```

It is a **route** signal, not human-presence detection. The contract:

- **Interactive route** (`handle_key -> forward_key_to_active`) **updates** it.
- **Injected route** (`send-keys` / `send-paste` / `send-text` ->
  `send_text_to_active`) does **not** update it. App output never does either,
  so it distinguishes interactive text from injected text, something
  `capture-pane` can't.
- **Key scope:** printable text counts; `Enter`, arrows/navigation, shortcuts
  and any `Ctrl`/`Alt` chord do not.
- **Caveat:** a bot that injects *real key events* through the interactive
  route (not via `send-keys`) will also update it. This measures the route,
  not who's behind it.

Useful when a tool drives a pane programmatically and wants to yield the moment
typing arrives on the interactive route. Consumers own all policy (e.g. treat
"value < N ms" as "active"); psmux just exposes the timestamp, kept on the pane
(no file, freed with the pane).

### Special-key route signal (`#{pane_last_special_key}`)

The sibling of `#{pane_last_text_input}` for **non-text** keys. Two read-only
format variables describing the last key, other than printable text, that
reached this pane via the interactive input route:

- `#{pane_last_special_key}` is its canonical bind-key name (`Escape`, `Enter`,
  `Tab`, `Up`, `F9`, `C-c`, `M-a`, ...), empty until the first one.
- `#{pane_last_special_key_ms}` is milliseconds since it arrived, empty if none.

```powershell
psmux display-message -t dev -p '#{pane_last_special_key} #{pane_last_special_key_ms}'
# e.g. "Escape 320", or " " if none yet
```

Same route contract as `#{pane_last_text_input}`:

- **Interactive route** (`handle_key -> forward_key_to_active`) **updates** it.
- **Injected route** (`send-keys` / `send-paste` / `send-text`) does **not**.
- **Scope:** every key that is *not* printable text input: `Escape`, `Enter`,
  `Tab`, `Backspace`, arrows/navigation, function keys, and any `Ctrl`/`Alt`
  chord. Printable text goes to `#{pane_last_text_input}` instead; together the
  two partition all interactive keys. Names come from the same renderer
  `list-keys` uses.

Consumers own all policy (e.g. "name is `Escape` and `_ms` < N"); psmux just
exposes the last key + its age, kept on the pane (no file, freed with it).

## Machine-Readable Format Variables

These are the variables worth reaching for when a tool, rather than a human, is reading psmux
state. Query them with `display-message -p` for one value, or with `-F` on a list command for one
row per object. The full catalogue, including the human facing status bar variables, is in
[scripting.md, "Format Variables"](scripting.md#format-variables).

| Variable | Example | Description |
|----------|---------|-------------|
| `#{session_id}` | `$0` | Stable session id, safe to key on across renames |
| `#{window_id}` | `@1` | Stable window id |
| `#{pane_id}` | `%3` | Stable pane id |
| `#{session_name}` | `work` | Session name, may change under the tool's feet |
| `#{session_created}` | `1785161678` | Session creation time as a unix timestamp |
| `#{session_path}` | `C:\Projects\app` | Directory the session was created in |
| `#{session_group}` | `backend` | Session group name, empty if ungrouped |
| `#{window_layout}` | `a8fe,120x30,0,0,1` | tmux layout string with checksum. Capture it and hand it back to `select-layout` to restore geometry |
| `#{window_flags}` | `*` | Rendered window flag string |
| `#{pane_pid}` | `32944` | PID of the pane's shell, for process tree work |
| `#{pane_tty}` | `/dev/pty1` | Pseudo terminal name |
| `#{pane_current_command}` | `pwsh` | Executable name of the pane's immediate child. Never a program modified process title on Windows, see "Identifying the Program Running in a Pane" below |
| `#{pane_title}` | `openclaw-gateway` | Console title of the pane, which is where a Windows program's logical name surfaces. Requires `allow-set-title on` |
| `#{pane_start_command}` | `node server.mjs` | The command psmux was asked to run in the pane, empty when the pane got the default shell |
| `#{pane_current_path}` | `C:\Projects\app` | Working directory, in native Windows form. Read from the foreground process. Inside `wsl` or `ssh` there is no Windows process that knows the answer, so it uses the directory the shell announced over `OSC 7` or `OSC 9;9` and keeps the last observed one when the shell announces nothing. See the WSL entry in [the FAQ](faq.md) |
| `#{pane_path}` | `/mnt/c/Users` | Exactly what the shell announced over `OSC 7` or `OSC 9;9`, untranslated, or empty when it announced nothing |
| `#{pane_dead}` | `0` | `1` when the process exited and `remain-on-exit` kept the pane |
| `#{pane_in_mode}` | `0` | `1` when the pane is in copy mode or another mode |
| `#{pane_mode}` | `copy-mode` | Name of the current mode, empty when in none |
| `#{pane_at_top}` / `#{pane_at_bottom}` / `#{pane_at_left}` / `#{pane_at_right}` | `1` | Whether the pane touches that window edge, for edge aware key routing |
| `#{history_size}` | `240` | Lines currently held in the pane's scrollback |
| `#{scroll_position}` | `0` | Lines scrolled back from the live bottom |
| `#{cursor_x}` / `#{cursor_y}` | `60` / `0` | Cursor position in the active pane, zero based |
| `#{selection_present}` | `1` | `1` when a copy mode selection exists |
| `#{buffer_size}` / `#{buffer_name}` / `#{buffer_sample}` / `#{buffer_created}` | `12` / `config` | Paste buffer metadata |
| `#{client_pid}` | `32944` | PID of the attached client |
| `#{client_key_table}` | `root` | Key table the client is currently in |
| `#{version}` | `3.3.7` | psmux version, for capability gating |
| `#{pid}` / `#{server_pid}` | `19004` | PID of the server process that answered. **Session-scoped**, see below |
| `#{server_instance}` | `b644f0a347fa5e14` | Stable identity of the `-L` namespace. Poll this to detect a real restart |
| `#{socket_path}` | `C:\Users\me/.psmux/default` | The server that answered: `<psmux dir>/default`, `<psmux dir>/<label>` under `-L`, or the exact path given to `-S`. Passing it back to `-S` reaches the same server |
| `#{host}` / `#{host_short}` / `#{user}` | `BOX` / `me` | Host and user identity |

> **Supervising a namespace.** Unlike tmux, psmux runs one server process per
> session, so `#{pid}` (and its alias `#{server_pid}`) report whichever session's
> server handled the request. Creating a session changes the value even though
> nothing restarted. A watchdog that polls `#{pid}` to answer *"is this still the
> server I was talking to?"* will read every new session as a server restart.
>
> Poll `#{server_instance}` instead. It is minted by the first server in a `-L`
> namespace, reported identically by every server in that namespace, and changes
> only when the namespace has genuinely gone away and come back. An unknown or
> not-yet-started namespace reports an empty value, which should be treated as
> *unknown* rather than as a restart.

Any option name also resolves inside `#{...}`, which is usually cheaper and more reliable than
parsing `show-options` output:

```powershell
psmux display-message -p "#{mouse}"            # on
psmux display-message -p "#{history-limit}"    # 2000
psmux display-message -p "#{@my-tool-state}"   # a bare @name is a user option
```

### Identifying the Program Running in a Pane

Three variables answer three different questions, and a supervisor that treats any one of them as
a service identity will eventually be wrong. This came out of a gateway automation report
([#647](https://github.com/psmux/psmux/issues/647)).

**`#{pane_current_command}` is an executable name.** It reports the image of the pane's immediate
child, so a pane running `node server.mjs` reports `node` and returns to `pwsh` the moment that
process exits. It is stable and cheap to poll. It can never reflect a program modified process
title on Windows: `process.title` in Node, or the equivalent in any runtime, changes nothing that
the process tree exposes. `Win32_Process.Name` stays the image name, a console process has no
main window title, and ConPTY has no `tcgetpgrp` equivalent that would identify a foreground
process group. On Linux, tmux reads the controlling terminal's foreground process group and
therefore does show the modified title there; that difference is a platform limit, not a psmux
choice.

**`#{pane_title}` is the console title, and this is where the logical name actually appears.** A
Windows program that names itself calls `SetConsoleTitleW`, which is exactly what Node's
`process.title` setter does. ConPTY turns that call into an OSC title on the pane's output
stream, and psmux parses it into the pane title. The pane title is only updated when
`allow-set-title` is on, which is not the default:

```powershell
psmux set-option -g allow-set-title on
psmux list-panes -t gateway -F '#{pane_id}|#{pane_current_command}|#{pane_title}'
# %1|node|openclaw-gateway
```

The value is valid only while the program that set it owns the console. An interactive shell
rewrites the title constantly: PowerShell sets it to the working directory on every prompt, so
the title of an idle shell pane tells you nothing about any service. It is trustworthy for a
long running foreground process and not for a prompt. See [pane-titles.md](pane-titles.md) for
the wider consequences of turning `allow-set-title` on, including its effect on the status bar.

**`#{pane_start_command}` is the command psmux was asked to run.** It records what was passed at
pane creation, so it survives the process exiting and is not affected by anything the program
does to itself. It is **empty** for a pane that was given the default shell, which includes the
first pane of a plain `new-session`, so a supervisor that relies on it must start its service
pane with an explicit command:

```powershell
psmux new-window -d -t gateway: -n api -- node server.mjs --port 18789
psmux display-message -p -t gateway:api '#{pane_start_command}'
# node server.mjs --port 18789
```

**Recommended recipe for identifying a service.** No single variable is sufficient. Combine four
signals, in roughly this order of reliability:

1. A dedicated window or pane name that your controller chose, addressed by the stable
   `#{window_id}` or `#{pane_id}` so a rename cannot break the link.
2. `#{pane_start_command}`, which is what you asked for and cannot drift.
3. Pane liveness, `#{pane_dead}` plus `#{pane_pid}`, to tell a running service from a pane that
   `remain-on-exit` is holding open.
4. A real health check that does not involve psmux at all, such as connecting to the port the
   service is supposed to be listening on.

`#{pane_title}` is a useful fifth signal once `allow-set-title` is on, and `#{pane_current_command}`
is a reasonable coarse filter, for example to tell a `node` pane from a `pwsh` one. Neither
should be the thing your controller keys on.

### Accepted but not yet meaningful

About twenty five names exist for tmux format compatibility but always return a fixed
placeholder. They will expand without error, which makes them a quiet source of wrong behaviour
in a tool that keys on them. Do not build on these:

`session_stack`, `window_bigger`, `window_offset_x`, `window_offset_y`, `window_stack_index`,
`window_cell_width`, `window_cell_height`, `window_linked_sessions_list`, `pane_dead_signal`,
`pane_dead_status`, `pane_dead_time`, `pane_start_path`, `pane_tabs`, `cursor_flag`,
`scroll_region_upper`, `client_utf8`, `client_cell_width`, `client_cell_height`,
`client_written`, `client_discarded`, `alternate_saved_x`, `alternate_saved_y`, `origin_flag`,
`insert_flag`, `keypad_cursor_flag`, `keypad_flag`, `wrap_flag`, `line`, `command`,
`command_list_name`, `command_list_alias`, `command_list_usage`, `config_files`.

To tell attached clients apart, use `list-clients -F '#{client_name} #{client_pid}
#{client_readonly} #{client_flags}'` or `display-message -c <client> -p '...'`: those are live
per client values. The per-variable placeholder values are tabulated in
[scripting.md, "Accepted but not yet meaningful"](scripting.md#accepted-but-not-yet-meaningful).

## Named Paste Buffers

psmux supports named paste buffers for structured inter-pane data exchange:

```powershell
# Set a named buffer
psmux set-buffer -b config "key=value"

# Read it from another pane or script
psmux show-buffer -b config

# Delete when done
psmux delete-buffer -b config

# Paste into the active pane
psmux paste-buffer -b config
```

Named buffers are useful for passing structured data between automation steps without relying on environment variables or temporary files.

## Cross-Platform Project Structure

For projects that need to work on both Linux/macOS (tmux) and Windows (psmux), here is a recommended pattern:

### 1. Use the `tmux` Binary Name

psmux installs `tmux.exe` as an alias. Your code can call `tmux` on all platforms:

```python
binary = "tmux"  # Works on Linux (real tmux) and Windows (psmux alias)
```

### 2. Set Encoding on Windows

The only platform-specific code you need:

```python
import platform

def get_mux_kwargs():
    kwargs = {"capture_output": True, "text": True}
    if platform.system() == "Windows":
        kwargs["encoding"] = "utf-8"
    return kwargs
```

### 3. Handle Path Separators

tmux uses Unix paths (`/home/user/project`), psmux uses Windows paths (`C:\Users\user\project`). Format variables like `#{pane_current_path}` return the native path format. If your code compares paths, normalize them:

```python
from pathlib import Path

pane_path = Path(mux_cmd(["display-message", "-p", "#{pane_current_path}"]))
```

### 4. Shell Differences

On Linux, the default shell in tmux is usually `bash` or `zsh`. On Windows, psmux defaults to PowerShell 7 (`pwsh`). Keep this in mind when sending commands:

```python
import platform

if platform.system() == "Windows":
    mux_cmd(["send-keys", "-t", target, "Get-ChildItem", "Enter"])
else:
    mux_cmd(["send-keys", "-t", target, "ls -la", "Enter"])
```

### 5. Test Matrix

A typical CI/CD matrix for a cross-platform tmux integration:

```yaml
# GitHub Actions example
strategy:
  matrix:
    os: [ubuntu-latest, windows-latest]
    include:
      - os: ubuntu-latest
        mux: tmux
      - os: windows-latest
        mux: psmux

steps:
  - name: Install multiplexer
    run: |
      if [ "${{ matrix.mux }}" = "psmux" ]; then
        cargo install --git https://github.com/psmux/psmux
      else
        sudo apt-get install -y tmux
      fi
    shell: bash

  - name: Run integration tests
    run: python -m pytest tests/test_mux_integration.py
    env:
      PYTHONUTF8: "1"
```

## Environment Variables

psmux sets these environment variables in child processes, matching tmux:

| Variable | Example | Description |
|----------|---------|-------------|
| `TMUX` | `/tmp/psmux-58828/default,51961,0` | Indicates a tmux/psmux session is active. The shape is `/tmp/psmux-<server pid>/<socket name>,<port>,0` (for a server started with `-S`, the first field is that exact path), so the middle field is the server's TCP port rather than a Unix pid. The first field passed to `-S` reaches the same server |
| `TMUX_PANE` | `%1` | The pane ID of the current pane |
| `PSMUX_SESSION` | `work` | The session the pane belongs to (psmux extension) |
| `TERM` | `xterm-256color` | Terminal type |
| `COLORTERM` | `truecolor` | Indicates 24-bit color support |

Tools that check for `$TMUX` to detect tmux will correctly detect psmux as well. Git Bash and
MSYS2 shells are told not to convert `TMUX` into a Windows path (`MSYS2_ENV_CONV_EXCL=TMUX`), so
the value survives intact there too. Windows gives each process a private copy of its environment,
so psmux can only set these when the pane's first process starts; nothing can add, change or remove
a variable inside a program that is already running.

### Propagating Environment Variables

Use `set-environment` to pass configuration to panes:

```powershell
# Global: all new panes inherit this
psmux set-environment -g API_KEY "sk-..."

# Session-scoped
psmux set-environment PROJECT_ROOT "C:\Projects\myapp"

# On session creation
psmux new-session -s work -e "NODE_ENV=development"
```

## Server Namespaces

Use `-L` to run isolated psmux instances (each with its own sessions, windows, and options):

```powershell
# Create isolated servers for different projects
psmux -L frontend new-session -d -s app
psmux -L backend new-session -d -s api

# Each namespace is completely independent
psmux -L frontend list-sessions   # Only shows "app"
psmux -L backend list-sessions    # Only shows "api"

# Attach to a specific namespace
psmux -L frontend attach -t app
```

In control mode, the session name includes the namespace:

```powershell
$env:PSMUX_SESSION_NAME = "frontend__app"
psmux -CC
```

The double underscore separates namespace from session name.

### A Separate Data Root

`-L` shares one registry directory and prefixes the names. For a tool that must not see or touch
the user's own sessions at all (a test harness, a sandboxed agent), point `PSMUX_DATA_DIR` at an
absolute directory of its own instead. Everything psmux keeps on disk (`.port`, `.key`, `.pid`,
`last_session`, the warm server) lives under that root, and two roots can hold sessions of the
same name at the same time, including their own `__warm__` standbys
([#599](https://github.com/psmux/psmux/issues/599)). Set it in the environment of every psmux
process you launch, server and CLI alike:

```powershell
$env:PSMUX_DATA_DIR = "C:\work\agent-registry"
psmux new-session -d -s work      # invisible to a plain `psmux ls` in another shell
```

### Which Session a Bare Command Hits

A command with no `-t` and no `$TMUX` in its environment (a script run from a plain PowerShell
window, for example) is routed to the session with the most recent activity: the last one a client
attached to or typed into, ranked the way tmux's `cmd_find_best_session` ranks by `activity_time`
([#603](https://github.com/psmux/psmux/issues/603)). A session that was attached once and detached
long ago does not outrank the one the user is sitting in. Do not rely on this in a tool: pass `-t`
with the session name or `$N` id every time.

## Targeting Syntax Reference

psmux supports the full tmux target syntax for the `-t` flag:

| Target | Meaning |
|--------|---------|
| `mysession` | Session by name |
| `$0` | Session by stable ID |
| `mysession:2` | Window 2 in session "mysession" |
| `mysession:editor` | Window named "editor" in session "mysession" |
| `:2` | Window 2 in the current session |
| `@3` | Window by stable ID |
| `%5` | Pane by stable ID |
| `mysession:2.1` | Pane 1 of window 2 in session "mysession" |
| `.+1` | Next pane |
| `.-1` | Previous pane |
| `=mysession` | Session by name, exact match |

Prefer the stable ids (`$N`, `@N`, `%N`) in a tool. Names and indices move when the user renames
a window, reorders windows, or has `renumber-windows` on.

psmux also has geometric pane tokens (`{top-right}`, `{bottom-left}` and friends), but they are
resolved server side and only by `swap-pane`. A tool calling the CLI cannot use them: the front
end parses a leading `{` in a `-t` value as a session name. See
[scripting.md, "Positional pane targets"](scripting.md#positional-pane-targets).

### Moving a Pane Between Sessions

`join-pane` and `move-pane` accept a `-s` source in another session, including one on an
independent server. The pane's real console stays put and its input and output are tunnelled over
TCP, so a long running process survives the move:

```powershell
psmux new-session -d -s alpha
psmux new-session -d -s beta
psmux -t alpha join-pane -h -s 'beta:0.0'
```

### Session Groups

`set -g session-group <name>` tags a session so a tool can treat several sessions as one logical
unit. `#{session_group}`, `#{session_group_list}`, `#{session_group_size}`,
`#{session_group_attached}` and `#{session_grouped}` report the grouping and work in any `-F`
format.

## Hooks for Event-Driven Automation

Hooks let you react to session events without polling:

```powershell
# Run a script when a new window is created
psmux set-hook -g after-new-window "run-shell 'echo window created >> events.log'"

# Notify on session attach
psmux set-hook -g client-attached "display-message 'Welcome back!'"

# Auto-layout on split
psmux set-hook -g after-split-window "select-layout tiled"
```

psmux fires 30 hook events. The canonical list, with what each one fires on, lives in
[scripting.md, "Available Hook Events"](scripting.md#available-hook-events). It is maintained in
one place so the two documents cannot drift.

Three things matter when a tool installs hooks rather than a human:

1. **Hook names are not validated.** `set-hook` stores any name it is given. A typo is accepted
   silently, shows up in `show-hooks` like a real hook, and simply never fires. There is no
   error to catch. After installing hooks, read `show-hooks` back and diff it against what you
   intended.
2. **Use `-a` / `-ga` to coexist with other tools.** The plain form replaces the whole handler
   list for that event, which will silently uninstall another tool's handler. The append form
   keeps both. Appends are deduplicated, so a tool that re-runs its own setup, or a user who
   re-sources a config, cannot stack duplicate handlers.
3. **Clean up with `-u` / `-gu`.** That removes every handler registered for the event, so
   remove and reinstall rather than trying to remove one entry of several.

```powershell
psmux set-hook -ga after-new-window "run-shell 'my-tool notify window'"
psmux show-hooks
# after-new-window[0] -> select-layout tiled
# after-new-window[1] -> run-shell 'my-tool notify window'
```

Many of these events also surface as control mode notifications (`%window-add`, `%window-close`,
`%window-renamed`, `%session-window-changed`, `%window-pane-changed`, `%session-renamed`,
`%session-changed`, `%client-detached`, `%layout-change`), so a `-C` / `-CC` client often does not
need to install hooks at all. See [control-mode.md](control-mode.md).

## Synchronization with `wait-for`

For multi-step automation that needs coordination between panes:

```powershell
# Pane 1: Wait for a signal
psmux send-keys -t %0 "psmux wait-for ready && echo 'proceeding'" Enter

# Pane 2: Do some work, then signal
psmux send-keys -t %1 "cargo build && psmux wait-for -S ready" Enter
```

`wait-for` supports `-L` (lock), `-S` (signal/unlock), and bare wait. Use it for producer/consumer patterns across panes.

## Troubleshooting

### "no server running", "no sessions" and "can't find session" Errors

psmux requires a running session. A bare `psmux attach` with nothing to attach to prints
`no sessions` and exits 1; `psmux attach -t work` against a session that does not exist, or whose
server has gone away and left a stale `.port` behind, prints `can't find session: work` and exits
1 (and reaps the stale registration). `psmux ls` with nothing running prints
`no server running on <data dir>` and exits 1, and `kill-session -t work` on a name that is not a
session prints `can't find session: work` and exits 1. Those are tmux's words for the same
situations, and a tool can match on them. Create the session first:

```powershell
psmux new-session -d -s work
```

Or use `has-session` to check:

```powershell
psmux has-session -t work 2>$null
if ($LASTEXITCODE -ne 0) {
    psmux new-session -d -s work
}
```

### Empty Results from Format Queries on Windows

If `list-sessions -F`, `list-windows -F`, or `list-panes -F` returns garbled or empty output, your process is decoding psmux's UTF-8 output with the wrong encoding. See the [encoding section](#windows-encoding-fix) above.

### "unknown command" for a `command-alias`

`psmux x` asks the server for its `command-alias` array when `x` is not a psmux command, so it
fails with `psmux: unknown command: x` when no server runs in that namespace or that server has
no alias `x`. Check with `psmux show-options -s command-alias`. An alias named after a real
command (`ls`) is never looked up from the command line. See
[scripting.md, "User Defined Command Aliases"](scripting.md#user-defined-command-aliases).

### Control Mode Connection Issues

If `psmux -CC` exits immediately, ensure a session exists and `PSMUX_SESSION_NAME` is set:

```powershell
psmux new-session -d -s work
$env:PSMUX_SESSION_NAME = "work"
psmux -CC
```

### ConPTY Differences from Unix PTY

When porting Unix tmux integrations to Windows:

- **Alternate screen buffer**: `#{alternate_on}` reports `1` while a full screen program (nvim, htop, less) holds the alternate screen and `0` at a shell prompt, the same as tmux. It is read from the pane's own parser, so a program that switches buffers through the Win32 console API rather than by writing `ESC [ ? 1049 h` (a plain `Write-Host` of the sequence from PowerShell, for example) does not flip it. Real TUIs write the sequence and are detected.
- **Mouse tracking flags**: `#{mouse_any_flag}`, `#{mouse_standard_flag}`, `#{mouse_button_flag}`, `#{mouse_all_flag}`, `#{mouse_utf8_flag}` and `#{mouse_sgr_flag}` report the pane's own DECSET state, the same six variables tmux publishes and the same reading, so tmux's default wheel binding works unchanged:

  ```
  bind -n WheelUpPane { if -F '#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}' { send -M } { copy-mode -e } }
  ```

  Two Windows caveats. First, the flags describe the pane's terminal, and under ConPTY the terminal is a console: conhost publishes the console input mode word upstream, so a program that merely switches its stdin to raw mode can leave a pane reporting `mouse_all_flag=1` and `mouse_sgr_flag=1` without ever writing a DECSET, and the mode granularity an application asked for is only preserved when its own sequence comes after that. Second, psmux's own wheel handling does not simply follow `mouse_any_flag`: it also asks who turned the mode on, so that PowerShell's PSReadLine enabling tracking at a prompt does not take the scrollback away from the user ([#548](https://github.com/psmux/psmux/issues/548), [#598](https://github.com/psmux/psmux/issues/598)). The flags stay honest about what the pane reports; they are not a prediction of psmux's gate in that one case.
- **Output normalization**: ConPTY may normalize line endings. `%output` data may differ slightly from Unix tmux output.
- **Ctrl+C**: tmux writes a raw `0x03` and lets the pane's tty discipline decide. On Windows psmux routes `C-c` by what is in the foreground of the pane: a shell prompt or a native console program gets a console `CTRL_C_EVENT`, a raw mode TUI (vim, Copilot CLI) gets the byte, and a bridge such as `wsl.exe` or `ssh.exe` gets the byte with console processing turned off so conhost cannot convert it into a console wide event ([#579](https://github.com/psmux/psmux/issues/579)). Prefer app-specific quit keys over `C-c` in automation where you can.
- **Closing the terminal window**: closing the Windows Terminal tab or console window that hosts an attached client detaches that client, exactly as closing an xterm running `tmux attach` does. The server and every process inside every pane keep running until you `kill-session` or `kill-server` ([#585](https://github.com/psmux/psmux/issues/585)). A tool that wants "close the window, stop the work" must issue the kill itself.
- **TUI exit timing**: After a TUI exits, ConPTY needs 4 to 6 seconds to restore the screen. Add a delay before `capture-pane` after TUI exit.

## Related Documentation

- [compatibility.md](compatibility.md) : Full tmux command and feature compatibility matrix
- [control-mode.md](control-mode.md) : Control mode wire protocol reference
- [scripting.md](scripting.md) : Command reference and scripting examples
- [configuration.md](configuration.md) : All options and config file format
- [claude-code.md](claude-code.md) : Claude Code agent team integration
- [features.md](features.md) : Complete feature list
