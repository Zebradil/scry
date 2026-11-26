//! JSON hunks re-indented and diffed again, for the TUI's inspect window: a one-line JSON
//! file becomes a multi-line diff of its members.

use crate::files::{diff, highlight};
use crate::report::{Hunk, LineOp};

/// `hunks` with every hunk whose old and new sides are each a JSON object or array (or empty)
/// replaced by the diff of both sides re-indented, word highlight set. `None` when no hunk
/// changes. Line numbers in a replaced hunk count lines of the re-indented text.
pub fn json(hunks: &[Hunk]) -> Option<Vec<Hunk>> {
    let mut changed = false;
    let out = hunks
        .iter()
        .flat_map(|h| match reindent_hunk(h) {
            Some(hunks) => {
                changed = true;
                hunks
            }
            None => vec![h.clone()],
        })
        .collect();
    changed.then_some(out)
}

fn reindent_hunk(hunk: &Hunk) -> Option<Vec<Hunk>> {
    let side = |skip: LineOp| {
        let lines: Vec<&str> = hunk
            .lines
            .iter()
            .filter(|l| l.op != skip)
            .map(|l| l.text.as_str())
            .collect();
        let text = lines.join("\n");
        if text.trim().is_empty() {
            Some(String::new())
        } else {
            reindent(&text)
        }
    };
    let (old, new) = (side(LineOp::Added)?, side(LineOp::Removed)?);
    // Equal once re-indented: the change is in whitespace only, which the raw hunk shows.
    if old == new {
        return None;
    }
    let mut hunks = diff::hunks(&old, &new);
    highlight::highlight(&mut hunks);
    Some(hunks)
}

/// `text` with one member or element per line, indented two spaces per level and `: ` after
/// each key; `None` unless it is a JSON object or array. Strings, numbers and key order stay
/// as written, so the diff shows the file's own tokens.
fn reindent(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if !(value.is_object() || value.is_array()) {
        return None;
    }
    let mut out = String::with_capacity(text.len() * 2);
    let mut depth = 0usize;
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    };
    let (mut in_string, mut escaped) = (false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            // Outside strings, valid JSON has only insignificant whitespace.
            c if c.is_whitespace() => {}
            '{' | '[' => {
                out.push(c);
                while chars.next_if(|c| c.is_whitespace()).is_some() {}
                match chars.next_if(|&c| c == '}' || c == ']') {
                    Some(close) => out.push(close),
                    None => {
                        depth += 1;
                        newline(&mut out, depth);
                    }
                }
            }
            '}' | ']' => {
                depth -= 1;
                newline(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(c);
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            c => out.push(c),
        }
    }
    out.push('\n');
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Line;

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
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines,
        }
    }

    #[test]
    fn reindent_keeps_tokens_and_key_order() {
        let text = r#"{"z":1.0000000000000001,"a":[],"s":"x, {y}: \"q\"","o":{"k":[1, {}]}}"#;
        assert_eq!(
            reindent(text).unwrap(),
            "{\n  \"z\": 1.0000000000000001,\n  \"a\": [],\n  \"s\": \"x, {y}: \\\"q\\\"\",\n  \
             \"o\": {\n    \"k\": [\n      1,\n      {}\n    ]\n  }\n}\n"
        );
        assert_eq!(reindent("[ ]").unwrap(), "[]\n");
        assert_eq!(reindent("42"), None);
        assert_eq!(reindent("{"), None);
        assert_eq!(reindent("a = 1"), None);
    }

    #[test]
    fn one_line_json_becomes_a_multi_line_diff() {
        let hunks = [hunk(vec![
            line(LineOp::Removed, r#"{"a":1,"b":2,"c":3}"#),
            line(LineOp::Added, r#"{"a":1,"b":5,"c":3}"#),
        ])];
        let out = json(&hunks).unwrap();
        assert_eq!(out.len(), 1);
        let texts: Vec<(LineOp, &str)> = out[0]
            .lines
            .iter()
            .map(|l| (l.op, l.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            [
                (LineOp::Context, "{"),
                (LineOp::Context, r#"  "a": 1,"#),
                (LineOp::Removed, r#"  "b": 2,"#),
                (LineOp::Added, r#"  "b": 5,"#),
                (LineOp::Context, r#"  "c": 3"#),
                (LineOp::Context, "}"),
            ]
        );
        assert_eq!(out[0].lines[3].spans, [(7, 8)]);
    }

    #[test]
    fn an_added_file_and_non_json_hunks() {
        let added = [hunk(vec![line(LineOp::Added, r#"{"a":1}"#)])];
        let out = json(&added).unwrap();
        assert!(out[0].lines.iter().all(|l| l.op == LineOp::Added));
        assert_eq!(out[0].lines.len(), 3);

        let text = [hunk(vec![
            line(LineOp::Removed, "a = 1"),
            line(LineOp::Added, "a = 2"),
        ])];
        assert_eq!(json(&text), None);
        // Whitespace only: the raw hunk shows what changed.
        let spaces = [hunk(vec![
            line(LineOp::Removed, r#"{"a": 1}"#),
            line(LineOp::Added, r#"{"a":1}"#),
        ])];
        assert_eq!(json(&spaces), None);
    }
}
