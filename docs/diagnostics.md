# Diagnostics

psmux runs its server detached, with no visible stderr. When something goes wrong there is
nowhere for an error message to land on screen, so psmux writes what it knows to files instead.
This page lists every diagnostic file psmux can produce, the environment variables that turn the
optional ones on, and what to attach when you file a bug report.

Everything on this page is read only observation, with one exception called out where it appears:
`PSMUX_FAKE_WIN_BUILD` moves psmux onto a different branch on purpose, so that a platform specific
path can be reproduced on a machine that is not that platform.

## Debug Logging

psmux has eleven independent debug loggers. All of them are **off by default** and cost nothing
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
| `PSMUX_MOUSE_DEBUG=1` | `~/.psmux/mouse_debug.log` | Mouse injection and screen to pane coordinate mapping |
| `PSMUX_SSH_DEBUG=1` | `~/.psmux/ssh_input.log` | SSH escape sequence decoding into Win32 input records |
| `PSMUX_LATENCY_LOG=1` | `~/.psmux/latency.log` | Keypress to render latency, measured client side |
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
files behind for the next client to trip over.

If this file exists and its timestamp matches your problem, attach it. It is the single most useful
artifact in a bug report.

## Runtime State and Bookkeeping

psmux keeps all of its runtime bookkeeping in one directory, `%USERPROFILE%\.psmux\`. The home
directory is resolved from `USERPROFILE` first, then the Win32 profile API, then
`HOMEDRIVE` plus `HOMEPATH`, then `HOME`.

Per-session files are named after the session:

| File | Purpose |
|---|---|
| `<session>.port` | The TCP port that session's server is listening on |
| `<session>.key` | The auth key a client must present to that server |
| `<session>.sid` | The session's stable id, the `$N` used in targets |
| `<session>.pid` | Liveness anchor, stored as `pid:creation_filetime` |
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
| `last_session` | The session name to reattach to for a bare `psmux` invocation |
| `next_session_id` | Counter that hands out the next stable session id |

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
   not attach, use `PSMUX_AUTH_DEBUG`.
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
