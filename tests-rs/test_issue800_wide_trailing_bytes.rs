// Issue #800 follow up: what style_wide_trailing_cells costs and what it must
// leave alone, measured on the bytes the client really writes
// (platform::draw_cells over ratatui's diff, the same path as PsmuxBackend).
//
// * A frame with no wide character must come out byte identical, so the pass
//   can never change what a plain ASCII session writes.
// * A first paint of a wide frame must also be byte identical: the trailing
//   cells are skipped by the diff, the extra style only feeds the comparison
//   of the NEXT frame.
// * Per frame cost on large clients, printed by the ignored bench test.

use super::*;
use ratatui::buffer::Buffer;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

fn frame(width: u16, rows: &[Vec<(String, Style)>], popup: Option<Rect>, fixed: bool) -> Buffer {
    let area = Rect::new(0, 0, width, rows.len() as u16);
    let mut buf = Buffer::empty(area);
    // One span per run, like client.rs render_layout_json does for rows_v2.
    let lines: Vec<Line> = rows
        .iter()
        .map(|runs| Line::from(runs.iter().map(|(t, s)| Span::styled(t.clone(), *s)).collect::<Vec<_>>()))
        .collect();
    Paragraph::new(lines).render(area, &mut buf);
    if let Some(p) = popup {
        Clear.render(p, &mut buf);
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(Color::Indexed(17)))
            .render(p, &mut buf);
    }
    if fixed {
        style_wide_trailing_cells(&mut buf);
    }
    buf
}

fn bytes(prev: &Buffer, next: &Buffer) -> Vec<u8> {
    let mut out = Vec::new();
    crate::platform::draw_cells(&mut out, prev.diff(next).into_iter()).unwrap();
    out
}

fn ascii_rows(width: u16, height: u16, seed: usize) -> Vec<Vec<(String, Style)>> {
    let styles = [
        Style::default(),
        Style::default().bg(Color::Indexed(22)).fg(Color::Indexed(231)),
        Style::default().add_modifier(Modifier::REVERSED),
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    ];
    (0..height as usize)
        .map(|r| {
            let mut runs = Vec::new();
            let mut used = 0usize;
            let mut k = r + seed;
            while used < width as usize {
                let n = (7 + (k * 5) % 13).min(width as usize - used);
                let text: String = (0..n).map(|i| (b'a' + ((k + i) % 26) as u8) as char).collect();
                runs.push((text, styles[k % styles.len()]));
                used += n;
                k += 3;
            }
            runs
        })
        .collect()
}

fn cjk_rows(width: u16, height: u16, seed: usize) -> Vec<Vec<(String, Style)>> {
    (0..height as usize)
        .map(|r| {
            let shift = (r + seed) % 2;
            let mut s = String::new();
            let mut used = 0;
            if shift == 1 { s.push('+'); used += 1; }
            let mut k = r + seed;
            while used + 2 <= width as usize {
                s.push(char::from_u32(0xAC00 + (k as u32 * 37) % 11000).unwrap());
                used += 2;
                if used < width as usize && k % 3 == 0 { s.push(' '); used += 1; }
                k += 1;
            }
            s.push_str(&" ".repeat(width as usize - used));
            let style = if seed % 2 == 0 {
                Style::default().bg(Color::Indexed(22)).fg(Color::Indexed(231))
            } else {
                Style::default()
            };
            vec![(s, style)]
        })
        .collect()
}

#[test]
fn ascii_frames_write_identical_bytes_with_and_without_the_pass() {
    let (w, h) = (120u16, 30u16);
    let popup = Some(Rect::new(30, 8, 40, 10));
    let frames: Vec<(Vec<Vec<(String, Style)>>, Option<Rect>)> = vec![
        (ascii_rows(w, h, 0), None),
        (ascii_rows(w, h, 1), None),
        (ascii_rows(w, h, 1), popup),
        (ascii_rows(w, h, 2), popup),
        (ascii_rows(w, h, 2), None),
        (ascii_rows(w, h, 2), None),
    ];
    let mut prev_plain = Buffer::empty(Rect::new(0, 0, w, h));
    let mut prev_fixed = prev_plain.clone();
    let mut total = 0usize;
    for (rows, pop) in &frames {
        let plain = frame(w, rows, *pop, false);
        let fixed = frame(w, rows, *pop, true);
        assert_eq!(plain, fixed, "the pass changed a buffer with no wide character");
        let a = bytes(&prev_plain, &plain);
        let b = bytes(&prev_fixed, &fixed);
        assert_eq!(a, b, "ASCII frame bytes differ with the pass");
        total += a.len();
        prev_plain = plain;
        prev_fixed = fixed;
    }
    assert!(total > 1000, "the frame sequence wrote almost nothing ({total} bytes)");
}

