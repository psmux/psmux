// Issue #841: Escape arrives late in nvim when tmux runs inside a psmux pane
// (psmux -> wsl -> tmux -> nvim), whatever escape-time is set to.
//
// tmux 3.8 asks its outer terminal `CSI 18 t CSI 14 t` whenever the tty has
// no pixel size (tty.c:146-150), which a WSL pty never has, and holds every
// lone Escape for 500 ms while that question is open (tty-keys.c:980-987).
// The inbox console host answers `CSI 18 t` itself but forwards `CSI 14 t`
// (PSMUX_PANE_RAW capture on 26200: `ESC[14t` present, `ESC[18t` absent), and
// psmux answered nothing, so the question stayed open forever.  Measured with
// tmux 3.8 in WSL Ubuntu: every Escape 555 to 571 ms late before this change.
//
// tmux parity, input_csi_dispatch_winops (input.c:2163-2186 in tmux 3.8):
//   14 -> "\033[4;%u;%ut"  y * ypixel, x * xpixel
//   15 -> "\033[5;%u;%ut"  y * ypixel, x * xpixel
//   16 -> "\033[6;%u;%ut"  ypixel, xpixel
//   18 -> "\033[8;%u;%ut"  y, x
//   19 -> "\033[9;%u;%ut"  y, x
// with xpixel/ypixel defaulting to 16/32 when unknown (window.c:406-409).
// psmux answers with the 10 by 20 pixel cell its panes lay sixel images out
// in (vt100::image::DEFAULT_CELL_PX), as tmux answers with the cell it parses
// images in (input.c:2654).
// 18 and 19 (the text area in cells) reach psmux only from a host that leaves
// them unanswered: the inbox console host answers them itself and does not
// forward them, an OpenConsole forwards them with no answer, and chafa then
// sized its image for 80 by 24 cells.

use super::*;

fn sized(rows: u16, cols: u16) -> DecrqmState {
    DecrqmState { rows, cols, cursor_visible: true, ..DecrqmState::default() }
}

fn reply_for(bytes: &[u8], st: &DecrqmState) -> String {
    scan_device_queries(bytes)
        .into_iter()
        .map(|q| device_reply(q, st))
        .collect()
}

#[test]
fn tmux_38_size_probe_draws_one_answer_each() {
    // The exact bytes tmux 3.8 wrote to its tty, from its -vv server log.
    // Under the inbox console host only the `CSI 14 t` reaches psmux.
    assert_eq!(
        scan_device_queries(b"\x1b[18t\x1b[14t"),
        vec![DeviceQuery::WindowReport(18), DeviceQuery::WindowReport(14)]
    );
    assert_eq!(
        reply_for(b"\x1b[18t\x1b[14t", &sized(29, 120)),
        "\x1b[8;29;120t\x1b[4;580;1200t"
    );
}

#[test]
fn csi_14t_reports_pixel_size_like_tmux() {
    assert_eq!(reply_for(b"\x1b[14t", &sized(29, 120)), "\x1b[4;580;1200t");
}

#[test]
fn csi_14t_reply_is_one_tmux_38_accepts() {
    // tty_keys_winsz (tty-keys.c) clears TTY_WINSIZEQUERY only on a reply that
    // sscanf("4;%u;%u") accepts; anything else leaves Escape held.
    let reply = reply_for(b"\x1b[14t", &sized(40, 100));
    let body = reply.strip_prefix("\x1b[").and_then(|r| r.strip_suffix('t')).unwrap();
    let parts: Vec<u32> = body.split(';').map(|p| p.parse().unwrap()).collect();
    assert_eq!(parts, vec![4, 40 * 20, 100 * 10]);
}

#[test]
fn every_size_report_tmux_answers() {
    let st = sized(24, 80);
    assert_eq!(reply_for(b"\x1b[15t", &st), "\x1b[5;480;800t");
    assert_eq!(reply_for(b"\x1b[16t", &st), "\x1b[6;20;10t");
    assert_eq!(reply_for(b"\x1b[18t", &st), "\x1b[8;24;80t");
    assert_eq!(reply_for(b"\x1b[19t", &st), "\x1b[9;24;80t");
}

#[test]
fn chafa_size_probe_is_answered_in_the_pane_size() {
    // chafa 1.18 asks these four when it starts.  With no
    // `CSI 18 t` answer it took 80 by 24 cells and drew a 360 by 480 pixel
    // image into a 40 by 12 pane.
    assert_eq!(
        reply_for(b"\x1b[1t\x1b[18t\x1b[14t\x1b[16t", &sized(12, 40)),
        "\x1b[8;12;40t\x1b[4;240;400t\x1b[6;20;10t"
    );
}

#[test]
fn window_manipulation_that_is_not_a_report_draws_nothing() {
    // tmux pushes and pops titles with 22/23 at startup and exit, and
    // OpenConsole opens every pane with `CSI 1 t`.  None of them is a question.
    let st = sized(24, 80);
    for q in [&b"\x1b[22;0;0t"[..], b"\x1b[23;0;0t", b"\x1b[1t", b"\x1b[8;30;100t", b"\x1b[t"] {
        assert_eq!(reply_for(q, &st), "", "{q:?}");
    }
}
