# Live Preview in Choosers

The `choose-session` and `choose-tree` pickers in psmux include a live preview pane that shows the actual content of the highlighted session, window, or pane. The preview updates as you move the selection, so you can see what is in each window before switching.

## Quick Start

Open a chooser:

* **prefix + s** opens `choose-session` (sessions list).
* **prefix + w** opens `choose-tree` (sessions, windows, panes hierarchy).

Inside a chooser:

* Press **p** to toggle the preview pane on or off.
* Use the **arrow keys**, **j/k**, or **h/l** to move the selection. The preview updates automatically.
* Press **g** to jump to the top, **G** to jump to the bottom (matches tmux `mode-tree`).
* Type a **number** (e.g. `3`) to start a digit-jump buffer shown at the bottom as `go to 3`, then press **Enter** to jump to that row (1-based). **Backspace** edits the buffer; **Esc** cancels. Every row is prefixed with its number so the mapping is always visible.
* Press **Enter** with no digit buffer to switch to the arrow-cursor selection.
* Press **Esc** or **q** to close.

Full hjkl + g/G navigation and digit-jump also work in `choose-buffer` (prefix + =), the keybindings viewer (prefix + ?), and `customize-mode`.

## Make the Preview Visible by Default

By default the preview is hidden and you press `p` to show it. To open every chooser with the preview already visible, add the following to your psmux configuration file (`~/.psmux.conf` or `%USERPROFILE%\.psmux.conf`):

```tmux
set -g choose-tree-preview on
```

You can also set it interactively from any psmux pane:

```powershell
psmux set -g choose-tree-preview on
```

To turn it off again:

```tmux
set -g choose-tree-preview off
```

The option is read each time a chooser opens, so a change takes effect on the next `prefix + s` or `prefix + w`.

You can verify the current value with:

```powershell
psmux show-options -g | Select-String choose-tree-preview
```

Inside the chooser, `p` always toggles the preview for the current session regardless of the option. The option only controls the initial state when the chooser opens.

## How the Preview Renders

The preview pane is fed by the same renderer that draws the main viewport. Each open chooser fetches a JSON dump of the target window using the internal `window-dump` TCP command. The dump includes per-cell text, foreground and background colours, and style flags (bold, underline, italic, reversed, etc.) for every visible row of every pane in that window.

That dump is then drawn into the preview area using `render_layout_json`, the same function that draws the live psmux viewport. The result is a styled miniature of the serialized pane content, including:

* Pane borders, including their colours and the active pane highlight.
* Pane title bars and status indicators.
* Foreground and background colours from any TUI program running in the pane.
* Bold, italic, underline, reversed, dim, blink, and strikethrough attributes.
* True-colour (24-bit) and 256-colour palettes.
* Wide characters (CJK).

The preview is updated on a short cache window (about 1.5 seconds) so navigating quickly through a long session list does not flood the network with dump requests, but content still appears live for a steady selection.

## How psmux Handles Size Differences

Real panes are usually much larger than the preview area. For example, a 200x50 pane being shown inside a 60x25 preview slot. A naive scaler would either drop characters or distort the 2D grid that TUI applications rely on (htop, vim, less, pstop, etc.). psmux deliberately does not rescale.

Instead, the preview shows the pane at one to one with two simple rules:

1. **Bottom rows win.** Any trailing fully blank rows are trimmed first so that a shell prompt or the bottom edge of a TUI sits at the bottom of the preview rather than being scrolled off by empty viewport space. The bottom rows of what remains are then shown.
2. **Columns clip naturally.** Cells that fall outside the preview width are not drawn. The grid stays pixel accurate, so column aligned output (process tables, file listings, source code) keeps its alignment.

The trade off is that very wide content is cut on the right edge instead of being squeezed in. In practice this matches what tmux itself does in `choose-tree` previews and is much more useful than a scrambled "scaled" view.

If the preview area is the same size as the pane (rare), it shows the pane one to one with no clipping at all.

