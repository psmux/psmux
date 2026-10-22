# Scripting & Automation

psmux supports tmux-compatible commands for scripting and automation.

## Window & Pane Control

```powershell
# Create a new window
psmux new-window

# Split panes
psmux split-window -v          # Split vertically (top/bottom)
psmux split-window -h          # Split horizontally (side by side)

# Navigate panes
psmux select-pane -U           # Select pane above
psmux select-pane -D           # Select pane below
psmux select-pane -L           # Select pane to the left
psmux select-pane -R           # Select pane to the right

# Navigate windows
psmux select-window -t 1       # Select window by index (default base-index is 0)
psmux next-window              # Go to next window
psmux previous-window          # Go to previous window
psmux last-window              # Go to last active window

# Kill panes and windows
psmux kill-pane
psmux kill-window
psmux kill-session
```

## Sending Keys

```powershell
# Send text directly
psmux send-keys "ls -la" Enter

# Send keys literally (no parsing)
psmux send-keys -l "literal text"

# Paste mode (legacy compatibility)
psmux send-keys -p

# Repeat a key N times
psmux send-keys -N 5 Up

# Send a copy mode command by name (see "Copy Mode Commands (send-keys -X)").
# The pane must already be in copy mode; otherwise, as in tmux, the command
# fails with "not in a mode" (exit 1). -N repeats a motion.
psmux copy-mode
psmux send-keys -X begin-selection
psmux send-keys -X -N 5 cursor-up
psmux send-keys -X copy-selection-and-cancel

# Send a dash-leading operand: `--` ends option parsing (#562)
psmux send-keys -l -- "-rf"

# Send raw bytes, one hexadecimal byte per operand (tmux send-keys -H)
psmux send-keys -H 65 63 68 6f 20 68 69 0d      # types "echo hi" then CR

# Paste a block of text safely: send-paste takes the text base64 encoded, so
# newlines, quotes and semicolons can never be read as a second command
$b64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes("line one`nline two"))
psmux send-paste -t %1 $b64

# Special keys supported:
# Enter, Tab, Escape, Space, Backspace
# Up, Down, Left, Right, Home, End
# PageUp, PageDown, Delete, Insert
# F1-F12, C-a through C-z (Ctrl+key)
```

## Pane Information

```powershell
# List all panes in current window
psmux list-panes

# List all windows
psmux list-windows

# Capture pane content
psmux capture-pane

# Display formatted message with variables
psmux display-message "#S:#I:#W"   # Session:Window Index:Window Name
```

## Paste Buffers

```powershell
# Set paste buffer content
psmux set-buffer "text to paste"

# Paste buffer to active pane
psmux paste-buffer

# List all buffers
psmux list-buffers

# Show buffer content
psmux show-buffer

# Delete buffer
psmux delete-buffer

# Interactive buffer chooser (enter=paste, d=delete, esc=close)
psmux choose-buffer

# Named buffers (separate from anonymous stack)
psmux set-buffer -b mydata "key=value"
psmux show-buffer -b mydata
psmux paste-buffer -b mydata
psmux delete-buffer -b mydata
```

The command prompt keeps its own history, which is managed by `show-prompt-history` and
`clear-prompt-history`. Those are server side commands, not CLI commands. See
[Prompt History](#prompt-history) below.

## Pane Layout

```powershell
# Resize panes
psmux resize-pane -U 5         # Resize up by 5
psmux resize-pane -D 5         # Resize down by 5
psmux resize-pane -L 10        # Resize left by 10
psmux resize-pane -R 10        # Resize right by 10

# Swap panes
psmux swap-pane -U             # Swap with pane above
psmux swap-pane -D             # Swap with pane below

# Rotate panes in window
psmux rotate-window

# Toggle pane zoom
psmux zoom-pane
```

## Pane Titles

Programs running inside a pane can set the title via OSC escape sequences. PowerShell 7 does this automatically with the current working directory. See [pane-titles.md](pane-titles.md) for full details on how pane titles work, how to control them, and how different shells behave.

```powershell
# Set a title on the active pane
psmux select-pane -T "my build pane"

# Set pane title on a specific pane. -T and -P are attribute only (#592): the
# active window and pane stay where they are, even when %3 lives in another window
psmux select-pane -t %3 -T "logs"

# Set per-pane style (foreground/background color override)
psmux select-pane -P "bg=default,fg=blue"

# Display pane title using format variables
psmux display-message "#{pane_title}"
```

Enable `pane-border-format` and `pane-border-status` in your config to see titles on pane borders:

```tmux
set -g pane-border-status top
set -g pane-border-format " #{pane_index}: #{pane_title} "
```

## Popups

```powershell
# Open a popup running a command
psmux display-popup "Get-Process"

# Set width and height (absolute or percentage)
psmux display-popup -w 80% -h 50% "htop"

# Set the starting directory
psmux display-popup -d "C:\Projects" -w 100 -h 30

# Close popup on command exit (default behavior, -E inverts it)
psmux display-popup -E "git log --oneline -20"

# Keep popup open after command finishes
psmux display-popup -K "echo done"

# Title the frame, pick its lines and colours (tmux flags)
psmux display-popup -T "Logs" -b rounded -S "fg=#3b434d" -s "bg=colour235" "Get-Content app.log -Wait"

# No frame: the command draws its own and gets the whole 80x24 area
psmux display-popup -B -E -w 80 -h 24 "lazygit"
```

## Menus

```powershell
# Display an interactive menu
# Format: display-menu [-x x] [-y y] [-T title] name key command ...
psmux display-menu -T "Actions" \
  "New Window" n "new-window" \
  "Split Horizontal" h "split-window -h" \
  "Split Vertical" v "split-window -v" \
  "Close Pane" x "kill-pane"

# Position the menu at specific coordinates
psmux display-menu -x 10 -y 5 -T "Quick" \
  "Zoom" z "resize-pane -Z" \
  "Rename" r "command-prompt -I '#W' 'rename-window %%'"
```

## Session Management

```powershell
# Check if session exists (exit code 0 = exists)
psmux has-session -t mysession

# Rename session
psmux rename-session newname

# Switch to another session
psmux switch-client -t other-session

# Cycle through sessions
psmux switch-client -n          # Next session
psmux switch-client -p          # Previous session
psmux switch-client -l          # Last (most recently used) session

# Create a session with environment variables
psmux new-session -s work -e "MY_VAR=value"

# Respawn pane (restart shell, or restart with a different command)
psmux respawn-pane
psmux respawn-pane -k           # Kill the current process first
psmux respawn-pane -c /tmp      # Restart in a different directory
```

## Pane Reorganization

```powershell
# Break the current pane out into a new window
psmux break-pane

# Break a specific pane, keep it in background
psmux break-pane -d -s %3

# Join a pane from another window into the current window
psmux join-pane -s :2           # Bring pane from window 2

# Join horizontally or vertically
psmux join-pane -h -s :2        # Join side by side
psmux join-pane -v -s :3        # Join top/bottom

# Move a pane (same as join-pane)
psmux move-pane -s %5 -t %3

