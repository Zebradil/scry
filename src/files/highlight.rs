//! Word highlight inside hunks: line pairing, tokens and a token-level LCS (spec 7.3).

use std::ops::Range;

use similar::{Algorithm, DiffTag, capture_diff_slices};

use crate::files::churn::normalize_hashes;
use crate::report::{Hunk, Line, LineOp};

/// Index pairs `(removed, added)` in `lines`: each run of removed lines pairs with the run of
/// added lines right after it, line by line. Also the pairing of followed references (7.4).
pub fn pairs(lines: &[Line]) -> Vec<(usize, usize)> {
    let run = |from: usize, op: LineOp| {
        from + lines[from..]
            .iter()
            .take_while(|line| line.op == op)
            .count()
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].op != LineOp::Removed {
            i += 1;
            continue;
        }
        let added = run(i, LineOp::Removed);
        let end = run(added, LineOp::Added);
        out.extend((i..added).zip(added..end));
        i = end;
    }
    out
}

/// Sets `spans` on paired lines that differ, or `hash_only` on both when the pair differs only
/// in store hashes. Unpaired lines are left whole.
pub fn highlight(hunks: &mut [Hunk]) {
    for hunk in hunks {
        for (r, a) in pairs(&hunk.lines) {
            let (old, new) = (&hunk.lines[r].text, &hunk.lines[a].text);
            if normalize_hashes(old) == normalize_hashes(new) {
                let hash_only = old != new;
                hunk.lines[r].hash_only = hash_only;
                hunk.lines[a].hash_only = hash_only;
            } else {
                let (old_spans, new_spans) = spans(old, new);
                hunk.lines[r].spans = old_spans;
                hunk.lines[a].spans = new_spans;
            }
        }
    }
}

/// Byte ranges `[start, end)`, as in `report::Line::spans`.
type Spans = Vec<(usize, usize)>;

/// A run of Unicode alphanumerics or `_`, or any single other character, as byte ranges.
fn tokens(text: &str) -> Vec<Range<usize>> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut in_word = false;
    for (i, c) in text.char_indices() {
        let end = i + c.len_utf8();
        match out.last_mut() {
            Some(last) if in_word && word(c) => last.end = end,
            _ => out.push(i..end),
        }
        in_word = word(c);
    }
    out
}

/// Byte spans of the tokens outside a longest common subsequence, adjacent ones merged.
/// Myers' shortest edit script keeps a longest common subsequence.
fn spans(old: &str, new: &str) -> (Spans, Spans) {
    let (old_tokens, new_tokens) = (tokens(old), tokens(new));
    let old_words: Vec<&str> = old_tokens.iter().map(|r| &old[r.clone()]).collect();
    let new_words: Vec<&str> = new_tokens.iter().map(|r| &new[r.clone()]).collect();
    let (mut old_spans, mut new_spans) = (Vec::new(), Vec::new());
    for op in capture_diff_slices(Algorithm::Myers, &old_words, &new_words) {
        if op.tag() != DiffTag::Equal {
            push_span(&mut old_spans, &old_tokens, op.old_range());
            push_span(&mut new_spans, &new_tokens, op.new_range());
        }
    }
    (old_spans, new_spans)
}

fn push_span(spans: &mut Spans, tokens: &[Range<usize>], range: Range<usize>) {
    if range.is_empty() {
        return;
    }
    let (start, end) = (tokens[range.start].start, tokens[range.end - 1].end);
    match spans.last_mut() {
        Some(last) if last.1 == start => last.1 = end,
        _ => spans.push((start, end)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(op: LineOp, text: &str) -> Line {
        Line {
            op,
            text: text.into(),
            spans: vec![],
            hash_only: false,
            no_newline: false,
        }
    }

    fn hunk(lines: Vec<Line>) -> Hunk {
        Hunk {
            old_start: 1,
            old_lines: 0,
            new_start: 1,
            new_lines: 0,
            lines,
        }
    }

    #[test]
    fn tokenizer_splits_words_and_single_characters() {
        let text = "ab_1 = é-ü😀x";
        let words: Vec<&str> = tokens(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(words, ["ab_1", " ", "=", " ", "é", "-", "ü", "😀", "x"]);
    }

    #[test]
    fn pairs_runs_line_by_line() {
        use LineOp::*;
        let lines: Vec<Line> = [
            Context, Removed, Removed, Added, Context, Removed, Added, Added, Added,
        ]
        .into_iter()
        .map(|op| line(op, ""))
        .collect();
        assert_eq!(pairs(&lines), [(1, 3), (5, 6)]);
        // A removed run separated from the added run by context is not paired.
        let lines = [line(Removed, "a"), line(Context, "b"), line(Added, "c")];
        assert!(pairs(&lines).is_empty());
    }

    #[test]
    fn spec_example_spans() {
        let mut hunks = [hunk(vec![
            line(LineOp::Context, "substituters = https://cache.example"),
            line(LineOp::Removed, "trusted-public-keys = cache-a:AAAA"),
            line(
                LineOp::Added,
                "trusted-public-keys = cache-a:AAAA cache-b:BBBB",
            ),
        ])];
        highlight(&mut hunks);
        let lines = &hunks[0].lines;
        assert!(lines[0].spans.is_empty());
        assert!(lines[1].spans.is_empty());
        assert_eq!(lines[2].spans, [(34, 47)]);
        assert!(!lines[2].hash_only);
    }

    #[test]
    fn token_lcs_marks_changed_tokens_on_both_sides() {
        let (old, new) = spans(
            "exec app --workers 4 --log é",
            "exec app --workers 8 --log ü",
        );
        assert_eq!(old, [(19, 20), (27, 29)]);
        assert_eq!(new, [(19, 20), (27, 29)]);
    }

    #[test]
    fn hash_only_pair_is_marked_without_spans() {
        let mut hunks = [hunk(vec![
            line(
                LineOp::Removed,
                "ExecStart=/nix/store/1b9p07z77phvv2hf6gm9f28syp39f1ag-app/bin/app",
            ),
            line(LineOp::Removed, "unpaired"),
            line(
                LineOp::Added,
                "ExecStart=/nix/store/9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g-app/bin/app",
            ),
        ])];
        highlight(&mut hunks);
        let lines = &hunks[0].lines;
        assert!(lines[0].hash_only && lines[2].hash_only);
        assert!(lines[0].spans.is_empty() && lines[2].spans.is_empty());
        assert!(!lines[1].hash_only && lines[1].spans.is_empty());
    }
}