## Differences from tmux

psmux aims to keep the preview feature on par with tmux, with a few intentional differences listed below.

### Things that match tmux

* `choose-session` and `choose-tree` both have a preview pane.
* `p` toggles the preview while a chooser is open.
* The preview is a live mirror of the target, not a frozen snapshot.
* Serialized cell colours and styles, plus pane-border geometry, are preserved.
* Wide characters are handled correctly.
* The preview width is roughly half the popup width, with the picker list on the left.
* The preview never modifies the target session in any way (it is read only).

### Things that differ

* **`choose-tree-preview` option.** Standard tmux does not have an option to make the preview visible by default. You must press `p` every time. psmux adds the `choose-tree-preview` option (default `off`, matching tmux behaviour) so you can opt in to a preview that is always visible.
* **Render fidelity.** psmux uses its own `window-dump` snapshot pipeline rather than tmux's `capture-pane` text. This carries full per-cell styling (24-bit colour, all SGR attributes) into the preview, so a preview of a Powerline prompt or a syntax-highlighted file looks right rather than being plain text.
* **Resize behaviour.** tmux scales / squeezes the preview content when the pane is wider than the preview slot, which can produce visually surprising results for column aligned output. psmux clips at one to one as described above. The result is that long lines or wide TUIs are cropped on the right edge in psmux but stay perfectly aligned, while in tmux they may be scaled but mis aligned.
* **Cache window.** psmux caches the preview dump for about 1.5 seconds. tmux re-renders on every selection change. The psmux behaviour reduces network traffic when scrolling through many sessions but a very recent change to a target may take up to 1.5 seconds to appear in the preview.
* **Movable popup.** The chooser popup itself can be dragged with the mouse in psmux. Standard tmux choosers are fixed in place. The preview pane moves with the popup.

### Compatibility notes

* The option name `choose-tree-preview` is psmux specific. tmux does not recognise it. Adding it to a shared configuration file is safe because tmux's set-option command will warn but not fail; if you want to be strict, guard the line with `if-shell` or split your config.
* The option key in `show-options` output and in the JSON sent to the client uses kebab-case (`choose-tree-preview`) and snake_case (`choose_tree_preview`) respectively, matching the existing psmux convention.
* The preview uses the target's serialized cell styles and the chooser's pane-border styles. The `window-dump` response does not include target `window-style` or `window-active-style` options.

## Performance

The preview path is cheap: it shares the same dump cache between the main viewport and the chooser, so opening the preview adds at most one extra `window-dump` request per cached interval (about 1.5 seconds). Rendering is done client side using the existing renderer, so there is no extra server work for each frame after the dump is fetched.

If you have very many sessions and the chooser feels slow, that is almost always due to scanning many session port files in `~/.psmux/`, not the preview itself. The preview only fetches the dump for the currently highlighted target.

## Troubleshooting

**The preview shows an empty box.**
The target window may not have responded yet. Move the selection away and back, or wait about 1.5 seconds for the cache to expire.

**Long lines are cut off on the right.**
This is by design. See "How psmux Handles Size Differences" above. If you want to see the full content, switch to the target with Enter.

**The preview text looks fine but the borders are missing.**
Check that `pane-border-style` is set to a non empty value. Empty styles render as transparent which can hide borders against the popup background.

**Setting `choose-tree-preview on` does not seem to take effect.**
The option is read when the chooser opens, not while it is open. Close the chooser with Esc and reopen it. Verify the option is set with `psmux show-options -g | Select-String choose-tree-preview`.

## Related Options and Commands

* `pane-border-style`, `pane-active-border-style`: control how borders look in both the main view and the preview.
* `mode-style`: controls how the selected entry in the chooser list is highlighted.
* `mouse on`: enables clicking entries in the chooser list and dragging the popup.

## See Also

* [configuration.md](configuration.md) for the full options reference.
* [keybindings.md](keybindings.md) for the default keys that open the choosers.
* [features.md](features.md) for the broader feature overview.