# Find a window by name or content
psmux find-window "search term"
```

## Moving and Swapping Windows

`move-window` and `swap-window` resolve their `-s` and `-t` values on the server with the same
resolver tmux uses, so the whole tmux target vocabulary works (#602):

```powershell
# Move the current window to index 7 (a free index is fine)
psmux move-window -t 7

# Move a specific window, not the active one
psmux move-window -s work:2 -t work:9

# Replace whatever is at the destination
psmux move-window -s :2 -t :0 -k

# Renumber windows to close the gaps
psmux move-window -r

# Swap the current window with the next one, or with a named window
psmux swap-window -t +1
psmux swap-window -t logs

# The symbolic forms work everywhere a window target does
psmux swap-window -s '{last}' -t '{end}'
```

For `move-window -t`, `+N` and `-N` are arithmetic on the current index and a number that names no
window is a free slot. For `swap-window`, `+N` and `-N` step through the window list and wrap, and a
number that names no window is an error. Both commands push the change to attached clients at once
(#601). After a swap without `-d` the active window number stays put, so whichever window moved into
it is now current, as in tmux; with `-d` the window you were in stays current at its new number.

## Environment Variables

```powershell
# Set a global env var (inherited by all new panes)
psmux set-environment -g EDITOR vim

# Set a session-scoped env var
psmux set-environment MY_VAR value

# Unset a global env var
psmux set-environment -gu MY_VAR

# Show all environment variables
psmux show-environment
psmux show-environment -g
```

## Format Variables

Anywhere psmux takes a format string (`display-message`, `-F` on the list commands, status bar
options, `pane-border-format`, `if-shell -F`, `%if` in a config file) it expands `#X` shorthands
and `#{...}` expressions. Test any of them with `display-message -p`:

```powershell
psmux display-message -p "#{session_name}:#{window_index}.#{pane_index}"
```

### Shorthand escapes

| Variable | Description |
|----------|-------------|
| `#S` | Session name |
| `#I` | Window index |
| `#W` | Window name |
| `#P` | Pane index |
| `#T` | Pane title, falling back to the hostname |
| `#F` | Window flags (`*` active, `-` last) |
| `#H` / `#h` | Hostname |
| `#D` | Unique pane id, rendered as `%N` |
| `##` | A literal `#` |
| `#,` | A literal comma inside a conditional branch |

### Session variables

| Variable | Description |
|----------|-------------|
| `#{session_name}` | Session name |
| `#{session_id}` | Stable session id, rendered as `$N` |
| `#{session_windows}` | Number of windows in the session |
| `#{session_attached}` | Number of clients attached to the session |
| `#{session_many_attached}` | `1` if more than one client is attached |
| `#{session_created}` | Creation time as a unix timestamp |
| `#{session_created_string}` | Creation time, already formatted |
| `#{session_activity}` / `#{session_last_attached}` | Last activity time as a unix timestamp |
| `#{session_activity_string}` | Last activity time, already formatted |
| `#{session_path}` | Working directory the session was created in |
| `#{session_alerts}` | Alerts pending on the session |
| `#{session_group}` | Name of the session group, empty if ungrouped |
| `#{session_group_list}` | Sessions in the group |
| `#{session_group_size}` | Number of sessions in the group |
| `#{session_group_attached}` | Number of clients attached across the group |
| `#{session_grouped}` | `1` if the session belongs to a group |
| `#{session_format}` | `1` inside a session format context |

### Window variables

| Variable | Description |
|----------|-------------|
| `#{window_index}` | Window index |
| `#{window_id}` | Stable window id, rendered as `@N` |
| `#{window_name}` | Window name |
| `#{window_active}` | `1` if this is the active window |
| `#{window_panes}` | Number of panes in the window |
| `#{window_width}` / `#{window_height}` | Window size in cells |
| `#{window_flags}` | Rendered flag string, for example `*` or `-` |
| `#{window_raw_flags}` | The same flags without decoration |
| `#{window_layout}` | tmux layout string with checksum, for example `a8fe,120x30,0,0,1` |
| `#{window_visible_layout}` | Layout string for the visible panes |
| `#{window_zoomed_flag}` | `1` if the window has a zoomed pane |
| `#{window_activity_flag}` | `1` if activity was seen in the window |
| `#{window_silence_flag}` | `1` if the window is currently silent |
| `#{window_bell_flag}` | `1` if a bell is pending |
| `#{window_last_flag}` | `1` if this is the last used window |
| `#{window_start_flag}` / `#{window_end_flag}` | `1` for the first and last window in the list |
| `#{window_linked}` | `1` if the window is linked into more than one session |
| `#{window_linked_sessions}` | Number of sessions the window is linked into |
| `#{window_activity}` | Last activity time for the window |
| `#{window_format}` | `1` inside a window format context |

`#{window_layout}` is a tmux layout string with a checksum, and it round-trips: capture it, and
`select-layout <string>` restores exactly that geometry.

```powershell
$layout = psmux display-message -p "#{window_layout}"
# ... rearrange, resize, split ...
psmux select-layout $layout        # back to the captured geometry
```

Restoring a layout applies the captured geometry as-is, so panes created after the capture are
not part of it. Capture again after any split you want to keep.

### Pane variables

| Variable | Description |
|----------|-------------|
| `#{pane_index}` | Pane index within the window |
| `#{pane_id}` | Stable pane id, rendered as `%N` |
| `#{pane_title}` | Pane title |
| `#{pane_width}` / `#{pane_height}` | Pane size in cells |
| `#{pane_active}` | `1` if this pane is the active pane |
| `#{pane_last}` | `1` if this was the previously active pane |
| `#{pane_current_command}` | Foreground process name |
| `#{pane_current_path}` | Current working directory of the pane. Under `wsl` or `ssh` this needs shell integration, see [the FAQ](faq.md) |
| `#{pane_path}` | Path the pane announced over `OSC 7` or `OSC 9;9`, untranslated, empty if none |
| `#{pane_start_command}` | Command the pane was started with, **empty** when it was started with the default shell. A `--` argv is reported as its token list, a single command string is quoted when it contains spaces (tmux `args_escape`). A respawn replaces it only when the respawn carried a command of its own |
| `#{pane_pid}` | PID of the pane's shell |
| `#{pane_tty}` | Pseudo terminal name, for example `/dev/pty1` |
| `#{pane_in_mode}` | `1` if the pane is in copy mode or another mode |
| `#{pane_mode}` | Name of the current mode, empty when in none |
| `#{pane_dead}` | `1` if the pane's process has exited and `remain-on-exit` kept it |
| `#{pane_synchronized}` | `1` if `synchronize-panes` is on for this window |
| `#{pane_marked}` | `1` if this pane is the marked pane |
| `#{pane_marked_set}` | `1` if any pane is marked |
| `#{pane_left}` / `#{pane_top}` / `#{pane_right}` / `#{pane_bottom}` | Pane edges in client cell coordinates |
| `#{pane_at_top}` / `#{pane_at_bottom}` / `#{pane_at_left}` / `#{pane_at_right}` | `1` if the pane touches that edge of the window |
| `#{pane_fg}` / `#{pane_bg}` | Resolved foreground and background colour |
| `#{pane_search_string}` | Last copy mode search string |

