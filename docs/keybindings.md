# Key Bindings

Default prefix: `Ctrl+b` (same as tmux). Change with `set -g prefix C-a`.

Supported prefix keys include `C-a` through `C-z`, `C-Space`, and any printable character.

Changing the prefix with `set -g prefix <key>` also auto binds the new key to `send-prefix`, so
pressing the prefix twice always forwards one literal prefix byte to the active pane. That is how a
shell like bash or nushell still sees `Ctrl+a` as "go to start of line" when `C-a` is your prefix.

A second prefix is available with `set -g prefix2 <key>`. Both keys are checked on every keystroke,
so either one arms the prefix table.

## Case Sensitivity

Key bindings are **case-sensitive**, matching tmux behavior:

- `bind-key t` binds to lowercase `t` (just press `t`)
- `bind-key T` binds to uppercase `T` (`Shift+t`)

This is essential for plugins like PPM (`Prefix+I`/`Prefix+U`) and psmux-sensible (`Prefix+R`).

## Key Tables

A key table is a named set of bindings. psmux consults these tables:

| Table | How keys reach it | Default contents |
|-----|--------|--------|
| `root` | Every keystroke, before the prefix is armed. Bind with `bind-key -n <key>` or `bind-key -T root <key>` | **Empty.** psmux binds nothing here by default |
| `prefix` | Keystrokes that follow the prefix key. Bind with plain `bind-key <key>` | The full default set listed below |
| `copy-mode` | Keystrokes while copy mode is active and `mode-keys` is `emacs` (the default) | Empty. Bind with `bind-key -T copy-mode <key>` |
| `copy-mode-vi` | Keystrokes while copy mode is active and `mode-keys` is `vi` | Empty. Bind with `bind-key -T copy-mode-vi <key>` |
| any custom table | Only after `switch-client -T <table>`, and only for the very next keystroke | Whatever you bind into it |

The `copy-mode` and `copy-mode-vi` tables are empty by default because the built in copy mode keymap
is hardcoded. Your bindings in those tables are checked **first**, so binding a key there overrides
the built in behavior for that key; every key you do not bind falls through to the built in map.

`switch-client -T <table>` installs a one-shot table. The next key is looked up there, and the table
is discarded whether or not the key matched. This is how multi key chords are built.

Run `list-keys` (or `Prefix + ?`) to print every binding currently in effect.

### The root table is empty on purpose

Nothing is bound without a prefix by default. In particular a bare `PageUp` is **not** a copy mode
key: it is forwarded untouched so pagers, editors and full screen apps see it. This matches tmux,
where `PPage` is bound only in the prefix table.

To get the old psmux behavior back, bind it yourself:

```tmux
bind-key -n PageUp copy-mode -u
```

One caveat with that recipe. The `scroll-enter-copy-mode` option suppresses any **root** binding
whose command starts with `copy-mode` and contains `-u`. With `set -g scroll-enter-copy-mode off`
the binding above stops firing and the key is forwarded to the pane instead. The prefix binding
`Prefix + PageUp` is unaffected by that option.

### Overlay keys never reach a key table

Overlay UI keys are handled client side and **earlier than any key table**. While a chooser, menu,
confirmation prompt, customize view, keys viewer or the command prompt is open, the overlay consumes
the keystroke. A key you bound with `bind-key` or `bind-key -n` will not fire, and the key is not
forwarded to the pane either. Close the overlay first. The per overlay keymaps are listed below.

## Prefix Keys

### Sending the Prefix

| Key | Action |
|-----|--------|
| `Prefix + Ctrl+b` | Send one literal prefix byte to the pane (`send-prefix`) |

Because the prefix key is `Ctrl+b` by default, this is simply "press the prefix twice". If you
rebind the prefix, the new key is bound to `send-prefix` for you.

### Window Management

