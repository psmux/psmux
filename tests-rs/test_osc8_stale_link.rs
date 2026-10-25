use std::cell::RefCell;
use std::rc::Rc;

use ratatui::buffer::{Buffer, CellDiffOption, CellWidth};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

use crate::client::HyperlinkRun;
use crate::layout::LayoutJson;
use crate::platform::PsmuxBackend;

const COLUMNS: u16 = 40;
const ROWS: u16 = 4;
const URI: &str = "https://example.com/mcp";
const LINKED_LINE: &[u8] =
    b"Learn more in the \x1b]8;;https://example.com/mcp\x1b\\MCP documentation\x1b]8;;\x1b\\.";
const PLAIN_LINE: &[u8] = b"Learn more in the MCP documentation.";
const LINK_TEXT: &str = "MCP documentation";
const LINK_START: u16 = 18;

#[derive(Clone, Default)]
struct Recorder {
    bytes: Rc<RefCell<Vec<u8>>>,
}

impl std::io::Write for Recorder {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.bytes.borrow_mut().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

type LinkedCell = (u16, u16, String, String);

struct Client {
    recorder: Recorder,
    terminal: Terminal<PsmuxBackend<Recorder>>,
}

struct FrameResult {
    runs: Vec<HyperlinkRun>,
    buffer: Buffer,
}

impl Client {
    fn new() -> Self {
        let recorder = Recorder::default();
        let terminal = Terminal::with_options(
            PsmuxBackend::new(recorder.clone()),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, COLUMNS, ROWS)),
            },
        )
        .unwrap();
        crate::client::previous_hyperlink_rows_set(Vec::new());
        Self { recorder, terminal }
    }

    fn frame(&mut self, draw: impl FnOnce(&mut Frame)) -> FrameResult {
        let mut drawn = None;
        self.terminal.backend_mut().begin_frame();
        crate::client::frame_hyperlinks_clear();
        self.terminal
            .draw(|frame| {
                draw(frame);
                crate::rendering::style_wide_trailing_cells(frame.buffer_mut());
                crate::client::redraw_hyperlink_rows(
                    frame.buffer_mut(),
                    &crate::client::previous_hyperlink_rows_get(),
                );
                drawn = Some(frame.buffer_mut().clone());
            })
            .unwrap();
        let runs = crate::client::frame_hyperlinks_take();
        crate::client::previous_hyperlink_rows_set(crate::client::hyperlink_rows(&runs));
        if !runs.is_empty() {
            let overlay = crate::client::build_osc8_overlay(&runs);
            self.terminal
                .backend_mut()
                .queue_drawing(overlay.as_bytes())
                .unwrap();
        }
        self.terminal.backend_mut().request_cursor(None);
        self.terminal.backend_mut().end_frame().unwrap();
        FrameResult {
            runs,
            buffer: drawn.unwrap(),
        }
    }

    fn pane_frame(&mut self, layout: &LayoutJson) -> FrameResult {
        self.frame(|frame| render_pane(frame, layout))
    }

    fn host(&self) -> vt100::Parser {
        let mut host = vt100::Parser::new(ROWS, COLUMNS, 0);
        host.process(&self.recorder.bytes.borrow());
        host
    }

    fn linked_cells(&self) -> Vec<LinkedCell> {
        linked_host_cells(&self.host())
    }
}

fn pane_sized(content: &[u8], columns: u16) -> LayoutJson {
    let mut parser = vt100::Parser::new(ROWS, columns, 0);
    parser.process(content);
    LayoutJson::Leaf {
        id: 0,
        rows: ROWS,
        cols: columns,
        cursor_row: 0,
        cursor_col: 0,
        alternate_screen: false,
        wants_mouse: false,
        hide_cursor: true,
        cursor_shape: 0,
        active: true,
        copy_mode: false,
        scroll_offset: 0,
        view_offset: 0,
        sel_start_row: None,
        sel_start_col: None,
        sel_end_row: None,
        sel_end_col: None,
        sel_mode: None,
        copy_cursor_row: None,
        copy_cursor_col: None,
        content: Vec::new(),
        rows_v2: crate::layout::serialize_screen_rows(parser.screen(), ROWS, columns),
        title: None,
        copy_hl: Vec::new(),
    }
}

