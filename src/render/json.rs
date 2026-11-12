//! `--format json` (spec 8.1): the report as it serialises, pretty-printed, with a final
//! newline. The field rules live in the `report` types.

use std::io::{self, Write};

use crate::report::Report;

/// DEL and the C1 controls are written as `\u00XX` escapes, as serde_json does for C0, so a
/// terminal showing the JSON never receives them raw; the data decodes unchanged. Only string
/// contents can hold them, the JSON structure being ASCII.
pub fn write(report: &Report, out: &mut impl Write) -> io::Result<()> {
    let json = serde_json::to_string_pretty(report)?;
    let mut escaped = String::with_capacity(json.len());
    for c in json.chars() {
        match c {
            '\x7f'..='\u{9f}' => escaped.push_str(&format!("\\u{:04x}", u32::from(c))),
            _ => escaped.push(c),
        }
    }
    escaped.push('\n');
    out.write_all(escaped.as_bytes())
}
