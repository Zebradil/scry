//! Unified line diffs with 3 lines of context (spec 7.3).

use similar::{ChangeTag, TextDiff};

use crate::report::{Hunk, Line, LineOp};

const CONTEXT: usize = 3;

/// The hunks of a unified diff from `old` to `new`, without highlight. An added or removed file
/// is a diff against empty text, so its whole content is one hunk.
pub fn hunks(old: &str, new: &str) -> Vec<Hunk> {
    let diff = TextDiff::from_lines(old, new);
    diff.grouped_ops(CONTEXT)
        .iter()
        .map(|group| {
            let (first, last) = (&group[0], &group[group.len() - 1]);
            let old_range = first.old_range().start..last.old_range().end;
            let new_range = first.new_range().start..last.new_range().end;
            let lines = group
                .iter()
                .flat_map(|op| diff.iter_changes(op))
                .map(|change| Line {
                    op: match change.tag() {
                        ChangeTag::Equal => LineOp::Context,
                        ChangeTag::Delete => LineOp::Removed,
                        ChangeTag::Insert => LineOp::Added,
                    },
                    text: strip_terminator(change.value()).to_owned(),
                    spans: Vec::new(),
                    hash_only: false,
                    no_newline: !change.value().ends_with('\n'),
                })
                .collect();
            Hunk {
                old_start: start(&old_range),
                old_lines: old_range.len(),
                new_start: start(&new_range),
                new_lines: new_range.len(),
                lines,
            }
        })
        .collect()
}

/// 1-based first line; an empty range names the line before it, as `diff -u` does (`-0,0`).
fn start(range: &std::ops::Range<usize>) -> usize {
    if range.is_empty() {
        range.start
    } else {
        range.start + 1
    }
}

fn strip_terminator(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(hunks: &[Hunk]) -> String {
        let mut out = String::new();
        for h in hunks {
            out += &format!(
                "@@ -{},{} +{},{} @@\n",
                h.old_start, h.old_lines, h.new_start, h.new_lines
            );
            for l in &h.lines {
                let op = match l.op {
                    LineOp::Context => ' ',
                    LineOp::Removed => '-',
                    LineOp::Added => '+',
                };
                out += &format!("{op}{}\n", l.text);
                if l.no_newline {
                    out += "\\ No newline at end of file\n";
                }
            }
        }
        out
    }

    #[test]
    fn three_lines_of_context_and_separate_hunks() {
        let old: String = (1..=20).map(|i| format!("{i}\n")).collect();
        let new = old
            .replacen("2\n", "two\n", 1)
            .replace("\n18\n", "\neighteen\n");
        assert_eq!(
            render(&hunks(&old, &new)),
            "@@ -1,5 +1,5 @@\n 1\n-2\n+two\n 3\n 4\n 5\n\
             @@ -15,6 +15,6 @@\n 15\n 16\n 17\n-18\n+eighteen\n 19\n 20\n"
        );
    }

    #[test]
    fn added_and_removed_files_are_one_hunk() {
        assert_eq!(render(&hunks("", "a\nb\n")), "@@ -0,0 +1,2 @@\n+a\n+b\n");
        assert_eq!(render(&hunks("a\n", "")), "@@ -1,1 +0,0 @@\n-a\n");
        assert!(hunks("", "").is_empty());
        assert_eq!(
            render(&hunks("a\nb", "a\nb\n")),
            "@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n"
        );
        assert!(hunks("same\n", "same\n").is_empty());
    }
}
