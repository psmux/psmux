# Diagnostics

psmux runs its server detached, with no visible stderr. When something goes wrong there is
nowhere for an error message to land on screen, so psmux writes what it knows to files instead.
This page lists every diagnostic file psmux can produce, the environment variables that turn the
optional ones on, and what to attach when you file a bug report.

Everything on this page is read only observation, with three exceptions called out where they
appear: `PSMUX_FAKE_WIN_BUILD` moves psmux onto a different branch on purpose,
`PSMUX_FAKE_INJECT_FAIL` makes a Windows call fail on purpose, and `PSMUX_PASTE_INJECT` moves a
paste onto the other delivery channel on purpose, all so that a path you cannot otherwise reach on
your machine can be reproduced on it. The last of the three is also a real escape hatch: it is the
supported way to correct psmux's guess about your conhost.

## Debug Logging

psmux has fourteen independent debug loggers. All of them are **off by default** and cost nothing
when disabled (one atomic load per call site). Each writes a timestamped line per event, and the
loggers under `~/.psmux/` are capped at a fixed number of entries so an enabled log can never fill
your disk.

| Variable | Log file | What it records |
|---|---|---|
| `PSMUX_CLIENT_DEBUG=1` | `~/.psmux/client_debug.log` | Client frame receive, JSON parse, draw lifecycle, status bar rendering |
| `PSMUX_STYLE_DEBUG=1` | `~/.psmux/style_debug.log` | Style and theme parsing, inline `#[...]` directives |
| `PSMUX_INPUT_DEBUG=1` | `~/.psmux/input_debug.log` | Every input event plus the console mode in effect when it arrived |
| `PSMUX_SERVER_DEBUG=1` | `~/.psmux/server_debug.log` | Server side request tracing and session switching |
| `PSMUX_SESSION_DEBUG=1` | `~/.psmux/session_debug.log` | Session registry scans and stale port cleanup |
| `PSMUX_MOUSE_DEBUG=1` | `~/.psmux/mouse_debug.log` | Mouse injection, screen to pane coordinate mapping, the wheel gate's decision per notch (which mouse protocol the pane holds and who enabled it), every reply psmux injects into a pane, and a full console state report whenever an injection cannot attach to a pane |
| `PSMUX_SSH_DEBUG=1` | `~/.psmux/ssh_input.log` | SSH escape sequence decoding into Win32 input records |
| `PSMUX_LATENCY_LOG=1` | `~/.psmux/latency.log` | Keypress to render latency, measured client side |
| `PSMUX_PANE_RAW=1` | `~/.psmux/pane_raw.bin` | The raw byte stream a pane's child wrote, before psmux's VT parser touched it. The file to read when a colour, a mode switch or a mouse DECSET seems to be missing: it shows what the program (and conhost, which echoes console mode changes into the same stream as `ESC [ ? 1003 ; 1006 h` / `l`) actually emitted |
| `PSMUX_AUTORENAME_DEBUG=1` | `~/.psmux/autorename.log` | The process tree walk behind `automatic-rename` and `#{pane_current_command}` |
| `PSMUX_ROUTE_DEBUG=1` | stderr of the CLI process | Which session a bare `psmux <command>` (no `-t`, no `$TMUX`) was routed to and why |
| `PSMUX_POPUP_DEBUG=1` | `%TEMP%\psmux_popup_debug.log` | Popup creation, resize, and teardown |
| `PSMUX_WARM_DEBUG=1` | `%TEMP%\psmux_warm_debug.log` | Warm server and warm pane lifecycle |
| `PSMUX_AUTH_DEBUG=1` | `%TEMP%\psmux_auth_debug.log` | The `.port` and `.key` handshake between client and server |

Three of these deliberately live in `%TEMP%` rather than `~/.psmux/`. Popup, warm pool and auth
diagnostics are all about races between concurrent psmux processes, so those three logs are
append-only and per-machine rather than truncate-on-start, and no process can clobber another's
evidence. `PSMUX_AUTH_DEBUG` additionally echoes the client side of the handshake to stderr, which
is visible when you run the client in a normal console.

Two more details worth knowing:

- `PSMUX_SESSION_DEBUG` appends rather than truncates. Registry cleanup runs in every short-lived
  `psmux` CLI process, so truncating on open would erase the log before you could read it.
