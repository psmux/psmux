# Control Mode

Control mode lets external programs drive psmux programmatically over a structured text protocol. Instead of rendering a TUI, psmux sends machine-readable notifications and accepts commands over stdin/stdout, making it the foundation for building plugins, IDE integrations, custom dashboards, session monitors, and any tooling that needs to interact with terminal sessions.

This is the same protocol that tmux uses for its control mode (`tmux -C` / `tmux -CC`), so existing knowledge and many client libraries transfer directly to psmux.

## Quick Start

```powershell
# 1. Create a detached session
psmux new-session -d -s work -x 120 -y 30

# 2. Attach in control mode (no-echo)
psmux -CC
```

psmux connects to the running session and enters a command/response loop. You type commands on stdin, and psmux responds on stdout with structured output.

```
list-windows
%begin 1700000000 1 1
0: pwsh* (1 panes) [120x30]
%end 1700000000 1 1
```

To exit, close stdin (Ctrl+D / EOF) or send `kill-server`.

## Flags

| Flag | Mode | Behavior |
|------|------|----------|
| `-C` | Echo | Commands you send are echoed back to stdout before the response. Useful for debugging and interactive testing. |
| `-CC` | No-echo | Commands are not echoed. This is the mode you want for programmatic use. In this mode, `%exit` is followed by an ST sequence (`ESC \`). |

## Session Targeting

By default, control mode connects to the session stored in `PSMUX_SESSION_NAME`. You can set it before launching:

```powershell
$env:PSMUX_SESSION_NAME = "my-session"
psmux -CC
```

## Wire Protocol

### Command/Response Framing

Every command you send gets a response wrapped in `%begin` / `%end` (or `%error`) markers:

```
<your command>
%begin <timestamp> <command_number> <flags>
<response lines>
%end <timestamp> <command_number> <flags>
```

| Field | Description |
|-------|-------------|
| `timestamp` | Unix epoch seconds when the command was processed |
| `command_number` | Sequential counter (1, 2, 3, ...) for each command in the session |
| `flags` | Reserved, always `1` |

The `%begin` and `%end` lines always share the same timestamp, command number, and flags. If a command fails, the closing frame is `%error` instead of `%end`:

```
nonexistent-command
%begin 1700000000 1 1
unknown command: nonexistent-command
%error 1700000000 1 1
```

Command response blocks never interleave with each other. Notifications (described below) arrive between command blocks, never inside them.

### Notifications

Notifications are asynchronous lines that psmux sends whenever something happens in the session. They always start with `%` and arrive between command response blocks.

#### Window Notifications

| Notification | Meaning |
|---|---|
| `%window-add @<WID>` | A new window was created |
| `%window-close @<WID>` | A window was destroyed |
| `%window-renamed @<WID> <name>` | A window was renamed |
| `%window-pane-changed @<WID> %<PID>` | The active pane in a window changed |
| `%layout-change @<WID> <layout> <visible_layout> <flags>` | A window's pane layout changed (split, resize, etc.) |

#### Session Notifications

| Notification | Meaning |
|---|---|
| `%session-changed $<SID> <name>` | The attached session changed |
| `%session-renamed <name>` | The current session was renamed |
| `%session-window-changed $<SID> @<WID>` | The active window in a session changed |
| `%sessions-changed` | A session was created or destroyed |

#### Pane Output

| Notification | Meaning |
|---|---|
| `%output %<PID> <escaped_data>` | A pane produced output |
| `%pane-mode-changed %<PID>` | A pane entered or exited a special mode (e.g. copy mode) |

#### Flow Control

| Notification | Meaning |
|---|---|
| `%pause %<PID>` | Output for this pane has been paused (client is too far behind) |
| `%continue %<PID>` | Output for this pane has resumed |

#### Client and Buffer

| Notification | Meaning |
|---|---|
| `%client-detached <client>` | A client disconnected from the session |
| `%client-session-changed <client> $<SID> <name>` | Another client changed its attached session |
| `%paste-buffer-changed <name>` | A paste buffer was modified |
| `%paste-buffer-deleted <name>` | A paste buffer was deleted |
| `%message <text>` | A status message was generated (e.g. from `display-message`) |

#### Exit

| Notification | Meaning |
|---|---|
| `%exit` | The control client is disconnecting. In `-CC` mode, followed by `ESC \` (ST sequence). |
| `%exit <reason>` | Disconnecting with a reason (e.g. `too far behind`). |

### ID Formats

All IDs are stable, monotonically increasing integers that never get reused during a server's lifetime:

| Prefix | Entity | Example |
|--------|--------|---------|
| `$` | Session | `$0` |
| `@` | Window | `@0`, `@1`, `@2` |
| `%` | Pane | `%0`, `%1`, `%2` |

### Output Escaping

Data in `%output` notifications uses octal escaping for non-printable bytes:

| Byte | Encoding |
|------|----------|
| Printable ASCII (0x20 to 0x7E) | Passed through as-is |
| Tab (0x09) | Passed through as-is |
| Backslash (0x5C) | `\\` (doubled) |
| Carriage return (0x0D) | `\015` |
| Line feed (0x0A) | `\012` |
| Any other byte | `\NNN` (3-digit octal) |

Example: `hello\r\n` becomes `%output %0 hello\015\012`.

## Supported Commands

Control mode has its own command dispatcher, so it accepts a deliberate subset of the full psmux command set rather than everything the CLI accepts. Anything outside that subset comes back as `%error unknown command: <name>`. Send `list-commands` to see the catalog. Everything shown below is dispatched in control mode.

### Session and Window Management

```
new-window                     # Create a new window
new-window -n editor           # Create a named window
split-window -v                # Split vertically
split-window -h                # Split horizontally
kill-pane                      # Kill the active pane
kill-window                    # Kill the active window
select-window -t 1             # Switch to window 1
select-pane -t %3              # Switch to pane %3
rename-window new-name         # Rename the active window
rename-session new-name        # Rename the session
```

### Querying State

```
list-windows                        # List all windows
list-windows -F '#{window_id}'      # Custom format
list-panes                          # List panes in active window
list-panes -a                       # List all panes across all windows
list-sessions                       # List sessions
list-clients                        # List connected clients
display-message -p '#{pane_id}'     # Print a format variable
has-session -t my-session           # Check if session exists (exit code)
```

### Interacting with Panes

```
send-keys -t %0 "echo hello" Enter  # Send keystrokes to a pane
send-keys -t %0 -l "literal text"   # Send text literally (no key parsing)
capture-pane -t %0 -p               # Capture the visible content of a pane
```

### Configuration and Hooks

```
set-option -g status-style "bg=blue"              # Set an option
show-options -g                                     # Show all global options
set-hook -g after-new-window "display-message hi"  # Set a hook
bind-key M-x display-message "pressed!"            # Bind a key
```

### Server

```
list-commands      # List all available commands
server-info        # Server information
show-messages      # The same text as server-info
kill-server        # Shut down the server
```

### psmux Extension Commands

These commands exist in psmux but not in tmux. The "In control mode" column matters: only the first three are wired into the control mode dispatcher. The rest are server commands, reachable from a key binding, a config line or a raw socket connection, but a control mode client gets `%error unknown command` for them.

| Command | In control mode | Description |
|---|---|---|
| `dump-state` (alias `dump`) | Yes | Returns the entire session state as a JSON blob (windows, panes, options, sizes, screen content). Invaluable for building rich UIs. |
| `zoom-pane` | Yes | Toggle zoom on the active pane |
| `run-command <cmd>` (alias `runcmd`) | Yes | Run any server command by name and return its output, with a 15 second timeout |
| `dump-layout` | No | Returns the pane layout tree structure |
| `list-tree` | No | Returns a hierarchical session/window/pane tree view |
| `send-text <text>` | No | Send raw text directly to the active pane (no key name parsing) |
| `send-paste <base64>` | No | Send text as a bracketed paste sequence. The single argument is the text base64 encoded. Also available at the CLI as `psmux send-paste`. |
| `claim-session` | No | Internal warm pool claim used during session startup |
| `set-pane-title <title>` | No | Set the title of the current pane |
| `toggle-sync` | No | Toggle synchronized input across all panes in a window |

`run-command` is the escape hatch, but it only reaches commands that the config file layer also implements, so `run-command toggle-sync` works while `run-command send-text hello` does not. `dump-layout`, `list-tree`, `send-text`, `set-pane-title` and `claim-session` are reachable only over a raw socket connection or from a key binding, and `send-paste` additionally from the CLI.

## Building a Plugin

### Minimal Python Example

```python
import subprocess
import threading

proc = subprocess.Popen(
    ["psmux", "-CC"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    env={**__import__("os").environ, "PSMUX_SESSION_NAME": "work"},
)

def read_notifications():
    for line in proc.stdout:
        line = line.rstrip("\n")
        if line.startswith("%output"):
            parts = line.split(" ", 2)
            pane_id = parts[1]
            data = parts[2] if len(parts) > 2 else ""
            print(f"[{pane_id}] {data}")
        elif line.startswith("%window-add"):
            print(f"Window created: {line}")
        elif line.startswith("%begin"):
            pass  # Start of command response
        elif line.startswith("%end"):
            pass  # End of command response
        elif line.startswith("%error"):
            print(f"Command error: {line}")

reader = threading.Thread(target=read_notifications, daemon=True)
reader.start()

# Send a command
proc.stdin.write("list-windows\n")
proc.stdin.flush()

# Create a new window
proc.stdin.write("new-window -n build\n")
proc.stdin.flush()

# Run a command in it
proc.stdin.write('send-keys "cargo build" Enter\n')
proc.stdin.flush()

import time
time.sleep(5)
proc.stdin.close()
proc.wait()
```

### Minimal PowerShell Example

```powershell
$env:PSMUX_SESSION_NAME = "work"
$psi = [System.Diagnostics.ProcessStartInfo]::new()
$psi.FileName = (Get-Command psmux).Source
$psi.Arguments = "-CC"
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.UseShellExecute = $false

$proc = [System.Diagnostics.Process]::Start($psi)

# Send a command
$proc.StandardInput.WriteLine("list-windows")
$proc.StandardInput.Flush()
Start-Sleep -Seconds 1

# Read the response
while ($proc.StandardOutput.Peek() -ge 0) {
    $line = $proc.StandardOutput.ReadLine()
    Write-Host $line
}

$proc.StandardInput.Close()
$proc.WaitForExit(5000)
```

### Minimal Node.js Example

```javascript
const { spawn } = require("child_process");

const proc = spawn("psmux", ["-CC"], {
  env: { ...process.env, PSMUX_SESSION_NAME: "work" },
  stdio: ["pipe", "pipe", "pipe"],
});

proc.stdout.on("data", (chunk) => {
  for (const line of chunk.toString().split("\n")) {
    if (line.startsWith("%output")) {
      const [, paneId, ...rest] = line.split(" ");
      console.log(`[${paneId}] ${rest.join(" ")}`);
    } else if (line.startsWith("%begin")) {
      // Command response starting
    } else if (line.startsWith("%end")) {
      // Command response complete
    }
  }
});

proc.stdin.write("list-windows\n");
proc.stdin.write("new-window -n monitor\n");
proc.stdin.write('send-keys "top" Enter\n');

setTimeout(() => {
  proc.stdin.end();
}, 5000);
```

## Parsing Tips

1. **Read line by line.** Every notification and framing marker is a single line terminated by `\n`.

2. **Track command state.** When you send a command, set a flag. Lines between `%begin` and `%end`/`%error` are the command's output. Everything outside those blocks is asynchronous notifications.

3. **Match begin/end pairs by command number.** The second field in `%begin` and `%end` lines is the command counter. Use it to correlate responses with requests.

4. **Buffer line parsing for `%output`.** Split on the first two spaces: `%output`, pane ID, then the rest is escaped output data.

5. **Decode octal escapes.** Replace `\NNN` sequences in output data with the corresponding byte value. `\134` is a literal backslash.

6. **Handle connection loss gracefully.** If the session dies or the server shuts down, stdout will close (EOF). Your reader loop should exit cleanly.

## Differences from tmux

psmux control mode is wire-compatible with tmux's protocol. The flow control and subscription layer is implemented:

| Feature | Status | Notes |
|---------|--------|-------|
| `refresh-client -f` flags | Implemented | `pause-after=N` and `no-pause` are parsed and honored. Other tmux client flags are ignored. |
| `refresh-client -A` pane actions | Implemented | `-A '%N:continue'` resumes a paused pane. `pause` is accepted but only `continue` takes effect. |
| `refresh-client -B` subscriptions | Implemented | `-B 'name:target:format'` adds a subscription, `-B 'name:'` removes it. Values are re-checked at most once per second. |
| `refresh-client -C w,h` | Implemented | Sets the control client viewport size. Note the argument is comma separated (`-C 120,30`) on the control mode path, which is what iTerm2 sends. |
| `%extended-output` | Implemented | Emitted instead of `%output` once a client has set `pause-after=N`, carrying the output age in milliseconds. |
| `%subscription-changed` | Implemented | Emitted when a subscribed format string changes value. |
| Unlinked window notifications | Not implemented | psmux never emits `%unlinked-window-add`, `%unlinked-window-close` or `%unlinked-window-renamed`. Each psmux server process owns exactly one session, so no window is ever outside the attached session's window list. |
| Slow client handling | Write timeout | Without `pause-after`, output is not buffered per client. Writes to a control client carry a 5 second timeout, and a client that stalls longer than that is disconnected and must reconnect and resync with `capture-pane`. Set `refresh-client -f pause-after=N` to get `%pause` / `%continue` flow control instead of a drop. |
| `%output` line size | One line per read | A burst from a pane arrives as one `%output` line per server read, which can be large for a chatty pane. Parse by line, not by fixed buffer. |

The core protocol (framing, notifications, escaping, IDs, command dispatch) is fully compatible. Plugins targeting the basic tmux control mode protocol will work identically on psmux.

### One session per server, many sessions per machine

psmux runs a **separate server process for each session**, unlike tmux where one server holds many sessions. A control mode client therefore sees exactly one session, and `%session-changed` fires only when that client is pointed at a different session.

Multiple sessions still coexist on the machine, each with its own server, and psmux stitches them together:

- `list-sessions` enumerates every session on the machine (or every session in the `-L` namespace), not just the one this client is attached to.
- `%sessions-changed` fires when a session is created or destroyed anywhere, which is why it carries no session ID.
- `switch-client -t <other-session>` and cross session `join-pane -s <other-session>:...` reach across server processes.

So "one session per server" is a statement about process topology, not a limit of one session per machine.

### Windows ConPTY Considerations

If you are porting a Unix tmux plugin to psmux, be aware of these ConPTY behaviors:

- **SMCUP/RMCUP consumed internally.** ConPTY processes alternate screen buffer switches before the output reaches psmux. The `alternate_on` flag is always false. psmux uses a heuristic (last row content analysis) to detect fullscreen TUI applications.
- **Output normalization.** ConPTY may normalize line endings and process certain cursor movement sequences internally. `%output` data may look slightly different from what a Unix tmux session would produce for the same shell command.
- **`capture-pane` always reflects the primary screen buffer.** There is no reliable way to detect whether a pane is showing the alternate screen.
- **Ctrl+C propagation.** `GenerateConsoleCtrlEvent` sends to ALL processes sharing the console, not just the foreground process. When testing TUI apps via `send-keys`, prefer using the app's quit key (e.g. `q`) rather than `C-c`.
- **TUI exit timing.** After a TUI application exits and sends RMCUP, ConPTY needs time to generate the restore sequences. If you `capture-pane` immediately after a TUI exits, you may still see TUI content. Allow 4 to 6 seconds for the screen to settle.

### Namespace Isolation

Use `-L` to run multiple independent psmux servers on the same machine:

```powershell
psmux -L dev new-session -d -s myapp -x 120 -y 30
$env:PSMUX_SESSION_NAME = "dev__myapp"
psmux -CC
```

The `PSMUX_SESSION_NAME` value follows the format `<namespace>__<session>` when using `-L`. The double underscore is the separator.

## Format Variables

Use `display-message -p` to query any format variable:

```
display-message -p '#{session_name}: #{window_index} #{pane_id}'
```

Common variables for control mode plugins:

| Variable | Example | Description |
|----------|---------|-------------|
| `#{session_name}` | `work` | Session name |
| `#{session_id}` | `$0` | Session stable ID |
| `#{window_id}` | `@0` | Window stable ID |
| `#{window_index}` | `0` | Window index |
| `#{window_name}` | `pwsh` | Window name |
| `#{pane_id}` | `%0` | Pane stable ID |
| `#{pane_index}` | `0` | Pane index within window |
| `#{pane_pid}` | `12345` | Pane child process PID |
| `#{pane_current_command}` | `pwsh` | Pane running command |
| `#{pane_width}` | `120` | Pane width in columns |
| `#{pane_height}` | `30` | Pane height in rows |
| `#{cursor_x}` | `5` | Cursor column |
| `#{cursor_y}` | `10` | Cursor row |
