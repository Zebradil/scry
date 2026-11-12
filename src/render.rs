use std::ffi::OsStr;

use crate::cli::Color;

/// `--format json`: the report as JSON (spec 8.1).
pub mod json;
/// Plain text output (spec 8.3).
pub mod text;
/// The styled node tree behind text output, shared with the TUI (spec 8.3, 8.4, 8.5).
pub mod view;

#[cfg(test)]
mod tests;

/// Whether output gets colour (spec 4.6, 4.8): `auto` colours a terminal unless `NO_COLOR` is
/// set to a non-empty value. The run passes `std::env::var_os("NO_COLOR")` and whether stdout
/// is a terminal.
pub fn color_enabled(choice: Color, no_color: Option<&OsStr>, stdout_is_terminal: bool) -> bool {
    match choice {
        Color::Always => true,
        Color::Never => false,
        Color::Auto => stdout_is_terminal && no_color.is_none_or(OsStr::is_empty),
    }
}