| Key | Action |
|-----|--------|
| `Prefix + c` | Create new window |
| `Prefix + n` | Next window |
| `Prefix + p` | Previous window |
| `Prefix + l` | Last (previously active) window |
| `Prefix + w` | Interactive session/window/pane chooser (`choose-tree`) |
| `Prefix + &` | Kill current window (with confirmation) |
| `Prefix + ,` | Rename current window |
| `Prefix + '` | Prompt for window index (jump to any window) |
| `Prefix + .` | Prompt for an index and move the current window there (`move-window`) |
| `Prefix + 0-9` | Select window by number |

**Gotcha: `select-window-index` is not a real command.** `Prefix + '` is bound to
`select-window-index`, which looks like a command but is a client only pseudo command. It exists
solely so the client can open the window index prompt. It is not accepted by any command dispatcher,
so `psmux select-window-index` fails with "unknown command", and it cannot be used from a config
line, from a hook, or from `run-shell`. Rebinding another key to it does work, because key bindings
go through the client.

### Pane Splitting

| Key | Action |
|-----|--------|
| `Prefix + %` | Split pane left/right (horizontal) |
| `Prefix + "` | Split pane top/bottom (vertical) |

### Pane Navigation

| Key | Action |
|-----|--------|
| `Prefix + Arrow` | Navigate between panes (Up/Down/Left/Right), wraps at edges |
| `Prefix + o` | Select next pane (rotate) |
| `Prefix + ;` | Last (previously active) pane |
| `Prefix + q` | Display pane numbers (type number to switch, auto-dismisses) |

### Pane Management

| Key | Action |
|-----|--------|
| `Prefix + x` | Kill current pane (with confirmation) |
| `Prefix + z` | Toggle pane zoom (fullscreen) |
| `Prefix + {` | Swap pane up |
| `Prefix + }` | Swap pane down |
| `Prefix + !` | Break pane out to new window |

### Pane Resize

| Key | Action |
|-----|--------|
| `Prefix + Ctrl+Arrow` | Resize pane by 1 cell |
| `Prefix + Alt+Arrow` | Resize pane by 5 cells |

### Layout

| Key | Action |
|-----|--------|
| `Prefix + Space` | Cycle to next layout |
| `Prefix + Alt+1` | Even-horizontal layout |
| `Prefix + Alt+2` | Even-vertical layout |
| `Prefix + Alt+3` | Main-horizontal layout |
| `Prefix + Alt+4` | Main-vertical layout |
| `Prefix + Alt+5` | Tiled layout |

### Session

| Key | Action |
|-----|--------|
| `Prefix + d` | Detach from session |
| `Prefix + $` | Rename session |
| `Prefix + s` | Session chooser/switcher (`choose-session`) |
| `Prefix + (` | Switch to previous session |
| `Prefix + )` | Switch to next session |

### Copy / Paste

| Key | Action |
|-----|--------|
| `Prefix + [` | Enter copy/scroll mode |
| `Prefix + PageUp` | Enter copy mode scrolled up one page (`copy-mode -u`) |
| `Prefix + ]` | Paste from buffer |
| `Prefix + =` | Interactive buffer chooser (`choose-buffer`) |
| `Prefix + #` | List paste buffers (`list-buffers`) |
| `Prefix + v` | Toggle rectangle selection in copy mode (`rectangle-toggle`) |
| `Prefix + y` | Copy the current selection (`copy-yank`) |

`Prefix + v` and `Prefix + y` mirror the copy mode keys of the same name, so the muscle memory works
whether or not you have entered copy mode through the prefix table.

### Miscellaneous

| Key | Action |
|-----|--------|
| `Prefix + :` | Command prompt (with cursor, arrow key navigation, and history) |
| `Prefix + ?` | List keybindings (help overlay) |
| `Prefix + i` | Display window/pane info |
| `Prefix + t` | Clock mode |

### Repeat Bindings

Navigation and resize bindings support **repeat mode**: after pressing the prefix key once, successive keypresses within the `repeat-time` window (default 500ms) trigger the action without needing to re-enter the prefix. This applies to arrow-based pane navigation and resize bindings by default.

