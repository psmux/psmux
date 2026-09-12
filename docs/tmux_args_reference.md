# psmux Command and Flag Reference

This is the reference for the commands **psmux itself** accepts and the flags **psmux itself** parses. Every entry was read out of the psmux Rust sources, not out of the upstream tmux manual, so a flag listed here is a flag psmux actually looks at. Where tmux has a flag and psmux does not parse it, the flag is either absent from this page or listed under "Accepted but ignored".

`psmux list-commands` is the live authority. If this page and `list-commands` disagree, `list-commands` wins and this page is stale.

## How to read this page

**Layers.** psmux matches command names in four separate places, and they do not all accept the same set. A command is usable only on the layers listed for it.

| Layer | What reaches it |
|---|---|
| `CLI` | `psmux <command>` typed in a shell |
| `SRV` | key bindings, and anything the CLI forwards over the socket |
| `CFG` | config file lines, hooks, `run-shell`, `if-shell`, `bind-key` actions |
| `CTL` | control mode clients (`psmux -C` / `psmux -CC`) |

**The flags column** uses getopt style notation. A bare letter is a boolean, a letter followed by `:` takes a value. `-t:` therefore means `-t <target>`.

**A value taking flag must be given its value.** `psmux kill-window -t` with nothing after the `-t` is an error on every layer: `-t expects an argument` at exit 1, and the command does not run. This is tmux's rule (`arguments.c`, `args_parse_flags`) and it matters most for the commands whose no target default is destructive: before psmux enforced it, a bare `-t` made `kill-window` kill the current window and `kill-session` destroy the current session, both silently at exit 0 (issue #635). Note the shape tmux uses for the value itself: a required value swallows whatever token follows, so `kill-window -t -a` means the target is literally `-a`, and `resize-pane -x -5` passes `-5` as the size. Flag parsing ends at the first non flag argument and at `--`, so a `-t` after either is data, not a flag.

**`-t` is global at the CLI.** psmux scans the whole command line for `-t <target>` before the command name is even dispatched, so `psmux -t work:1 kill-pane` and `psmux kill-pane -t work:1` both work even for commands whose own parser ignores `-t`.

**Combined short flags** are expanded for `new-session` (`-As main` is `-A -s main`), for `set-option` and `show-options` (`-ga`, `-gu`, `-gq`), for `set-hook` (`-ga`, `-ug`) and for the config file forms of `if-shell` (`-bF`, `-Fb`). Elsewhere flags must be given separately.

## Complete command table