The `pane_at_*` flags are what make edge aware navigation possible, for example handing the key
to a neighbouring application when there is no pane in that direction:

```tmux
bind-key -n C-h if-shell -F "#{pane_at_left}" "send-keys C-h" "select-pane -L"
```

### Cursor, mouse and selection variables

| Variable | Description |
|----------|-------------|
| `#{cursor_x}` / `#{cursor_y}` | Cursor position in the active pane, zero based |
| `#{cursor_character}` | Character under the cursor |
| `#{mouse_x}` / `#{mouse_y}` | Position of the last mouse event |
| `#{mouse_line}` | Full line under the last mouse event |
| `#{mouse_word}` | Word under the last mouse event |
| `#{copy_cursor_x}` / `#{copy_cursor_y}` | Copy mode cursor position |
| `#{copy_cursor_word}` / `#{copy_cursor_line}` | Word and line under the copy mode cursor |
| `#{selection_present}` / `#{selection_active}` | `1` if a copy mode selection exists |
| `#{selection_start_x}` / `#{selection_start_y}` | Selection anchor |
| `#{selection_end_x}` / `#{selection_end_y}` | Selection end |
| `#{search_present}` | `1` if a copy mode search is active |
| `#{search_match}` | The current search match |
| `#{scroll_position}` | Lines scrolled back from the live bottom |
| `#{scroll_region_lower}` | Lower bound of the scroll region |

### Buffer variables

| Variable | Description |
|----------|-------------|
| `#{buffer_size}` | Size of the buffer in bytes |
| `#{buffer_name}` | Buffer name |
| `#{buffer_sample}` | Short preview of the buffer contents |
| `#{buffer_created}` | Creation time as a unix timestamp |

### Client variables

Each of these describes one client: the row's client in `list-clients -F`, the client named by
`display-message -c`, and otherwise the most recently active client of the session. With no
client attached, the identity variables (`client_name`, `client_pid`, `client_readonly`,
`client_flags`, `client_control_mode`) are empty, as in tmux.

| Variable | Description |
|----------|-------------|
| `#{client_name}` / `#{client_tty}` | Name of the client, `/dev/pts/<pid>`. Unique across the `-L` namespace and kept when the client switches session |
| `#{client_pid}` | PID of the attached client process (a TUI, `attach -r` or `-CC` client) |
| `#{client_readonly}` | `1` for a client attached with `attach -r` (or `-f read-only`), else `0` |
| `#{client_flags}` | tmux style flag list, for example `attached,read-only,UTF-8` (also `focused`, `control-mode`) |
| `#{client_control_mode}` | `1` for a `-CC` control mode client |
| `#{client_width}` / `#{client_height}` | Size of the client terminal |
| `#{client_prefix}` | `1` if the prefix key was pressed |
| `#{client_key_table}` | Key table the client is currently in, for example `root` |
| `#{client_session}` / `#{client_last_session}` | Current and previous session of the client |
| `#{client_activity}` / `#{client_created}` | Timestamps for the client |
| `#{client_activity_string}` / `#{client_created_string}` | The same, already formatted |
| `#{client_termname}` / `#{client_termtype}` | Terminal type reported by the client |

### Server, host and misc variables

| Variable | Description |
|----------|-------------|
| `#{host}` / `#{hostname}` | Full hostname |
| `#{host_short}` | Hostname up to the first dot |
| `#{user}` / `#{username}` | Current user name |
| `#{pid}` / `#{server_pid}` | PID of the server process that answered the request. psmux runs one server per session, so this is session-scoped and changes when a session is created. Use `#{server_instance}` to identify the namespace |
| `#{server_instance}` | Stable identity of the `-L` namespace. Constant while the namespace is up, whichever of its servers answers; changes only after a genuine restart. Empty for a namespace that has no server |
| `#{version}` | psmux version, for example `3.3.7` |
| `#{start_time}` | Server start time |
| `#{socket_path}` | The server that answered: `<psmux dir>/default`, `<psmux dir>/<label>` under `-L`, or the `-S` path |
| `#{history_size}` | Lines currently held in the pane's scrollback |
| `#{alternate_on}` | `1` if the pane is on the alternate screen |
| `#{mouse_any_flag}` | `1` if the pane has any mouse tracking mode on (DECSET 1000, 1002 or 1003) |
| `#{mouse_standard_flag}` | `1` if the pane enabled normal tracking, DECSET 1000 (or X10 tracking, DECSET 9) |
| `#{mouse_button_flag}` | `1` if the pane enabled button event tracking, DECSET 1002 |
| `#{mouse_all_flag}` | `1` if the pane enabled any event tracking, DECSET 1003 |
| `#{mouse_utf8_flag}` | `1` if the pane enabled UTF-8 mouse reporting, DECSET 1005 |
| `#{mouse_sgr_flag}` | `1` if the pane enabled SGR mouse reporting, DECSET 1006 |
| `#{current_file}` | Config file being parsed, during config parsing and while a command read from it runs |

### Options as format variables

Any option name resolves inside `#{...}`, so you can read configuration back without parsing
`show-options` output:

```powershell
psmux display-message -p "#{status-left}"      # [#S]
psmux display-message -p "#{mouse}"            # on
psmux display-message -p "#{history-limit}"    # 2000
```

A bare `@name` resolves as a user option, and an unqualified name that matches nothing else is
tried as `@name` too:

```powershell
psmux set -g @theme "nord"
psmux display-message -p "#{@theme}"           # nord
```

A few options also have underscore aliases: `mode_keys`, `history_limit`, `alternate_screen`.

### Accepted but not yet meaningful

These names are accepted by the format engine and always expand, but the value is a placeholder
rather than live state. They exist for tmux format compatibility. Do not build logic on them.

| Variable | Always returns |
|----------|----------------|
| `#{session_stack}` | empty |
| `#{window_bigger}` | `0` |
| `#{window_offset_x}` / `#{window_offset_y}` / `#{window_stack_index}` | `0` |
| `#{window_cell_width}` / `#{window_cell_height}` | `8` / `16` |
| `#{window_linked_sessions_list}` | empty |
| `#{pane_dead_signal}` / `#{pane_dead_status}` / `#{pane_dead_time}` | `0` |
| `#{pane_start_path}` / `#{pane_tabs}` | empty |
| `#{cursor_flag}` | `0` |
| `#{scroll_region_upper}` | `0` |
| `#{client_termfeatures}` | a fixed string |
| `#{client_utf8}` | `1` |
| `#{client_cell_width}` / `#{client_cell_height}` | a fixed value |
| `#{client_written}` / `#{client_discarded}` | `0` |
| `#{alternate_saved_x}` / `#{alternate_saved_y}` | `0` |
| `#{origin_flag}` / `#{insert_flag}` / `#{keypad_cursor_flag}` / `#{keypad_flag}` | `0` |
| `#{wrap_flag}` | `1` |
| `#{line}`, `#{command}`, `#{command_list_name}`, `#{command_list_alias}`, `#{command_list_usage}`, `#{config_files}` | empty |

### Format Modifiers

