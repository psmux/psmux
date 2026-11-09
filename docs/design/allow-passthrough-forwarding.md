# Design: raw forwarding for `allow-passthrough on` and `all`

Status: proposal, not implemented. Today `off` matches tmux (the passthrough
is dropped) and `on`/`all` read the wrapped sequence as pane output; see
`allow-passthrough` in [../compatibility.md](../compatibility.md). This page
describes the step that would make `on` and `all` behave like tmux.

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

## Proposed psmux implementation

1. **Parser (vt100-psmux).** `src/passthrough.rs` already skips the body of a
   passthrough before vte sees it and ends it at `ESC \`, at a lone ESC or at
   text after a BEL (the inbox conhost drops the closing ST, #1026), or after
   5 seconds. For forwarding, the skipper also collects the body with each
   `ESC ESC` folded to one ESC, up to `input-buffer-size` (over that the
   wrapper is dropped, like tmux's discard). At the wrapper's end the body is
   queued on the `Screen` (bounded queue, total bytes held to
   `input-buffer-size`). Collection only runs while the option is `on` or
   `all`, so `off` costs nothing. With forwarding in place the parser no longer
   reads the body as pane output for `on`/`all` either, which is the tmux
   behaviour; the current "read it as pane output" opt in goes away.
2. **Server.** In `helpers::take_frame_oneshots`, drain every pane's queue.
   `on`: forward bodies only from panes that are visible (in the active window
   and not hidden by a zoomed pane). `all`: forward from any pane of the
   session. With no frame receiver the bodies are dropped, as tmux writes only
   to clients that exist.
3. **Frame.** A new one shot field, `"passthrough":"<base64>"`, written by
   `frame_oneshot::append` with the clipboard and bell fields and read back by
   `take`. `merge_newer` and `supersede` concatenate bodies in order, with the
   same byte bound, so a superseded frame does not lose them. The shared mode
   frame builder (`server/shared/frame.rs`) gets the same field behind
   `PSMUX_SERVER_MODE`.
4. **Client.** Where the clipboard and bell one shots are written, write the
   decoded bytes raw to stdout, then force the next draw to resend cursor and
   attributes (the equivalent of `tty_invalidate`).
5. **Control clients.** Unchanged: `%output` carries the unwrapped body once;
   frames never go to control clients, so nothing is delivered twice.

## Gaps and risks

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