| Command | Aliases | Flags psmux parses | Layers |
|---|---|---|---|
| `attach-session` | `attach`, `a`, `at` | `t:` | CLI, SRV, CFG |
| `bind-key` | `bind` | `nrT:` | CLI, SRV, CFG, CTL |
| `break-pane` | `breakp` | `dt:` | CLI, SRV, CFG, CTL |
| `capture-pane` | `capturep` | `eJpb:E:S:t:` | CLI, SRV, CFG, CTL |
| `choose-buffer` | `chooseb` | none | CLI, SRV, CFG |
| `choose-client` | *(none)* | none | CLI, SRV, CFG |
| `choose-session` | *(none)* | none | CLI, SRV, CFG |
| `choose-tree` | *(none)* | none | CLI, SRV, CFG |
| `choose-window` | *(none)* | none | CLI, SRV, CFG |
| `clear-history` | `clearhist` | `Ht:` | CLI, SRV, CFG |
| `clear-prompt-history` | `clearphist` | none | SRV |
| `clock-mode` | *(none)* | none | CLI, SRV, CFG |
| `command-prompt` | *(none)* | `NWI:p:T:t:` | CLI, SRV, CFG |
| `confirm-before` | `confirm` | `p:` | CLI, SRV, CFG |
| `copy-mode` | *(none)* | `deHqut:` | CLI, SRV, CFG, CTL |
| `customize-mode` | *(none)* | none | CLI, SRV, CFG |
| `delete-buffer` | `deleteb` | `b:` | CLI, SRV, CFG |
| `detach-client` | `detach` | `aPE:s:t:` | CLI, SRV, CFG, CTL |
| `display-menu` | `menu` | `T:x:y:` | CLI, SRV, CFG |
| `display-message` | `display` | `pFd:I:t:` | CLI, SRV, CFG, CTL |
| `display-panes` | `displayp` | none | CLI, SRV, CFG |
| `display-popup` | `popup` | `EKc:d:h:w:` | CLI, SRV, CFG |
| `dump-layout` | *(none)* | none | SRV |
| `dump-state` | `dump` | none | CLI, SRV, CTL |
| `find-window` | `findw` | none that take effect | CLI, SRV, CFG |
| `has-session` | `has` | `t:` | CLI, SRV, CFG, CTL |
| `if-shell` | `if` | `bFt:` | CLI, SRV, CFG |
| `join-pane` | `joinp` | `dhvs:t:` | CLI, SRV, CFG |
| `kill-pane` | `killp` | `t:` | CLI, SRV, CFG, CTL |
| `kill-server` | *(none)* | none | CLI, SRV, CFG, CTL |
| `kill-session` | `kill-ses` | `t:` | CLI, SRV, CFG |
| `kill-window` | `killw` | `at:` | CLI, SRV, CFG, CTL |
| `last-pane` | `lastp` | none | CLI, SRV, CFG, CTL |
| `last-window` | `last` | none | CLI, SRV, CFG, CTL |
| `link-window` | `linkw` | `s:t:` | CLI, SRV, CFG |
| `list-buffers` | `lsb` | `F:t:` | CLI, SRV, CFG, CTL |
| `list-clients` | `lsc` | `F:` (SRV and CTL only) | CLI, SRV, CFG, CTL |
| `list-commands` | `lscm` | none | CLI, SRV, CFG, CTL |
| `list-keys` | `lsk` | `T:t:` | CLI, SRV, CFG, CTL |
| `list-panes` | `lsp` | `asF:t:` | CLI, SRV, CFG, CTL |
| `list-sessions` | `ls` | `F:f:` | CLI, SRV, CFG, CTL |
| `list-tree` | *(none)* | none | SRV |
| `list-windows` | `lsw` | `aJF:t:` | CLI, SRV, CFG, CTL |
| `load-buffer` | `loadb` | `wb:` | CLI, SRV, CFG |
| `lock-client` | `lockc` | none | CLI, SRV, CFG |
| `lock-server` | `lock` | none | CLI, SRV, CFG |
| `lock-session` | `locks` | none | CLI, SRV, CFG |
| `move-pane` | `movep` | `dhvs:t:` | CLI, SRV, CFG |
| `move-window` | `movew` | `abrdks:t:` | CLI, SRV, CFG |
| `new-pane` | `newp` | `dPEB:T:c:x:y:X:Y:` | CLI, SRV, CTL |
| `new-session` | `new` | `AdPc:e:F:f:n:s:t:x:y:` | CLI, SRV, CFG |
| `new-window` | `neww` | `dPEc:e:F:n:T:t:` | CLI, SRV, CFG, CTL |
| `next-layout` | `nextl` (not at the CLI) | none | CLI, SRV, CFG, CTL |
| `next-window` | `next` | none | CLI, SRV, CFG, CTL |
| `paste-buffer` | `pasteb` | `dpb:t:` | CLI, SRV, CFG |
| `pipe-pane` | `pipep` | `IOot:` | CLI, SRV, CFG |
| `previous-layout` | `prevl` (not at the CLI) | none | CLI, SRV, CFG |
| `previous-window` | `prev` | none | CLI, SRV, CFG, CTL |
| `refresh-client` | `refresh` | `SlC:t:` at the CLI, plus `A:B:f:` in control mode | CLI, SRV, CFG, CTL |
| `rename-session` | `rename` | none, takes the new name as a positional argument | CLI, SRV, CFG, CTL |
| `rename-window` | `renamew` | none, takes the new name as a positional argument | CLI, SRV, CFG, CTL |
| `resize-pane` | `resizep` | `UDLRZx:y:t:` | CLI, SRV, CFG, CTL |
| `resize-window` | `resizew` | `aADLRUx:y:t:` | CLI, SRV, CFG, CTL |
| `respawn-pane` | `respawnp`, `resp` | `kc:t:` plus `-- <command>` | CLI, SRV, CFG, CTL |
| `respawn-window` | `respawnw` | none | CLI, SRV, CFG |
| `rotate-window` | `rotatew` | `UDt:` | CLI, SRV, CFG, CTL |
| `run-command` | `runcmd` | none, takes the command line as positional arguments | SRV, CTL |
| `run-shell` | `run` | `b` | CLI, SRV, CFG |
| `save-buffer` | `saveb` | `ab:` | CLI, SRV, CFG |
| `select-layout` | `selectl` | `npoEt:` plus a positional layout name | CLI, SRV, CFG, CTL |
| `select-pane` | `selectp` | `UDLRlZmMedP:T:t:` | CLI, SRV, CFG, CTL |
| `select-window` | `selectw` | `lnpt:` | CLI, SRV, CFG, CTL |
| `send-keys` | `send`, `send-key` | `lRXHN:t:` plus `--` | CLI, SRV, CFG, CTL |
| `send-paste` | *(none)* | `t:` plus one base64 positional payload | CLI, SRV |
| `send-prefix` | *(none)* | none | CLI, SRV, CFG |
| `send-text` | *(none)* | none, takes the text as positional arguments | SRV |
| `server-info` | `info` | none | CLI, SRV, CFG, CTL |
| `set-buffer` | `setb` | `wb:` | CLI, SRV, CFG |
| `set-environment` | `setenv` | `grhut:` | CLI, SRV, CFG, CTL |
| `set-hook` | *(none)* | `gua` | CLI, SRV, CFG, CTL |
| `set-option` | `set` | `guaqopswUt:` plus `--` | CLI, SRV, CFG, CTL |
| `set-pane-title` | *(none)* | none, takes the title as positional arguments | SRV |
| `set-window-option` | `setw` | `guaqopswUt:` plus `--` | CLI, SRV, CFG, CTL |
| `show-buffer` | `showb` | `b:` | CLI, SRV, CFG, CTL |
| `show-environment` | `showenv` | `gsht:` | CLI, SRV, CFG, CTL |
| `show-hooks` | *(none)* | `g` | CLI, SRV, CFG, CTL |
| `show-messages` | `showmsgs` | none | CLI, SRV, CFG |
| `show-options` | `show`, `show-option` | `Agpqsvwt:` plus `--` | CLI, SRV, CFG, CTL |
| `show-prompt-history` | `showphist` | none | SRV |
| `show-window-options` | `showw`, `show-window-option` | `Agpqsvwt:` plus `--` | CLI, SRV, CFG, CTL |
| `source-file` | `source` | `qnv` | CLI, SRV, CFG, CTL |
| `split-window` | `splitw`, `split-pane`, `splitp` | `hvdPZc:e:F:l:p:T:t:` | CLI, SRV, CFG, CTL |
| `start-server` | `start`, `warmup` | none | CLI, SRV, CFG |
| `suspend-client` | `suspendc` | none | CLI, SRV, CFG |
| `swap-pane` | `swapp` | `UDds:t:` | CLI, SRV, CFG, CTL |
| `swap-window` | `swapw` | `ds:t:` | CLI, SRV, CFG |
| `switch-client` | `switchc` | `lnprc:t:` plus `T:` on SRV | CLI, SRV, CFG |
| `toggle-sync` | *(none)* | none | SRV, CFG |
| `unbind-key` | `unbind` | `anT:t:` | CLI, SRV, CFG, CTL |
| `unlink-window` | `unlinkw` | none, acts on the active window | CLI, SRV, CFG, CTL |
| `wait-for` | `wait` | `LSU` | CLI, SRV, CFG |
| `zoom-pane` | *(none)* | none | CLI, SRV, CFG, CTL |