- `PSMUX_INPUT_DEBUG` appends too, for the same reason. Two processes write it: the attached client
  (which decides whether a burst of characters is a paste) and the server (which decides which
  channel carries that paste into the pane). Each opens the file on its first line, so while it
  truncated, whichever wrote second erased the other's evidence, and the first one's next write
  landed at its old offset and left a hole of NUL bytes behind it. Each process writes a banner
  naming itself when it opens the file, so the two halves stay attributable, and the file holds
  more than one run: delete it before a reproduction if you want only that run in it.
- `PSMUX_LATENCY_LOG=1` also enables a server side companion at
  `%USERPROFILE%\psmux_server_latency.log` holding dump-state build times. Read the two together
  to tell a slow server from a slow client.

### Enabling a Log

Set the variable in the shell you launch psmux from, then reproduce the problem:

```powershell
$env:PSMUX_INPUT_DEBUG = "1"     # turn the input logger on for this shell only
psmux kill-server                # make sure the next launch is a fresh server
psmux new-session -s repro       # reproduce the problem here
Get-Content "$env:USERPROFILE\.psmux\input_debug.log" -Tail 40
```

The variable must be set before the process you want to trace starts. A logger that traces the
server (`PSMUX_SERVER_DEBUG`, `PSMUX_SESSION_DEBUG`, `PSMUX_WARM_DEBUG`) only takes effect on a
server started after the variable was set, which is why the example kills the old server first.
Warm servers are pre-spawned, so add `$env:PSMUX_NO_WARM = "1"` when you need the very first server
process to carry your setting. See [warm-sessions.md](warm-sessions.md).

To turn a logger back off, clear the variable and restart:

```powershell
Remove-Item Env:\PSMUX_INPUT_DEBUG
psmux kill-server
```

### The Windows Build Seam

| Variable | What it does |
|---|---|
| `PSMUX_FAKE_WIN_BUILD=19045` | Makes psmux report that build number instead of the real one |

Several mouse code paths branch on the Windows build number, because conhost handles VT mouse
input differently below build 22523 (see [mouse-ssh.md](mouse-ssh.md) and
[faq.md](faq.md)). Those branches are unreachable on a modern host, so this variable exists to
exercise them: set it to `19045` to see what a Windows 10 pane does, or to `26100` to see the
modern path from an older machine.

This is diagnostic only. It changes nothing about the machine and nothing about the console, it
only changes which branch psmux picks, so a build set here that does not match reality gives you
a mouse path your conhost was not built for. Set it while reproducing a report, then clear it. It
must be set on the **server** process to affect a pane, so kill the server first and add
`$env:PSMUX_NO_WARM = "1"` so that a warm server spawned earlier, without your setting, does not
answer instead.

### Reading an Injection Failure

Some things psmux sends a pane cannot go down the pane's input pipe, because the ConPTY host in
front of the pane consumes them: the XTVERSION and OSC colour replies psmux answers a program's
own questions with, a Win32 mouse record, a Ctrl+C. Those are written straight into the pane's
console input buffer instead, and to do that the server has to briefly leave its own console and
join the pane's, with `FreeConsole` followed by `AttachConsole`.

When that attach is refused, nothing psmux injects reaches the pane, and the symptom is broad and
confusing: a program's terminal probes go unanswered, the wheel does nothing, colours come out
wrong. `PSMUX_MOUSE_DEBUG=1` writes one line per attempt, and on a refusal a full report:

```
[platform] send_vt_response: AttachConsole(18572) FAILED err=5 | ERROR_ACCESS_DENIED: the caller is
  still attached to a console, or the target console refuses this caller (integrity or session
  mismatch) | server pid=37420 elevated=false integrity=0x2000 | console before FreeConsole:
  window=0x1080BAA clients=[37420] | FreeConsole ok=true err=6 | console after: window=0x0
  clients=NONE(err=6) | target alive=true image=pwsh.exe parent=37420 (psmux.exe) elevated=false
  integrity=0x2000 | os build=19045 | retry: FreeConsole ok=true err=6 AttachConsole err=5 (...)
```

Read it in this order:

- **`console after`** is the field that splits the two meanings of `err=5`. `window=0x0` with
  `clients=NONE` means the server really did let go of its console, so a refusal is the pane's
  console turning the server away. Anything else means the detach did not take, and the server was
  still attached when it asked.
- **`FreeConsole ok=`** says whether the detach itself reported success. `err=6` alongside
  `ok=true` is normal: `FreeConsole` leaves a stale error code behind when it succeeds.