fn pane(content: &[u8]) -> LayoutJson {
    pane_sized(content, COLUMNS)
}

fn render_pane(frame: &mut Frame, layout: &LayoutJson) {
    let area = Rect::new(0, 0, COLUMNS, ROWS);
    let active_rect = crate::client::compute_active_rect_json(layout, area);
    crate::client::render_layout_json(
        frame,
        layout,
        area,
        false,
        Style::default(),
        Style::default(),
        false,
        Color::Reset,
        active_rect,
        "",
        false,
        "off",
        "",
        1,
        crate::border_lines::border_chars("single"),
        None,
        crate::client::WindowContentStyles::default(),
        crate::pane_border::PaneBorderIndicators::Colour,
    );
}

fn linked_host_cells(host: &vt100::Parser) -> Vec<LinkedCell> {
    let screen = host.screen();
    (0..ROWS)
        .flat_map(|row| (0..COLUMNS).map(move |column| (row, column)))
        .filter_map(|(row, column)| {
            let cell = screen.cell(row, column)?;
            let hyperlink_id = cell.hyperlink_id();
            let uri = screen
                .hyperlink_uri(hyperlink_id)
                .unwrap_or_default()
                .to_string();
            (hyperlink_id != 0).then(|| (row, column, cell.contents().to_string(), uri))
        })
        .collect()
}

fn host_after(frames: &[&[u8]]) -> Vec<LinkedCell> {
    let mut client = Client::new();
    for content in frames {
        client.pane_frame(&pane(content));
    }
    client.linked_cells()
}

fn ascii_link(row: u16, start: u16, text: &str, uri: &str) -> Vec<LinkedCell> {
    text.chars()
        .enumerate()
        .map(|(offset, character)| {
            (
                row,
                start + offset as u16,
                character.to_string(),
                uri.to_string(),
            )
        })
        .collect()
}

fn on_row(row: u16, line: &[u8]) -> Vec<u8> {
    let mut content = Vec::new();
    content.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
    content.extend_from_slice(line);
    content
}

fn host_color(color: Color) -> vt100::Color {
    match color {
        Color::Reset => vt100::Color::Default,
        Color::Rgb(red, green, blue) => vt100::Color::Rgb(red, green, blue),
        Color::Indexed(index) => vt100::Color::Idx(index),
        other => panic!("fixture uses only Reset, Rgb and Indexed colors, got {other:?}"),
    }
}

struct WidthGuard;

impl WidthGuard {
    fn set(entries: &[&str]) -> Self {
        vt100::set_codepoint_widths(entries);
        Self
    }
}

impl Drop for WidthGuard {
    fn drop(&mut self) {
        vt100::clear_codepoint_widths();
    }
}

fn row_options(buffer: &Buffer, row: u16) -> Vec<CellDiffOption> {
    let area = buffer.area;
    (area.left()..area.right())
        .map(|column| buffer[(column, row)].diff_option)
        .collect()
}

fn diff_positions(previous: &Buffer, next: &Buffer) -> Vec<(u16, u16, String)> {
    previous
        .diff(next)
        .into_iter()
        .map(|(column, row, cell)| (column, row, cell.symbol().to_string()))
        .collect()
}

fn run(column: u16, row: u16, text: &str) -> HyperlinkRun {
    HyperlinkRun {
        x: column,
        y: row,
        text: text.to_string(),
        uri: URI.to_string(),
        style: Style::default(),
    }
}

#[test]
fn case01_linked_text_reaches_the_host_with_its_link() {
    let linked = host_after(&[LINKED_LINE]);
    assert_eq!(linked, ascii_link(0, LINK_START, LINK_TEXT, URI));
}