#[test]
fn first_paint_of_a_wide_frame_writes_identical_bytes() {
    let (w, h) = (80u16, 12u16);
    let blank = Buffer::empty(Rect::new(0, 0, w, h));
    for seed in 0..4 {
        let rows = cjk_rows(w, h, seed);
        let popup = if seed >= 2 { Some(Rect::new(11, 3, 30, 6)) } else { None };
        let a = bytes(&blank, &frame(w, &rows, popup, false));
        let b = bytes(&blank, &frame(w, &rows, popup, true));
        assert_eq!(a, b, "seed {seed}: the pass changed what a full paint writes");
    }
}

#[test]
fn wide_frame_after_wide_frame_only_adds_writes() {
    // The pass may make the diff write MORE cells (that is the fix), never
    // fewer: every cell master writes is still written with the pass.
    let (w, h) = (80u16, 12u16);
    for seed in 0..4 {
        let first = cjk_rows(w, h, seed);
        let second = cjk_rows(w, h, seed + 1);
        let p0 = frame(w, &first, None, false);
        let p1 = frame(w, &second, None, false);
        let f0 = frame(w, &first, None, true);
        let f1 = frame(w, &second, None, true);
        let plain: Vec<(u16, u16, String)> = p0.diff(&p1).into_iter().map(|(x, y, c)| (x, y, c.symbol().to_string())).collect();
        let fixed: Vec<(u16, u16, String)> = f0.diff(&f1).into_iter().map(|(x, y, c)| (x, y, c.symbol().to_string())).collect();
        for cell in &plain {
            assert!(fixed.contains(cell), "seed {seed}: {cell:?} is no longer written");
        }
    }
}

/// The pass as PR #801 first wrote it (unicode-width on every cell), kept as
/// the reference the faster version must agree with.
fn reference_pass(buf: &mut Buffer) {
    use ratatui::buffer::CellWidth;
    let width = (buf.area.width as usize).max(1);
    let mut index = 0;
    while index < buf.content.len() {
        let row_end = (index / width + 1) * width;
        let end = (index + (buf.content[index].cell_width() as usize).max(1)).min(row_end);
        let style = buf.content[index].style();
        for hidden in buf.content[index + 1..end].iter_mut().filter(|cell| cell.symbol() == " ") {
            hidden.set_style(style);
        }
        index = end;
    }
}

#[test]
fn fast_pass_matches_the_reference_pass() {
    let mut mixed = cjk_rows(97, 9, 0);
    mixed.extend(ascii_rows(97, 5, 3));
    // Emoji, VS16, combining marks and a wide char in the last column.
    mixed.push(vec![("a\u{1F600}b\u{2764}\u{FE0F}e\u{301} \u{4E2D}".to_string(), Style::default().bg(Color::Blue))]);
    let mut last = " ".repeat(96);
    last.push('\u{4E2D}');
    mixed.push(vec![(last, Style::default().bg(Color::Red))]);
    for popup in [None, Some(Rect::new(5, 2, 20, 6)), Some(Rect::new(6, 3, 21, 7))] {
        let mut a = frame(97, &mixed, popup, false);
        let mut b = a.clone();
        style_wide_trailing_cells(&mut a);
        reference_pass(&mut b);
        assert_eq!(a.content, b.content, "popup {popup:?}");
        assert!(a.content.iter().zip(b.content.iter()).all(|(x, y)| x.style() == y.style()));
    }
}

#[test]
#[ignore]
fn bench_style_wide_trailing_cells() {
    for (w, h) in [(300u16, 80u16), (500, 150)] {
        for (label, rows) in [("ascii", ascii_rows(w, h, 0)), ("cjk", cjk_rows(w, h, 0))] {
            // A hot buffer reused every frame, as the client's is.
            let mut buf = frame(w, &rows, None, false);
            let iters = 500;
            for (name, pass) in [("fast", style_wide_trailing_cells as fn(&mut Buffer)), ("reference", reference_pass)] {
                pass(&mut buf);
                let t = std::time::Instant::now();
                for _ in 0..iters {
                    pass(std::hint::black_box(&mut buf));
                }
                let per = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
                eprintln!("BENCH {w}x{h} {label} {name}: {per:.1} us per frame");
            }
            // For scale: what the frame itself costs (Paragraph render of the
            // runs plus the diff against a blank screen).
            let blank = Buffer::empty(buf.area);
            let t = std::time::Instant::now();
            for _ in 0..50 {
                let f = frame(w, &rows, None, false);
                std::hint::black_box(blank.diff(&f).len());
            }
            let per = t.elapsed().as_secs_f64() * 1e6 / 50.0;
            eprintln!("BENCH {w}x{h} {label} render+diff: {per:.1} us per frame");
        }
    }
}