## Overlay Keys

Each overlay owns its own keymap. Remember that these keys are consumed before any key table.

### Chooser Navigation (choose-tree, choose-session)

Open with `Prefix + w` (tree) or `Prefix + s` (sessions). This matches tmux's `mode-tree` behavior,
so muscle memory carries over.

| Key | Action |
|-----|--------|
| `Up` / `k` / `h` | Move selection up |
| `Down` / `j` / `l` | Move selection down |
| `g` / `Home` | Jump to first entry |
| `G` / `End` | Jump to last entry |
| `PageUp` / `PageDown` | Move selection by 10 rows |
| `1`..`9`, `0` | Append digit to the jump buffer (Enter consumes it) |
| `Backspace` | Edit the jump buffer |
| `Enter` | Switch to the selected entry (or to the jump buffer index if non-empty) |
| `f` | Enter session-name filter mode (choose-session only) |
| `p` | Toggle live preview |
| `x` | Kill the highlighted entry (session in choose-session, window in choose-tree) |
| `Esc` | Clear an active session filter; otherwise close the chooser |

In the session picker, `f` starts filter mode, which matches session names
case-insensitively as you type. `Backspace` edits the filter. `Esc` with filter
text entered clears the filter and shows every session again; `Esc` with no
filter text closes the picker.

`q` does **not** close these two choosers. Every printable key other than the ones above is
swallowed so it cannot leak into the focused pane, and `q` falls into that group. Use `Esc`.

### Buffer Chooser (choose-buffer)

Open with `Prefix + =`.

| Key | Action |
|-----|--------|
| `Up` / `k` / `h` | Move selection up |
| `Down` / `j` / `l` | Move selection down |
| `g` / `Home` | Jump to first buffer |
| `G` / `End` | Jump to last buffer |
| `PageUp` / `PageDown` | Move selection by 10 rows |
| `1`..`9`, `0` | Append digit to the jump buffer |
| `Backspace` | Edit the jump buffer |
| `Enter` | Paste the selected buffer (or the jump buffer index if non-empty) |
| `d` / `Delete` | Delete the selected buffer and stay open |
| `Esc` / `q` | Close the chooser |

### Keys Viewer (list-keys)

Open with `Prefix + ?`. This overlay scrolls a rendered list, it has no selection.

| Key | Action |
|-----|--------|
| `Up` / `k` / `h` | Scroll up one line |
| `Down` / `j` / `l` | Scroll down one line |
| `PageUp` / `PageDown` | Scroll by 20 lines |
| `g` / `Home` | Jump to the top |
| `G` / `End` | Jump to the bottom |
| `Esc` / `q` | Close the viewer |

### Customize Mode (customize-mode)

An interactive options editor. Navigation keys apply while browsing; a separate set applies once you
press `Enter` on an option and start editing its value.

Browsing:

| Key | Action |
|-----|--------|
| `Up` / `k` / `h` | Move selection up |
| `Down` / `j` / `l` | Move selection down |
| `PageUp` / `PageDown` | Move selection by 20 rows |
| `g` / `Home` | Jump to the first option |
| `G` / `End` | Jump to the last option |
| `1`..`9`, `0` | Append digit to the jump buffer |
| `Backspace` | Edit the jump buffer |
| `Enter` | Edit the highlighted option, or jump to the jump buffer index if non-empty |
| `d` | Reset the highlighted option to its built in default |
| `/` | Clear the active option filter |
| `Esc` / `q` | Close the editor |

Editing a value:

| Key | Action |
|-----|--------|
| Any character | Insert at the cursor |
| `Backspace` | Delete the character before the cursor |
| `Enter` | Confirm the new value |
| `Esc` | Cancel and keep the old value |

Note on `/`: it only clears a filter that is already set. There is no way to type a new filter from
inside the overlay, so `/` on an unfiltered list does nothing.

### Menu Overlay (display-menu)

