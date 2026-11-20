# Design: raw forwarding for `allow-passthrough on` and `all`

Status: implemented for normal attached clients in the legacy and shared server.

Only an explicit tmux passthrough reaches the host terminal, as in tmux 3.8.
A pane's own notification OSCs (OSC 9 text, `777;notify`, OSC 99, OSC 7501,
OSC 9001) are not forwarded, whatever `allow-passthrough` says: tmux's
`input_exit_osc` (input.c) acts on OSC 9;4 progress and drops every other
OSC 9 and every OSC it does not know. A program that wants a desktop
notification inside tmux or psmux wraps it, `ESC P tmux; ESC ESC ]9;text BEL
ESC \`, and the user opts in with `set -g allow-passthrough on`. With the
default `off` nothing a pane writes is sent raw to the host terminal.

OSC 99 identifiers share the host terminal's namespace. Programs in visible
split panes should use distinct identifiers; psmux does not rewrite IDs or
merge chunks from separate panes. Visibility is evaluated when the server
drains pane output; hidden and detached events are discarded, not replayed
on a later window switch or attach.

## What tmux does

A program writes `ESC P tmux; <body with every ESC doubled> ESC \`.

- tmux keeps the whole DCS: in `dcs_handler` an ESC moves to `dcs_escape`,
  where any byte but `\` goes back into the buffer and `\` dispatches
  (input.c:683-702). A doubled ESC becomes one ESC.
- At the ST, `input_dcs_dispatch` returns early when `allow-passthrough` is
  off (input.c:2672-2674). Otherwise it calls
  `screen_write_rawstring(sctx, buf + 5, len - 5, allow_passthrough == 2)`
  (input.c:2676-2680).
- `screen_write_rawstring` (screen-write.c:3029-3041) hands the bytes to
  `tty_write(tty_cmd_rawstring, ...)`, which writes them to every client
  whose tty is ready and that shows the pane. `tty_client_ready`
  (tty.c:1610-1629) skips the redraw and freeze checks only with
  `TTY_CTX_INVISIBLE_PANES`, which `all` sets: `on` reaches clients that show
  the pane, `all` reaches every client of the session.
- `tty_cmd_rawstring` (tty.c:2151-2156) writes the bytes and then calls
  `tty_invalidate`, so the next redraw resends cursor position and
  attributes.
- The pane's own screen never sees the body.
- Control clients have no tty (`server_client_open` returns early for them,
  server-client.c:328), so this path never reaches them. They get the pane's
  raw bytes in `%output`. psmux sends them the body unwrapped there (#1026),
  which is unchanged by this design.
- An unterminated DCS is bounded by `input-buffer-size` (past it the data is
  discarded, input.c:1289) and by the 5 second ground timer started when the
  DCS began (input.c:826-832, 2547).

## Implementation

1. **Parser.** The tmux passthrough skipper collects unescaped bodies only while enabled;
   the pane screen never interprets them. Fragmented reads, doubled inner ESC,
   lost ConPTY closing ST recovery and the existing five second DCS timeout
   retain their parser behavior. A complete single allowlisted OSC ending in
   BEL is committed immediately, even if ConPTY drops the outer ST and the
   writer goes silent. It is removed from the collected body, so a later ST
   cannot replay it. This commits a notification prefix before the parser can
   know whether generic content follows. A later suffix waits for normal
   wrapper completion/recovery and can be dropped on timeout, overflow or
   disabling passthrough without recalling the already delivered prefix.
   Bodies with generic content before a notification are not committed early.
2. **Server.** Each pane has an ordered consume-once event queue. The server
   drains the live parser during copy mode; snapshots contain no pending events.
   Passthrough `on` forwards from visible panes (active window, zoom aware),
   `all` permits hidden panes too, and `off` drops passthrough bodies. A
   process wide pending count lets the server skip the per pane drain (and
   its parser locks) whenever nothing is queued, which is always the case
   with `off`.
   Tiled, floating and popup panes are covered. Events are discarded when the
   session has no registered normal client; control clients and normal clients
   in another shared-server session cannot retain events for a later attach.
   Warm-pane claim reconciliation also discards its queued startup events.
3. **Frames.** Exact bytes travel as a base64 array in one-shot `host_events`,
   with `host_raw` requesting cache invalidation for arbitrary passthrough.
   Clipboard, bell and image one-shots retain their existing semantics.
   Every frame superseding location carries events forward in order. A legacy
   dump-state reply absorbs the requester's older unread events before its new
   events, and its confirming pushed screen omits those one-shots. Other clients
   retain their own ordered events. Cached screen snapshots contain no events,
   so redraw cannot replay them.
4. **Client.** Event bytes are written once to stdout even when the screen
   snapshot is unchanged. A raw passthrough write resets attributes and the
   open hyperlink and invalidates the screen before the next redraw.
5. **Bounds.** Each event is limited by the smaller of `input-buffer-size`
   and 1 MiB. Pane, staged frame and coalesced client queues each hold at most
   256 events and 1 MiB of raw bytes. When full, newer complete events are
   discarded; no event is byte-truncated. These are transport bounds, so a
   dropped OSC 99 chunk can leave the producer's notification incomplete.
6. **Control clients.** Unchanged: `%output` carries unwrapped pane bytes;
   normal frame side effects are not sent through the control stream.

## Gaps and risks

- ConPTY can change bytes before psmux sees them. In direct control and raw
  pane-reader captures, the tested inbox console host discarded whole
  ST-terminated wrapped OSCs, unwrapped generic ANSI with normalized resets,
  and retained BEL-terminated wrapped OSCs while dropping their outer ST.
  Forwarding only applies to bytes received by psmux; an option cannot recover
  a wrapper the console host already consumed. Regression captures must use
  the direct control and raw pane ingress to distinguish these cases.
- tmux's option is per window and per pane (options-table.c:1257-1259). psmux
  keeps one value per server (`app.allow_passthrough`, applied to the parsers
  through `vt100::set_allow_passthrough`). In shared server mode the value is
  one per process for every session in it.
- Images: an iTerm2 `OSC 1337 File=` or a kitty graphics body draws at the
  outer terminal's cursor. tmux writes it wherever its tty cursor is at that
  moment, and so would the client; neither places it on the pane exactly.
- Frame size: a body of up to 1 MiB becomes about 1.4 MiB of base64 in one
  frame. The frame one shot parser reads from the end of the frame, so it
  needs the same quoting care as `clipboard_osc52`.
- Remote and ssh attached clients take the same frame path.

## Measurements to take

- A/B with alternate builds, several runs: frame build time and the client
  main loop with `allow-passthrough off` (expected no change) and `on` under
  a pane that writes a passthrough per frame.
- E2E: an attached client inside a ConPTY capture (as in the #1026 review
  harness) shows the body once with `on` from the active pane, not from a
  pane in another window, and once with `all` from either; never in `%output`
  twice; never with `off`.
