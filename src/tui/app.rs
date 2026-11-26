//! The viewer's state and what each key does to it (spec 8.4), with no terminal: the event loop
//! feeds it keys, sizes and arrivals, and draws it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use super::inspect::Inspect;
use super::rows::{self, Key, Row, Shape};
use super::{Finished, Options, osc52};
use crate::render::text;
use crate::render::view::{self, Color, Kind, Node, Span, Style};
use crate::report::{Configuration, FileChange, Mode, SharedChange, SharedItem, Status};
use crate::shared;
use crate::size::format_delta;

/// Opens and closes the inspect window. shortcut: `i` is provisional, pending a decision on the
/// key (issue #60); change `HINTS` in `draw` with it.
pub const INSPECT_KEY: char = 'i';

/// `y`/`Y` warn above this much OSC 52 payload (spec 8.4; a judgement, unmeasured).
const CLIPBOARD_LIMIT: usize = 100 * 1024;

/// The keys the viewer acts on, apart from the library that reads them: crossterm in a
/// terminal, the browser's events on wasm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Tab,
    BackTab,
    Up,
    Down,
    PageUp,
    PageDown,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
}

impl From<KeyCode> for KeyEvent {
    fn from(code: KeyCode) -> Self {
        Self {
            code,
            ctrl: false,
            alt: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Detail,
}

/// One left-pane entry's detail state, kept across selection changes and filter edits.
#[derive(Debug, Default)]
pub struct View {
    pub open: HashMap<Key, bool>,
    /// Row index.
    pub cursor: usize,
    /// First screen line shown.
    pub top: usize,
}

pub enum Entry {
    All,
    /// Index into the names.
    Configuration(usize),
}

/// The screen split: the list, the detail pane, the bottom line.
pub struct Areas {
    pub list: Rect,
    pub detail: Rect,
    pub bottom: Rect,
}

/// One left-pane row: index (`1`–`9`, or empty), name, status.
pub struct ListRow {
    pub index: String,
    pub name: String,
    pub status: Vec<Span>,
}

pub struct App {
    pub mode: Mode,
    /// Selected configurations in name order.
    pub names: Vec<String>,
    /// Configurations received so far, in name order.
    pub received: Vec<Configuration>,
    /// Shared changes of `received`.
    pub shared: Vec<SharedChange>,
    pub options: Options,
    /// One per entry.
    pub views: Vec<View>,
    pub selected: usize,
    pub list_top: usize,
    pub focus: Focus,
    pub wrap: bool,
    pub tree: bool,
    pub filter: String,
    /// The filter being edited after `/`.
    pub editing: Option<String>,
    pub notice: Option<String>,
    /// The inspect window, over both panes while open.
    pub inspect: Option<Inspect>,
    /// An OSC 52 escape for the event loop to write.
    pub clipboard: Option<String>,
    pub quit: bool,
    /// Terminal width and height.
    pub size: (u16, u16),
    /// Counts [`App::arrive`] calls, so the rows cache sees a replaced configuration.
    arrivals: usize,
    /// [`App::rows`] by its inputs: building them is the costly part of a key press.
    rows_cache: RefCell<Option<(RowsKey, Rc<Vec<Row>>)>>,
}

/// What [`App::rows`] depends on besides fixed state.
#[derive(PartialEq)]
struct RowsKey {
    selected: usize,
    arrivals: usize,
    filter: String,
    tree: bool,
    open: HashMap<Key, bool>,
}

impl App {
    pub fn new(mode: Mode, names: Vec<String>, options: Options) -> Self {
        let entries = names.len() + usize::from(names.len() >= 2);
        Self {
            mode,
            names,
            received: Vec::new(),
            shared: Vec::new(),
            options,
            views: (0..entries.max(1)).map(|_| View::default()).collect(),
            selected: 0,
            list_top: 0,
            focus: Focus::List,
            wrap: true,
            tree: true,
            filter: String::new(),
            editing: None,
            notice: None,
            inspect: None,
            clipboard: None,
            quit: false,
            size: (80, 24),
            arrivals: 0,
            rows_cache: RefCell::new(None),
        }
    }

    /// A configuration finished: its row and the shared changes and `(also in N)` marks update.
    pub fn arrive(&mut self, configuration: Configuration) {
        self.keeping_cursor(|app| {
            let received = &mut app.received;
            let at = received.partition_point(|c| c.name < configuration.name);
            if received
                .get(at)
                .is_some_and(|c| c.name == configuration.name)
            {
                received[at] = configuration;
            } else {
                received.insert(at, configuration);
            }
            app.shared = shared::shared(&app.received);
            app.arrivals += 1;
        });
        self.settle();
    }

    /// Runs `change`, then puts the cursor back on its row if that row still shows: the same
    /// occurrence of rows that look alike (`rows::id`), else the one nearest the old index.
    fn keeping_cursor(&mut self, change: impl FnOnce(&mut Self)) {
        let rows = self.rows();
        let at = self.views[self.selected].cursor.min(rows.len() - 1);
        let before = rows::id(&rows, at);
        let nth = (0..at).filter(|&i| rows::id(&rows, i) == before).count();
        change(self);
        let rows = self.rows();
        let alike: Vec<usize> = (0..rows.len())
            .filter(|&i| rows::id(&rows, i) == before)
            .collect();
        let found = alike
            .get(nth)
            .or_else(|| alike.iter().min_by_key(|i| i.abs_diff(at)));
        if let Some(&i) = found {
            self.view().cursor = i;
        }
    }

    pub fn finish(self) -> Finished {
        let names = &self.names;
        let all_finished = names
            .iter()
            .all(|n| self.received.iter().any(|c| &c.name == n));
        Finished {
            all_finished,
            configurations: self.received,
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.size = (width, height);
        self.settle();
        self.layout_inspect();
    }

    fn has_all(&self) -> bool {
        self.names.len() >= 2
    }

    fn entries(&self) -> usize {
        self.names.len() + usize::from(self.has_all())
    }

    pub fn entry(&self, index: usize) -> Entry {
        if self.has_all() {
            match index {
                0 => Entry::All,
                i => Entry::Configuration(i - 1),
            }
        } else {
            Entry::Configuration(index)
        }
    }

    fn configuration(&self, index: usize) -> Option<&Configuration> {
        let name = self.names.get(index)?;
        self.received.iter().find(|c| &c.name == name)
    }

    pub fn title(&self) -> String {
        let name = match self.entry(self.selected) {
            Entry::All => "All",
            Entry::Configuration(i) => self.names.get(i).map_or("", String::as_str),
        };
        let mut title = view::escape(name);
        if !self.filter.is_empty() {
            title += &format!(" [filter: {}]", view::escape(&self.filter));
        }
        title
    }

    pub fn list(&self) -> Vec<ListRow> {
        let all = self.has_all().then(|| ListRow {
            index: String::new(),
            name: "All".into(),
            status: vec![span(format!("{} shared", self.shared.len()), PLAIN)],
        });
        let configurations = self.names.iter().enumerate().map(|(i, name)| ListRow {
            index: if i < 9 {
                (i + 1).to_string()
            } else {
                String::new()
            },
            name: view::escape(name),
            status: self.status(i),
        });
        all.into_iter().chain(configurations).collect()
    }

    /// The status column of spec 8.4's table.
    fn status(&self, index: usize) -> Vec<Span> {
        let Some(c) = self.configuration(index) else {
            return vec![span("fetching…", DIM)];
        };
        match &c.status {
            Status::Done(s) => {
                let (old, new) = (s.packages.size_old, s.packages.size_new);
                let delta = if new >= old {
                    i64::try_from(new - old).unwrap_or(i64::MAX)
                } else {
                    -i64::try_from(old - new).unwrap_or(i64::MAX)
                };
                vec![
                    span("done", fg(Color::Green)),
                    span(format!(" {}", format_delta(delta)), PLAIN),
                ]
            }
            Status::NoChanges => vec![span("no changes", DIM)],
            Status::Error { .. } => vec![span("error", fg(Color::Red))],
            Status::Timeout { .. } => vec![span("timeout", fg(Color::Red))],
            Status::AbsentAtBase => vec![span("absent at base", DIM)],
        }
    }

    /// Left pane, detail pane and bottom line. The list is as wide as its widest row and its
    /// borders, at most a third of the screen (spec 8.4).
    pub fn areas(&self) -> Areas {
        let (width, height) = self.size;
        let list = self.list();
        let name = list.iter().map(|r| r.name.width()).max().unwrap_or(0);
        let status = list
            .iter()
            .map(|r| spans_width(&r.status))
            .max()
            .unwrap_or(0);
        // Borders, `> 1 `, the name, two spaces, the status.
        let wanted = 2 + 4 + name + 2 + status;
        let list_width = wanted.min(usize::from(width) / 3);
        let list_width = u16::try_from(list_width).unwrap_or(width);
        let main = height.saturating_sub(1);
        Areas {
            list: Rect::new(0, 0, list_width, main),
            detail: Rect::new(list_width, 0, width - list_width, main),
            bottom: Rect::new(0, main, width, height.min(1)),
        }
    }

    /// Columns and lines of the detail pane's rows: inside the border, after the cursor gutter.
    pub fn detail_size(&self) -> (usize, usize) {
        let detail = self.areas().detail;
        (
            usize::from(detail.width).saturating_sub(3),
            usize::from(detail.height).saturating_sub(2),
        )
    }

    /// The inspect window: the whole screen above the bottom line, inside the border.
    pub fn inspect_area(&self) -> Rect {
        let (width, height) = self.size;
        Rect::new(0, 0, width, height.saturating_sub(1))
    }

    fn inspect_size(&self) -> (usize, usize) {
        let area = self.inspect_area();
        (
            usize::from(area.width).saturating_sub(2),
            usize::from(area.height).saturating_sub(2),
        )
    }

    fn layout_inspect(&mut self) {
        let (width, height) = self.inspect_size();
        let (wrap, word) = (self.wrap, self.options.word_highlight);
        if let Some(inspect) = &mut self.inspect {
            inspect.layout(width, wrap, word);
            inspect.clamp(height);
        }
    }

    fn list_height(&self) -> usize {
        usize::from(self.areas().list.height).saturating_sub(2)
    }

    /// The selected entry's rows as the detail pane shows them, before wrapping.
    pub fn rows(&self) -> Rc<Vec<Row>> {
        let key = RowsKey {
            selected: self.selected,
            arrivals: self.arrivals,
            filter: self.filter.clone(),
            tree: self.tree,
            open: self.views[self.selected].open.clone(),
        };
        let mut cache = self.rows_cache.borrow_mut();
        match &*cache {
            Some((k, rows)) if *k == key => Rc::clone(rows),
            _ => {
                let rows = Rc::new(self.build_rows());
                *cache = Some((key, Rc::clone(&rows)));
                rows
            }
        }
    }

    fn build_rows(&self) -> Vec<Row> {
        let (nodes, rebuilt) = match self.entry(self.selected) {
            Entry::All => (view::shared(&self.shared), None),
            Entry::Configuration(i) => match self.configuration(i) {
                None => (vec![note("fetching…")], None),
                Some(c) => {
                    let rebuilt = match &c.status {
                        Status::Done(s) => Some(&s.packages.rebuilt),
                        _ => None,
                    };
                    (
                        view::configuration(
                            &self.mode,
                            &self.shared,
                            c,
                            self.options.word_highlight,
                        ),
                        rebuilt,
                    )
                }
            },
        };
        let filter = self.filter.to_lowercase();
        let shape = Shape {
            filter: &filter,
            tree: self.tree,
            expand_churn: self.options.expand_churn,
            open: &self.views[self.selected].open,
            rebuilt,
        };
        let mut rows = rows::rows(nodes, &shape);
        if let Entry::All = self.entry(self.selected) {
            rows::align_names(&mut rows);
        }
        if rows.is_empty() {
            let finished = self.received.len() >= self.names.len();
            let text = if finished {
                "no shared changes"
            } else {
                "no shared changes yet"
            };
            rows = rows::rows(vec![note(text)], &shape);
        }
        rows
    }

    /// The first screen line of each row, and the total after the last.
    pub fn starts(&self, rows: &[Row]) -> Vec<usize> {
        let width = self.detail_size().0;
        let mut starts = vec![0];
        let mut line = 0;
        for row in rows {
            line += rows::lines(row, width, self.wrap).len();
            starts.push(line);
        }
        starts
    }

    /// Clamps the selection and cursor and scrolls both panes to keep the margin.
    fn settle(&mut self) {
        let entries = self.entries();
        self.selected = self.selected.min(entries.saturating_sub(1));
        let list_height = self.list_height();
        self.list_top = rows::scroll(
            self.list_top,
            (self.selected, self.selected + 1),
            entries,
            list_height,
        );
        let rows = self.rows();
        let starts = self.starts(&rows);
        let height = self.detail_size().1;
        let view = &mut self.views[self.selected];
        view.cursor = view.cursor.min(rows.len() - 1);
        view.top = rows::scroll(
            view.top,
            (starts[view.cursor], starts[view.cursor + 1]),
            starts[rows.len()],
            height,
        );
    }

    fn view(&mut self) -> &mut View {
        &mut self.views[self.selected]
    }

    pub fn key(&mut self, key: KeyEvent) {
        self.notice = None;
        let ctrl = key.ctrl;
        if ctrl && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if let Some(text) = &mut self.editing {
            match key.code {
                KeyCode::Enter => {
                    let filter = std::mem::take(text);
                    self.editing = None;
                    self.keeping_cursor(|app| app.filter = filter);
                }
                KeyCode::Esc => self.editing = None,
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(c) if !ctrl && !key.alt => {
                    text.push(c);
                }
                _ => {}
            }
            self.settle();
            return;
        }
        if self.inspect.is_some() {
            self.inspect_key(key);
            return;
        }
        let detail = self.focus == Focus::Detail;
        match key.code {
            KeyCode::Char('d') if ctrl => self.half(true),
            KeyCode::Char('u') if ctrl => self.half(false),
            _ if ctrl => {}
            KeyCode::PageDown => self.half(true),
            KeyCode::PageUp => self.half(false),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.step(true),
            KeyCode::Char('k') | KeyCode::Up => self.step(false),
            KeyCode::Char('J') if detail => self.jump(true),
            KeyCode::Char('K') if detail => self.jump(false),
            KeyCode::Char('J') => self.step(true),
            KeyCode::Char('K') => self.step(false),
            KeyCode::Char('g') => self.view().cursor = 0,
            KeyCode::Char('G') => self.view().cursor = usize::MAX,
            KeyCode::Char('h') if detail => self.left(),
            KeyCode::Char('l') if detail => self.expand(),
            KeyCode::Enter | KeyCode::Char(' ') if detail => self.toggle(),
            KeyCode::Char('l') | KeyCode::Enter | KeyCode::Char(' ') => {
                self.focus = Focus::Detail;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = if detail { Focus::List } else { Focus::Detail };
            }
            KeyCode::Char(c @ '1'..='9') => {
                let n = usize::from(c as u8 - b'0');
                if n <= self.names.len() {
                    self.selected = n - 1 + usize::from(self.has_all());
                }
            }
            KeyCode::Char('w') => self.wrap = !self.wrap,
            KeyCode::Char('t') => self.keeping_cursor(|app| app.tree = !app.tree),
            KeyCode::Char('/') => self.editing = Some(self.filter.clone()),
            KeyCode::Char(INSPECT_KEY) if detail => self.open_inspect(),
            KeyCode::Char('y') => self.copy(false),
            KeyCode::Char('Y') => self.copy(true),
            _ => {}
        }
        self.settle();
    }

    /// Keys in the inspect window: scrolling by screen lines, `n`/`N` between changes, `s` side
    /// by side, `w` wrap; Esc, `q` or the key that opened it close it.
    fn inspect_key(&mut self, key: KeyEvent) {
        let height = self.inspect_size().1;
        let half = (height / 2).max(1).cast_signed();
        let Some(inspect) = &mut self.inspect else {
            return;
        };
        match key.code {
            KeyCode::Char('d') if key.ctrl => inspect.scroll(half, height),
            KeyCode::Char('u') if key.ctrl => inspect.scroll(-half, height),
            _ if key.ctrl => {}
            KeyCode::PageDown => inspect.scroll(half, height),
            KeyCode::PageUp => inspect.scroll(-half, height),
            KeyCode::Char('j') | KeyCode::Down => inspect.scroll(1, height),
            KeyCode::Char('k') | KeyCode::Up => inspect.scroll(-1, height),
            KeyCode::Char('g') => inspect.top = 0,
            KeyCode::Char('G') => inspect.scroll(isize::MAX, height),
            KeyCode::Char('n') => inspect.jump(true, height),
            KeyCode::Char('N') => inspect.jump(false, height),
            KeyCode::Char('s') => {
                inspect.side_by_side = !inspect.side_by_side;
                self.layout_inspect();
            }
            KeyCode::Char('w') => {
                self.wrap = !self.wrap;
                self.layout_inspect();
                self.settle();
            }
            KeyCode::Esc | KeyCode::Char('q' | INSPECT_KEY) => self.inspect = None,
            _ => {}
        }
    }

    /// Opens the inspect window on the file change at the cursor or holding the cursor's hunk
    /// line.
    fn open_inspect(&mut self) {
        let rows = self.rows();
        let mut at = Some(self.views[self.selected].cursor);
        let file = loop {
            let Some(i) = at else { break None };
            if let Kind::File { path, via } = &rows[i].kind {
                let nth = rows[..i].iter().filter(|r| r.kind == rows[i].kind).count();
                break self.file_change(path, via, nth);
            }
            at = rows[i].parent;
        };
        match file {
            Some(change) if !change.hunks.is_empty() => {
                self.inspect = Some(Inspect::new(change));
                self.layout_inspect();
            }
            Some(_) => self.notice = Some("no text diff to inspect".into()),
            None => self.notice = Some("not on a file change".into()),
        }
    }

    /// The selected entry's file change by path and `via`, as its `Kind::File` row names it,
    /// the `nth` of its kind: All can list one path once per distinct change.
    fn file_change(&self, path: &str, via: &[String], nth: usize) -> Option<&FileChange> {
        let is = |c: &FileChange| c.path == path && c.via == via;
        match self.entry(self.selected) {
            Entry::All => self
                .shared
                .iter()
                .filter_map(|s| match &s.item {
                    SharedItem::Files(c) if is(c) => Some(c),
                    _ => None,
                })
                .nth(nth),
            Entry::Configuration(i) => match &self.configuration(i)?.status {
                Status::Done(s) => s.files.changes.iter().find(|c| is(c)),
                _ => None,
            },
        }
    }

    /// `j`/`k`: one row in the focused pane.
    fn step(&mut self, down: bool) {
        let (at, last) = match self.focus {
            Focus::List => {
                let last = self.entries().saturating_sub(1);
                (&mut self.selected, last)
            }
            Focus::Detail => {
                let last = self.rows().len() - 1;
                (&mut self.views[self.selected].cursor, last)
            }
        };
        *at = if down {
            (*at + 1).min(last)
        } else {
            at.saturating_sub(1)
        };
    }

    /// `J`/`K` in detail: the next or previous collapsible row or change.
    fn jump(&mut self, down: bool) {
        let rows = self.rows();
        let cursor = self.view().cursor;
        let found = if down {
            (cursor + 1..rows.len()).find(|&i| rows[i].actionable())
        } else {
            (0..cursor).rev().find(|&i| rows[i].actionable())
        };
        if let Some(i) = found {
            self.view().cursor = i;
        }
    }

    /// Ctrl-D/Ctrl-U, PgDn/PgUp: half the focused pane, in screen lines.
    fn half(&mut self, down: bool) {
        if self.focus == Focus::List {
            let half = (self.list_height() / 2).max(1);
            let last = self.entries().saturating_sub(1);
            self.selected = if down {
                (self.selected + half).min(last)
            } else {
                self.selected.saturating_sub(half)
            };
            return;
        }
        let height = self.detail_size().1;
        let half = (height / 2).max(1);
        let starts = self.starts(&self.rows());
        let view = &mut self.views[self.selected];
        let (start, end) = (starts[view.cursor], starts[view.cursor + 1]);
        // A cursor row taller than the pane scrolls through before the cursor leaves it.
        if down && view.top + height < end {
            view.top = (view.top + half).min(end - height);
            return;
        }
        if !down && view.top > start {
            view.top = view.top.saturating_sub(half).max(start);
            return;
        }
        let (target, top) = if down {
            ((start + half).max(end), view.top + half)
        } else {
            (start.saturating_sub(half), view.top.saturating_sub(half))
        };
        // The row holding the target line; `starts` ends with the total, which is no row.
        view.cursor = starts[..starts.len() - 1]
            .partition_point(|&s| s <= target)
            .saturating_sub(1);
        view.top = top;
    }

    /// `h`: collapse the open node, else go to its parent, else focus the list.
    fn left(&mut self) {
        let rows = self.rows();
        let row = &rows[self.view().cursor];
        match (&row.key, row.parent) {
            (Some(key), _) if row.open => {
                let key = key.clone();
                self.view().open.insert(key, false);
            }
            (_, Some(parent)) => self.view().cursor = parent,
            _ => self.focus = Focus::List,
        }
    }

    /// `l` in detail: expand a closed node.
    fn expand(&mut self) {
        let rows = self.rows();
        let row = &rows[self.view().cursor];
        if let (Some(key), false) = (&row.key, row.open) {
            let key = key.clone();
            self.view().open.insert(key, true);
        }
    }

    /// Enter/Space in detail: toggle the node; on a leaf, collapse its parent and move to it.
    fn toggle(&mut self) {
        let rows = self.rows();
        let row = &rows[self.view().cursor];
        if let Some(key) = &row.key {
            let (key, open) = (key.clone(), !row.open);
            self.view().open.insert(key, open);
        } else if let Some(parent) = row.parent.filter(|_| row.leaf)
            && let Some(key) = &rows[parent].key
        {
            let key = key.clone();
            self.view().open.insert(key, false);
            self.view().cursor = parent;
        }
    }

    /// `y` copies the pane as shown, unwrapped; `Y` the entry as text output prints it. Both
    /// without colour, through OSC 52 (spec 8.4).
    fn copy(&mut self, as_text: bool) {
        let entry = self.entry(self.selected);
        let fetching = self.received.len() < self.names.len();
        let empty = match entry {
            Entry::All => self.shared.is_empty().then_some(fetching),
            Entry::Configuration(i) => self.configuration(i).is_none().then_some(true),
        };
        if let Some(fetching) = empty {
            let yet = if fetching { " yet" } else { "" };
            self.notice = Some(format!("nothing to copy{yet}"));
            return;
        }
        let text = if as_text {
            let nodes = match entry {
                Entry::All => view::shared(&self.shared),
                Entry::Configuration(i) => {
                    let c = self.configuration(i).expect("checked above");
                    view::configuration(&self.mode, &self.shared, c, self.options.word_highlight)
                }
            };
            let options = text::Options {
                color: false,
                expand_churn: self.options.expand_churn,
                word_highlight: self.options.word_highlight,
            };
            let mut out = Vec::new();
            text::write_as_text(&nodes, options, &mut out).expect("writing to memory");
            String::from_utf8(out).expect("spans are UTF-8")
        } else {
            self.rows().iter().map(|r| r.text() + "\n").collect()
        };
        let payload = osc52(&text);
        let mut notice = format!(
            "copied {} lines ({:.1} KiB)",
            text.lines().count(),
            payload.len() as f64 / 1024.0
        );
        if payload.len() > CLIPBOARD_LIMIT {
            notice += ", may exceed the terminal's clipboard limit";
        }
        self.notice = Some(notice);
        self.clipboard = Some(payload);
    }
}

const PLAIN: Style = Style::PLAIN;
const DIM: Style = Style { dim: true, ..PLAIN };

fn fg(color: Color) -> Style {
    Style {
        color: Some(color),
        ..PLAIN
    }
}

fn span(text: impl Into<String>, style: Style) -> Span {
    Span {
        text: text.into(),
        style,
    }
}

pub fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.text.width()).sum()
}

/// A dimmed placeholder row.
fn note(text: &str) -> Node {
    Node {
        indent: 0,
        spans: vec![span(text, DIM)],
        kind: Kind::Text,
        children: Vec::new(),
    }
}