```powershell
# Conditional
psmux display-message -p "#{?window_zoomed_flag,ZOOMED,normal}"

# Comparison
psmux display-message -p "#{==:#{pane_index},0}"

# Regex substitution
psmux display-message -p "#{s/old/new/:pane_title}"

# Basename and dirname
psmux display-message -p "#{b:pane_current_path}"
psmux display-message -p "#{d:pane_current_path}"

# Loop over all windows
psmux display-message -p "#{W:#{window_index}:#{window_name} }"

# Loop over all panes
psmux display-message -p "#{P:#{pane_index} }"
```

Modifiers are separated from their target by the first top level `:`, and several modifiers can
be chained with `;`, for example `#{d;b:pane_current_path}`.

#### `#{t:var}` format a unix timestamp

Renders a numeric timestamp as `%a %b %e %H:%M:%S %Y` in local time.

```powershell
psmux display-message -p "#{t:session_created}"
# Mon Jul 27 19:44:38 2026
```

#### `#{E:var}` expand the value again as a format

Reads a value that itself contains `#{...}` and expands it a second time. This is how you resolve
an option whose stored text is a format.

```powershell
psmux display-message -p "#{status-left}"     # [#S]           (raw, unexpanded)
psmux display-message -p "#{E:status-left}"   # [work]         (expanded)
```

#### `#{T:var}` expand, then apply strftime

Expands the value as a format and then runs the result through strftime, so time codes stored in
an option or a variable are honoured.

```powershell
psmux display-message -p "#{T:#{l:%Y-%m-%d}}"
# 2026-07-27
```

#### `#{w:var}` display width in cells

Returns how many terminal cells the value occupies, which is not the same as its character count
for wide characters.

```powershell
psmux display-message -p "#{w:host_short}"
# 9
```

#### `#{=/N/marker:var}` trim to N with a trailing marker

`#{=N:var}` trims to N characters. The `/N/marker` form appends a marker when it actually had to
cut. A negative N trims from the right and puts the marker in front. Either `/` or `|` works as
the separator.

```powershell
psmux display-message -p "#{=/6/...:session_path}"
# C:\Use...
```

#### `#{e|op|flags|decimals:a,b}` arithmetic

`op` is one of `+`, `-`, `*`, `/`, `m` (modulo). Add the `f` flag for floating point, and give a
decimals count to control the printed precision. Division or modulo by zero yields `0`.

```powershell
psmux display-message -p "#{e|+||:10,32}"        # 42
psmux display-message -p "#{e|/|f|2:10,4}"       # 2.50
psmux display-message -p "#{e|*||:#{pane_width},2}"
```

#### `#{m:a,b}` match

Returns `1` or `0`. The first argument is the pattern, the second is the subject. By default the
pattern is a glob (`*` and `?`). Add the `r` flag for a regular expression and the `i` flag for
case insensitivity; both can be combined as `m/ri`.

```powershell
psmux display-message -p "#{m:pw*,#{pane_current_command}}"        # 1
psmux display-message -p "#{m/r:^pwsh$,#{pane_current_command}}"   # 1
psmux display-message -p "#{m/ri:^PWSH$,#{pane_current_command}}"  # 1
```

`m` is what makes vim aware split navigation work. The binding tests the foreground command and
either forwards the key to the application or moves the psmux pane:

```tmux
bind-key -n C-h if-shell -F "#{m/r:^(pwsh|n?vim)$,#{pane_current_command}}" \
  "send-keys C-h" "select-pane -L"
```

#### Parsed but not functional

`#{C:...}` and `#{N...}` are recognised by the modifier scanner but neither is a supported
surface. `#{N...}` has no implementation at all and falls through to a plain variable lookup.
Treat both as unavailable rather than as documented behaviour.

## Advanced Commands

```powershell
# Discover supported commands
psmux list-commands

# Server/session management
psmux kill-server
psmux list-clients
psmux switch-client -t other-session

# Config at runtime
psmux source-file ~/.psmux.conf
psmux show-options
psmux show-option -g status-left           # tmux's singular spelling works too (#586)
psmux set-option -g status-left "[#S]"

# Flags parse only before the option name (#583): a dash-leading VALUE is data
psmux set -g @my-flag -u                   # stores the string "-u"
psmux set -gu @my-flag                     # this is how you unset it
psmux set -- @literal -g                   # `--` ends option parsing

# Layout/history/stream control
psmux next-layout
psmux previous-layout
psmux select-layout tiled         # Apply a specific layout
psmux clear-history
psmux pipe-pane -o "cat > pane.log"    # Direct file sink (see below)
psmux pipe-pane                        # Stop piping

# Hooks (event callbacks) - see Hooks section below for full reference
psmux set-hook -g after-new-window "display-message created"
psmux set-hook -g client-attached "run-shell 'echo attached'"
psmux set-hook -gu after-new-window     # Unset (remove) a hook
psmux show-hooks

# Run shell commands
psmux run-shell "echo hello"           # Output shown in status bar
psmux run-shell -b "long-running.ps1"  # Fire-and-forget (background)

# Conditional execution
psmux if-shell "test -f ~/.psmux.conf" "source-file ~/.psmux.conf"
psmux if-shell -F "#{window_zoomed_flag}" "" "resize-pane -Z"

# User confirmation dialogs
psmux confirm-before -p "Kill this pane? (y/n)" kill-pane

# Wait channels for cross-pane synchronization
psmux wait-for -L mychannel             # Lock a channel
psmux wait-for -S mychannel             # Signal (unlock) a channel
psmux wait-for mychannel                # Wait until channel is signaled
```

## Hooks (Event Callbacks)

Hooks let you run commands automatically when events occur. They are one of the most powerful
scripting features in psmux. This section is the canonical hook reference for psmux; other docs
link here rather than repeating the list.

### Setting Hooks

```powershell
# Global hook (applies to all sessions)
psmux set-hook -g after-new-window "display-message 'New window created'"

# Session-scoped hook
psmux set-hook after-split-window "select-layout tiled"

# Chain multiple commands in a hook
psmux set-hook -g session-created "set -g status-left '[#S] ' \; display-message 'Session ready'"
```

### Setting, appending, and unsetting

| Form | Effect |
|------|--------|
| `set-hook <name> <command>` | Replace the handler list for `<name>` with this one command |
| `set-hook -g <name> <command>` | The same, written globally |
| `set-hook -a <name> <command>` | Append a handler, keeping the existing ones |
| `set-hook -ga <name> <command>` | The same, written globally. `-ag` is also accepted |
| `set-hook -u <name>` | Unset, removing every handler for `<name>` |
| `set-hook -gu <name>` | The same, globally. `-ug` is also accepted |
| `show-hooks` | Print every registered hook and its handlers |

The plain (non append) form **replaces**, so re-running a config cannot stack handlers on a hook
you set with `set-hook`. Appends are **deduplicated**: appending a command that is already
registered for that hook is a no-op, so re-sourcing a config that uses `-ga` cannot accumulate
duplicate handlers either.

```powershell
psmux set-hook -ga after-new-window "display-message one"
psmux set-hook -ga after-new-window "display-message one"   # ignored, identical
psmux set-hook -ga after-new-window "display-message two"
psmux show-hooks
# after-new-window[0] -> display-message one
# after-new-window[1] -> display-message two
```