#[test]
fn case02_link_kept_over_two_frames_stays_the_same() {
    let linked = host_after(&[LINKED_LINE, LINKED_LINE]);
    assert_eq!(linked, ascii_link(0, LINK_START, LINK_TEXT, URI));
}

#[test]
fn case03_blank_pane_leaves_no_link() {
    let linked = host_after(&[LINKED_LINE, b""]);
    assert_eq!(linked, Vec::<LinkedCell>::new());
}

#[test]
fn case04_dropping_only_the_link_leaves_no_link() {
    let linked = host_after(&[LINKED_LINE, PLAIN_LINE]);
    assert_eq!(linked, Vec::<LinkedCell>::new());
}

#[test]
fn case05_link_moved_to_another_row_is_linked_only_there() {
    let mut moved = on_row(0, PLAIN_LINE);
    moved.extend_from_slice(&on_row(2, LINKED_LINE));
    let linked = host_after(&[LINKED_LINE, &moved]);
    assert_eq!(linked, ascii_link(2, LINK_START, LINK_TEXT, URI));
}

#[test]
fn case06_shrinking_the_link_keeps_only_the_new_range() {
    let shorter: &[u8] =
        b"Learn more in the \x1b]8;;https://example.com/mcp\x1b\\MCP\x1b]8;;\x1b\\ documentation.";
    let linked = host_after(&[LINKED_LINE, shorter]);
    assert_eq!(linked, ascii_link(0, LINK_START, "MCP", URI));
}

#[test]
fn case07_removing_one_of_two_links_on_a_row_keeps_the_other() {
    let both: &[u8] = b"see \x1b]8;;https://example.com/a\x1b\\alpha\x1b]8;;\x1b\\ and \x1b]8;;https://example.com/b\x1b\\beta\x1b]8;;\x1b\\ end";
    let one: &[u8] = b"see alpha and \x1b]8;;https://example.com/b\x1b\\beta\x1b]8;;\x1b\\ end";
    let linked = host_after(&[both, one]);
    assert_eq!(linked, ascii_link(0, 14, "beta", "https://example.com/b"));
}

#[test]
fn case08_truncated_run_is_cleared_after_the_link_goes() {
    let wide_columns = COLUMNS + 20;
    let linked_line: &[u8] = b"0123456789012345678901234567890\x1b]8;;https://example.com/mcp\x1b\\linked-text-crossing-edge\x1b]8;;\x1b\\";
    let plain_line: &[u8] = b"0123456789012345678901234567890linked-text-crossing-edge";
    let mut client = Client::new();
    let first = client.pane_frame(&pane_sized(linked_line, wide_columns));
    let texts: Vec<(u16, u16, String)> = first
        .runs
        .iter()
        .map(|run| (run.x, run.y, run.text.clone()))
        .collect();
    assert_eq!(texts, vec![(31, 0, "linked-te".to_string())]);
    client.pane_frame(&pane_sized(plain_line, wide_columns));
    assert_eq!(client.linked_cells(), Vec::<LinkedCell>::new());
}

#[test]
fn case09_hangul_link_with_the_uri_removed_leaves_no_link() {
    let linked_line = "go \x1b]8;;https://example.com/ko\x1b\\한글 링크\x1b]8;;\x1b\\ end";
    let plain_line = "go 한글 링크 end";
    let linked = host_after(&[linked_line.as_bytes(), plain_line.as_bytes()]);
    assert_eq!(linked, Vec::<LinkedCell>::new());
}

fn emoji_frames() -> (Vec<u8>, Vec<u8>) {
    let mut linked = on_row(
        1,
        "a \x1b]8;;https://example.com/heart\x1b\\\u{2764}\u{FE0F}x\x1b]8;;\x1b\\ b".as_bytes(),
    );
    linked.extend_from_slice(&on_row(
        2,
        "a \x1b]8;;https://example.com/key\x1b\\1\u{FE0F}\u{20E3}x\x1b]8;;\x1b\\ b".as_bytes(),
    ));
    let mut plain = on_row(1, "a \u{2764}\u{FE0F}x b".as_bytes());
    plain.extend_from_slice(&on_row(2, "a 1\u{FE0F}\u{20E3}x b".as_bytes()));
    (linked, plain)
}