| Key | Action |
|-----|--------|
| `Up` / `k` | Move selection up |
| `Down` / `j` | Move selection down |
| `Enter` | Run the highlighted item |
| Any other character | Run the item whose mnemonic key is that character |
| `Esc` / `q` | Close the menu |

Digits are mnemonics, not positions. A digit runs the item whose shortcut key is that digit, and
does nothing if no item claims it. `q` closes the menu even if an item uses `q` as its mnemonic.

### Confirmation Prompt (confirm-before)

| Key | Action |
|-----|--------|
| `y` / `Y` | Confirm and run the command |
| `n` / `N` / `Esc` | Cancel |

Every other key is ignored, so the prompt cannot be dismissed by accident.

### Text Popup Viewer

Shown by commands that display text in a popup, for example `show-messages`. A `display-popup`
running a real program is a PTY popup instead, and forwards keys to that program rather than using
this map.

| Key | Action |
|-----|--------|
| `Up` / `k` | Scroll up one line |
| `Down` / `j` | Scroll down one line |
| `PageUp` / `PageDown` | Scroll by 10 lines |
| `g` / `Home` | Jump to the top |
| `G` / `End` | Jump to the bottom |
| `Esc` / `q` | Close the popup |

### Display Panes Overlay

Shown by `Prefix + q`.

| Key | Action |
|-----|--------|
| `0`..`9` | Select the pane with that number |
| Any other key | Dismiss the overlay |

## Command Prompt

Press `Prefix + :` to open the command prompt at the bottom of the screen. You can type any
psmux/tmux command here.

| Key | Action |
|-----|--------|
| `Left` / `Right` | Move cursor within the command |
| `Home` / `Ctrl+A` | Jump to start of line |
| `End` / `Ctrl+E` | Jump to end of line |
| `Backspace` | Delete character before cursor |
| `Delete` | Delete character at cursor |
| `Ctrl+U` | Kill line (clear to start) |
| `Ctrl+K` | Kill to end of line |
| `Ctrl+W` | Delete word backward |
| `Up` / `Down` | Browse command history (older/newer) |
| `Tab` | Command name completion |
| `Enter` | Execute the command (saved to history) |
| `Escape` | Cancel and close the prompt |

The command prompt remembers your history across the session. Use Up/Down arrows to recall previous
commands. Inspect it with `show-prompt-history` and wipe it with `clear-prompt-history`.

You can run any command from the prompt that you would run from the CLI. For example:

- `:split-window -h` to split horizontally
- `:new-window -n logs` to create a named window
- `:source-file ~/.psmux.conf` to reload your config
- `:set -g status-style "bg=blue"` to change a setting live
- `:list-keys` to see all current key bindings

## Copy/Scroll Mode (Vi)

Enter copy mode with `Prefix + [`, or with `Prefix + PageUp` to start one page up.

Mouse scroll wheel also enters copy mode by default. To disable this, set `scroll-enter-copy-mode off` in your config.

Copy mode belongs to the pane it was entered in, as in tmux. Splitting, opening a window or
killing another pane leaves that pane in copy mode and starts the new or newly focused pane live;
several panes can be in copy mode at once, and `#{pane_in_mode}` answers for the pane you target.

The keys below are the built in copy mode map. Anything you bind with
`bind-key -T copy-mode-vi <key>` is checked first and wins for that key.

### Cursor Movement

| Key | Action |
|-----|--------|
| `h` / `Left` | Move cursor left |
| `j` / `Down` | Move cursor down |
| `k` / `Up` | Move cursor up |
| `l` / `Right` | Move cursor right |
| `Ctrl+p` / `Ctrl+n` | Move cursor up / down (same as `k` / `j`, emacs style, works in vi mode too) |
| `Ctrl+a` | Start of line (emacs style, works in vi mode too) |
| `Ctrl+e` | End of line, **`mode-keys emacs` only** (in vi mode `Ctrl+e` scrolls, use `$`) |

