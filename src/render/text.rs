//! Text output (spec 8.3): the [`view`] nodes of the shared section and of each configuration,
//! written with text output's open state, with or without ANSI colour (spec 8.5).

use std::io::{self, Write};

use crate::render::view::{self, Color, Kind, Node, Span};
use crate::report::{Configuration, Report};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// ANSI colour, bold and dim; see `render::color_enabled`.
    pub color: bool,
    /// `--expand-churn`.
    pub expand_churn: bool,
}

/// The whole report: the shared section when there are two or more configurations, then each
/// configuration in name order, blocks separated by one blank line.
pub fn write(report: &Report, options: Options, out: &mut impl Write) -> io::Result<()> {
    let mut configurations: Vec<&Configuration> = report.configurations.iter().collect();
    configurations.sort_by(|a, b| a.name.cmp(&b.name));
    let shared = if configurations.len() >= 2 {
        view::shared(&report.shared)
    } else {
        Vec::new()
    };
    let blocks = std::iter::once(shared).chain(
        configurations
            .iter()
            .map(|c| view::configuration(&report.mode, &report.shared, c)),
    );
    for (i, nodes) in blocks.filter(|b| !b.is_empty()).enumerate() {
        if i > 0 {
            writeln!(out)?;
        }
        write_as_text(&nodes, options, out)?;
    }
    Ok(())
}

/// Text output's open state: the rebuilt group closed, churn groups open only with
/// `--expand-churn`, everything else open.
pub fn open(node: &Node, expand_churn: bool) -> bool {
    match node.kind {
        Kind::Rebuilt => false,
        Kind::Churn(_) => expand_churn,
        _ => true,
    }
}

/// Writes `nodes` as text output prints them: its open state, and a closed churn group's row
/// with the `--expand-churn` hint. The TUI's `Y`.
pub fn write_as_text(nodes: &[Node], options: Options, out: &mut impl Write) -> io::Result<()> {
    let open = |node: &Node| open(node, options.expand_churn);
    if options.expand_churn {
        return write_nodes(nodes, open, options.color, out);
    }
    let mut nodes = nodes.to_vec();
    hint_churn(&mut nodes);
    write_nodes(&nodes, open, options.color, out)
}

fn hint_churn(nodes: &mut [Node]) {
    for node in nodes {
        if let Kind::Churn(class) = node.kind {
            let style = node.spans.first().map(|s| s.style).unwrap_or_default();
            let text = view::churn_label(class, node.children.len(), true);
            node.spans = vec![Span { text, style }];
        }
        hint_churn(&mut node.children);
    }
}

/// Writes `nodes` one row per line, the children of the nodes `open` accepts, with or without
/// ANSI colour. The TUI's `y` passes its own open state.
pub fn write_nodes(
    nodes: &[Node],
    open: impl Fn(&Node) -> bool,
    color: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    write_level(nodes, 0, &open, color, out)
}

fn write_level<W: Write>(
    nodes: &[Node],
    indent: usize,
    open: &impl Fn(&Node) -> bool,
    color: bool,
    out: &mut W,
) -> io::Result<()> {
    for node in nodes {
        let indent = indent + node.indent;
        write!(out, "{:indent$}", "")?;
        for s in &node.spans {
            write_span(out, s, color)?;
        }
        writeln!(out)?;
        if open(node) {
            write_level(&node.children, indent, open, color, out)?;
        }
    }
    Ok(())
}

fn write_span(out: &mut impl Write, span: &Span, color: bool) -> io::Result<()> {
    let style = span.style;
    if !color || style == Default::default() {
        return out.write_all(span.text.as_bytes());
    }
    let mut codes = Vec::new();
    if style.bold {
        codes.push("1");
    }
    if style.dim {
        codes.push("2");
    }
    codes.extend(style.color.map(|c| match c {
        Color::Red => "31",
        Color::Green => "32",
        Color::Yellow => "33",
        Color::Magenta => "35",
        Color::Cyan => "36",
    }));
    write!(out, "\x1b[{}m{}\x1b[0m", codes.join(";"), span.text)
}
