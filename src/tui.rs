//! The interactive viewer (spec 8.4). The run calls [`run`] after the build step, before
//! fetching, with the report header, the selected names and the receiving end of the live
//! status channel (`docs/architecture.md`). The viewer shows each configuration as it arrives,
//! recomputes shared changes and `(also in N)` from what has arrived (ADR 0003), and returns
//! when the user quits.
//!
//! Rows come from `render::view`, so the detail pane shows what text output prints (8.3); the
//! viewer adds folding, the filter, the tree view of file paths, wrapping and scrolling
//! (`rows`), the state and keys (`app`), and the screen (`draw`).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::cli::WordHighlight;
use crate::report::Configuration;

pub mod app;
pub mod draw;
mod rows;
#[cfg(not(target_arch = "wasm32"))]
mod terminal;

#[cfg(not(target_arch = "wasm32"))]
pub use terminal::run;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// `--expand-churn`: churn groups start open.
    pub expand_churn: bool,
    /// Colours and attributes; decided by `render::color_enabled`.
    pub color: bool,
    /// `--word-highlight`.
    pub word_highlight: WordHighlight,
}

/// What the viewer hands back when the user quits.
#[derive(Debug)]
pub struct Finished {
    /// Every selected configuration arrived. When not, the missing ones count as failed
    /// (spec 4.7).
    pub all_finished: bool,
    /// The configurations received, in name order.
    pub configurations: Vec<Configuration>,
}

/// The OSC 52 escape that sets the system clipboard to `text`.
fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text))
}

#[cfg(test)]
mod tests;