Cursor motions keep the viewport still until the cursor reaches the top or the bottom row of the
pane. Only then does the view follow, one line at a time. To move the view itself while the cursor
stays put, use the scrolling keys below.

### Word Motions

| Key | Action |
|-----|--------|
| `w` / `b` / `e` | Next word / prev word / end of word |
| `W` / `B` / `E` | WORD variants (whitespace-delimited) |
| `Alt+f` / `Alt+b` | Word forward / backward (emacs style, works in vi mode too) |

### Line Motions

| Key | Action |
|-----|--------|
| `0` / `Home` | Start of line |
| `$` / `End` | End of line |
| `^` | First non-blank character |

### Scrolling

| Key | Action |
|-----|--------|
| `Ctrl+u` / `Ctrl+d` | Half page up / down |
| `Ctrl+b` / `PageUp` | Full page up |
| `Ctrl+f` / `PageDown` | Full page down |
| `Ctrl+Up` / `Ctrl+Down` | Scroll up / down one line (cursor stays where it is) |
| `Ctrl+y` / `Ctrl+e` | Scroll up / down one line, **`mode-keys vi` only** |
| `K` / `J` | Scroll up / down one line, **`mode-keys vi` only** |
| `g` | Top of scrollback |
| `G` | Bottom (live output) |
| `z` | Centre the cursor line in the pane (scroll-middle) |
| `r` | Toggle following live output (see below) |

`r` is a psmux extension with no tmux equivalent. Copy mode normally anchors the view so new output
cannot shift the text under your cursor. `r` releases that anchor, so the pane follows live output
again and jumps to the bottom of the history. Press `r` again to re-anchor.

### Screen Position

| Key | Action |
|-----|--------|
| `H` | Jump to top of visible area |
| `M` | Jump to middle of visible area |
| `L` | Jump to bottom of visible area |

### Character Find

| Key | Action |
|-----|--------|
| `f{char}` / `F{char}` | Find char forward / backward |
| `t{char}` / `T{char}` | Till char forward / backward |
| `;` | Repeat the last find in the same direction |
| `,` | Repeat the last find in the opposite direction |

### Marks

| Key | Action |
|-----|--------|
| `X` | Set the mark at the cursor |
| `Alt+x` | Exchange the cursor and the mark |

`Alt+x` swaps rather than jumps, matching tmux. Pressing it twice returns you to where you started.

### Bracket / Paragraph

| Key | Action |
|-----|--------|
| `%` | Jump to matching bracket (`()`, `[]`, `{}`, `<>`) |
| `{` | Jump to previous paragraph (blank line) |
| `}` | Jump to next paragraph (blank line) |

### Selection

| Key | Action |
|-----|--------|
| `Space` | Begin character selection at the cursor |
| `V` | Begin line selection at the cursor |
| `Ctrl+Space` | Set the selection anchor at the cursor |
| `v` | Toggle rectangle mode on the selection (does not start one) |
| `Ctrl+v` | Force rectangle mode on the selection |
| `o` | Swap cursor/anchor ends |

**psmux follows tmux here, not vi.** In vi, `v` starts a character selection. In tmux and in psmux,
`v` is `rectangle-toggle`: it flips the selection between character mode and block mode and does not
set an anchor, so pressing `v` on its own selects nothing. Use `Space` (or `Ctrl+Space`) to start a
selection and `V` to start a line selection. `Ctrl+v` differs from `v` in that it always switches to
rectangle mode rather than toggling out of it. If you want vi muscle memory, rebind it:

```tmux
bind-key -T copy-mode-vi v send-keys -X begin-selection
```

### Yank (Copy)

| Key | Action |
|-----|--------|
| `y` / `Enter` | Copy selection and exit |
| `Alt+w` | Copy selection and exit (emacs style, works in vi mode too) |
| `D` | Copy from the cursor to end of line and exit |
| `A` | Append selection to the buffer and exit |

### Search