`show-hooks` prints `name -> command` when a hook has a single handler and `name[N] -> command`
when it has several.

### Warning: hook names are not validated

`set-hook` stores **any** name you give it. A misspelled hook is accepted silently, appears in
`show-hooks`, and then simply never fires:

```powershell
psmux set-hook -g after-new-windwo "display-message oops"   # accepted, never fires
psmux show-hooks
# after-new-windwo -> display-message oops
```

There is no error and no warning. After adding a hook, run `show-hooks` and check the name
against the table below before assuming the hook is broken for some other reason.

### Available Hook Events

Every hook below is fired by psmux. Names not in this table are accepted by `set-hook` but never
fire.

| Hook | Fires when... |
|------|---------------|
| `after-new-window` | A window is created |
| `after-split-window` | A pane is split |
| `after-kill-pane` | A pane is killed |
| `after-select-window` | A different window becomes active. Fires ONCE per `select-window`; it fired twice before issue #690 |
| `before-select-window` | A `select-window` is about to switch, before the window changes |
| `after-select-pane` | A `select-pane` moved the active pane. It does NOT fire when the target pane is already the active one, and `-T`, `-P`, `-m`, `-M`, `-e` and `-d` never fire it, because none of them moves the pane (tmux `cmd-select-pane.c`). Before issue #691 the `-t` form fired `after-select-window` instead of this |
| `after-select-layout` | A layout is applied with `select-layout` |
| `after-rename-window` | A window is renamed |
| `after-rename-session` | The session is renamed |
| `after-resize-pane` | A pane is resized |
| `after-swap-pane` | Two panes are swapped. A psmux extension: upstream tmux gives `swap-pane` no hook |
| `after-swap-window` | Two windows are swapped. A psmux extension, symmetric with `after-swap-pane` |
| `after-rotate-window` | Panes in a window are rotated |
| `after-break-pane` | A pane is broken out into its own window |
| `after-join-pane` | A pane is joined into a window |
| `after-respawn-pane` | A pane is respawned |
| `client-attached` | A client attaches, and once at server start |
| `client-detached` | A client detaches |
| `client-resized` | The client terminal is resized |
| `client-session-changed` | A client starts looking at this session, which in psmux means it attaches to this session's server. See the note on one server per session below |
| `session-created` | Fires ONCE, at server start. See the note below: it has to be set in a config file to be registered in time |
| `session-closed` | The session ends |
| `pane-died` | A pane's process exits |
| `pane-exited` | Fired alongside `pane-died` when a pane's process exits |
| `pane-focus-in` | Focus enters a pane: a different pane becomes active, or the client's terminal regains focus. Requires `focus-events on`, which is off by default, exactly as in tmux |
| `pane-focus-out` | Focus leaves a pane, under the same `focus-events` condition |
| `pane-mode-changed` | A pane enters or leaves copy mode, clock mode or a chooser |
| `pane-set-clipboard` | A pane writes the clipboard through OSC 52 |
| `window-linked` | A window joins the session's window list: `new-window`, `break-pane` or `link-window` |
| `window-unlinked` | A window leaves it: `kill-window` or `unlink-window` |
| `window-closed` | A window goes away |
| `alert-activity` | Activity detected in a monitored window |
| `alert-silence` | Silence detected in a monitored window |
| `alert-bell` | Bell received from a pane |

There is no `after-new-session` hook in psmux. It is accepted by `set-hook`, like any other
name, but nothing ever fires it. Use `session-created` instead.

These tmux hook names are likewise accepted and never fired: `after-copy-mode`,
`after-set-option`, `after-bind-key`, `after-unbind-key`, `after-source`,
`after-kill-window`, `after-move-window`, `after-link-window`, `after-unlink-window`,
`session-renamed`, `session-window-changed`, `window-renamed`, `window-pane-changed`,
`client-focus-in`, `client-focus-out`.

### One server per session, and the two hooks it shapes

psmux runs one server process per session, so `app.hooks` belongs to a session and a hook set
in one session cannot see anything that happens in another. Two hooks in the table above are
shaped by that:

* `session-created` fires once, at server start, which is the only moment it can: a new session
  is a new process whose hook map is empty until its config is read. So
  `psmux set-hook -g session-created "..."` in a running session can never fire, and the hook
  has to come from a config file:

  ```powershell
  # in ~/.psmux.conf, or a file passed with -f
  set-hook -g session-created "display-message 'session ready'"
  ```

* `client-session-changed` fires when a client attaches to this session, because attaching IS
  the client changing which session it looks at. tmux fires it from the same place, for an
  attach and a `switch-client` alike (`server-client.c` `server_client_set_session`).

### Removing Hooks

```powershell
# Remove a global hook
psmux set-hook -gu after-new-window

# View all active hooks
psmux show-hooks
```

## Display Panes

Show numbered overlays on all panes, then type a number to jump to that pane:

```powershell
# Show pane number overlay (also: Prefix + q)
psmux display-panes
```

The overlay shows each pane's number according to `pane-base-index`. Press a number key while the overlay is visible to switch to that pane. The overlay auto-dismisses after `display-panes-time` milliseconds.

## Run Shell

Run an external command and display the output:

```powershell
# Output appears in the status bar message area
psmux run-shell "echo hello"

# Run in background (fire-and-forget, no output displayed).
# The command's OUTPUT is discarded, but a failure to start it is reported.
psmux run-shell -b "long-running-script.ps1"

# Use format variables in shell commands
psmux run-shell "echo 'Current pane: #{pane_index}'"
```

`#{...}` variables are expanded against the live server state before the command
runs, so a bind can hand a helper the current pane's context:

```powershell
bind-key e run-shell -b "my-helper.ps1 -Pane '#{pane_id}' -Path '#{pane_current_path}'"
```

> **Note:** expansion here was missing until psmux 3.3.8. On earlier versions
> the helper received the literal text `#{pane_id}`, and with `-b` also
> swallowing spawn errors, such a bind failed completely silently. If you are
> targeting an older psmux, pass the values from the caller instead of relying
> on expansion.

## Piping Pane Output (pipe-pane)

Stream a pane's raw output (ANSI escapes included) to a file or a command,
like tmux's `pipe-pane` (one deviation: tmux expands format variables such
as `#I` in the sink command; psmux does not):

```powershell
# Log the pane to a file (direct file sink - see below)
psmux pipe-pane -o "cat > pane.log"
psmux pipe-pane -o "cat >> pane.log"     # append instead of truncate
psmux pipe-pane -t %2 -o "cat > out.log" # pipe a specific pane

# Stop piping (or use -o again to toggle off)
psmux pipe-pane
```

**Direct file sink.** The canonical tmux logging idiom `cat > <path>` /
`cat >> <path>` (path may be quoted) is serviced by the psmux server itself:
the pane's raw ConPTY bytes are written straight to the file, byte-for-byte,
with no shell in between. This exists because arbitrary sink commands run
under PowerShell, where `cat` is the `Get-Content` alias, it never reads
stdin, so the idiom cannot work as a shell command on Windows.

