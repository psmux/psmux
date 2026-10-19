use crate::tree::{adjust_split_sizes, compute_rects, compute_split_borders};
use crate::types::{DragState, LayoutKind, Node, Pane};
use ratatui::layout::Rect;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn leaf(id: usize) -> Node {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize {
        rows: 10,
        cols: 10,
        pixel_width: 0,
        pixel_height: 0,
    });
    Node::Leaf(Pane {
        master,
        writer,
        child: crate::util::StubChild::exited(),
        term: Arc::new(Mutex::new(vt100::Parser::new(10, 10, 0))),
        last_rows: 10,
        last_cols: 10,
        id,
        title: String::new(),
        title_locked: false,
        child_pid: None,
        data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: Instant::now(),
        last_infer_title: Instant::now(),
        dead: false,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None,
        win32_input_latched: false,
        input_off: false,
        scroll_fg_cache: None,
        mouse_proto_owner: None,
        wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None,
        live_term: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(Default::default())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    })
}

fn root(kind: LayoutKind, weights: &[u16]) -> Node {
    Node::Split {
        kind,
        sizes: weights.to_vec(),
        children: (0..weights.len()).map(leaf).collect(),
    }
}

fn weights(root: &Node) -> Vec<u16> {
    match root {
        Node::Split { sizes, .. } => sizes.clone(),
        _ => panic!("expected split"),
    }
}

fn rects(root: &Node, area: Rect) -> Vec<Rect> {
    let mut out = Vec::new();
    compute_rects(root, area, &mut out);
    out.into_iter().map(|(_, rect)| rect).collect()
}

fn drag(root: &Node, area: Rect, index: usize) -> DragState {
    let mut borders = Vec::new();
    compute_split_borders(root, area, &mut borders);
    let (path, kind, idx, pos, pixels) = borders
        .into_iter()
        .find(|(path, _, i, _, _)| path.is_empty() && *i == index)
        .unwrap();
    let sizes = weights(root);
    DragState {
        split_path: path,
        kind,
        index: idx,
        start_x: pos,
        start_y: pos,
        left_initial: sizes[index],
        _right_initial: sizes[index + 1],
        total_pixels: pixels,
    }
}

fn move_by(root: &mut Node, d: &DragState, delta: i32) {
    let pos = (i32::from(d.start_x) + delta) as u16;
    match d.kind {
        LayoutKind::Horizontal => adjust_split_sizes(root, d, pos, d.start_y),
        LayoutKind::Vertical => adjust_split_sizes(root, d, d.start_x, pos),
    }
}

#[test]
fn split_drag_moves_the_actual_border_by_cells_on_both_axes() {
    // Each case gives independently known initial cell lengths. Parent spans
    // include one separator per adjacent pair, not just pane content.
    let cases: &[(&[u16], &[u16], u16, usize, i32)] = &[
        (&[60, 59], &[60, 59], 120, 0, 1),
        (&[60, 29, 29], &[60, 29, 29], 120, 0, -1),
        (&[60, 29, 29], &[60, 29, 29], 120, 1, 1),
        (&[2, 2], &[20, 20], 41, 0, 10),
        (&[3, 2, 3], &[30, 20, 30], 82, 1, -10),
        (&[1000, 1000, 1000], &[100, 100, 100], 302, 0, 10),
        (&[40, 60], &[80, 120], 201, 0, -2),
    ];
    for kind in [LayoutKind::Horizontal, LayoutKind::Vertical] {
        let horizontal = matches!(kind, LayoutKind::Horizontal);
        for &(initial, lengths, span, index, delta) in cases {
            let area = if horizontal {
                Rect::new(17, 19, span, 40)
            } else {
                Rect::new(17, 19, 40, span)
            };
            let mut root = root(kind, initial);
            let before = rects(&root, area);
            assert_eq!(before.len(), initial.len());
            assert_eq!(
                before
                    .iter()
                    .map(|r| if horizontal { r.width } else { r.height })
                    .collect::<Vec<_>>(),
                lengths
            );
            let d = drag(&root, area, index);
            assert_eq!(d.total_pixels, span);
            move_by(&mut root, &d, delta);
            let after = rects(&root, area);
            let border = |r: Rect| i32::from(if horizontal { r.right() } else { r.bottom() });
            assert_eq!(
                border(after[index]) - border(before[index]),
                delta,
                "weights={initial:?}, horizontal={horizontal}"
            );
            let updated = weights(&root);
            assert_eq!(
                u32::from(updated[index]) + u32::from(updated[index + 1]),
                u32::from(initial[index]) + u32::from(initial[index + 1])
            );
            for i in 0..initial.len() {
                if i != index && i != index + 1 {
                    assert_eq!(updated[i], initial[i]);
                    assert_eq!(after[i], before[i], "unrelated sibling must not move");
                }
            }
            // Repeated events are relative to the press, not the last motion.
            move_by(&mut root, &d, delta);
            assert_eq!(rects(&root, area), after);
            move_by(&mut root, &d, 0);
            assert_eq!(weights(&root), initial);
            assert_eq!(rects(&root, area), before);
        }
    }
}