#[test]
fn case10_vs16_and_keycap_links_with_the_uri_removed_leave_no_link() {
    let _guard = WidthGuard::set(&[]);
    let (linked, plain) = emoji_frames();
    assert_eq!(host_after(&[&linked, &plain]), Vec::<LinkedCell>::new());
}

#[test]
fn case10_vs16_and_keycap_links_with_an_unrelated_width_override_leave_no_link() {
    let _guard = WidthGuard::set(&["U+E000=1"]);
    let (linked, plain) = emoji_frames();
    assert_eq!(host_after(&[&linked, &plain]), Vec::<LinkedCell>::new());
}

#[test]
fn case11_marked_row_emits_the_wide_lead_even_when_unchanged() {
    let area = Rect::new(0, 0, 6, 3);
    let mut previous = Buffer::empty(area);
    previous.set_string(0, 1, "한x", Style::default());
    let mut next = previous.clone();
    crate::client::redraw_hyperlink_rows(&mut next, &[1]);
    assert_eq!(row_options(&next, 1), vec![CellDiffOption::AlwaysUpdate; 6]);
    assert_eq!(row_options(&next, 0), vec![CellDiffOption::None; 6]);
    assert_eq!(row_options(&next, 2), vec![CellDiffOption::None; 6]);
    let emitted = diff_positions(&previous, &next);
    assert!(emitted.contains(&(0, 1, "한".to_string())), "{emitted:?}");
}

#[test]
fn case11_overlay_that_started_inside_a_wide_glyph_leaves_no_link() {
    let draw_row = |frame: &mut Frame, with_run: bool| {
        frame.buffer_mut().set_string(0, 1, "한", Style::default());
        if with_run {
            crate::client::frame_hyperlinks_push(run(1, 1, "x"));
        }
    };
    let mut client = Client::new();
    client.frame(|frame| draw_row(frame, true));
    let mut first = vec![(1, 0, String::new(), URI.to_string())];
    first.extend(ascii_link(1, 1, "x", URI));
    assert_eq!(client.linked_cells(), first);
    client.frame(|frame| draw_row(frame, false));
    assert_eq!(client.linked_cells(), Vec::<LinkedCell>::new());
}

#[test]
fn case12_marked_row_emits_width_three_and_width_four_leads() {
    let area = Rect::new(0, 0, 12, 3);
    let mut previous = Buffer::empty(area);
    previous.set_string(0, 1, "\u{17D8}", Style::default());
    previous.set_string(4, 1, "\u{FF76}\u{FF9E}\u{FF9E}\u{FF9E}", Style::default());
    assert_eq!(previous[(0, 1)].cell_width(), 3);
    assert_eq!(previous[(4, 1)].cell_width(), 4);
    let mut next = previous.clone();
    crate::client::redraw_hyperlink_rows(&mut next, &[1]);
    let emitted = diff_positions(&previous, &next);
    assert!(
        emitted.contains(&(0, 1, "\u{17D8}".to_string())),
        "{emitted:?}"
    );
    assert!(
        emitted.contains(&(4, 1, "\u{FF76}\u{FF9E}\u{FF9E}\u{FF9E}".to_string())),
        "{emitted:?}"
    );
}