- **`integrity=`** on the two processes should match. `0x2000` is a normal session, `0x3000` is
  elevated. A server and a pane child at different levels is a refusal that no retry can fix.
- **`target alive=false`** means the pane's program had already exited, which is benign.
- **`retry:`** appears only for `err=5`, and shows the second attempt psmux makes behind another
  detach. If the retry succeeds there is no report at all, only a one line note saying so.

A healthy pane logs successes instead, one per reply:

```
[platform] send_vt_response: pid=36960 text_len=16 records=16 written=16 ok=true
```

And a reply that could not be delivered is named rather than dropped in silence:

```
[platform] XTVERSION reply LOST: 16 bytes for pid=Some(40776) could not be injected ...
```

### The Injection Failure Seam

| Variable | What it does |
|---|---|
| `PSMUX_FAKE_INJECT_FAIL=1` | Points every console attach at a process id that does not exist, so injection fails for real |

The report above is unreachable on a host where injection works, which is most of them. This
variable makes it reachable: the detach still happens, the attach fails for real, the failure
report is written and every caller takes its real failure branch, so you see exactly what a pane
looks like when psmux cannot reach it.

This is diagnostic only, and while it is set a pane gets no XTVERSION reply, no OSC colour reply,
no injected mouse record and no Ctrl+C injection. Set it on the **server** process, with
`$env:PSMUX_NO_WARM = "1"` and a killed server first, reproduce, then clear it.

### The Paste Route Seam

| Variable | What it does |
|---|---|
| `PSMUX_PASTE_INJECT=1` | Delivers a bracketed paste as `KEY_EVENT` records, as if this host's conhost stripped the markers from the pipe |
| `PSMUX_PASTE_INJECT=0` | Pins the ConPTY input pipe, whatever the build says |

