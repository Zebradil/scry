//! The screen (spec 8.4, 8.5): the list, the detail pane, the bottom line. The focused pane
//! has a thick border and the cursor a `>` mark, so both show without colour. Styles come from
//! `render::view` as they are.

use ratatui::Frame;
use ratatui::style::{Color, Modifier};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::{App, Focus};
use super::inspect::Inspect;
use super::rows;
use crate::render::view::{self, Span, Style};

const HINTS: &str =
    "j/k move  enter fold  i inspect  / filter  w wrap  t tree  y/Y copy  tab  q quit";
const INSPECT_HINTS: &str = "j/k scroll  n/N next/prev change  s side by side  w wrap  esc back";

pub fn draw(app: &App, frame: &mut Frame) {
    if let Some(inspect) = &app.inspect {
        draw_inspect(app, inspect, frame);
        return;
    }
    let areas = app.areas();
    let color = app.options.color;
    let style = |s: Style| ratatui_style(s, color);
    let mark = |focused: bool| {
        let s = Style {
            bold: focused,
            dim: !focused,
            ..Style::default()
        };
        ratatui::text::Span::styled(">", style(s))
    };
    let blank = || ratatui::text::Span::raw(" ");

    let focused = app.focus == Focus::List;
    let block = pane(focused);
    let inner = block.inner(areas.list);
    frame.render_widget(block, areas.list);
    let list = app.list();
    let name_width = list.iter().map(|r| r.name.width()).max().unwrap_or(0);
    let lines: Vec<Line> = list
        .into_iter()
        .enumerate()
        .skip(app.list_top)
        .take(usize::from(inner.height))
        .map(|(i, row)| {
            let pad = name_width - row.name.width();
            let label = format!(" {:1} {}{:pad$}  ", row.index, row.name, "");
            let mut spans = vec![
                if i == app.selected {
                    mark(focused)
                } else {
                    blank()
                },
                ratatui::text::Span::raw(label),
            ];
            spans.extend(row.status.iter().map(|s| span(s, color)));
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);

    let focused = app.focus == Focus::Detail;
    let block = pane(focused).title(format!(" {} ", app.title()));
    let inner = block.inner(areas.detail);
    frame.render_widget(block, areas.detail);
    let rows = app.rows();
    let view = &app.views[app.selected];
    let (width, height) = app.detail_size();
    let mut lines = Vec::new();
    let mut line = 0;
    for (i, row) in rows.iter().enumerate() {
        if line >= view.top + height {
            break;
        }
        for (j, spans) in rows::lines(row, width, app.wrap).into_iter().enumerate() {
            if line >= view.top && line < view.top + height {
                let gutter = if i == view.cursor && j == 0 {
                    mark(focused)
                } else {
                    blank()
                };
                let spans = std::iter::once(gutter).chain(spans.iter().map(|s| span(s, color)));
                lines.push(Line::from(spans.collect::<Vec<_>>()));
            }
            line += 1;
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);

    let bottom = if let Some(text) = &app.editing {
        let text = format!("/{}", view::escape(text));
        let x = areas.bottom.x + u16::try_from(text.width()).unwrap_or(u16::MAX);
        frame.set_cursor_position((x.min(areas.bottom.right()), areas.bottom.y));
        Line::raw(text)
    } else {
        hints(app, HINTS)
    };
    frame.render_widget(Paragraph::new(bottom), areas.bottom);
}

fn draw_inspect(app: &App, inspect: &Inspect, frame: &mut Frame) {
    let area = app.inspect_area();
    let mut title = inspect.title.clone();
    if inspect.reindented {
        title += " [JSON re-indented]";
    }
    let block = pane(true).title(format!(" {title} "));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines: Vec<Line> = inspect
        .lines
        .iter()
        .skip(inspect.top)
        .take(usize::from(inner.height))
        .map(|spans| {
            Line::from(
                spans
                    .iter()
                    .map(|s| span(s, app.options.color))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
    frame.render_widget(
        Paragraph::new(hints(app, INSPECT_HINTS)),
        app.areas().bottom,
    );
}

/// The bottom line: the notice, else `text` dimmed.
fn hints(app: &App, text: &'static str) -> Line<'static> {
    if let Some(notice) = &app.notice {
        return Line::raw(notice.clone());
    }
    Line::styled(text, ratatui_style(Style::DIM, app.options.color))
}

fn pane(focused: bool) -> Block<'static> {
    let border = if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    Block::bordered().border_type(border)
}

fn span(s: &Span, color: bool) -> ratatui::text::Span<'static> {
    ratatui::text::Span::styled(s.text.clone(), ratatui_style(s.style, color))
}

/// Colours and attributes; nothing without colour (spec 8.5).
fn ratatui_style(s: Style, color: bool) -> ratatui::style::Style {
    let mut out = ratatui::style::Style::new();
    if !color {
        return out;
    }
    if let Some(c) = s.color {
        out = out.fg(ratatui_color(c));
    }
    if let Some(c) = s.background {
        out = out.bg(ratatui_color(c));
    }
    for (on, modifier) in [
        (s.bold, Modifier::BOLD),
        (s.dim, Modifier::DIM),
        (s.underline, Modifier::UNDERLINED),
        (s.reverse, Modifier::REVERSED),
    ] {
        if on {
            out = out.add_modifier(modifier);
        }
    }
    out
}

fn ratatui_color(c: view::Color) -> Color {
    match c {
        view::Color::Black => Color::Black,
        view::Color::Red => Color::Red,
        view::Color::Green => Color::Green,
        view::Color::Yellow => Color::Yellow,
        view::Color::Magenta => Color::Magenta,
        view::Color::Cyan => Color::Cyan,
        // ratatui's `White` is SGR 97; its `Gray` is 37.
        view::Color::BrightWhite => Color::White,
    }
}