| Key | Action |
|-----|--------|
| `/` | Search forward |
| `?` | Search backward |
| `Ctrl+s` / `Ctrl+r` | Search forward / backward (emacs style, works in vi mode too) |
| `n` / `N` | Next / previous match |

Search covers the whole scrollback, not just the rows on screen. A match that is off screen
scrolls the view to it, parked a quarter of a screen from the bottom the way tmux does, and
`?` stops at the nearest match above the cursor.

### Text Objects & Registers

| Key | Action |
|-----|--------|
| `"a` to `"z` | Named registers (set register for next yank) |
| `aw` / `iw` | Select a word / inner word |
| `aW` / `iW` | Select a WORD / inner WORD |
| `1` to `9` | Numeric prefix for motions (up to 9999) |

### Exit

| Key | Action |
|-----|--------|
| `Esc` / `q` / `]` | Exit copy mode |
| `Ctrl+C` / `Ctrl+G` | Exit copy mode |

### Copy Mode Search Prompt

Opened by `/`, `?`, `Ctrl+s` or `Ctrl+r`. While it is open the copy mode keys are inactive.

| Key | Action |
|-----|--------|
| Any character | Append to the search pattern |
| `Backspace` | Delete the last character |
| `Enter` | Accept the search and jump to the match |
| `Esc` | Cancel the search |

### Emacs Copy Mode

`set -g mode-keys emacs` does not add a separate keymap. Most emacs style keys listed above are
always active, in vi mode too. What `mode-keys emacs` changes is the meaning of the keys tmux binds
differently in its `copy-mode` and `copy-mode-vi` tables:

| Key | With `mode-keys vi` | With `mode-keys emacs` (default) |
|-----|--------|--------|
| `Ctrl+b` | Page up | Move cursor left |
| `Ctrl+f` | Page down | Move cursor right |
| `Ctrl+v` | Force rectangle selection | Page down |
| `Ctrl+e` | Scroll down one line | End of line |
| `Ctrl+y` | Scroll up one line | Nothing |
| `J` / `K` | Scroll down / up one line | Nothing |

In vi mode `Ctrl+e` scrolls, so use `$` (or `End`) to reach the end of the line there. This is
tmux's split: `copy-mode` binds `Ctrl+e` to end-of-line and leaves `Ctrl+y`, `J` and `K` unbound,
while `copy-mode-vi` binds `Ctrl+e` and `J` to scroll down and `Ctrl+y` and `K` to scroll up.

These keys behave the same under both settings: `Ctrl+p` / `Ctrl+n` move the cursor up / down,
`Ctrl+Up` / `Ctrl+Down` scroll one line, `Ctrl+a` goes to line start, `Alt+f` / `Alt+b` move by
word, `Alt+v` pages up, `Alt+w` copies and exits, `Ctrl+s` / `Ctrl+r` search forward / backward.

`Ctrl+p` / `Ctrl+n` are cursor motions, not scrolling, which is what tmux binds them to in its
`copy-mode` table. psmux keeps them available in vi mode as well, where tmux leaves them unbound.

When in copy mode:
- The pane border turns **yellow**
- `[copy mode]` appears in the title
- A scroll position indicator shows in the top-right corner
- Mouse drag-select copies to Windows clipboard on release

## Mouse Bindings

When `mouse on` (default):

| Action | Behavior |
|--------|----------|
| Left-click status tab | Switch to clicked window |
| Left-click pane | Focus that pane |
| Left-click/drag border | Resize split interactively |
| Scroll up/down | Scroll pane (or enter copy mode at prompt) |
| Mouse drag in copy mode | Select text, auto-copy on release |
| Drag held on the pane's first or last row | Auto-scroll the selection through scrollback (faster once the pointer leaves the pane) |
| Right-click | Paste clipboard |

## Supported Key Names

Key names for `bind-key` and `send-keys`:

