//! The inspect window: one file change on the whole screen, its JSON hunks re-indented
//! (`files::pretty`), unified or side by side, scrolled by screen lines.

use unicode_width::UnicodeWidthStr;

use super::app::spans_width;
use super::rows::{self, Row};
use crate::cli::WordHighlight;
use crate::files::pretty;
use crate::files::refs::display_path;
use crate::render::view::{self, Kind, Span, Style};
use crate::report::{FileChange, Hunk, Line, LineOp};

/// Between the two halves of a side-by-side line.
const SEPARATOR: &str = " │ ";

pub struct Inspect {
    pub title: String,
    hunks: Vec<Hunk>,
    pub reindented: bool,
    pub side_by_side: bool,
    /// Screen lines as last laid out.
    pub lines: Vec<Vec<Span>>,
    /// Screen line where each change starts: a run of removed and added lines.
    changes: Vec<usize>,
    /// First screen line shown.
    pub top: usize,
}

/// One row before wrapping: a full-width cell, or the old and new halves of a side-by-side row.
struct Logical {
    cells: Vec<Vec<Span>>,
    change: bool,
}

impl Inspect {
    pub fn new(change: &FileChange) -> Self {
        let reindented = pretty::json(&change.hunks);
        Self {
            title: view::escape(&display_path(change)),
            reindented: reindented.is_some(),
            hunks: reindented.unwrap_or_else(|| change.hunks.clone()),
            side_by_side: false,
            lines: Vec::new(),
            changes: Vec::new(),
            top: 0,
        }
    }

    /// Lays the hunks out `width` columns wide.
    pub fn layout(&mut self, width: usize, wrap: bool, word: WordHighlight) {
        self.lines.clear();
        self.changes.clear();
        for row in self.logical(word) {
            if row.change {
                self.changes.push(self.lines.len());
            }
            match &row.cells[..] {
                [old, new] => {
                    let half = width.saturating_sub(SEPARATOR.width()) / 2;
                    // Cut, not wrapped, at the half's edge when wrap is off.
                    let cell = |spans: &[Span]| {
                        let mut lines = wrapped(spans, half, true);
                        if !wrap {
                            lines.truncate(1);
                        }
                        lines
                    };
                    let (old, new) = (cell(old), cell(new));
                    for i in 0..old.len().max(new.len()) {
                        let mut line = old.get(i).cloned().unwrap_or_default();
                        let pad = half.saturating_sub(spans_width(&line));
                        line.push(plain(format!("{:pad$}{SEPARATOR}", "")));
                        line.extend(new.get(i).cloned().unwrap_or_default());
                        self.lines.push(line);
                    }
                }
                cells => {
                    for cell in cells {
                        self.lines.extend(wrapped(cell, width, wrap));
                    }
                }
            }
        }
        self.top = self.top.min(self.lines.len().saturating_sub(1));
    }

    fn logical(&self, word: WordHighlight) -> Vec<Logical> {
        let mut out = Vec::new();
        for h in &self.hunks {
            let header = format!(
                "@@ -{},{} +{},{} @@",
                h.old_start, h.old_lines, h.new_start, h.new_lines
            );
            out.push(Logical {
                cells: vec![vec![dim(header)]],
                change: false,
            });
            for run in runs(&h.lines) {
                let change = run[0].op != LineOp::Context;
                let cells = |skip: Option<LineOp>| -> Vec<Vec<Span>> {
                    run.iter()
                        .filter(|l| Some(l.op) != skip)
                        .flat_map(|l| {
                            let marker = l
                                .no_newline
                                .then(|| vec![dim("\\ No newline at end of file")]);
                            std::iter::once(view::line_spans(l, word)).chain(marker)
                        })
                        .collect()
                };
                let rows: Vec<Vec<Vec<Span>>> = if self.side_by_side {
                    let (old, new) = (cells(Some(LineOp::Added)), cells(Some(LineOp::Removed)));
                    (0..old.len().max(new.len()))
                        .map(|i| {
                            let half =
                                |side: &[Vec<Span>]| side.get(i).cloned().unwrap_or_default();
                            vec![half(&old), half(&new)]
                        })
                        .collect()
                } else {
                    cells(None).into_iter().map(|c| vec![c]).collect()
                };
                for (i, cells) in rows.into_iter().enumerate() {
                    out.push(Logical {
                        cells,
                        change: change && i == 0,
                    });
                }
            }
        }
        out
    }

    /// `n`/`N`: puts the next or previous change [`rows::MARGIN`] lines below the top, the
    /// line a jump lands on, as far as the last page allows.
    pub fn jump(&mut self, down: bool, height: usize) {
        let at = self.top + rows::MARGIN;
        let found = if down {
            self.changes.iter().find(|&&c| c > at)
        } else {
            self.changes.iter().rev().find(|&&c| c < at)
        };
        if let Some(&c) = found {
            self.top = c.saturating_sub(rows::MARGIN);
            self.clamp(height);
        }
    }

    /// Moves the top by `lines`, keeping the last page full.
    pub fn scroll(&mut self, lines: isize, height: usize) {
        self.top = self.top.saturating_add_signed(lines);
        self.clamp(height);
    }

    pub fn clamp(&mut self, height: usize) {
        self.top = self.top.min(self.lines.len().saturating_sub(height));
    }
}

/// `lines` split into runs of context lines and runs of removed lines with the added lines
/// right after them, so a side-by-side row holds a removed line and the added line it pairs
/// with (as in `files::highlight::pairs`).
fn runs(lines: &[Line]) -> Vec<&[Line]> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..=lines.len() {
        let ends = lines.get(i).is_none_or(|next| {
            let prev = lines[i - 1].op;
            match next.op {
                LineOp::Context => prev != LineOp::Context,
                LineOp::Removed => prev != LineOp::Removed,
                LineOp::Added => prev == LineOp::Context,
            }
        });
        if ends {
            out.push(&lines[start..i]);
            start = i;
        }
    }
    out
}

fn wrapped(spans: &[Span], width: usize, wrap: bool) -> Vec<Vec<Span>> {
    let row = Row {
        indent: 0,
        spans: spans.to_vec(),
        key: None,
        open: false,
        parent: None,
        kind: Kind::Text,
        leaf: true,
    };
    rows::lines(&row, width, wrap)
}

fn plain(text: String) -> Span {
    Span {
        text,
        style: Style::PLAIN,
    }
}

fn dim(text: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        style: Style::DIM,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(op: LineOp) -> Line {
        Line {
            op,
            text: String::new(),
            spans: vec![],
            hash_only: false,
            no_newline: false,
        }
    }

    #[test]
    fn runs_split_context_from_removed_then_added() {
        use LineOp::*;
        let lines: Vec<Line> = [
            Context, Removed, Removed, Added, Added, Context, Added, Removed,
        ]
        .into_iter()
        .map(line)
        .collect();
        let lengths: Vec<usize> = runs(&lines).iter().map(|r| r.len()).collect();
        assert_eq!(lengths, [1, 4, 1, 1, 1]);
        assert!(runs(&[]).is_empty());
    }
}
