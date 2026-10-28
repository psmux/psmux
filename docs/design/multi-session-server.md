# Design: a shared multi session server (issue #798)

Status: stage 1, steps 1 and 2 implemented. Step 1 is the switch (`PSMUX_SERVER_MODE`
and `server-mode`); step 2 is a minimal shared server behind it (one process per
namespace holding its sessions, see "Step 2 (implemented)" below). Everything else
here is still a proposal.

> **Owner ruling, 2026-10-17. This overrides anything below that disagrees with it.**
>
> 1. The shared server is a **switch**, not a structural change. It is turned on with
>    `PSMUX_SERVER_MODE=shared` (or `set -s server-mode shared`) and turned off by
>    unsetting it. With the switch off, psmux runs exactly the code it runs today.
> 2. **Everything** for the shared server lives behind that switch. No PR may change
>    the code path a default (per session) build runs, not even as a "zero behaviour
>    change" refactor. Stage 0 as first written (refactors of the shared code paths)
>    is withdrawn; the stage 0a id allocator (PR #838, 13064512) was merged and then
>    reverted for this reason.
> 3. There is **no planned default flip**. Stage 4 is withdrawn. One server per session
>    stays the default; the shared mode stays opt in for as long as the owner decides.
> 4. Multiple servers stay possible: each `-L` namespace is its own server, and in
>    shared mode each one hosts many sessions, like tmux. Every number below was
measured on master `98392248` (psmux 3.4.0, Windows 11 26200) with a private
`PSMUX_DATA_DIR` and a unique `-L` namespace, or counted with `grep` over `src/`,
`crates/`, `tests/` and `tests-rs/` at that commit. Where something could not be
measured it says so, in the last section.

## 1. The question

Today psmux runs one server process per session. tmux runs one server per socket
(`-L name` or `-S path`) and that server holds every session on the socket. Issue
#798 asks whether psmux will move to the tmux model. The owner's answer is yes, with
three conditions:

1. Multiple servers must still be possible: each `-L` namespace is one server, and
   each server hosts many sessions, exactly like tmux.
2. It ships as an opt in switch, step by step, and today's one server per session
   model stays the default. Turning the switch off must always give back exactly
   today's behaviour (see the owner ruling at the top).
3. Every step must be shippable, reversible and tested on its own, because a bug in
   the process model is the kind that takes weeks to find on a project with 3.7k stars.

A contributor (tanaeakihiko) also asked whether cross session `move-window` and
`link-window` could be done inside the current architecture first. Section 4 answers
that with measurements.

## 2. Goals and non goals

Goals

* `-L ns` names one server; `new-session` in a namespace that already has a shared
  server adds a session to it instead of starting a process.
* Window ids `@N` and pane ids `%N` unique across a server, as in tmux, so a bare
  `%N` or `@N` is never ambiguous.
* Server, session, window and pane option scopes with tmux's inheritance chain
  (`set -g` reaches every session that does not override it).
* Server wide paste buffers, key tables, global environment and global hooks.
* Cross session `move-window`, `join-pane`, `swap-pane`, `link-window`, real session
  groups, and `switch-client`/`choose-tree` without a client reconnect.
* A panic while serving one session must not end the others.
* Older clients, scripts and plugins keep working against a shared server without
  being changed.

Non goals

* Changing the default. The shared mode is opt in only (owner ruling).
* Changing the code that a default (per session) build runs.
* A Unix socket, or any change to the TCP plus key file transport.
* Moving a live ConPTY between processes (section 4.3 explains why).
* Merging namespaces. `-L a` and `-L b` stay separate processes forever, which is
  also the user's escape hatch for isolation.

## 3. Inventory: everything that assumes one server is one session

### 3.1 How the state is shaped today

* `AppState` (`src/types.rs:1477` to `2039`) is one struct with **225 public fields**.
  It mixes server state (key tables, paste buffers, control clients, the warm pane
  pool, `socket_name`), session state (`session_name`, `session_id`, `active_idx`,
  `last_window_idx`, hooks, environment, user options, 54 session scoped options as
  flat fields) and per client UI state (copy mode cursor, prompt, drag state).
* `run_server` (`src/server/mod.rs:2088` to `8990`) is a single **6,900 line
  function**. It keeps **43 locals** before its main loop, about 25 of which are per
  session render caches and timers (`state_dirty`, `cached_dump_state`,
  `cached_windows_json`, `last_client_activity`, `echo_pending_until`, ...).
* `CtrlReq` (`src/types.rs:3200` to `4034`) has about **232 variants**, matched at
  **761** sites. None of them says which session it is for, because the process is
  the session.
* `app.windows` alone is touched at **653** sites in `src/`; the session level fields
  (`windows`, `active_idx`, `session_name`, `user_options`, `environment`, `hooks`,
  `next_win_id`, `next_pane_id`) at **1,009** sites in `src/` and **1,058** in
  `tests-rs/` (82 files touch `app.windows`/`app.active_idx` directly, 148 build an
  `AppState`).

### 3.2 Call site inventory

| Area | What assumes one session per process | Count |
|---|---|---|
| Registry files | `<ns>__<session>.port/.key/.pid/.sid` written and read through `port_file`/`key_file`/`pid_file`/`sid_file` helpers | 232 hits in 14 files; 10 sites enumerate `*.port` in the data dir (`main.rs` ls, 3 in `client.rs` pickers, 6 in `session.rs`) |
| `port_file_base()` | "this server's one registry name" | 57 |
| Warm standby | `__warm__` server claimed and renamed into a session | 61 hits in 11 files; `warm` in 916 lines of 25 files; `app.warm_pane` at 87 sites |
| CLI routing | `PSMUX_TARGET_SESSION` repointed by the CLI to pick a server | 18 `set_var` sites; `resolve_routing_target` 6 |
| `$TMUX` | `path,port,0`; the third field is always `0`, and `session_base_owning_tmux_port` maps a port back to a session (`session.rs:3092`). With several sessions on one port it returns whichever `.port` file `read_dir` yields first | `set_tmux_env` 7 sites |
| Unqualified ids | `cli_resolve_unqualified_id` (`main.rs:330`) asks every server for `%N`/`@N` and **refuses as ambiguous** when two sessions both own it (#627) | 6 sites |
| Session ids | `allocate_session_id` uses a data dir counter file plus a lock file; already unique across servers | 12 `.session_id` sites |
| Window and pane ids | `next_win_id`/`next_pane_id` per `AppState` | 44 sites in 6 files |
| Cross session refusals | `foreign_session_in_spec` + `cross_session_refusal` for move-window, link-window, swap-pane, new-window | 6 + 4 sites |
| Join pane proxy | `proxy_pane.rs`, `cross_session.rs`, `cross_session_server.rs` | 893 lines, 38 hits in 8 files |
| switch-client | Server sends a `SWITCH <session>` directive and the client reconnects to the other server (`server/mod.rs:7168`, `7286`; `client.rs:4169`) | 4 sites |
| Session end | `std::process::exit` from `KillSession`, 4 `destroy-unattached` paths, `exit-empty`, `kill-server` x3, warm orphan | 106 `process::exit` hits in 5 files; in `server/` 9 real calls plus 3 calls of `shutdown_this_server` |
| kill-server (#649) | `kill_server_targets` builds one target per `.port` file, no dedupe by port or pid | 76 hits in 14 files |
| Session groups | `session_group` is a label only | 17 hits in 5 files |
| exit-empty, destroy-unattached | session teardown is process exit | 22 hits |
| Attached clients | `attached_clients`, `client_registry`, `control_clients` | 21, 37, 39 |
| Control mode | `%sessions-changed` is emitted only when a control client attaches (`control.rs:222`), never when another session is created or killed, because the server cannot see other sessions | 8 hits |
| Hooks | `app.hooks`, one map per process | 45 |
| Options | `user_options` one map per process (`types.rs:1822`, doc comment: "psmux's one `user_options` map"); catalog has 13 server, 54 session, 23 window, 4 pane options, all stored as flat `AppState` fields with no global layer | 79 + 8 |
| Environment | `app.environment` / `env_scopes` | 43 |
| Buffers and key tables | per process (tmux: server wide) | 100 and 33 |
| Process statics | 107 `static` items in `src/`. Server wide wake flags (`PTY_DATA_READY`, `WAKE_TX`, `CPR_DATA_PENDING`) are fine in one process; pane keyed tables (`PIPE_WRITERS`, `DEVICE_REPLIES`, `PANE_PALETTES`) are keyed by pane id and **need server unique pane ids**; client tables (`FRAME_PUSH_SLOTS`, `DIRECTIVE_CHANNELS`, `PERSISTENT_STREAMS`) are broadcast by `send_directive_to_all_clients` (6 sites) and **must become session scoped**; `SERVER_NAMESPACE` is fine |
| Panic handling | the panic hook (`server/mod.rs:2101`) deletes this session's registry files; only 9 `catch_unwind` hits in 2 files, none around request dispatch | 9 |
| Blocking in the loop | 14 `thread::sleep` and 5 blocking `output()`/`wait()`/`recv()` calls inside the main loop body. Today they stall one session; in a shared server they stall all of them | 19 |

### 3.3 Runtime probes (two detached sessions `A` and `B` in one namespace)

| Probe | Result today | tmux 3.4 |
|---|---|---|
| `list-panes -a -F '#{session_id} #{window_id} #{pane_id}'` | `A $0 @1 %1`, `A $0 @2 %2`, `B $1 @1 %1`: **window and pane ids collide** | ids unique per server |
| `set -g @x fromA` then `show -gv @x` in B | `invalid option: @x` | `fromA` |
| `set -g status-left AAA` in A, read in B | B still `[#S] ` | `AAA` |
| `setenv -g FOO bar` in A, `showenv -g FOO` in B | `unknown variable: FOO` | `FOO=bar` |
| `set-buffer hello` in A, `list-buffers` in B | empty | `buffer0` |
| `new-session -d -t A -s G` | G gets its own new window `@1` with a different shell, `session_grouped=0`, a new window in A never appears in G | G shares A's windows |

Process cost of the model, 7 sessions plus the standby: 8 server processes, each
15.7 to 18.2 MB working set, 4.0 to 7.4 MB private, 21 to 38 threads, about 240
handles. `ls` across 7 servers took 33 ms and `list-panes -a` 27 ms.

Creation latency in the same namespace (6 samples each):

| Operation | Median | Range |
|---|---|---|
| first `new-session -d`, cold | 379 ms | one sample |
| `new-session -d` claiming the warm standby | 362 ms | 356 to 364 |
| `new-window` | 16 ms | 15 to 25 |
| `split-window` | 19 ms | 18 to 26 |

A second session costs about 20 times a new window. Inside a shared server a new
session would be a session object plus one pooled pane, so it should approach the
new-window number. That is a projection, not a measurement.

### 3.4 Consumers outside `src/`

* **Claude Code teammate backend** (`docs/claude-code.md`): `new-session -d -s
  <team> -- cat`, then `split-window`, `send-keys`, `respawn-pane`, `set-option -p`
  all against bare `%N`. In a shared server `%N` must be unique server wide or these
  commands hit the wrong pane. Pinned by `tests/test_issue580_teammate_backend.ps1`.
* **psmux-resurrect** talks only through the CLI (`list-sessions`, `has-session`,
  `new-session -P -F '#{pane_id}'`, `split-window -t`). `restore.ps1:185` already
  carries a workaround: "with a second session alive [bare `%id` targets] can land on
  the wrong server". Server unique ids remove the reason for it.
* **psmux-continuum** uses `session-created` and `client-attached` hooks. In a
  shared server `session-created` fires inside the same process for every new
  session, which is tmux's behaviour; today each process fires it once for itself.
* **Periodic work and hook affinity** (raised by MattKotsenas). psmux-continuum runs
  its own save loop. Driving it from the status refresh instead, the way
  tmux-continuum does with a `#()` job in `status-right`, would today run the save
  once per SERVER, so N times per interval in a namespace with N sessions, although
  one save already covers every session. tmux does not deduplicate this by hook
  scope either: a `#()` job is cached per client (`format_job_get`, format.c:396 to
  403, `ft->client->jobs`), so two attached clients run it twice, and
  tmux-continuum stays correct only because the script itself checks a last save
  timestamp (`@continuum-save-last-timestamp`) and returns early. The rules for
  psmux:
  * In shared mode a server or session event (`session-created`,
    `session-closed`) fires once in the one server, for that session; a status
    `#()` job runs once per attached client, as in tmux.
  * Work that covers the whole namespace must be idempotent per interval no matter
    how often it is triggered: the plugin keeps a namespace wide timestamp (a
    server option `@continuum-save-last-timestamp`, which is shared in shared mode,
    or a file in the data dir in today's mode) and returns early, exactly like
    tmux-continuum.
  * Stage 1 adds a test: two sessions in one shared namespace with a status driven
    save fire it once per interval, and with one server per session the timestamp
    check still keeps it to one save.
* No plugin in `psmux-plugins` reads `.port` or `.key` files directly (grep over
  `*.ps1`, `*.psm1`, `*.conf`).

### 3.5 Test suite assumptions

* 930 `tests/*.ps1` suites. 532 read `.port` files, 410 read `.key` files, 65 name
  `__warm__` or `server -s`, 142 find psmux processes through `Get-Process` or
  `Win32_Process`, 93 kill psmux by name, 869 run `new-session`, 379 `kill-server`.
* 31 suites hard code `%0`..`%9` or `@N` literals.
* `tests-rs/`: 389 files, 5,345 `#[test]`; 148 construct `AppState` directly.

The registry compatibility rule in section 6.6 exists so the 532 and 410 suites need
no edit: in shared mode every session still has its own `.port` and `.key`.

## 4. Can cross session move-window or link-window work today?

### 4.1 What exists: the join-pane proxy

`join-pane -s S:0.0 -t T:1` across sessions extracts the pane in S, keeps the real
ConPTY and child in S's process, and gives T a `ProxyMasterPty` that tunnels input
and output over a loopback TCP connection (`src/proxy_pane.rs`,
`src/cross_session_server.rs`). Output reaches the tunnel as a tee in `PIPE_WRITERS`.

### 4.2 Measured cost and failure modes

Keystroke to screen latency, `tests/keylat.cs` against an attached client, pane
running `tests/echo_load_child.cs`, oracle `cell:0,0`, 80 keystrokes per cell:

| Pane | Median | p90 | p99 | Max | Missing |
|---|---|---|---|---|---|
| native pane in T | 1.90 ms | 2.35 ms | 2.63 ms | 2.69 ms | 0 |
| pane joined from S (proxied) | 2.01 ms | 2.42 ms | 2.91 ms | 3.54 ms | 0 |

The tunnel costs about 0.1 ms at the median and 0.3 ms at p99. The `join-pane` call
itself took 21 to 24 ms. **Latency is not the problem. Ownership is.**

Failure modes, each reproduced:

1. **Moving the last pane of a session kills it.** With S holding one pane, `join-pane
   -s S:0.0 -t T:0` exits 0, S's server process exits (its window list is empty),
   and the real shell dies with it (source server and moved shell both gone two
   seconds later). T keeps a pane reporting `pane_dead=0` and `pane_current_command
   =shell` whose keystrokes go nowhere: in the latency harness 65 of 65 keystrokes
   timed out. `S.pid` and `S.sid` are left behind in the data dir. tmux moves the
   pane and destroys the empty session.
2. **Killing the source kills a pane the user sees in another session.** With S
   kept alive by a second window, `kill-session -t S` ended the moved shell; T's
   pane again stayed `pane_dead=0`, no `pane-exited` fired.
3. **Pane identity changes.** S's `%1` became `%3` in T. tmux keeps `%1`.

Status: failure modes 1 and 2 are fixed in place (see `tests/test_joinpane_proxy_ownership.ps1`):
the source server now stays up as a host for a forwarded pane after its own session
ends (registry files removed, name released, not listed), exits once the pane is over,
and a proxy whose host is gone reads as an exited child, so the target fires
`pane-exited` / `pane-died` and honours `remain-on-exit`. The same fix found that the
proxy pane took "highest id in the tree plus one" as its id, which a later split
could hand out again (`%1 %3 %3 %2 %4` measured); it now takes the allocator's next
id. Failure mode 3 stays as documented: pane ids are per server today, so a moved
pane gets an id of its new server until ids are server or namespace unique (stage 0a
and 0g).
4. **Two processes own one pane.** Resize travels over a separate control connection,
   `pane_pid` names a process whose parent is another server, and `kill-server` in
   one namespace has to find the owner by scanning.

### 4.3 Can the ConPTY itself be handed to the other process?

Measured with a probe (`CreatePseudoConsole` from kernel32, then `GetHandleInformation`
and `DuplicateHandle`):

* `HPCON` is **not** a kernel handle. Both calls fail with `ERROR_INVALID_HANDLE` (6).
  It is a pointer into the creating process's heap.
* That heap block holds three real handles (signal pipe, pty reference device,
  conhost process). All three, and the input and output pipe ends, can be duplicated.

So a transfer would mean duplicating five or six handles into the target and
rebuilding the undocumented `PseudoConsole` struct there so `ResizePseudoConsole`
and `ClosePseudoConsole` work. That depends on a kernel32 implementation detail that
Microsoft can change in any update, and the child process stays parented to the
original server. It is not a foundation for a feature users rely on.

### 4.4 Answer to tanaeakihiko

`move-window` across sessions could be built today as "proxy every pane of the
window", and it would be fast (about 0.1 ms per keystroke). It would also inherit
all four failure modes above for every pane of the window, and `link-window` cannot
be built that way at all: one window shown in two sessions needs one layout, one size
negotiation and one input arbiter, and today those live in two processes.

Recommendation: do not extend the proxy. Fix its two data loss bugs in place (refuse
or correctly hand over the last pane, and mark a proxied pane dead with `pane-exited`
when its tunnel closes), and implement `move-window`/`link-window` as in process
moves in the shared server (stages 2 and 3). In a shared server a cross session move
is a `Vec` move under one lock, with no tunnel at all.

## 5. How tmux does it

Read from `server.c`, `session.c`, `window.c`, `server-fn.c`, `cmd-find.c`,
`options.c` and `environ.c` in the local tmux checkout (`2848a125`).

* **Sessions.** A global red black tree `sessions`; `session_create` takes ids from a
  process global `next_session_id` and inserts the session (`session.c:111`). A
  session owns `windows`, a tree of **winlinks**, not windows.
* **Windows and panes are server objects.** `window.c` keeps the global `windows`
  tree and `all_window_panes`, with `next_window_id` and `next_window_pane_id` as
  file statics. That is why `@N` and `%N` are unique per server.
* **Winlinks.** A winlink is `(session, index, window)`. `server_link_window`
  (`server-fn.c:288`) adds a second winlink to the same window, refusing when both
  sessions are in the same group. `window_add_ref`/`window_remove_ref` count
  references; the window is destroyed when the last winlink goes. `move-window` is
  link then unlink. Session groups are sessions whose winlink sets are kept identical
  by `session_group_synchronize_to`.
* **Options.** Three global tables (`global_options` server, `global_s_options`,
  `global_w_options`, `tmux.c:36`). Each session gets `options_create(global_s_options)`
  (`cmd-new-session.c:273`), each window `options_create(global_w_options)`
  (`window.c:430`), each pane `options_create(w->options)` (`window.c:1308`). Lookup
  walks the parent chain. Hooks are options, so they follow the same chain.
* **Environment.** `global_environ` plus one `environ` per session; a child gets the
  merge. `TMUX` is `socket_path,server_pid,session_id` (`environ.c:282`) and
  `TMUX_PANE` is `%id` (`spawn.c:394`).
* **Targets.** `cmd_find_target` starts from the client's current state, or from the
  pane whose tty matches the client, or `TMUX_PANE` (`cmd-find.c:80`). A bare `%N`
  is looked up in the server wide pane tree, so it can never be ambiguous.
* **Exit.** `server_loop` (`server.c:266`) exits when `exit-empty` is on and no
  session remains (or when `exit-unattached` is on and no client is attached), after
  flushing clients and waiting for jobs.
* **Paste buffers and key tables** are file statics (`paste.c:35`,
  `key-bindings.c:109`): server wide.
* **Failure isolation.** None: tmux is one process, and `fatal()` ends every session
  on the socket. psmux must do better than that (section 8) because a Rust panic in
  one handler is recoverable and the user base is used to per session isolation.

## 6. The staged plan

Every stage is a separate release train item, ships behind a switch where it changes
behaviour, has its own test gate, and can be reverted without touching the stages
before it.

### Stage 0: withdrawn (owner ruling, 2026-10-17)

The items below were written as refactors of the code every build runs. Under the
owner ruling they may not land that way. Where the shared server needs any of them
(an id allocator, a session id on requests, per session loop state, option layering,
an exit funnel), it gets its own version inside the shared mode code, entered only
when the switch is on. The list stays here as a record of what the shared mode has to
provide, not as a plan for the default path. Each item, when built, is its own PR.

* **0a. Server owned id allocator.** Move `next_win_id`, `next_pane_id` and the
  forward id counter out of `AppState` into a `ServerIds` struct owned by the server
  and lent to the session. With one session per process the ids handed out are the
  same sequence as today. (First PR, section 11.)
* **0b. Session envelope on requests.** Connection threads attach a `SessionId` to
  every `CtrlReq` they send (always the one session). The main loop looks up `&mut
  AppState` by id. No variant changes shape; this is a wrapper at the channel.
* **0c. Per session loop state.** Move the ~25 per session locals of `run_server`
  into a `SessionLoop` struct stored beside the session.
* **0d. Option layering, internal only.** Introduce an option table with a parent
  link (server, global session, session; global window, window; pane). Today's flat
  fields become views that read through the chain. In a one session server the global
  and session tables are both present, so `set -g` and `set` keep their current
  observable results; parity suites against tmux decide where today's behaviour is
  already a bug.
* **0e. Process static audit.** Classify all 107 statics as server wide, pane keyed
  or client keyed, and move client keyed broadcasts behind a session filter that, with
  one session, is "all clients".
* **0f. Exit funnel.** Route the 9 `process::exit` calls and 3 `shutdown_this_server` calls in `server/` through one
  `end_session(app, why)` that, with one session, ends the process exactly as today.

Gate: full sweep green, perf gates unchanged, `cargo check`, targeted `tests-rs`.

### Stage 0g (optional, opt in): namespace unique ids in today's model

`PSMUX_NS_UNIQUE_IDS=1` makes each server lease blocks of window and pane ids from a
namespace counter file (the same lock file scheme `allocate_session_id` already uses,
one lock per block of 64, not per pane). This fixes the #627 ambiguity refusal and the
resurrect workaround without a shared server, and gives shared mode's id rules a
release of field testing early. Ids stop being dense; tmux scripts do not depend on
density, but 31 of our suites hard code literals and must be checked under the flag.

### Stage 1: opt in shared mode

Switch: environment variable `PSMUX_SERVER_MODE=shared` read by the CLI when it is
about to start a server, plus `set -s server-mode shared` in the config for the next
server start. The default stays `per-session`. A namespace is in shared mode only if
its server was started in shared mode; the mode is written to a namespace marker
(`<ns>.server`) so CLIs without the variable still route correctly.

#### Step 1 (implemented): the switch itself

The switch exists and nothing behind it is built yet. The contract:

* **Where it is read.** Only where a server is about to be started or the warm
  standby is about to be claimed: CLI `new-session` / `new` (which also covers the
  `attach` bootstrap from a config `new-session`, and the one shot `new-session -d`
  a server forwards to the CLI for `if-shell` and TCP), CLI `start-server` /
  `start` / `warmup`, the bare `psmux` and `-C` / `-CC` start (only when no live
  session is being attached to), `new-session` run inside a server (hook, key
  binding, command prompt: `execute_command_string` in `commands.rs`), the TCP
  `new-session` spawn route in `connection.rs`, and the top of the internal
  `psmux server` process, which every server process passes through however it was
  spawned (including the warm standby refill). Nothing else reads it: `ls`,
  `attach`, `kill-session`, `kill-server` and every other command behave as before
  whatever its value.
* **Values.** `PSMUX_SERVER_MODE` is `per-session` (the default) or `shared`.
  Unset, empty or only whitespace is unset. Case and surrounding spaces are
  ignored. If the variable is unset, the user's config file is scanned the way
  the CLI already peeks at `warm` and `priority` before a server exists: a top
  level `set` / `set-option` line (any flags) naming `server-mode`, last one wins,
  `set -u server-mode` means the default. Lines inside `if-shell` or a sourced
  file are not seen. Otherwise `per-session`. The environment outranks the config,
  as `PSMUX_PRIORITY` outranks `priority`.
* **Unknown values.** Any other value is an error, never a fallback, at exit 1
  with no server started or claimed:
  `psmux: PSMUX_SERVER_MODE: unknown value 'bogus' (expected per-session or shared)`
  and, from the config,
  `psmux: server-mode: unknown value 'x' in <file> (expected per-session or shared)`.
  This is stricter than `PSMUX_PRIORITY` (which warns and ignores) on purpose: a
  typo must never quietly run the other process model.
* **`shared` in step 1.** It is handed to `src/server/shared/mod.rs`, the only
  entry for the mode, which prints
  `psmux: shared server mode is not available in this build yet (step 1 of #798); ...`
  and exits 1. A start a running server makes on the user's behalf (hook, binding,
  prompt, TCP) shows the same text as a status message or an `ERROR:` reply and
  creates nothing. It never falls back to per session.
* **The warm pool.** The gate runs before the warm claim on every route, so a
  `shared` request is never served by the per session `__warm__` standby, and the
  standby is left parked and untouched (the E2E checks the process list before and
  after). If the switch says `shared` when a per session server tries to refill its
  standby, the new standby process refuses at its own entry, so no per session
  standby is created while the switch says shared. The shared server will not use
  the `__warm__` standby at all; it will keep spare panes in its own process.
* **`server-mode` as an option.** Known to `set` and `show` (a validation only name,
  so `show -s`, `show -g` and the customize list are byte for byte unchanged), and
  `#{server-mode}` expands. `show -s server-mode` answers the mode the server
  actually runs in, so on every server today `server-mode per-session`, whatever
  the asking shell's variable says. The mode is fixed when a server starts: `set -s
  server-mode per-session` on a per session server is a silent no op (so a config
  that names it, and a later `source-file` of that config, do not warn), and
  `shared` or an unknown value is refused at exit 1 (`server-mode cannot change on
  a running server`). tmux has no such option; it is psmux only.
* **Proof the default path is untouched.**
  `tests/test_issue798_server_mode_switch.ps1` runs one scenario (two
  `new-session`, `new-window`, `ls`, `list-windows`, `display-message`,
  `kill-session`, the registry file names, the server processes per session, the
  warm standby) with the variable unset, `per-session` and empty, and requires
  one identical signature, equal to a master build's when `PSMUX_BASELINE_EXE` is
  given. `tests-rs/test_issue798_server_mode_switch.rs` covers the parser.

#### Step 2 (implemented): a minimal shared server

`shared` now runs one server process per namespace that holds every session of
the namespace. Everything is in `src/server/shared/`; **no line of code a default
build runs was changed** (the step 1 gates already live in `shared/mod.rs`, and
only their `shared` branches changed). `tests/test_issue798_shared_server.ps1`
Part 0 checks that per commit.

**Shape.** Not run_server on a thread per session: run_server is written for a
process that is one session (it exits the process on kill-session, pushes frames
to every client of the process, sets the process environment and directory for
its session, owns the one wake channel), and making it reentrant would change the
default path. The shared server has its own loop (`server.rs`) and calls the per
session building blocks unchanged:

* `cli.rs`: the CLI side, entered from `gate_server_start`. `new-session` reads the
  namespace marker, starts the server if there is none (`psmux server
  --shared-server [-L ns]`, entered through `gate_server_process`), sends the
  request to the server's control listener, and without `-d` attaches by running
  the ordinary `attach-session -t <name>`. `start-server` starts the server.
  `-C`/`-CC` is refused (deferred).
* `server.rs`: the process. Each session is a full `AppState` with its own
  loopback listener; each accepted connection goes to the existing
  `connection::handle_connection` with that session's channel; one forwarding
  thread per session tags requests with the session and hands them to the one
  loop thread. Before a request the loop *enters* the session: it lends the
  server wide id counters to the session (`ids.rs`), and points what a pane child
  inherits (PSMUX_TARGET_SESSION, the job environment's `$TMUX`, the working
  directory) at that session. Every request runs inside `catch_unwind`; a panic
  ends that one session (files, clients, panes) and the others keep running.
* `dispatch.rs`, `frame.rs`: copies of run_server's arms for the requests step 2
  serves, each naming its source line at 0145958f, and the frame builder. A
  frame is pushed only to the session's own attached clients (through their
  directive channel; `types::push_frame` would reach every client of the
  process).
* `router.rs`, `reply.rs`: before a one shot connection reaches the protocol
  handler, its opening (`AUTH`, optional `TARGET`, command line) is peeked; a
  command the shared server does not run is answered by the ONE reply helper as
  `ERROR: unknown command: <name>`, byte for byte the per session server's
  unknown command reply (owner instruction on #865: the future error flag change
  only touches `reply.rs`).

**Registry layout (settles #844 for shared mode).** Session files keep today's
names and contents, per session: `<base>.port` (the session's own listener),
`<base>.key` (its own key), `<base>.sid`, `<base>.pid` (the shared server's pid,
the same for all its sessions), with `<base>` = `<ns>__<session>` or `<session>`.
Because each session keeps its own port, `ls`, `attach`, `has-session`,
`kill-session`, `kill-server` (the first of its per `.port` targets ends the
process), `$TMUX` and older CLIs work unchanged, and `$TMUX` never needs a port
tie break. New: one namespace marker per shared server in a directory of its own,
`<data>/shared/default.server` or `<data>/shared/L-<ns>.server` (the `L-` prefix
keeps `-L default` apart from the default namespace), holding `mode`, `pid`,
`creation` (so a recycled pid is never taken for the server) and the `port`/`key`
of the server's control listener. A server writes the data dir's reaper ownership
marker only once a `.port` names it, so the orphan reaper cannot take it during
start up, and removes it before its last `.port`.

**What step 2 serves.** new-session (`-d -s -n -c -x -y -e -P -F -A`, a shell
command or `--` argv), new-window, split-window, kill-pane, kill-window,
select-window, select-pane, next/previous/last-window, last-pane, rename-window,
send-keys, capture-pane, display-message, list-windows, list-panes, ls,
has-session, kill-session, kill-server, set/show-options, set/show-environment,
set-hook/show-hooks, bind/unbind-key, list-keys, switch-client (`-t <session>`,
`-n`, `-p`, `-l`, `-T`: the attached client follows today's `SWITCH` directive to
the other session's own listener; a window or pane in another session is refused
as deferred), and a real attached client (dump-state, keys, paste, resize, focus,
detach). Ids `@N`/`%N` are unique and
dense across the server. Scoping as in tmux: `set -g`, `set -s`, `setw -g`,
`setenv -g`, `set-hook -g` and key bindings are server wide (delivered to every
session through its own listener and replayed on sessions created later; a
delivery the server makes itself is an echo and is not propagated again); session
options stay in their session. `show -s server-mode` answers `shared`.
kill-session ends one session; the server exits when its last session ends
(tmux with `exit-empty`). The per session `__warm__` standby is never claimed and
the shared server keeps no spare panes, so the first pane of every session is a
cold spawn. Pane shells inherit the server's environment as the launching shell
had it: the switch is never planted in it.

**Deferred to step 3 and later.** Control mode (`-C`/`-CC`); `switch-client` to a
window or pane of another session, `choose-tree` and the session chooser;
`rename-session`;
cross session move-window/join-pane/swap-pane/link-window and session groups
(`new-session -t` is refused); `new-session` from a hook, binding or prompt;
run-shell, if-shell, source-file, buffers, popups, menus and the rest of the
command set (refused with the unknown command reply); `update-environment` from a
later session's client (panes inherit the server's start environment); warm
spares inside the process; and per session status-interval hooks. A command run
from inside the loop (a hook, the config) that targets ANOTHER session of the
same server goes over TCP to that session's listener, which this same loop
serves, so it waits for its reply timeout; the server wide writes are delivered
from connection threads for exactly that reason. The config is loaded once per
session (as each per session server loads it), so a config's `run-shell` runs per
session.

**Known differences from the per session model that are visible.** A single
unsupported command answers `ERROR: unknown command: <name>`; whether the CLI then
exits 1 depends on that command's CLI path, exactly as for any `ERROR` reply
today. A crash that `catch_unwind` cannot catch (stack overflow, abort, a native
fault) ends every session of the namespace (section 8).

**Measured** (`tests/perf_issue798_shared_vs_per_session.ps1`, 5 rounds of three
`new-session -d` in a fresh namespace, Windows 11 26200, metrics JSON in
`~/.psmux-test-data/metrics/`):

| | per session | shared |
|---|---|---|
| first `new-session -d` (starts the server) | p50 232 ms | p50 93 ms |
| second and third `new-session -d` | p50 237 ms | p50 35 ms |
| server processes for 3 sessions | 3, plus the `__warm__` standby | 1 |
| server working set / private bytes | 48.1 MB / 12.7 MB, plus 15.1 MB standby | 9.9 MB / 3.3 MB |

Pane shells are separate processes in both modes and are not counted. The
shared server's later sessions are close to the cost of `new-window`, which is
what section 3.3 projected.

**Tests.** `tests/test_issue798_shared_server.ps1` (end to end, 102 checks),
`tests/test_issue798_server_mode_switch.ps1` (step 1's default path proof,
unchanged Part 1, against a master build), `tests-rs/test_issue798_shared_server.rs`
(allocator, router, reply helper, marker) and
`tests-rs/test_issue798_shared_server_requests.rs` (a panicking request ends only
its session and keeps the ids; each request runs with its own session entered).

What the later steps add in shared mode (the original stage 1 list):

* `new-session` in a namespace with a live shared server sends `new-session` to it.
  The server creates an `AppState` (session) with a spare pane from its in process
  pool. No standby process is involved for the second and later sessions.
* Registry: the server writes the same four files per session as today. `.port` and
  `.pid` are identical across its sessions, `.key` is **per session**, `.sid` is the
  session id. The server maps the key used in `AUTH` to the session. An old CLI, a
  plugin or a test that reads `S.port` and `S.key` reaches session S with no protocol
  change.
* `$TMUX` third field carries the session id (tmux parity), and
  `session_base_owning_tmux_port` uses it to break the port tie.
* Window and pane ids unique per server (stage 0a allocator, shared).
* `kill-session` ends one session; the process exits when the last session ends and
  `exit-empty` is on. Today it also exits with `exit-empty off` (#897 D): an empty
  shared server has no `.port` file for `kill-server` to find and would be exposed
  to the orphan reaper, so honouring the option waits for both of those (step 3).
  `kill-server` ends the shared server once:
  `kill_server_targets` dedupes by `(port, pid)`.
* Frames, directives and `DETACH` go only to the clients of the affected session.
* `%sessions-changed` is sent to every control client on create and destroy.
* `set -g`, `setenv -g`, buffers, key tables and global hooks become server wide,
  which is what tmux users expect and what section 3.3 shows is missing.
* Still refused in stage 1, with the current messages: cross session move-window,
  link-window, swap-pane; session groups keep today's behaviour.
* Mixed namespaces: a namespace can hold a shared server and older per session
  servers at the same time (left over from before the switch). Everything that
  enumerates `.port` files already handles many servers, so `ls`, `has-session`,
  `attach` and pickers see both kinds.

Gate: section 7 in full, twice (both modes), plus the shared mode only suites.

### Stage 2: cross session moves in shared mode

* `move-window -s A:2 -t B:` moves the `Window` between two `AppState`s in one
  process. Ids are kept.
* `join-pane`, `swap-pane`, `break-pane` between sessions of one shared server become
  in process tree moves. The proxy remains only for moves between different servers
  (different namespaces, or a mixed namespace), and only after its stage 0 bug fixes.
* Parity suites: tmux 3.4 transcripts of each command and its error strings.

### Stage 3: shared windows, groups, and in process switching

* Window storage moves to a server slab keyed by window id; a session holds winlinks
  `(index, window id)` with reference counts, as in tmux. This is the largest
  refactor (653 `app.windows` sites) and lands only after stage 2 has shipped.
* `link-window` and `unlink-window` with tmux semantics; `window_linked_sessions`
  becomes a real count.
* Session groups (`new-session -t`) share winlinks.
* `switch-client -t`, `-n`, `-p`, `-l` and `choose-tree` switch sessions inside the
  server; the `SWITCH` reconnect remains for targets on other servers.

### Stage 4: withdrawn (owner ruling, 2026-10-17)

There is no planned default flip. The text below is kept only as a record of the old
plan. If the owner ever reconsiders, these were the minimum conditions:

* Three consecutive releases with shared mode available, and the full sweep green in
  both modes on every release candidate.
* No open P1 or data loss issue that reproduces only in shared mode.
* Perf gates (section 9) green in shared mode on the reference machines.
* The owner signs off.

Then `server-mode` defaults to `shared`, and `PSMUX_SERVER_MODE=per-session` stays
supported for at least two more releases.

## 7. Test strategy

* **Both modes, full sweep.** `tests/run_all_tests.ps1` gains a `-ServerMode
  shared|per-session` switch that exports `PSMUX_SERVER_MODE` for every suite. CI
  runs both. The registry compatibility rule is what lets the 532 suites that read
  `.port` run unchanged.
* **Known mode dependent suites.** The 65 that name `__warm__` or `server -s` and the
  142 that count psmux processes assume one process per session. Each gets an explicit
  expectation per mode rather than a skip; a suite that silently passes in the wrong
  mode is worse than a failing one. The 93 that kill psmux by name must not run in
  parallel with anything (already a known hazard).
* **New shared mode suites.**
  * isolation: two sessions, `set -g`, `set`, `setenv -g`, buffers, bind, hooks, each
    checked from the other session against tmux's answers;
  * ids: 50 sessions x 4 windows x 3 panes, all `%N` and `@N` unique, `TMUX_PANE`
    resolves to the right session from inside each pane;
  * teardown: kill-session, exit-empty, destroy-unattached, kill-server, crash of one
    session (section 8) with the others still answering and typing;
  * registry: old CLI binary (3.4.0) against a shared server for `ls`, `attach`,
    `send-keys`, `kill-session`;
  * churn: 500 create/kill cycles, then zero leaked panes, conhosts, threads, handles;
  * plugins: resurrect save and restore, continuum, teammate backend (#580).
* **Parity vs tmux 3.4 in WSL.** One script drives the same command list through
  `tmux -L x` in WSL and `psmux -L x` and diffs the outputs: id formats, option
  inheritance, `list-sessions -F`, `move-window`, `link-window`, group behaviour,
  error strings. Extends the existing parity suites (19 `*parity*`/`tmux_*` files).
* **Unit tests.** `tests-rs` tests construct `AppState` directly (148 files); stage 0
  keeps that constructor working and adds a `ServerState` builder for multi session
  cases. Unit tests must not start servers (AGENTS.md).

## 8. Failure isolation

The risk the owner named. Today a crash ends one session; in tmux it ends all of
them. Shared mode must sit in between, and say honestly where it cannot.

* **Dispatch boundary.** Every request for a session runs inside
  `catch_unwind(AssertUnwindSafe(..))` together with that session's timer tick. A
  caught panic quarantines **that** session: append to `crash.log` with the session
  name and backtrace, kill its panes, remove its registry files, send its clients a
  `DETACH` with a reason, emit `%session-closed` / `%sessions-changed`. The other
  sessions keep running. Release profile keeps `panic = unwind` (it does today; there
  is no `panic` key in `Cargo.toml`).
* **Panic hook.** Today it deletes the registry files of the process's session. In
  shared mode it must only log; cleanup belongs to the quarantine path, otherwise a
  caught panic would unpublish every session.
* **Poisoned locks.** Pane parser mutexes are shared with reader threads. After a
  caught panic every `lock()` on that session's panes uses `into_inner` and the
  session is torn down anyway, so poisoned state is never read by another session.
* **What cannot be caught.** Stack overflow, out of memory, a panic during unwind,
  `abort`, and native faults in ConPTY or the console APIs end the process and every
  session in it. That is tmux's blast radius and is the residual risk of the mode.
  Mitigations: shared mode stays opt in; users who want isolation keep using one
  namespace per session, which stays a first class configuration; crash reports
  include the session that was being served.
* **Head of line blocking.** 19 blocking calls in the loop body (section 3.2) stall
  one session today and all sessions in shared mode. Stage 1 must move them off the
  loop (worker thread plus reply channel) and add a loop watchdog that logs any
  iteration over 200 ms with the request that caused it.
* **Per session state.** No session may hold a reference into another session's
  state except through ids resolved at use time; stage 3's window slab is the one
  place sessions share objects, and it is reference counted.

## 9. Performance gates

Existing gates, run in both modes, must not regress against per session mode on the
same machine and build:

| Gate | Suite | Baseline measured for this document |
|---|---|---|
| keystroke to screen | `tests/test_keystroke_latency_gate.ps1` | echo child median 1.90 ms, p99 2.63 ms |
| startup to prompt | `tests/bench_startup_latency.ps1`, `tests/probe_attach_latency.ps1` | not rerun here |
| new-session / new-window / split | `tests/test_creation_latency_gate.ps1` | 362 / 16 / 19 ms medians |
| burst typing | `tests/test_burst_typing_benchmark.ps1` | not rerun here |
| idle CPU, memory | perf metrics envelope (`perf_metrics_common.ps1`) | 15.7 to 18.2 MB working set per session server today |

Shared mode specific gates:

* keystroke latency in session A while session B runs a flood of output (like `yes`):
  A's p99 within 1 ms of A alone. This is the cost of sharing one loop and one
  `PTY_DATA_READY` wake, and the number most likely to regress.
* frame build cost scales with the attached session only, not with the total number
  of sessions in the server.
* `ls` and `list-panes -a` with 20 sessions: one connection, not 20.

## 10. Migration and compatibility

* **Registry format** unchanged in shape (section 6, stage 1). New: the namespace
  marker `<ns>.server` holding `mode`, `port`, `pid:creation_time`. Older psmux
  versions ignore it.
* **Older clients against a shared server** work because each session keeps its own
  `.key`, and the wire protocol is unchanged. The one behavioural difference an old
  client can see is server wide `set -g`, which is the tmux behaviour.
* **Newer CLI against older per session servers** keeps today's code paths; mode is
  per server, not per CLI.
* **Plugins** use the CLI only; the resurrect `%id` workaround becomes unnecessary
  but stays harmless.
* **Claude Code teammates** rely on bare `%N`; server unique ids make that safe in
  shared mode, and `TMUX_PANE` keeps pointing at the right pane.
* **Rollback.** Unset `PSMUX_SERVER_MODE` (or `set -s server-mode per-session`) and
  restart the namespace; nothing on disk needs converting.

## 11. The first contributor sized PR

**Superseded by the owner ruling.** The PR described below (stage 0a) was merged as
#838 and reverted, because it changed code every build runs. The first PR now is the
switch itself: read `PSMUX_SERVER_MODE` / `server-mode` where a server is about to be
started, with `per-session` (the default) taking exactly today's path, unchanged, and
`shared` entering new code of its own. A test must prove that with the switch unset
the default path is untouched (same ids, same files, same processes as master).

That switch PR is now done (stage 1, step 1 above). The hunks it adds to code a
default build runs, and nothing else:

1. `src/main.rs`: one gate call each at the top of `new-session`, `start-server`
   and the internal `server` entry, and one before the bare / `-CC` warm claim
   (only when no live session is being attached).
2. `src/commands.rs` and `src/server/connection.rs`: one gate each before the in
   server `new-session` claims the warm standby or spawns a server; on refusal it
   reports like the existing `duplicate session` branch beside it.
3. `src/server/option_catalog.rs`: `server-mode` as a validation only name, and a
   one line hand off in `validate_option_value` for that name.
4. `src/server/options.rs`: the `server-mode` arm in `get_option_value` (answers
   `per-session`) and in `apply_set_option` (no op for the running mode).
5. `src/server/mod.rs`: `mod shared;`.

With the switch unset each gate reads one environment variable and the config file
and returns; every other line of the default path is the old code.

Step 2, the minimal shared server inside `src/server/shared/`, is done too (see
"Step 2 (implemented)" in section 6), again with no hunk outside
`src/server/shared/` but the tests and this document.

Step 3, the next one, each item its own PR and still entirely behind the switch:

1. In process routing between the sessions of one shared server, so nothing run
   from inside the loop (a hook, a binding, the config, switch-client with a
   window or pane) has to reach a sibling session over TCP, which today waits on
   the loop serving it. With it: `new-session` from a hook, binding or prompt,
   `switch-client -t s:w.p`, `choose-tree` and the session chooser.
2. Control mode (`-C`/`-CC`) on the shared server, with `%sessions-changed` to
   every control client on create and destroy (section 6, stage 1 list).
3. The rest of the command set, each a copied arm with its test. Done so far
   (copied arms, `tests-rs/test_shared_dropped_requests.rs` and
   `tests/test_shared_dropped_requests.ps1`): select-layout, next-layout,
   previous-layout, rotate-window, swap-pane (directional, `{position}` and
   `-s`/`-t` within the session), select-pane by index, `select-pane -P`,
   paste-buffer, set-buffer, list-buffers, delete-buffer, save-buffer,
   load-buffer, clear-history and respawn-pane, from an attached client and as
   one shot commands. Buffers are server wide, as in tmux (#925): the server
   keeps one set and lends it to a session for each request, as it lends the
   id counters (`src/server/shared/buffers.rs`).

   Still refused. A one shot command gets `unknown command`; an attached client's
   binding, prompt or menu puts `not supported in shared server mode yet: <name>`
   on that session's status line instead of doing nothing (`refused.rs`):
   break-pane, join-pane, new-pane (floats), respawn-window, resize-window,
   pipe-pane, move-window, swap-window, link-window, unlink-window,
   find-window, rename-session (the registry rename), `new-session -t` (groups),
   choose-buffer, display-menu, display-popup, customize-mode, list-clients,
   detach-client (`-a`, `-s`, another client), lock-client, suspend-client,
   refresh-client, source-file, show-messages, show-window-options,
   list-commands, the prompt history commands, run-shell and wait-for. The
   requests the server sends itself (finished config jobs, cross session
   forwarding, control mode bookkeeping) are dropped silently, as before.
4. Spare panes inside the shared process, so a new session's first pane is not a
   cold spawn.
5. Then stage 2 of the plan: cross session move-window/join-pane/swap-pane as in
   process moves.

The original stage 0a text, for the record:

**Stage 0a: a server owned id allocator, no behaviour change.**

* Add `ServerIds { next_window: usize, next_pane: usize, next_forward: u64 }` with
  `alloc_window()`, `alloc_pane()`, `alloc_forward()`.
* Replace the 44 `next_win_id`/`next_pane_id` sites in 6 files and the
  `next_forward_id` use in `cross_session_server.rs` with calls on it. In this PR it
  still lives inside `AppState` (one field, `ids: ServerIds`) so nothing else moves.
* `healed_pane_ids` and the warm claim paths that renumber panes must call the same
  allocator.
* Tests: a `tests-rs` unit test that a fresh `AppState` hands out `@1`/`%1` first
  exactly as today, and that 1,000 interleaved window and pane allocations never
  repeat; the existing id related suites (`test_issue627_unqualified_targets.rs`,
  the #580 teammate suite) unchanged and green.
* Size: about 60 changed lines plus tests, no wire or file format change, trivially
  revertible.

It is the smallest step that every later stage needs, and the step 0g and stage 1
switch on top of.

## 12. Top risks

1. **Head of line blocking and shared wakes.** One slow request or one noisy pane
   degrades every session in the server. Mitigation: section 8 watchdog, worker
   threads, the cross session latency gate in section 9.
2. **Uncatchable crashes widen the blast radius** from one session to the namespace.
   Mitigation: opt in, catch_unwind quarantine, one namespace per session remains
   supported.
3. **Option and scope semantics change** (`set -g` becomes server wide; 54 session
   options move from flat fields to a chain). Visible to every config. Mitigation:
   stage 0d with parity diffs before anything is exposed.
4. **Routing ambiguity during the transition**: `$TMUX` port to session mapping,
   mixed namespaces, `kill-server` targets without dedupe, 18 `PSMUX_TARGET_SESSION`
   rewrite sites. Mitigation: session id in `$TMUX`, `(port, pid)` dedupe, mixed
   namespace suites.
5. **Teardown and leaks**: 12 exit paths become "end one session" paths, each of which
   used to rely on process exit to clean up threads, pipes, conhosts and statics keyed
   by pane id. Mitigation: the exit funnel (0f) and the churn suite.

## 13. What I could not determine

* Whether rebuilding an `HPCON` from duplicated handles in another process actually
  works end to end (resize, close, child exit). Only the handle facts in 4.3 were
  measured; the rebuild was not attempted.
* tmux 3.4 in WSL was not run for this document; the tmux column in 3.3 comes from
  reading the source, not from a transcript.
* Real new-session latency in a shared server (only projected from new-window).
* The cross session latency under load (section 9 gate) has no baseline yet, because
  no shared server exists to measure.
* Startup to prompt and burst typing gates were not rerun for this document.
* How many third party scripts outside `psmux-plugins` read registry files directly.
* Whether the `next_session_id` counter should stay data dir wide (today) or become
  per server (tmux); both satisfy uniqueness, the visible difference is the numbering.
* The keystroke runs were one machine, one build, 80 samples per cell; the 0.1 ms
  proxy delta is within run to run noise of the gate.