| Key | Name |
|-----|------|
| Arrow keys | `Up`, `Down`, `Left`, `Right` |
| Function keys | `F1` through `F12` |
| Special keys | `Enter`, `Tab`, `Escape`, `Space`, `Backspace` |
| Navigation | `Home`, `End`, `PageUp`, `PageDown`, `Insert`, `Delete` |
| Ctrl modifier | `C-a` through `C-z`, `C-Space` |
| Alt modifier | `M-a` through `M-z`, `M-Left`, `M-Right`, etc. |
| Shift+key | Use uppercase letter: `T` for `Shift+t` |
| Shift+Enter | `S-Enter` (sends proper escape sequence) |
| Shift+Tab | `BTab` (sends `ESC [ Z`) |
| Ctrl+Backspace | `C-BSpace` (writes `0x08`, the native Windows encoding, so PSReadLine runs BackwardKillWord; `M-BSpace` writes `ESC 0x7f`). A terminal that sends a bare `0x08` for Ctrl+Backspace is seen as `C-h`, which is also what tmux reports for it |
| Non-ASCII keys | Any single Unicode character: `ы`, `é`, `ß`, with the usual modifiers (`M-ф`, `C-ы`, `S-ы`). `list-keys` prints the character as typed |
| Mouse key names | tmux's mouse names (`WheelUpPane`, `MouseDown1Pane`, `DoubleClick1Pane`, and so on), `Any`, `None`, `Mouse`, `FocusIn`/`FocusOut`, `PasteStart`/`PasteEnd` and `UserN` are accepted so a ported tmux config loads, but psmux does not act on them |

Aliases are accepted for several of these: `Return` for `Enter`, `Esc` for `Escape`,
`BSpace` for `Backspace`, `PPage` or `PgUp` for `PageUp`, `NPage` or `PgDn` for `PageDown`,
`IC` for `Insert`, `DC` for `Delete`, `BackTab` for `BTab`.

A key name psmux cannot parse is never dropped silently. `psmux bind-key WheelUpPain ...` prints
`unknown key: WheelUpPain` and exits 1, like tmux; the same line in a config file is reported in
the boot summary and in `~/.psmux/config-warnings.log` (see [diagnostics.md](diagnostics.md)).

## Custom Key Bindings

```tmux
# Bind in prefix table (default)
bind-key h split-window -h
bind-key v split-window -v

# Bind in root table (no prefix needed)
bind-key -n C-h select-pane -L

# Restore the pre-issue-488 bare PageUp behavior
bind-key -n PageUp copy-mode -u

# Bind inside copy mode
bind-key -T copy-mode-vi v send-keys -X begin-selection

# Repeatable binding (stay in prefix mode)
bind-key -r H resize-pane -L 5

# Unbind a key
unbind-key C-b

# Unbind all
unbind-key -a
```

Default bindings are loaded into the `prefix` table like any other binding, so `unbind-key <key>`
really does remove a default rather than being shadowed by it.

## Confirmation Prompts (confirm-before)

By default, destructive keybindings like `Prefix + x` (kill-pane) and `Prefix + &` (kill-window) show a y/n confirmation prompt before executing. This uses the `confirm-before` wrapper, matching tmux behavior.

### Skipping Confirmation

To bind kill commands **without** confirmation, bind the command directly in your config:

```tmux
# Kill pane immediately (no y/n prompt)
bind-key x kill-pane

# Kill window immediately (no y/n prompt)
bind-key & kill-window

# Kill session on a custom key (no prompt)
bind-key X kill-session
```

### Adding Confirmation to Any Command

You can wrap any command with `confirm-before` to require y/n confirmation:

```tmux
# Confirm before killing pane (this is the default)
bind-key x confirm-before -p 'kill-pane #P? (y/n)' kill-pane

# Confirm before killing window (this is the default)
bind-key & confirm-before -p 'kill-window #W? (y/n)' kill-window

# Confirm before killing session
bind-key X confirm-before -p 'kill-session? (y/n)' kill-session

# Confirm before detaching
bind-key d confirm-before -p 'detach? (y/n)' detach-client
```

The `-p` flag sets a custom prompt string. Without it, a generic prompt is shown.