104 command names. `warmup`, `resp`, `a` and `at` are CLI only aliases. `show-option` and `show-window-option` are the singular spellings tmux also accepts (#586). `nextl` and `prevl` are recognised on the server, config and control layers but not by the CLI front end, so use the full name when typing in a shell.

### Commands deliberately not listed

The server also matches a set of internal wire commands that the client sends on your behalf and that are not meant to be typed: the overlay and chooser wire (`popup-input`, `menu-select`, `customize-navigate`, `confirm-respond` and friends), the cross session pane transfer wire (`pane-forward-extract`, `pane-forward-inject` and friends), the layout drag wire (`split-sizes`, `split-resize-done`), `claim-session`, `session-info`, `client-attach`, `client-detach`, `server-access`, `window-layout`, `window-dump`, and the copy mode key wire (`copy-enter`, `copy-move`, `copy-anchor`, `copy-yank`, `rectangle-toggle`, `copy-mode-page-up`, `delete-buffer-at`, `paste-buffer-at`).

Five mouse wire commands are an exception and are genuinely usable for scripting: `mouse-down`, `mouse-drag`, `mouse-up`, `mouse-down-right` and `mouse-up-right` are accepted at the CLI as well as on the socket.

`select-window-index` appears in the default prefix bindings but is a client side pseudo command. `psmux select-window-index` fails with "unknown command" and it cannot be used from a config line, a hook or `run-shell`.

## Flag details by command

### Session commands

**new-session** (`new`)
- Boolean: `-A` (attach if the session already exists), `-d` (detached), `-P` (print the new session info)
- Value: `-s` (session name), `-n` (first window name), `-c` (start directory), `-x` (initial width), `-y` (initial height), `-e` (environment `KEY=VALUE`, repeatable), `-F` (format), `-f` (value consumed and discarded), `-t` (session group target)
- `--` ends option parsing. Everything after it is the raw command to run instead of a shell.
- Accepted but ignored, for tmux script compatibility: `-D`, `-E`, `-X`
- Not accepted: `-g`. Session groups are set with `set -g session-group <name>`, not with a `new-session` flag.
- Combined short flags are expanded, so `-As main` means `-A -s main` and `-dP` means `-d -P`.

**attach-session** (`attach`, `a`, `at`)
- Value: `-t` (target session)
- A bare positional argument is also accepted as the session name, so `psmux attach work` works.
- Not accepted: `-d`, `-D`, `-E`, `-r`, `-c`, `-f`, `-x`, `-y`

**has-session** (`has`)
- Value: `-t` (target session). A leading `=` is stripped, matching tmux exact match semantics.

**kill-session** (`kill-ses`)
- Value: `-t` (target session)
- Not accepted: `-a`, `-C`

**rename-session** (`rename`)
- No flags. The first positional argument is the new name.

**list-sessions** (`ls`)
- Value: `-F` (format, also accepted glued as `-F<format>`), `-f` (filter)
- Any other `-x` token is rejected with a usage error and a non-zero exit, so scripts see the failure.

**switch-client** (`switchc`)
- Boolean: `-l` (last), `-n` (next), `-p` (previous), `-r` (toggle read only)
- Value: `-c` (client), `-t` (target, a full `session:window.pane` target is honored)
- Server layer only: `-T` (switch the active key table)
- Not accepted: `-E`, `-F`, `-O`, `-Z`

**detach-client** (`detach`)
- Boolean: `-a` (detach all other clients), `-P` (kill the parent process)
- Value: `-t` (target client), `-s` (target session), `-E` (command that replaces the shell on detach)

**suspend-client** (`suspendc`)
- No flags. This is a no-op on Windows, which has no SIGTSTP.

**lock-server** (`lock`), **lock-session** (`locks`), **lock-client** (`lockc`)
- No flags. All three are no-ops on Windows, which has no terminal locking concept.

**kill-server**
- No flags.
- Note: bare `kill-server` stops every socket and session (the default plus all `-L` namespaces), unlike tmux which only kills the current socket. Use `-L <name> kill-server` to scope it to one namespace. See [compatibility.md](compatibility.md#kill-server-with-multiple-sockets).

**start-server** (`start`, `warmup`)
- No flags. Pre-spawns a warm server so the next `new-session` is instant.

**server-info** (`info`)
- No flags.

**list-clients** (`lsc`)
- Value: `-F` (format), honored on the server and control mode layers. The CLI front end forwards a bare `list-clients`, so `psmux list-clients -F ...` silently drops the format.

**choose-client**
- No flags. psmux has a single client model, so this returns the current client info.

**refresh-client** (`refresh`)
- Boolean: `-S` (status line only), `-l` (request the clipboard)
- Value: `-C` (client size), `-t` (target client)
- Control mode adds: `-A '%<pane>:continue'` (resume a paused pane), `-B 'name:target:format'` (add a subscription, `-B 'name:'` removes it), `-f pause-after=N` and `-f no-pause` (flow control)
- The control mode `-C` argument is comma separated (`-C 120,30`), which is the form iTerm2 sends.
- Not accepted: `-c`, `-D`, `-L`, `-R`, `-U`, `-r`, `-F`

### Window commands

**new-window** (`neww`)
- Boolean: `-d` (do not switch to the new window), `-P` (print the new window info), `-E` (empty pane, no shell)
- Value: `-n` (window name), `-c` (start directory), `-T` (pane title), `-e` (environment `KEY=VALUE`), `-F` (format, also accepted glued as `-F<format>`), `-t` (target, value consumed)
- Accepted but ignored: `-a`, `-D`, `-k`, `-S`
- Not accepted: `-b`
- `--` ends option parsing. What follows is the command for the new pane, with tmux's two shapes (#582): a **single** token after `--` is a shell command string and runs through the pane shell (so `-- "cargo build 2>&1 | tee log.txt"` keeps its pipes, redirections and variables), while **two or more** tokens are an argv that psmux executes directly with no shell in between, exactly as tmux's `execvp` does. `-- node app.js --port 8080` therefore starts `node` itself, not `pwsh -c "node app.js --port 8080"`, so a shell profile cannot slow it down or alter its arguments, and `#{pane_current_command}` reports the program you named. If you need shell features with several tokens, wrap the whole thing in one quoted string. The same rule applies to `split-window` and `new-session`.

**kill-window** (`killw`)
- Boolean: `-a` (kill all windows but the current one)
- Value: `-t` (target window)

**unlink-window** (`unlinkw`)
- No flags. Acts on the active window.
- Not accepted: `-k`

**rename-window** (`renamew`)
- No flags. The first positional argument is the new name.

**select-window** (`selectw`)
- Boolean: `-l` (last), `-n` (next), `-p` (previous)
- Value: `-t` (target window, `session:@id` form is honored)
- Not accepted: `-T`

**next-window** (`next`), **previous-window** (`prev`), **last-window** (`last`)
- No flags.

**move-window** (`movew`)
- Boolean: `-a` (after), `-b` (before), `-r` (renumber), `-d` (do not switch), `-k` (kill the target if it exists)
- Value: `-s` (source window, honoured since #602; before that the active window always moved), `-t` (destination window)
- Both values are resolved on the server by the one tmux parity window resolver (#602), in tmux's order: `@id`, `+N` / `-N`, the symbols `^` `!` `$` `+` `-` and their `{start}` `{last}` `{end}` `{next}` `{previous}` spellings, a window index, then an exact window name. A leading `session:` is dropped because routing already chose the server. For `-t`, `+N` / `-N` are arithmetic on the current window index and a number that names no window is a free destination slot, as in tmux. An index that is already in use fails with `index in use` at exit 1 unless `-k` is given.
- An attached client sees the new window list immediately (#601).

**link-window** (`linkw`)
- Value: `-s` (source window index), `-t` (destination window index)
- Not accepted: `-a`, `-b`, `-d`, `-k`

**swap-window** (`swapw`)
- Boolean: `-d` (keep the current window current: the active window follows the swap to its new index)
- Value: `-s` (source window, defaults to the current window), `-t` (destination window, required)
- Targets go through the same resolver as `move-window` (#602). Here `+N` / `-N` step through the session's window list and wrap, and a number that names no window is `can't find window: N` at exit 1. The two windows exchange index numbers. Without `-d` the active window **number** is left alone, so the window that moved into it becomes the active one, as in tmux; with `-d` the active window keeps its identity and its new number. Either way an attached client sees the change at once (#601).

**rotate-window** (`rotatew`)
- Boolean: `-U` (rotate up), `-D` (rotate down)
- Value: `-t` (target window)
- Not accepted: `-Z`

**resize-window** (`resizew`)
- Boolean: `-A` (largest client), `-a` (smallest client), `-L`, `-R`, `-U`, `-D`
- Value: `-x` (width), `-y` (height), `-t` (target window)
- Positional: optional positive adjustment for `-L`, `-R`, `-U`, or `-D` (default `1`)
- The target window enters manual sizing; later client viewport changes do not overwrite it.

**find-window** (`findw`)
- The first positional argument is the search pattern. That is the only thing that has an effect.
- Accepted but ignored: `-C`, `-N`, `-T`, `-i`, `-r`, `-Z`, `-t`

**respawn-window** (`respawnw`)
- No flags. Respawns the active pane of the window.

**list-windows** (`lsw`)
- Boolean: `-a` (all sessions), `-J` (JSON output, a psmux extension)
- Value: `-F` (format), `-t` (target session)

### Pane commands

**split-window** (`splitw`, `split-pane`, `splitp`)
- Boolean: `-h` (horizontal), `-v` (vertical), `-d` (do not switch), `-P` (print the new pane info)
- Boolean, since PR #572: `-Z` (zoom the new pane once the split is made)
- Value: `-p` (percentage size), `-l` (size in cells or a percentage), `-c` (start directory), `-T` (pane title), `-F` (format), `-e` (environment `KEY=VALUE`), `-t` (target, value consumed)
- `--` ends option parsing and follows the same single string versus argv rule as `new-window` (#582).
- Accepted but ignored: `-b`, `-f`, `-I`

**new-pane** (`newp`)
A psmux extension that creates a pane floating above the tiled layout.
- Boolean: `-d` (detached), `-P` (print the new pane id), `-E` (empty, no shell)
- Value: `-B` (border style), `-T` (title), `-c` (start directory), `-x` (width), `-y` (height), `-X` (column position), `-Y` (row position)

**select-pane** (`selectp`)
- Boolean: `-U`, `-D`, `-L`, `-R` (directional), `-l` (last pane), `-Z` (keep the zoom while navigating), `-m` (mark), `-M` (unmark), `-e` (enable input), `-d` (disable input)
- Value: `-T` (set and lock the pane title), `-P` (pane style), `-t` (target pane)
- `-T` and `-P` without a direction flag are attribute only (#592): `select-pane -t %7 -T logs` titles pane `%7` and leaves the active window and pane exactly where they were, as tmux does. Combine with `-U`/`-D`/`-L`/`-R`/`-l` if you also want to move.
- `-t` also accepts positional targets: `{top}`, `{bottom}`, `{left}`, `{right}`, `{top-left}`, `{top-right}`, `{bottom-left}`, `{bottom-right}`
- Not accepted: `-g`

**last-pane** (`lastp`)
- No flags.

**kill-pane** (`killp`)
- Value: `-t` (target pane, via the global `-t` handler)
- Not accepted: `-a`

**resize-pane** (`resizep`)
- Boolean: `-U`, `-D`, `-L`, `-R` (directional), `-Z` (toggle zoom)
- Value: `-x` (absolute width in cells), `-y` (absolute height in cells), `-t` (target pane)
- Not accepted: `-M`, `-T`

**zoom-pane**
- No flags. A psmux extension with no tmux equivalent. `resize-pane -Z` does the same thing.

**swap-pane** (`swapp`)
- Boolean: `-U` (swap up), `-D` (swap down), `-d` (do not move the active pane)
- Value: `-s` (source pane), `-t` (destination pane)
- Not accepted: `-Z`

**join-pane** (`joinp`) and **move-pane** (`movep`)
- Boolean: `-h` (horizontal), `-v` (vertical, the default), `-d` (do not switch)
- Value: `-s` (source pane), `-t` (destination pane)
- A `-s <other-session>:...` source moves a live pane between independent servers.
- Not accepted: `-b`, `-f`, `-p`, `-l`

**break-pane** (`breakp`)
- Boolean: `-d` (do not switch to the new window)
- Value: `-t` (target)
- Not accepted: `-a`, `-b`, `-P`, `-F`, `-n`, `-s`

**respawn-pane** (`respawnp`, `resp`)
- Boolean: `-k` (kill the existing process first)
- Value: `-c` (start directory), `-t` (target pane)
- `-- <command>` is honored and replaces the pane command.
- Not accepted: `-e`
- Without `-k`, a pane whose process is still running is refused, like tmux:
  `respawn pane failed: pane <session>:<window>.<pane> still active` on stderr
  at exit 1. The session and the pane are left exactly as they were.

**capture-pane** (`capturep`)
- Boolean: `-p` (print to stdout), `-e` (include escape sequences), `-J` (join wrapped lines)
- Value: `-S` (start line), `-E` (end line), `-b` (buffer name), `-t` (target pane)
- Not accepted: `-a`, `-C`, `-M`, `-N`, `-P`, `-q`, `-T`

**clear-history** (`clearhist`)
- Boolean: `-H` (also clear the alternate screen)
- Value: `-t` (target pane)

**list-panes** (`lsp`)
- Boolean: `-a` (all sessions), `-s` (session scope)
- Value: `-F` (format), `-t` (target)

**display-panes** (`displayp`)
- No flags. Shows the pane number overlay, then a digit key selects a pane.

**pipe-pane** (`pipep`)
- Boolean: `-I` (pipe input), `-O` (pipe output), `-o` (toggle)
- Value: `-t` (target pane)

**set-pane-title**
- No flags. A psmux extension. The positional arguments are joined into the title.

**toggle-sync**
- No flags. A psmux extension that toggles `synchronize-panes` for the active window.

### Layout commands

**select-layout** (`selectl`)
- Boolean: `-n` (next layout), `-p` (previous layout)
- Value: `-t` (target, value consumed)
- Accepted but ignored: `-o`, `-E`
- The first positional argument is a layout name: `even-horizontal` (alias `even-h`), `even-vertical`, `main-horizontal` (alias `main-h`), `main-vertical` (alias `main-v`), `tiled`.

**next-layout**, **previous-layout**
- No flags. `nextl` and `prevl` work on the server, config and control layers but not at the CLI.

### Copy and paste commands

**copy-mode**
- Boolean: `-u` (scroll up one page on entry), `-d` (scroll down), `-e` (exit at the bottom), `-H` (hide the position indicator), `-q` (quit copy mode)
- Value: `-t` (target pane)
- Not accepted: `-M`, `-S`, `-s`
- In control mode `copy-mode` is a success returning no-op, because iTerm2 implements copy mode locally on captured content.

**paste-buffer** (`pasteb`)
- Boolean: `-d` (delete the buffer after pasting), `-p` (use bracketed paste)
- Value: `-b` (buffer name), `-t` (target pane)
- Not accepted: `-r`, `-s`
- When the buffer stack is empty, psmux falls back to the Windows clipboard.

**set-buffer** (`setb`)
- Boolean: `-w` (also write to the Windows clipboard)
- Value: `-b` (buffer name)
- Not accepted: `-a`, `-n`, `-t`

**delete-buffer** (`deleteb`) and **show-buffer** (`showb`)
- Value: `-b` (buffer name)

**save-buffer** (`saveb`)
- Boolean: `-a` (append)
- Value: `-b` (buffer name). The first positional argument is the path.

**load-buffer** (`loadb`)
- Boolean: `-w` (propagate to the clipboard)
- Value: `-b` (buffer name). The first positional argument is the path.

**list-buffers** (`lsb`)
- Value: `-F` (format), `-t` (value consumed and discarded)

**choose-buffer** (`chooseb`)
- No flags. Opens the interactive buffer chooser.

### Key binding commands

**bind-key** (`bind`)
- Boolean: `-n` (shorthand for `-T root`), `-r` (repeatable)
- Value: `-T` (key table)
- Not accepted: `-N` (note)

**unbind-key** (`unbind`)
- Boolean: `-a` (unbind everything, also matched inside a combined token), `-n` (root table)
- Value: `-T` (key table), `-t` (target, value consumed)
- Not accepted: `-q`

**list-keys** (`lsk`)
- Value: `-T` (key table), `-t` (value consumed and discarded)
- Not accepted: `-1`, `-a`, `-N`, `-P`

**send-keys** (`send`, `send-key`)
- Boolean: `-l` (literal), `-R` (reset the terminal state), `-X` (run a copy mode command), `-H` (every operand is one hexadecimal byte value, written to the pane verbatim, PR #524)
- Value: `-N` (repeat count, value consumed), `-t` (target pane)
- `--` ends option parsing (#562). Every token after it is a key to deliver even if it begins with `-`, so `send-keys -l -- -rf` types `-rf`. An empty argument sends nothing, as in tmux.
- Not accepted: `-c`, `-F`, `-K`, `-M`
- Named key tokens accepted as arguments: `ENTER`, `TAB`, `BTAB` / `BACKTAB`, `ESCAPE` / `ESC`, `SPACE`, `BSPACE` / `BACKSPACE`, `UP`, `DOWN`, `LEFT`, `RIGHT`, `HOME`, `END`, `PAGEUP` / `PPAGE`, `PAGEDOWN` / `NPAGE`, `DELETE` / `DC`, `INSERT` / `IC`

**send-prefix**
- No flags.
- Not accepted: `-2`, `-t`

**send-text**, **send-paste**
- psmux extensions. `send-text` takes the raw text as positional arguments and does no key name parsing. `send-paste` wraps the payload in a bracketed paste sequence and also accepts `-t` at the CLI.
- `send-paste` takes exactly one positional argument, the text **base64 encoded** (PR #584). Base64 is the wire boundary: the payload can carry newlines, quotes and `;` without ever being read as a second command, and it is not subject to the 8191 character command line limit that `-EncodedCommand` wrappers hit. Anything that is not valid base64, or a second positional, fails with `send-paste requires exactly one UTF-8 base64 payload` at exit 1. PowerShell: `psmux send-paste -t %1 ([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($text)))`.

### Configuration commands

**set-option** (`set`) and **set-window-option** (`setw`)
- Boolean: `-g` (global, accepted, psmux has one scope so this is effectively a selector no-op), `-s` (server scope, #618), `-u` (unset), `-U` (unset alias of `-u`, tmux parity, #553), `-a` (append to the current value), `-q` (quiet), `-o` (only set if currently unset), `-w` (window scope), `-p` (pane scope, #580). None of these consume the next argument.
- `-s` is the tmux 3.2+ server scope flag, the documented way to write `default-terminal`, `extended-keys` and `extended-keys-format`. psmux runs one server per session and keeps a single option store, so `-s` resolves to the same store as `-g`: the write lands where a caller using tmux syntax expects to find it, but this is **not** genuine cross-session server-option storage. tmux's own `set-window-option` table has no `-s`; psmux shares one flag guard across all four spellings and accepts it on `setw` too.
- Value: `-t` (target, value consumed)
- Flags parse only **before** the option name, as tmux's getopt does (#583). Once the option name has been seen, everything that follows is the value, dashes included: `set @k -u` stores the literal string `-u`, where it used to consume `-u` as the unset flag and delete `@k` at exit 0. `--` ends option parsing outright, so `set -- @k -g` also stores `-g`.
- Flags are also matched inside combined tokens such as `-ga`, `-gu` and `-gq`.
- `-u` **restores the option's table default** on every route, and it clears the record that the user ever set it, so a following `-o` sees an unset option and applies (#619). `set -g escape-time 5` then `set -gu escape-time` reads 500 again, and `set -gu status-left` reads `[#S] `. All three unset routes (config file, server request loop, plugin drain loop) share one restore driven by the option catalog, which `tests-rs/test_option_default_parity.rs` pins to a freshly started server. Before that the server carried a hand written table of about thirty options and silently did nothing for the rest (`set -su default-terminal` left `screen-256color` in place), while the config-file parser wrote an empty value instead of the default, so the CLI and a `.tmux.conf` disagreed about what an unset means. Measured on the pre-fix build, 37 of 42 probed options did not come back to the value a fresh server reports.
- On a `@name` option `-u` removes the key outright rather than restoring anything, which is what tmux does: a user option carries no table entry, so `options_remove_or_default` takes its `options_remove` branch.
- `-u` and `-o` in the same command is an unset, never a set: tmux skips the `-o` guard whenever `-u` is present, so `set -guo status-left NEVER` unsets `status-left` and stores nothing.
- `-o` on an option that **is** set fails: `already set: <name>` on stderr at exit 1, matching tmux's `cmdq_error` (#619). `-q` restores tmux's silent exit 0. The value is left untouched either way. Through a config file the refusal is recorded in `~/.psmux/config-warnings.log` as `<file>:<line>: already set: <name>`, the same shape as `unknown option`, and the in-TUI command prompt shows it as a status message. Before this, every route swallowed the refusal at exit 0 with empty output, so a caller seeding a default could not tell "I set it" from "the user already had it", which is `-o`'s entire purpose.
- A `@name` argument is never treated as a flag.
- Not accepted: `-F`. Since #553 any flag outside the accepted set is rejected with `unknown flag -X` at exit 1 instead of being silently dropped while the write lands.

**show-options** (`show`, `show-option`) and **show-window-options** (`showw`, `show-window-option`)
- Boolean: `-A` (include inherited), `-g` (global), `-s` (server scope), `-w` (window scope), `-p` (pane scope, #580), `-v` (value only), `-q` (quiet)
- Value: `-t` (window selector)
- A bare `show-options -s` lists the server-scope options only, as tmux does (#618). Before that fix the flag was parsed and then ignored, so it printed the whole store, session options included. A named query such as `show-options -s escape-time` ignores `-s`, which is what tmux does for a table option too.
- `-v` prints the value and nothing else in every scope, including `-p` (#647). A named `-p` query answers for that one option; before this fix it ignored both the name and `-v` and printed the whole pane store as `name value` pairs, so a script comparing stdout with `on` never matched. An option that is not set in the pane's own store prints nothing at exit 0, matching tmux; `-A` falls back to the inherited value and marks it with `*` when the name is printed. A bare `show-options -p` still lists the whole store (#580).
- Combined tokens are handled the same way as for `set-option`.
- The singular aliases `show-option` and `show-window-option` are accepted on every layer (#586), so a `.tmux.conf` or plugin that uses tmux's singular spelling works unchanged.
- Not accepted: `-H`. `-g` is absorbed rather than acted on, psmux having one store. Since #553 flags outside `-A -g -p -q -s -v -w` (plus `-t <target>`) are rejected with `unknown flag -X` at exit 1.

**set-hook** and **show-hooks**
- Boolean: `-u` (unset, also as `-gu` or `-ug`), `-a` (append, also as `-ga` or `-ag`), `-g` (global, accepted and absorbed)
- Not accepted: `-p`, `-R`, `-w`, `-t`
- `set-hook` accepts **any** hook name with no validation, so a typo silently never fires.

**set-environment** (`setenv`)
- Boolean: `-g` (global), `-r` (remove from the environment), `-u` (unset), `-h` (hidden)
- Value: `-t` (target session)
- Not accepted: `-F`

**show-environment** (`showenv`)
- Boolean: `-g` (global), `-s` (shell format), `-h` (hidden)
- Value: `-t` (target session)

**source-file** (`source`)
- Boolean: `-q` (quiet), `-n` (parse only, do not execute), `-v` (verbose)
- Not accepted: `-F`, `-t`

**list-commands** (`lscm`)
- No flags.
- Not accepted: `-F`

### Display and overlay commands

**display-message** (`display`)
- Boolean: `-p` (print to stdout), `-F` (format mode)
- Value: `-d` (display duration in ms), `-I` (value consumed), `-t` (target)
- `--` ends option parsing.
- Not accepted: `-a`, `-C`, `-c`, `-l`, `-N`, `-v`

**display-menu** (`menu`)
- Value: `-x` (column), `-y` (row), `-T` (title)
- Menu items follow as `<label> <key> <command>` triples.

**display-popup** (`popup`)
- Boolean: `-E` (close when the command exits, which is already the default), `-K` (keep the popup open after the command exits)
- Value: `-w` (width, cells or a percentage), `-h` (height), `-d` or `-c` (start directory)

**confirm-before** (`confirm`)
- Value: `-p` (prompt text)
- Everything that is not the prompt and does not begin with `-` becomes the command to confirm.
- Not accepted: `-b`, `-c`, `-y`, `-t`

**command-prompt**
- Boolean: `-N` (numeric input only), `-W` (word input only)
- Value: `-I` (initial value), `-p` (prompt list), `-T` (prompt type), `-t` (target)
- Not accepted: `-1`, `-b`, `-e`, `-F`, `-i`, `-k`, `-l`
- `-T` is parsed and its value consumed, but the prompt type itself is ignored: psmux has no target completion. `-t` is consumed and ignored the same way. The flags end at the first argument that is not a flag, so a template given unquoted (`command-prompt move-window -t %%`) keeps its own flags.

**choose-tree**, **choose-window**, **choose-session**, **choose-client**, **choose-buffer**, **customize-mode**, **clock-mode**
- No flags. These open client side overlays. While an overlay is open its keys are handled before any key table, so a bound key does not reach the tables until the overlay closes.

**show-messages** (`showmsgs`)
- No flags.

**clear-prompt-history** (`clearphist`) and **show-prompt-history** (`showphist`)
- No flags. Server layer only, so reach them from a key binding or a raw socket connection rather than from `psmux <command>`.

### Shell and flow control commands

**run-shell** (`run`)
- Boolean: `-b` (background)
- Everything else, including tokens that look like flags, is treated as part of the shell command. `-C`, `-d`, `-s`, `-c` and `-t` are **not** parsed, so passing them sends them straight to the shell.

**if-shell** (`if`)
- Boolean: `-b` (background), `-F` (evaluate the condition as a format string instead of running a shell)
- Value: `-t` (value consumed and discarded)
- Positional arguments: the condition, the command to run when it succeeds, and optionally the command to run when it fails.
- The config file parser also accepts the glued combined forms `-bF` and `-Fb`.

**wait-for** (`wait`)
- Boolean: `-L` (lock), `-S` (signal), `-U` (unlock)
- The first positional argument is the channel name.

**run-command** (`runcmd`)
- No flags. Runs the given command line through the config file command layer and returns its output, with a 15 second timeout. Available on the server and control mode layers.

**dump-state** (`dump`), **dump-layout**, **list-tree**
- No flags. `dump-state` returns the whole live server state as JSON and is available at the CLI and in control mode. `dump-layout` and `list-tree` are server layer only.

## Global flags, before the command name

These are parsed before psmux looks at the command name.

| Flag | Meaning |
|---|---|
| `-L <name>` | Socket namespace. Sessions in a namespace are stored as `<name>__<session>`. |
| `-f <file>` | Config file to load, exported as `PSMUX_CONFIG_FILE`. |
| `-C` | Control mode with command echo. |
| `-CC` | Control mode without command echo. |
| `-t <target>` | Target `session`, `session:window` or `session:window.pane` for the command that follows. |
| `-S <path>` | Socket path, accepted and its value consumed. |
| `-h`, `--help` | Usage. |
| `-V`, `-v`, `--version` | Version. |

Glued short flags of the form `-x=VALUE` are normalized to `-x VALUE` before parsing. tmux's attached spellings are accepted too: `-Lwork` is `-L work`, and a command level `-tname` is `-t name`.

When a command carries no `-t` at all, psmux picks the session the way tmux's `cmd_find_best_session` does (#603): a `$TMUX` inherited from the pane you are typing in wins if it belongs to the requested namespace, otherwise the session with the newest activity does, where activity is stamped on attach, on `switch-client`, and on every key an attached client sends. The `last_session` file is only a tie break. See the target syntax section of [scripting.md](scripting.md) for the full rule.

## User defined command aliases

`set -g command-alias '<alias>=<expansion>'` defines your own command names. There is one asymmetry worth knowing: aliases resolve from key bindings and config lines, and are consulted by the config file unknown command warning, but the **CLI front end does not resolve them**. `psmux <alias>` fails with "unknown command" while the same alias in a key binding or a config line works.