Prefer an **absolute path**: a relative path resolves against the psmux
server's working directory (the session's start directory), not your
shell's. The path must be a literal **local file** path. Anything a shell
would expand (`$var`, `` `...` ``, `%var%`, `#I`, leading `~`) falls
through to the shell sink, and UNC paths, mapped network drives, and DOS
device names (`CON`, `NUL`, ...) are refused loudly: an unreachable host
would stall the whole server, and a device is not a log file. To log to a
network location, use a sink command that reads stdin, which runs as a child
process and stalls only itself.

**Arbitrary command sinks.** Any other command runs under the sink shell
(`pwsh`/`powershell -NoProfile -Command`, falling back to `cmd /c`) and
**must actually read its stdin**, because that is where the pane bytes arrive. In
PowerShell, read `$input` or `[Console]::In`:

```powershell
# PowerShell sink: line-oriented processing of pane output
psmux pipe-pane -o '$input | Out-File -Encoding utf8 pane.log'
```

A sink that never reads stdin (like the `cat` alias) exits immediately and
captures nothing.

**Errors are loud.** If a direct file sink cannot be opened (e.g. the log
file's directory does not exist), `pipe-pane` prints `ERROR: ...` and exits
non-zero instead of silently recording a dead pipe. A shell sink that fails
to spawn is reported on the status bar and is likewise never recorded.

## Interactive Choosers

```powershell
# Interactive session/window/pane tree browser
psmux choose-tree

# Show only sessions
psmux choose-tree -s

# Show only windows
psmux choose-tree -w

# Interactive buffer picker (enter=paste, d=delete)
psmux choose-buffer

# Interactive client picker
psmux choose-client

# Interactive options editor
psmux customize-mode
```

## Target Syntax (`-t`)

Most commands accept a `-t` flag naming the session, window, or pane to act on. psmux supports
the tmux target grammar:

```powershell
# Target a session by name
psmux has-session -t mysession
psmux switch-client -t mysession

# Target a session by stable id
psmux switch-client -t '$0'

# Window by index in a session
psmux select-window -t work:2

# Window by name in a session
psmux select-window -t work:editor

# Window by index in the current session
psmux select-window -t 3
psmux select-window -t :2

# Window by stable window id
psmux select-window -t @4

# Pane by stable pane id
psmux send-keys -t %3 "pwd" Enter

# Pane within a window in the current session
psmux select-pane -t :2.1              # window 2, pane 1

# Full session:window.pane path
psmux send-keys -t dev:0.2 "make build" Enter

# Relative targets
psmux select-pane -t +                 # next pane
psmux select-pane -t -                 # previous pane
psmux select-window -t !               # last (previous) window
```

Prefix a name with `=` for an exact match, for example `-t '=work'`, when a session name would
otherwise be ambiguous.

### Which session a bare command reaches

A command with no `-t` still has to land somewhere. psmux resolves it the way tmux's
`cmd_find_best_session` does (#603), in this order:

1. `-t`, if given, always wins. So does a positional session name on `attach`.
2. Inside a psmux pane, the `$TMUX` variable names the server the pane belongs to, and that
   session is used when it is in the namespace you asked for (`-L`).
3. Otherwise the session with the **newest activity** in the namespace. Activity is stamped when a
   client attaches, on `switch-client`, and on every key an attached client sends, so a bare
   `psmux display -p '#S'` from a second terminal answers with the session you are actually
   typing in. Warm standby servers never qualify.
4. With `-L <ns>` and no session at all, the namespaced default `<ns>__default` is used.

The `last_session` file that older releases relied on is now only a tie break between sessions
whose activity stamps are identical. A bare `psmux attach` that finds no session prints
`psmux: no sessions` and exits 1, like tmux, while an explicit `-t` that does not resolve prints
`can't find session: <name>` (#605).

### Positional pane targets

`swap-pane` also accepts geometric position tokens, which resolve against the current layout
rather than an index, so they keep working after a split or a layout change:

`{top-left}`, `{top}`, `{top-right}`, `{left}`, `{right}`, `{bottom-left}`, `{bottom}`,
`{bottom-right}`.

```tmux
# Swap the active pane with whatever currently occupies the top right corner
bind-key S swap-pane -t '{top-right}'
```

Two limits are worth knowing before you script against these. They are resolved on the server
side, so they belong in a key binding rather than on the command line: the `psmux` CLI parses a
leading `{` in a `-t` value as a session name and fails with
`no server running on session '{top-right}'`. And they are honoured by `swap-pane` only, not by
`select-pane`.

## Server Namespaces (`-L`)

Use `-L` to run multiple isolated psmux servers on the same machine. Each namespace gets its own
server process, its own sessions, and its own discovery files:

```powershell
# Start a session in a named server namespace
psmux -L work new-session -s dev

# Attach to a session in that namespace
psmux -L work attach -t dev

# List only that namespace's sessions
psmux -L work list-sessions

# A second namespace is completely independent
psmux -L personal new-session -s play

# Without -L, the default namespace is used
psmux list-sessions
```

This is useful for running completely separate psmux environments, for example one for
development and one for monitoring. On disk the state files become `<namespace>__<session>.*`
under `~\.psmux\`.

Because `__` joins the namespace and the session name on disk, a session in the default
namespace (no `-L`) cannot have `__` in its name: `new-session` and `rename-session` refuse it
with `invalid session: NAME`. A session named `a__b` would otherwise share its files with
session `b` of `-L a`. Under `-L` any name works, including one that starts with the namespace
itself (`psmux -L work new-session -s work__x`, then `-t work__x`).

## Key Binding Management

```powershell
# Bind a key in the default prefix table
psmux bind-key h split-window -h

# Bind with format variable expansion (-F flag)
psmux bind-key -F M-h "resize-pane -L #{pane_width}"

# Bind with repeat (successive presses within repeat-time don't need prefix)
psmux bind-key -r Left select-pane -L
psmux bind-key -r Right select-pane -R

# Bind in root table (no prefix needed)
psmux bind-key -n M-Left select-pane -L

# Bind in a specific key table
psmux bind-key -T copy-mode-vi y send-keys -X copy-selection

# Unbind a single key
psmux unbind-key h

# Unbind ALL keys (reset to clean slate)
psmux unbind-key -a

# Unbind all keys in a specific key table only
psmux unbind-key -a -T copy-mode-vi
psmux unbind-key -a -T prefix
psmux unbind-key -a -T root
psmux unbind-key -a -T copy-mode
```

## Command Chaining

Chain multiple commands with `\;` in config files:

```tmux
# Split and select in one binding
bind-key M-v split-window -v \; select-pane -U

# Create a 3-pane layout
bind-key M-d split-window -h \; split-window -v \; select-pane -t 0

# Conditional chaining
bind-key M-z if-shell -F "#{window_zoomed_flag}" "resize-pane -Z" ""
```

From the CLI, use `\;` or quote the command:

```powershell
psmux split-window -h `; select-pane -L
```

## Querying Lists with Custom Formats

```powershell
# List all sessions with custom format
psmux list-sessions -F "#{session_name}:#{session_windows}"

# List all windows with custom format
psmux list-windows -F "#{window_index}:#{window_name}:#{window_panes}"

# List all panes across the session (-s flag)
psmux list-panes -s -F "#{window_index}.#{pane_index}: #{pane_current_command} [#{pane_width}x#{pane_height}]"

# List all panes across all sessions (-a flag)
psmux list-panes -a

# Capture pane content to stdout
psmux capture-pane -p -t %0

# Capture with line range (negative = scrollback)
psmux capture-pane -p -S -100 -E -1

# Print a format variable
psmux display-message -p "#{pane_current_path}"
```

## Window and Pane Creation Options

### new-window

```powershell
# Create a window with a name
psmux new-window -n "logs"

# Create a window in the background (don't switch to it)
psmux new-window -d -n "background"

# Create a window in a specific directory
psmux new-window -c "C:\Projects\myapp"

# Create a window running a command
psmux new-window -n "build" -- cargo watch

# Create a window at a specific index
psmux new-window -t 5

# Two or more tokens after `--` are an argv and run WITHOUT a shell (#582, tmux execvp)
psmux new-window -n api -- node server.js --port 8080

# One token after `--` is a shell command string, so pipes and redirections work
psmux new-window -n log -- "cargo watch -x test 2>&1 | tee watch.log"
```

When you set a window name with `-n`, automatic renaming is disabled for that window so the foreground process name does not overwrite your chosen name.

The `--` rule mirrors tmux exactly: a single token is handed to the pane's shell as a command string, while a multi token argv is executed directly, so no PowerShell profile runs first, arguments reach the program byte for byte, and `#{pane_current_command}` names the program rather than `pwsh`. Environment assignments, `&&`, pipes and wildcards need the shell, so put those in one quoted string. `split-window` and `new-session` follow the same rule.

### split-window

```powershell
# Split with percentage size
psmux split-window -v -p 30            # Bottom pane gets 30%
psmux split-window -h -p 70            # Right pane gets 70%

# Split in the current pane's directory
psmux split-window -h -c "#{pane_current_path}"

# Split with a specific command
psmux split-window -v -- python

# Split a specific target pane
psmux split-window -v -t %3

# Split without switching focus
psmux split-window -d -v
```

### new-session

```powershell
# Create a named session
psmux new-session -s work

# Create in a specific directory
psmux new-session -s project -c "C:\Projects\myapp"

# Create with environment variables
psmux new-session -s dev -e "NODE_ENV=development"

# Create in background (detached)
psmux new-session -d -s background

# Create with an initial command
psmux new-session -s monitor -- htop

# Create a session with a named first window
psmux new-session -s work -n "editor"
```

### new-pane (floating panes)

`new-pane` (alias `newp`) creates a pane that floats **above** the tiled layout instead of taking
space from it. It has its own border and title, and with `mouse` on it can be dragged to move and
dragged by its edge to resize.

```powershell
# A 60x20 floating pane at column 10, row 5
psmux new-pane -X 10 -Y 5 -x 60 -y 20 -T "notes"

# Choose the border glyph set and run a command in it
psmux new-pane -X 4 -Y 2 -x 80 -y 24 -B double -T "logs" "Get-Content -Wait app.log"

# Create it without focusing it, and print its pane id
psmux new-pane -d -P -x 40 -y 10
# %4

# An empty floating pane with no shell in it
psmux new-pane -E -x 40 -y 10
```

| Flag | Meaning |
|------|---------|
| `-X <col>` | Column of the pane's top left corner |
| `-Y <row>` | Row of the pane's top left corner |
| `-x <w>` | Width in cells |
| `-y <h>` | Height in cells |
| `-T <title>` | Pane title, shown in the border |
| `-B <border>` | Border style for the floating frame: `double`, `heavy`, or `none`. Any other value, including the default, draws a plain single line box |
| `-c <dir>` | Start directory |
| `-d` | Do not focus the new pane |
| `-P` | Print the new pane id |
| `-E` | Create the pane empty, with no shell |

A floating pane is not part of the window's layout tree, so it does not appear in `list-panes`
output and layout commands such as `select-layout` leave it alone.

## Copy Mode Commands (`send-keys -X`)

Copy mode has a name addressable command surface. Every name below can be driven two ways: from a
script with `send-keys -X <name>`, and from a key binding with
`bind-key -T copy-mode-vi <key> send-keys -X <name>`.

```powershell
# Drive copy mode from a script
psmux copy-mode
psmux send-keys -X history-top
psmux send-keys -X begin-selection
psmux send-keys -X cursor-down
psmux send-keys -X copy-selection-and-cancel
```

```tmux
# Rebind copy mode keys to these commands
bind-key -T copy-mode-vi v send-keys -X begin-selection
bind-key -T copy-mode-vi y send-keys -X copy-selection-and-cancel
bind-key -T copy-mode-vi C-v send-keys -X rectangle-toggle
```

`send-keys -X` needs a pane that is already in copy mode, exactly as in tmux: on
any other pane it prints `not in a mode` and exits 1. `-N <count>` repeats the
commands that take a count (cursor, word, page and scroll motions, search
again); the rest run once.

`list-keys -T copy-mode-vi` (or `-T copy-mode` for `mode-keys emacs`) prints
the keys the built-in copy mode handles, as the `send-keys -X` command each one
runs, together with your own bindings in those tables. `unbind-key -T
copy-mode-vi <key>` takes a built-in key away (it then does nothing, as in
tmux), and `unbind-key -a -T copy-mode-vi` takes all of them.

### Movement

| Name | Description |
|------|-------------|
| `cursor-up` | Move the cursor up one line |
| `cursor-down` | Move the cursor down one line |
| `cursor-left` | Move the cursor left one cell |
| `cursor-right` | Move the cursor right one cell |
| `start-of-line` | Move to column 0 |
| `end-of-line` | Move to the end of the line |
| `back-to-indentation` | Move to the first non blank character |
| `next-word` | Move to the start of the next word |
| `previous-word` | Move to the start of the previous word |
| `next-word-end` | Move to the end of the next word |
| `next-space` | Move to the next whitespace delimited word |
| `previous-space` | Move to the previous whitespace delimited word |
| `next-space-end` | Move to the end of the next whitespace delimited word |
| `top-line` | Move to the top visible line |
| `middle-line` | Move to the middle visible line |
| `bottom-line` | Move to the bottom visible line |
| `history-top` | Move to the top of the scrollback |
| `goto-line <n>` | Move the view to line `n`, counted the way `copy-mode-line-numbers` counts |
| `history-bottom` | Move to the live bottom of the scrollback |
| `next-paragraph` | Move to the next blank line |
| `previous-paragraph` | Move to the previous blank line |
| `next-matching-bracket` | Jump to the matching bracket |

### Scrolling

| Name | Description |
|------|-------------|
| `halfpage-up` | Scroll up half a screen |
| `halfpage-down` | Scroll down half a screen |
| `page-up` | Scroll up a full screen |
| `page-down` | Scroll down a full screen |
| `scroll-up` | Scroll up one line |
| `scroll-down` | Scroll down one line |
| `scroll-middle` | Centre the current line on screen |

### Character jumps and marks

| Name | Description |
|------|-------------|
| `jump-forward` | Jump forward to the next occurrence of a character |
| `jump-backward` | Jump backward to the previous occurrence of a character |
| `jump-to-forward` | Jump forward to just before the next occurrence |
| `jump-to-backward` | Jump backward to just after the previous occurrence |
| `jump-again` | Repeat the last jump in the same direction |
| `jump-reverse` | Repeat the last jump in the opposite direction |
| `set-mark` | Set the mark at the cursor |
| `jump-to-mark` | Jump to the mark |

### Selection and copying

| Name | Description |
|------|-------------|
| `begin-selection` | Start a selection at the cursor |
| `stop-selection` | Stop extending the selection without clearing it |
| `clear-selection` | Discard the selection and stay in copy mode (Escape with `mode-keys vi`) |
| `select-line` | Select the whole current line |
| `select-word` | Select the word under the cursor |
| `rectangle-toggle` | Toggle block (rectangular) selection |
| `other-end` | Move the cursor to the other end of the selection |
| `copy-selection` | Copy the selection and stay in copy mode |
| `copy-selection-and-cancel` | Copy the selection and leave copy mode |
| `copy-selection-no-clear` | Copy the selection without clearing it |
| `copy-end-of-line` | Copy from the cursor to the end of the line |
| `copy-line` | Copy the whole current line (psmux extension) |
| `append-selection` | Append the selection to the current buffer |
| `append-selection-and-cancel` | Append the selection and leave copy mode |

### Search

| Name | Description |
|------|-------------|
| `search-forward [term]` | Search forward. With a term the search runs straight away, without one the prompt opens. `search-forward-incremental` is an accepted synonym |
| `search-backward [term]` | Search backward. With a term the search runs straight away, without one the prompt opens. `search-backward-incremental` is an accepted synonym |
| `search-again` | Repeat the last search in the same direction |
| `search-reverse` | Repeat the last search in the opposite direction |

Searching covers the whole pane buffer, the scrollback history included, and the
viewport scrolls to bring an off screen match into view.

### Mode control

| Name | Description |
|------|-------------|
| `cancel` | Leave copy mode |
| `refresh-from-pane` | Copy the pane's current output into copy mode once, keeping the line at the top of the view (tmux 3.4 to 3.7c, bound to `r`). `refresh-now` is an accepted synonym |
| `refresh-on` / `refresh-off` / `refresh-toggle` | Turn on, off or toggle automatic refresh: copy mode takes a new copy whenever the pane prints, following the output while the cursor is on the last row at the live bottom, and pausing during a selection (tmux after 3.7c) |

`copy-line` has no tmux equivalent. Everything else in these tables is named the same way it is in tmux.

## Session Groups

A session group ties sessions together so grouping aware formats and tooling can see them as one
logical unit.

```powershell
# Put the current session in a group
psmux set -g session-group backend

# Read it back
psmux display-message -p "group=#{session_group} size=#{session_group_size} grouped=#{session_grouped}"
# group=backend size=1 grouped=1

# Clear the grouping
psmux set -g session-group none
```

`#{session_group}`, `#{session_group_list}`, `#{session_group_size}`,
`#{session_group_attached}` and `#{session_grouped}` all report group state and can be used in a
`-F` format or in the status bar.

The server spawn path also accepts a group directly:

```powershell
psmux server -g backend -s api
```

`psmux server` is the low level headless server entry point. For everyday use prefer
`set -g session-group <name>` in a config file or at runtime.

## User Defined Command Aliases

`command-alias` maps a short name to a command line:

```tmux
set -g command-alias 'sph=split-window -h'
set -g command-alias 'bigger=resize-pane -R 20'
```

```powershell
psmux show-options | Select-String command-alias
# command-alias "sph=split-window -h"
```

Aliases are resolved by the server's command dispatcher, which is the path a key binding takes.
They are **not** resolved by the psmux CLI front end:

```powershell
psmux sph
# psmux: unknown command: sph
```

The same asymmetry applies to the config file and hook execution path and to the control mode
dispatcher, which also report `unknown command` for an alias. Treat `command-alias` as a
key binding convenience rather than as a way to add a new CLI verb, and use a PowerShell function
or an alias in your profile if you want a short name on the command line.

## Prompt History

The `command-prompt` overlay keeps a persistent history that `Up` and `Down` walk through. Two
commands manage it:

| Command | Alias | Description |
|---------|-------|-------------|
| `show-prompt-history` | `showphist` | Print the saved command prompt history |
| `clear-prompt-history` | `clearphist` | Discard the saved command prompt history |

Both are server side commands, reachable from a key binding or from the command prompt itself,
not from the `psmux` CLI. `psmux show-prompt-history` reports `unknown command`.

```tmux
bind-key H show-prompt-history
bind-key M-H clear-prompt-history
```

## Cross Session Pane Transfer

`join-pane` and `move-pane` accept a `-s` source in **another session**, including a session that
lives on an independent server. The pane's real console stays where it was created and its input
and output are tunnelled to the new home over TCP, so a long running process survives the move.

```powershell
psmux new-session -d -s alpha
psmux new-session -d -s beta

# Pull beta's first pane into alpha, side by side
psmux -t alpha join-pane -h -s 'beta:0.0'

psmux -t alpha list-panes -F '#{pane_id} #{pane_left},#{pane_top}'
# %1 0,0
# %2 50,0
```

`move-pane` behaves the same way and also removes the pane from the source window. Use `-h` or
`-v` to choose the split direction and `-d` to avoid focusing the transplanted pane.

The moved pane belongs to its new session from then on, as in tmux. If it was the last pane of
the source session, that session is gone (`ls` and `has-session` no longer see it and the name is
free again), and `kill-session` on the source no longer touches a pane that was moved out of it.
The source server process keeps hosting the console in the background and exits by itself when the
pane exits or is killed. If that process is ended some other way, the pane in the new session
turns dead like any exited pane: `pane-exited` or `pane-died` fires and `remain-on-exit` decides
whether it stays. One difference from tmux remains: pane ids are per server in psmux, so the moved
pane gets a new `%id` in its new session.

## Mouse Wire Commands

The client normally speaks these to the server on your behalf, but five of them are also accepted
at the CLI, which makes them a usable hook for driving mouse behaviour from a script or a test:

| Command | Arguments | Description |
|---------|-----------|-------------|
| `mouse-down` | `<x> <y>` | Left button press at that client cell |
| `mouse-drag` | `<x> <y>` | Drag to that client cell with the button held |
| `mouse-up` | `<x> <y>` | Left button release at that client cell |
| `mouse-down-right` | `<x> <y>` | Right button press |
| `mouse-up-right` | `<x> <y>` | Right button release |

```powershell
# Click at column 40, row 12 of the client terminal
psmux mouse-down 40 12
psmux mouse-up 40 12

# Drag a selection from column 10 to column 30 on row 5
psmux mouse-down 10 5
psmux mouse-drag 30 5
psmux mouse-up 30 5
```

Coordinates are client cell coordinates, zero based, the same space `#{mouse_x}` and
`#{mouse_y}` report. These commands act on the client's view, so they need `mouse` to be on and a
client attached to have a visible effect.