#[test]
fn split_drag_extremes_preserve_positive_weights_and_pair_sum() {
    for kind in [LayoutKind::Horizontal, LayoutKind::Vertical] {
        for initial in [vec![2, 2], vec![60, 29, 29], vec![60000, 60000], vec![1, 1]] {
            let area = Rect::new(0, 0, 120, 120);
            let mut root = root(kind, &initial);
            let d = drag(&root, area, 0);
            assert_eq!(weights(&root), initial);
            assert!(initial.iter().all(|&s| s > 0));
            let before = rects(&root, area);
            let border = |r: Rect| {
                if matches!(kind, LayoutKind::Horizontal) {
                    r.right()
                } else {
                    r.bottom()
                }
            };
            for pos in [0, u16::MAX] {
                adjust_split_sizes(&mut root, &d, pos, pos);
                let updated = weights(&root);
                assert!(updated.iter().all(|&s| s > 0));
                assert_eq!(
                    u32::from(updated[0]) + u32::from(updated[1]),
                    u32::from(initial[0]) + u32::from(initial[1])
                );
                assert_eq!(&updated[2..], &initial[2..]);
                let after = rects(&root, area);
                assert!(after.iter().all(|r| r.width > 0 && r.height > 0));
                if initial != [1, 1] {
                    if pos == 0 {
                        assert!(
                            border(after[0]) < border(before[0]),
                            "left bound must move the separator left"
                        );
                    } else {
                        assert!(
                            border(after[0]) > border(before[0]),
                            "right bound must move the separator right"
                        );
                    }
                } else {
                    assert_eq!(
                        after, before,
                        "a pair of unit weights cannot shrink further"
                    );
                }
                let once = weights(&root);
                adjust_split_sizes(&mut root, &d, pos, pos);
                assert_eq!(weights(&root), once);
            }
            move_by(&mut root, &d, 0);
            assert_eq!(weights(&root), initial);
        }
    }
}

#[test]
fn split_drag_extremes_keep_other_pane_rectangles_and_feasible_floors() {
    for kind in [LayoutKind::Horizontal, LayoutKind::Vertical] {
        let horizontal = matches!(kind, LayoutKind::Horizontal);
        let area = Rect::new(0, 0, 120, 120);
        for initial in [&[20, 20, 78][..], &[2000, 2000, 60000][..]] {
            let mut root = root(kind, initial);
            let before = rects(&root, area);
            let d = drag(&root, area, 0);
            assert_eq!(weights(&root), initial);
            assert!(before.iter().all(|r| r.width >= 2 && r.height >= 1));
            for pos in [0, u16::MAX] {
                adjust_split_sizes(&mut root, &d, pos, pos);
                assert_eq!(
                    rects(&root, area)[2],
                    before[2],
                    "an extreme drag must not steal cells from another sibling"
                );
                assert_eq!(weights(&root)[2], initial[2]);
            }
            move_by(&mut root, &d, 0);
            assert_eq!(weights(&root), initial);
        }
        let area = if horizontal {
            Rect::new(0, 0, 101, 40)
        } else {
            Rect::new(0, 0, 40, 101)
        };
        let mut root = root(kind, &[50, 50]);
        let d = drag(&root, area, 0);
        assert_eq!(weights(&root), [50, 50]);
        adjust_split_sizes(&mut root, &d, 0, 0);
        assert_eq!(
            weights(&root),
            [5, 95],
            "preserve the feasible five-percent floor"
        );
        adjust_split_sizes(&mut root, &d, u16::MAX, u16::MAX);
        assert_eq!(weights(&root), [95, 5]);
        let mut narrow = self::root(kind, &[2, 98]);
        let d = drag(&narrow, area, 0);
        assert_eq!(weights(&narrow), [2, 98]);
        move_by(&mut narrow, &d, 0);
        assert_eq!(
            weights(&narrow),
            [2, 98],
            "starting below the policy floor must not force growth"
        );
    }
}

#[test]
fn split_drag_ignores_orthogonal_motion_and_zero_content_span() {
    for kind in [LayoutKind::Horizontal, LayoutKind::Vertical] {
        let mut root = root(kind, &[20, 30, 40]);
        let mut d = drag(&root, Rect::new(0, 0, 92, 92), 1);
        assert_eq!(weights(&root), [20, 30, 40]);
        match kind {
            LayoutKind::Horizontal => adjust_split_sizes(&mut root, &d, d.start_x, u16::MAX),
            LayoutKind::Vertical => adjust_split_sizes(&mut root, &d, u16::MAX, d.start_y),
        }
        assert_eq!(weights(&root), [20, 30, 40]);
        for span in [0, 1, 2] {
            d.total_pixels = span; // Three children need two separators.
            adjust_split_sizes(&mut root, &d, u16::MAX, u16::MAX);
            assert_eq!(weights(&root), [20, 30, 40]);
        }
        for initial in [&[100, 0][..], &[0, 100][..], &[0, 0][..]] {
            let mut masked = self::root(kind, initial);
            let d = drag(&masked, Rect::new(0, 0, 120, 120), 0);
            assert_eq!(weights(&masked), initial);
            adjust_split_sizes(&mut masked, &d, u16::MAX, u16::MAX);
            assert_eq!(
                weights(&masked),
                initial,
                "dragging must preserve a zoom mask"
            );
        }
    }
}