A bracketed paste has to reach the pane child with `ESC[200~` in front of it and `ESC[201~` behind
it. psmux writes those into the pane's ConPTY input pipe, and on Windows 10 19045 the inbox
conhost (10.0.19041.1) removes exactly those twelve bytes and hands the child the payload alone.
The write reports success, so there is nothing for psmux to notice. Measured with a standalone
pseudoconsole host and no psmux in the chain (issue #684):

```
19045  input pipe          host wrote 28 bytes, child received 16, markers gone
19045  WriteConsoleInputW  host wrote 31 records, child received 31 bytes, markers intact
26200  input pipe          host wrote 28 bytes, child received 28 bytes, markers intact
26200  WriteConsoleInputW  host wrote 31 records, child received 31 bytes, markers intact
```

So psmux gates the paste on the build number, at 22523, the same seam the mouse path already draws
for the same defect on the same channel in the same direction. Below it, a pane whose child has
`ENABLE_VIRTUAL_TERMINAL_INPUT` set gets the paste as `KEY_EVENT` records instead. Only 19045 and
26200 are measured; everything between them is unknown, which is what this variable is for. If
your pastes arrive unbracketed on a build above 22523, set it to `1`; if injection misbehaves on a
build below it, set it to `0`, and please report either.

`=1` opens the build half of the gate only. A pane whose child reads `INPUT_RECORD`s rather than
bytes always keeps the pipe, because it cannot reassemble a VT sequence out of per character key
events and would show the markers as the literal characters `[200~` in the editor (issue #98).
No setting of this variable can ask for that.

`PSMUX_INPUT_DEBUG=1` records the whole decision and the channel that carried the paste:

```
[paste] use_bracket=true text_len=490 text_preview="LINE0-..."
[paste] route decision: vt_byte_reader=true build=Some(26200) gate=22523 PSMUX_PASTE_INJECT=Some(true) -> Inject
[paste] route=inject pid=29004 text_len=490 ok=true
```

Set it on the **server** process, with `$env:PSMUX_NO_WARM = "1"` and a killed server first,
reproduce, then clear it.

### The Console Host Seam

| Variable | What it does |
|---|---|
| `PSMUX_CONPTY_DIR=<dir>` | Loads `<dir>\conpty.dll` and uses the `OpenConsole.exe` next to it as every pane's console host |
| unset (the default) | The system ConPTY in `kernel32.dll`, driving the inbox `conhost.exe` |

Every psmux pane is a pseudoconsole, and the process that owns that pseudoconsole is a console
host. By default that host is the `conhost.exe` shipped in the Windows you are running, reached
through the `CreatePseudoConsole` export in `kernel32.dll`. Microsoft also publishes the same
component out of band, as a `conpty.dll` plus an `OpenConsole.exe`, so an application can carry a
newer console host than the one in the box. That is how Windows Terminal and WezTerm work.

psmux does not ship either file and never loads one on its own. Nothing but `kernel32.dll` is
touched unless you name a directory yourself, which is the point of this variable: it is an
escape hatch for a host whose inbox `conhost.exe` has a defect you are stuck with, not a
supported configuration. The DLL is loaded by absolute path with
`LOAD_WITH_ALTERED_SEARCH_PATH`, so the `OpenConsole.exe` that gets spawned is the one beside the
DLL you named and not something else off the search path. If the directory does not exist, holds
no `conpty.dll`, or holds one that will not load or does not export the three ConPTY entry
points, psmux logs the reason and falls back to `kernel32.dll`, so a bad value costs you nothing
but the default behaviour.

Where to get the files: the `Microsoft.Windows.Console.ConPTY` package on nuget.org, which is
MIT licensed and carries `runtimes\win-x64\native\conpty.dll` and
`build\native\runtimes\x64\OpenConsole.exe`. Put the two side by side in one directory and point
the variable at it.

On Windows 10 19045 this is the one setting that repairs the whole family of inbox conhost
defects at once, measured by a reporter on #597 and #684 with the package at 1.24.2607.10001:
device queries (DA1, DA2, DSR, DECRQM) are answered while a program waits on them instead of
being held until the next paint, bracketed pastes keep their `ESC[200~` / `ESC[201~` markers on
the ConPTY input pipe, and non ASCII pastes (Latin 1, CJK) arrive byte exact where the inbox
host on that build mangles them. Under it psmux also takes the cheaper pipe route for a
bracketed paste rather than the `WriteConsoleInputW` injection it uses on the inbox host below
build 22523; `PSMUX_PASTE_INJECT=1` still forces injection if you need it.

Measured on Windows 11 26200 with that package at 1.24.2607.10001, against the inbox host, three
runs each:

```
                              inbox conhost      OpenConsole 1.24.2607.10001
launch to prompt, median      596 ms             711 ms    (bare pwsh 415 / 457 ms)
keystroke to screen, pwsh     16.74 ms median    2.19 to 2.47 ms median
DA1 / DA2 / DSR / DECRQM      answered by host   answered by psmux
XTVERSION                     answered by psmux  answered by psmux
```

#### Who answers the terminal's round trip questions

A program in a pane can ask the terminal what it is and what modes it has on. psmux answers
**DA1** (`ESC[c`), **DA2** (`ESC[>c`), **DSR** (`ESC[5n` and `ESC[6n`) and **DECRQM**
(`ESC[?<mode>$p`) itself, with the same bytes tmux sends: `ESC[?1;2c`, `ESC[>84;0;0c`, `ESC[0n`,
`ESC[<row>;<col>R` and `ESC[?<mode>;<value>$y` with 1 set, 2 reset, 0 not recognised
(`input.c`, `INPUT_CSI_DA`, `INPUT_CSI_DA_TWO`, `INPUT_CSI_DSR`, `INPUT_CSI_QUERY_PRIVATE`).
`ESC[>q` (XTVERSION) is answered too, and `ESC[1t` is ignored, again like tmux.

On the inbox `conhost.exe` none of that is visible, because that host answers those four itself
and never forwards them to psmux. A host that forwards them instead, which is what recent
OpenConsole builds do, used to leave them unanswered, and that cost two things at once: a program
in the pane got zero bytes back for DA1, DA2 and DECRQM, and **every pane launch stalled about
three seconds**, because OpenConsole opens a pane by writing `ESC[1t ESC[c` toward psmux and
parks the child inside its console connect until the DA1 reply arrives. psmux answering the
queries removes both: prompt at 616 / 617 / 615 ms where it used to be 3777 / 3747 / 3799 ms.

One deliberate difference from tmux: DECRQM for mode **2026** (synchronized output) reports `0`,
not recognised, rather than tmux's `2`. psmux has no synchronized output to offer, and `0` is
byte for byte what the inbox host already answers on this build, so a pane sees the same reply
whichever console host is under it.

The keystroke win is real and large. Set this when you want it, knowing you are also taking on a
console host that Windows Update does not patch for you.

`PSMUX_NO_PASSTHROUGH=1` is independent of this and does not change either number.

## Always On Diagnostic Files

These three are written without any variable being set. They exist because the failures they
describe happen where you cannot see them.

### `~/.psmux/server-startup.log`

Written when the server fails during startup, typically when the first pane cannot be spawned.
The classic case is a `default-shell` pointing at a path that does not exist or cannot be executed,
where all the user sees is psmux flashing and returning to the prompt. The log records:

- the real error text, including the raw Windows error (for example `CreateProcessW err 87`),
- the exact path psmux tried to spawn,
- the psmux build that produced the failure,
- the size of the inherited environment block and its largest single variable, because an
  oversized environment is itself a common cause of `err 87` on profiles where OneDrive and
  WindowsApps entries have inflated it toward the 32 KB Windows limit.

The attaching client reads this file back and echoes a fresh failure to your terminal rather than
leaving it buried, so in most cases you will see the real cause on screen. The file is the fuller
record. Only a log written during the current startup attempt is surfaced. An older one is treated
as stale and ignored.

Every server sharing the data directory appends to this one file, each report under a header
`=== session <name> pid <pid> at epoch <seconds> ===`, so two servers that fail together keep both
reports, and each client echoes the report of the server it started. When the file reaches 512 KB it
is renamed to `server-startup.log.1` (replacing any older copy) and a fresh file is started.

### `~/.psmux/config-warnings.log`

Non-fatal config parse problems, such as an unknown option name, a malformed value, or an unknown
command in your config file. The detached server has no stderr to complain to, so it records the
warnings here and the next client to attach echoes them to your terminal. As with the startup log,
only warnings from the current startup are shown, and the file is removed once a config parses
cleanly, so a fixed config stops reporting.

This is the file to check first when a config line appears to have been silently ignored.

### `~/.psmux/crash.log`

Written by the server's panic handler: the panic message and a full backtrace. The handler also
removes that session's `.port` and `.key` files, so a crashed server does not leave stale discovery
files behind for the next client to trip over. Like the startup log it is appended to by every
server in the data directory, one headed report per panic, and rotated to `crash.log.1` at 512 KB.

If this file exists and its timestamp matches your problem, attach it. It is the single most useful
artifact in a bug report.

## Runtime State and Bookkeeping

psmux keeps all of its runtime bookkeeping in one directory, `%USERPROFILE%\.psmux\`. The home
directory is resolved from `USERPROFILE` first, then the Win32 profile API, then
`HOMEDRIVE` plus `HOMEPATH`, then `HOME`. Setting `PSMUX_DATA_DIR` to an absolute path moves the
whole directory, which gives you a second, fully independent registry (its own sessions, its own
warm server, its own `last_session`) without touching the default one. See
[configuration.md](configuration.md#environment-variables).

Per-session files are named after the session:

| File | Purpose |
|---|---|
| `<session>.port` | The TCP port that session's server is listening on |
| `<session>.key` | The auth key a client must present to that server |
| `<session>.sid` | The session's stable id, the `$N` used in targets |
| `<session>.pid` | Liveness anchor, stored as `pid:creation_filetime` |
| `<session>.act` | Last activity stamp, Unix epoch microseconds. Written on client attach and on real keystrokes from an attached client (throttled to one write per second), the same `activity_time` tmux keeps. It is what a bare `psmux <command>` with no `-t` uses to pick the session you are actually working in ([#603](https://github.com/psmux/psmux/issues/603)). A detached session that was never attached has none and ranks by its `.port` file's age |
| `<session>.spawnlock` | Warm pool spawn lock, prevents two racing spawns |

The `.pid` file is worth understanding, because it is what makes psmux safe to clean up after
itself. A bare pid can be recycled by Windows and reused by an unrelated process. Storing the
process creation time alongside it means psmux can prove a pid is still the same process it wrote
down before it acts on it, so the orphan server reaper and `kill-server` can never terminate
something that merely inherited the number.

The warm server uses the reserved session name `__warm__`, so it owns a full set of the files above
named `__warm__.port`, `__warm__.key` and so on. That is why `psmux ls` does not show it.

Socket namespaces created with `-L <name>` prefix the session name, so the files become
`<namespace>__<session>.port` and so on. A namespaced warm server is therefore
`<namespace>____warm__.port`, with the doubled underscore coming from the namespace separator plus
the warm name.

Three files have fixed names and are not tied to any session:

| File | Purpose |
|---|---|
| `latency.log` | Client latency trace, only written under `PSMUX_LATENCY_LOG=1` |
| `last_session` | The session name written on the most recent attach. A bare `psmux` invocation no longer trusts it outright: routing ranks sessions by their `.act` stamp first, and this file only breaks a tie between sessions with identical activity, which in practice means a registry written by an older psmux that nobody has attached to since |
| `next_session_id` | Counter that hands out the next stable session id |

A stale `.port` (a server that is gone but whose registry files survived) is reaped by the next
client that trips over it. `psmux attach -t name` against one prints tmux's
`can't find session: name` and exits 1, and a bare `psmux attach` that finds nothing left prints
`no sessions`. Neither surfaces the underlying winsock error any more
([#605](https://github.com/psmux/psmux/issues/605)). If you see a raw
`No connection could be made because the target machine actively refused it` you are on an older
build.

The same directory also holds `plugins\` (see [plugins.md](plugins.md)) and, in control mode,
`cc_debug.log` (see [iterm2-control-mode.md](iterm2-control-mode.md)).

### Inspecting the Directory

```powershell
Get-ChildItem "$env:USERPROFILE\.psmux" | Sort-Object LastWriteTime -Descending
```

```text
Mode    LastWriteTime        Length Name
----    -------------        ------ ----
d----   7/27/2026  9:14 AM          plugins
-a---   7/27/2026  9:14 AM      21  work.port
-a---   7/27/2026  9:14 AM      44  work.key
-a---   7/27/2026  9:14 AM      26  work.pid
-a---   7/27/2026  9:12 AM      21  __warm__.port
-a---   7/27/2026  9:12 AM       4  last_session
```

Deleting a stale `.port` or `.key` file for a session whose server is gone is safe. psmux also
cleans these up on its own, both at startup and when a server exits or panics. Never delete files
belonging to a session you still have attached.

## What to Attach to a Bug Report

Collect these before opening an issue at
[GitHub Issues](https://github.com/psmux/psmux/issues). The first four are almost always enough.

1. **Version and build.** `psmux -V` prints the version, the commit hash, and the build date.
2. **Your config**, or the smallest config that still reproduces it. If you are unsure whether your
   config is involved, test with an empty one: `psmux -f NUL new-session`.
3. **The always on logs, if they exist and are recent:** `~/.psmux/crash.log`,
   `~/.psmux/server-startup.log`, `~/.psmux/config-warnings.log`.
4. **Exact reproduction steps**, including which shell you launched psmux from and which terminal
   emulator you are running in. Windows Terminal, conhost and ConEmu behave differently.
5. **A relevant debug log**, if you can guess the subsystem. Keys or modifiers not arriving,
   use `PSMUX_INPUT_DEBUG`. Mouse misbehaving, use `PSMUX_MOUSE_DEBUG`. Anything over SSH, use
   `PSMUX_SSH_DEBUG`. Wrong colors or a broken theme, use `PSMUX_STYLE_DEBUG`. A client that will
   not attach, use `PSMUX_AUTH_DEBUG`. A program inside a pane that seems to lose a colour, a
   screen mode or its mouse registration, use `PSMUX_PANE_RAW` and look at what the program
   really wrote. A bare command landing in the wrong session, use `PSMUX_ROUTE_DEBUG`.
6. **A directory listing** of `%USERPROFILE%\.psmux\` when the problem is about sessions not being
   found, servers not starting, or stale sessions appearing in `psmux ls`.
7. **Your Windows version.** `[System.Environment]::OSVersion.Version` prints it. Several
   subsystems, notably mouse over SSH, are gated on the Windows build number. See
   [mouse-ssh.md](mouse-ssh.md).

A one-liner that gathers the common set:

```powershell
psmux -V
Get-ChildItem "$env:USERPROFILE\.psmux" -Filter "*.log" | ForEach-Object {
    "===== $($_.Name) ====="
    Get-Content $_.FullName -Tail 50
}
```

Debug logs can contain the text you typed and the output of your panes. Read them before you paste
them into a public issue.

> **Note:** psmux also reads a small number of `PSMUX_TEST_*` variables and one build-number
> override. These are internal test seams used by the test suite to force timing races and are not
> supported knobs. Do not set them.

## See Also

- [configuration.md](configuration.md) for the supported options and user facing environment variables
- [warm-sessions.md](warm-sessions.md) for the `__warm__` server and how to disable it
- [performance.md](performance.md) for what the latency numbers should look like
- [faq.md](faq.md) for the common questions these logs usually answer
