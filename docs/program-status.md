# Quiet program attention (OSC 7501)

Programs can report their state with [OSC 7501](https://www.superlogical.com/rex/docs/build/program-status), revision 0.4. psmux stores those reports separately from output monitoring. Enable [tab indicators](#opt-in-to-attention-indicators) for blocked tasks and unseen results:

| Indicator | Meaning |
|---|---|
| `[?%12]` | Pane `%12` needs input |
| `[!%12]` | Pane `%12` has an unseen error |
| `[+%12]` | Pane `%12` has an unseen completion |

Working and idle records have no indicator. Window attention includes tiled and floating panes, including panes retained after exit. Blocked takes priority over error, then done; the most recently changed record breaks ties. An identical retransmission does not change priority or make an acknowledged result unseen again.

Returning through pane selection or a window switch acknowledges the target pane's current done/error records. This acknowledgement is shared across clients and preserves the reported state for inspection. Blocked remains visible until the application changes it or its protocol lifetime ends. A result arriving while you stay in a pane remains unseen until you leave and return. On return, the server reads current records and marks them seen in one operation under the live parser lock. A result processed before that atomic acknowledgement can be seen; one processed afterward remains unseen. Copy mode continues to use live status behind its frozen screen.

## Opt in to attention indicators

The default tab formats match tmux and omit program attention. To show indicators, add these lines to your psmux configuration:

```tmux
set -g window-status-format '#I:#W#{?window_flags,#{window_flags}, }#{window_ps_attention}'
set -g window-status-current-format '#I:#W#{?window_flags,#{window_flags}, }#{window_ps_attention}'
```

For custom formats, append `#{window_ps_attention}` to your `window-status-format` and `window-status-current-format` values, including window-local overrides. The variable includes its leading space and expands to empty without pending attention.

## Reporting and lifetime

For example, a shell script can send these sequences (shown with shell escape syntax):

```sh
printf '\033]7501;state=working:app=build\033\\'
# Run the work, then report its result.
printf '\033]7501;state=done:app=build\033\\'
# Withdraw all records when they are no longer relevant.
printf '\033]7501;state=clear\033\\'
```

BEL is also accepted as a terminator. Programs can query support with `OSC 7501 ; ? ST`. psmux replies through the pane input pipe before a following primary-device-attributes reply. It consumes reports locally; it does not forward them to the outer terminal or produce desktop notifications.

The root record has no id. Children use slash-separated ids, replace their own record completely, and inherit app from their nearest ancestor with an explicit app. A hierarchical clear removes the addressed record and its descendants. Each pane holds at most 64 records, evicting the least recently updated record when full. Valid identical reports refresh eviction order without changing acknowledgement.

Primary/alternate screen switches and soft resets preserve records. Full reset (RIS) clears them. A new shell prompt (`OSC 133 A`) or attached-process exit removes idle, working, and blocked records; done/error survive. Respawning the pane clears its records and acknowledgement. Records are not persisted across server restarts.

Validation runs before record mutation and checks field limits, ids, base64, UTF-8, and decoded control characters. Completed reports use a 4096-byte dispatched-sequence budget, including standard OSC/terminator framing. Existing OSC intake behavior is unchanged, including ignored control bytes, termination semantics, and temporary buffering of unterminated sequences. This feature adds no general OSC or clipboard memory limit.

## Formats and inspection

| Format | Meaning |
|---|---|
| `pane_ps_state` | Root state |
| `pane_ps_kind` | Root blocked kind |
| `pane_ps_progress` | Root percentage; empty for indeterminate, `0` for zero |
| `pane_ps_app` | Root application identifier |
| `pane_ps_attention_state` | Selected attention state in the pane |
| `pane_ps_attention_id` | Selected record id; empty for the root or no attention |
| `window_ps_attention_state` | Selected attention state across the window |
| `window_ps_attention_pane` | Selected pane id, such as `%12` |
| `window_ps_attention` | Quiet text indicator |

Custom formats can show `#{pane_ps_state}` to distinguish `working` from `idle`; root formats do not select child records. Absent fields expand to empty. These names are psmux extensions, inspired by a tmux proposal; they do not imply shipped tmux OSC 7501 support.

The window objects in `dump-state` and JSON window listings include optional `ps_attention` and `ps_panes` fields. `ps_panes` contains pane ids and records with id, state, kind, progress, inherited app, title, msg, and seen. Titles and messages remain literal, untrusted text in JSON. They have no status-format variables and must never be treated as markup or commands by an inspection tool. Old clients can ignore the additive fields; old servers can omit them.

## Windows console detection

Certain versions of ConPTY swallow the OSC reply and answer DA1 themselves, so programs can fail to detect support even when psmux implements the protocol. With psmux's default console host, updating Windows Terminal alone does not replace the ConPTY version used for its panes.

Output monitoring remains available with its existing tmux-compatible semantics. For an attention-only setup, enable the tab indicators and disable `monitor-activity` and `monitor-silence` in your configuration.