#[test]
fn case13_widgets_drawn_over_a_vanished_link_match_the_buffer_in_the_host() {
    let underlined_link: &[u8] = b"\x1b[2;1HLearn more in the \x1b[4;58;2;200;100;50;38;2;10;20;30m\x1b]8;;https://example.com/mcp\x1b\\MCP documentation\x1b]8;;\x1b\\\x1b[0m.";
    let underlined_plain: &[u8] = b"\x1b[2;1HLearn more in the \x1b[4;58;2;200;100;50;38;2;10;20;30mMCP documentation\x1b[0m.";
    let popup_style = Style::default()
        .fg(Color::Rgb(250, 240, 230))
        .bg(Color::Rgb(20, 30, 40));
    let mut client = Client::new();
    client.pane_frame(&pane(underlined_link));
    let plain = pane(underlined_plain);
    let result = client.frame(|frame| {
        render_pane(frame, &plain);
        let popup = Rect::new(22, 0, 8, 3);
        frame.render_widget(Clear, popup);
        frame.render_widget(Block::bordered().style(popup_style), popup);
        frame.render_widget(
            Paragraph::new(Line::from("ok")).style(popup_style),
            Rect::new(23, 1, 6, 1),
        );
    });
    assert!(result.runs.is_empty());
    let host = client.host();
    let screen = host.screen();
    let mut mismatches = Vec::new();
    for column in 0..COLUMNS {
        let expected = &result.buffer[(column, 1)];
        let cell = screen.cell(1, column).unwrap();
        let host_symbol = if cell.contents().is_empty() {
            " ".to_string()
        } else {
            cell.contents().to_string()
        };
        let expected_underline = if expected.modifier.contains(Modifier::UNDERLINED) {
            host_color(expected.underline_color)
        } else {
            vt100::Color::Default
        };
        let actual = (
            host_symbol,
            cell.fgcolor(),
            cell.bgcolor(),
            cell.underline_color(),
            cell.hyperlink_id(),
        );
        let wanted = (
            expected.symbol().to_string(),
            host_color(expected.fg),
            host_color(expected.bg),
            expected_underline,
            0,
        );
        if actual != wanted {
            mismatches.push((column, actual, wanted));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:?}");
}

#[test]
fn case14_rows_outside_an_offset_area_are_ignored() {
    let area = Rect::new(5, 10, 4, 3);
    let mut buffer = Buffer::empty(area);
    crate::client::redraw_hyperlink_rows(&mut buffer, &[]);
    for row in area.top()..area.bottom() {
        assert_eq!(row_options(&buffer, row), vec![CellDiffOption::None; 4]);
    }
    crate::client::redraw_hyperlink_rows(&mut buffer, &[0, 9, 10, 10, 12, 13, u16::MAX]);
    assert_eq!(
        row_options(&buffer, 10),
        vec![CellDiffOption::AlwaysUpdate; 4]
    );
    assert_eq!(row_options(&buffer, 11), vec![CellDiffOption::None; 4]);
    assert_eq!(
        row_options(&buffer, 12),
        vec![CellDiffOption::AlwaysUpdate; 4]
    );
}

#[test]
fn case14_hyperlink_rows_drops_empty_runs_and_sorts_unique_rows() {
    let runs = vec![
        run(0, 5, "a"),
        run(3, 2, "b"),
        run(0, 5, "c"),
        run(1, 7, ""),
        run(9, 2, "d"),
    ];
    assert_eq!(crate::client::hyperlink_rows(&runs), vec![2, 5]);
    assert_eq!(crate::client::hyperlink_rows(&[]), Vec::<u16>::new());
}

#[test]
fn case15_rows_survive_a_second_attach_scope_and_clear_after_an_empty_frame() {
    crate::client::previous_hyperlink_rows_set(Vec::new());
    let mut client = Client::new();
    {
        client.pane_frame(&pane(LINKED_LINE));
    }
    assert_eq!(crate::client::previous_hyperlink_rows_get(), vec![0]);
    {
        let second = client.pane_frame(&pane(PLAIN_LINE));
        assert_eq!(
            row_options(&second.buffer, 0),
            vec![CellDiffOption::AlwaysUpdate; COLUMNS as usize]
        );
        assert!(second.runs.is_empty());
    }
    assert_eq!(
        crate::client::previous_hyperlink_rows_get(),
        Vec::<u16>::new()
    );
    assert_eq!(client.linked_cells(), Vec::<LinkedCell>::new());
}
