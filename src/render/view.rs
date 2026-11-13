//! The view model of text output (spec 8.3), built to be reused by the TUI (spec 8.4).
//!
//! [`configuration`] and [`shared`] turn report data into a tree of [`Node`]s. A node is one
//! output row: styled [`Span`]s (text, foreground colour, bold, dim; spec 8.5), an indent in
//! columns relative to its parent row, a [`Kind`], and its child rows. Rows are complete as
//! built: labels, tags, colours, column alignment and `(also in N)`. What a reader decides is
//! which nodes are open:
//!
//! - Collapsible nodes are [`Kind::Rebuilt`], [`Kind::Churn`], and [`Kind::File`] with children
//!   (its hunk lines); see [`Node::collapsible`]. Every other node always shows its children.
//!   Within one configuration's tree a collapsible node's `Kind` is unique, so it can key the
//!   TUI's open state.
//! - Writing: `text::write_nodes` writes nodes under any open state (the TUI's `y`);
//!   `text::write_as_text` writes them as text output does (the TUI's `Y`): everything open
//!   except the rebuilt group, churn groups only with `--expand-churn`, and a closed churn
//!   group's row with the `--expand-churn` hint.
//! - Filtering: [`Kind::Package`] carries the pname, [`Kind::File`] the path and `via`, both
//!   raw. [`Kind::Boot`] rows, and [`Kind::Text`] rows (headers, status lines, hunk lines),
//!   carry no name. A filtered group's count goes stale: relabel it with [`rebuilt_label`] or
//!   [`churn_label`].
//! - Tree view: a file row's spans are `[tag, " ", display path, details…]`, the display path
//!   at [`FILE_PATH_SPAN`]; regrouping replaces that span's text with the part below the added
//!   directory node, passed through [`escape`]. `via` (after ` → `) never splits.
//!
//! Span text is escaped ([`escape`]): file contents, paths, link targets and messages come from
//! the machines compared and must not reach the terminal or the clipboard as control
//! sequences. Widths are display widths of the escaped text.

use crate::files::refs::display_path;
use crate::report::{
    Boot, BootChange, ChurnClass, ClosureKind, Configuration, FileChange, FileStatus, FileType,
    Files, Hunk, Line, LineOp, LinkChange, Mode, PackageChangeKind, Packages, Rebuilt,
    SharedChange, SharedItem, Side, SpecialisationStatus, Status,
};
use crate::shared::{AlsoIn, boot_rows};
use crate::size::{format_delta, format_size};
use unicode_width::UnicodeWidthStr;

/// Foreground colours of the 16-colour palette that the tables of spec 7.2, 7.3 and 7.5 use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Red,
    Green,
    Yellow,
    Magenta,
    Cyan,
}

/// `color: None` is the terminal's default foreground.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub color: Option<Color>,
    pub bold: bool,
    pub dim: bool,
}

impl Style {
    const PLAIN: Self = Self {
        color: None,
        bold: false,
        dim: false,
    };
    const BOLD: Self = Self {
        bold: true,
        ..Self::PLAIN
    };
    const DIM: Self = Self {
        dim: true,
        ..Self::PLAIN
    };

    fn fg(color: Option<Color>) -> Self {
        Self {
            color,
            ..Self::PLAIN
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

/// What a row is, for folding and filtering.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Headers, status lines, hunk lines: no name, never collapsible.
    Text,
    /// A package change, a rebuilt package, or a shared package change, by pname.
    Package(String),
    /// The rebuilt group; its children are [`Kind::Package`] rows.
    Rebuilt,
    /// A file change; its children are its hunk lines.
    File { path: String, via: Vec<String> },
    /// A churn group (`hash-only` or `version-only`); its children are [`Kind::File`] nodes.
    Churn(ChurnClass),
    /// A Boot row.
    Boot,
}

/// Index of the display path in a [`Kind::File`] row's spans.
pub const FILE_PATH_SPAN: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Columns, relative to the parent row's indent.
    pub indent: usize,
    pub spans: Vec<Span>,
    pub kind: Kind,
    pub children: Vec<Node>,
}

impl Node {
    fn new(indent: usize, spans: Vec<Span>, kind: Kind, children: Vec<Node>) -> Self {
        Self {
            indent,
            spans,
            kind,
            children,
        }
    }

    fn text(indent: usize, text: impl Into<String>, style: Style) -> Self {
        Self::new(indent, vec![span(text, style)], Kind::Text, Vec::new())
    }

    pub fn collapsible(&self) -> bool {
        !self.children.is_empty()
            && matches!(
                self.kind,
                Kind::Rebuilt | Kind::Churn(_) | Kind::File { .. }
            )
    }
}

fn span(text: impl Into<String>, style: Style) -> Span {
    Span {
        text: escape(&text.into()),
        style,
    }
}

/// `text` with control characters made visible: C0 controls except tab, and DEL, in caret
/// notation (`^[` for ESC, `^J` for a newline, `^?` for DEL); C1 controls as `\u{9b}`.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\t' => out.push(c),
            '\0'..='\x1f' => {
                out.push('^');
                out.push(char::from(c as u8 + 0x40));
            }
            '\x7f' => out.push_str("^?"),
            '\u{80}'..='\u{9f}' => out.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
            _ => out.push(c),
        }
    }
    out
}

/// The shared section: a `Shared across configurations` row whose children are the shared
/// changes, each followed by its configurations. Empty without shared changes.
pub fn shared(items: &[SharedChange]) -> Vec<Node> {
    if items.is_empty() {
        return Vec::new();
    }
    let pname_width = items
        .iter()
        .filter_map(|s| match &s.item {
            SharedItem::Packages(p) => Some(width(&p.pname)),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let rows: Vec<(Vec<Span>, Kind)> = items
        .iter()
        .map(|s| match &s.item {
            SharedItem::Packages(p) => {
                let versions = version_text(p.kind, &p.old_versions, &p.new_versions);
                let rest = columns(&p.pname, pname_width, None, &versions);
                (
                    package_row(p.kind, &p.pname, rest),
                    Kind::Package(p.pname.clone()),
                )
            }
            SharedItem::Files(f) => (file_row(f, false), file_kind(f)),
            SharedItem::Boot(row) => (boot_row(row), Kind::Boot),
        })
        .collect();
    let label_width = rows.iter().map(|(s, _)| spans_width(s)).max().unwrap_or(0);
    let children = rows
        .into_iter()
        .zip(items)
        .map(|((mut spans, kind), s)| {
            let pad = label_width - spans_width(&spans) + 2;
            let names = s.configurations.join(", ");
            spans.push(span(format!("{:pad$}{names}", ""), Style::PLAIN));
            Node::new(2, spans, kind, Vec::new())
        })
        .collect();
    vec![Node::new(
        0,
        vec![span("Shared across configurations", Style::BOLD)],
        Kind::Text,
        children,
    )]
}

/// One configuration as in spec 8.3: its header row (with the `old`, `new` and `system` rows,
/// or the status row, as children), then the Packages, Files and Boot sections of a `done`
/// configuration. `shared` gives the `(also in N)` marks.
pub fn configuration(mode: &Mode, shared: &[SharedChange], c: &Configuration) -> Vec<Node> {
    let header = format!("{} · {} ({})", c.name, kind_name(c.kind), location(mode, c));
    let mut rows = Vec::new();
    let mut sections = Vec::new();
    match &c.status {
        Status::Done(s) => {
            for (label, side) in [("old", &c.old), ("new", &c.new)] {
                if let Some(side) = side {
                    rows.push(Node::text(2, format!("{label} {}", side.path), Style::DIM));
                }
            }
            rows.extend(system(c).map(|line| Node::text(2, line, Style::PLAIN)));
            if c.kind == ClosureKind::Unknown {
                let note = "unknown closure kind: no followed trees, no Boot section";
                rows.push(Node::text(2, note, Style::PLAIN));
            }
            let also = AlsoIn::new(shared);
            sections.push(packages(&s.packages, &also));
            sections.extend(files(&s.files, &also));
            sections.extend(s.boot.as_ref().and_then(|b| boot(b, &also)));
        }
        Status::NoChanges => {
            let what = match mode {
                Mode::Deployed => "already running",
                Mode::Revision { .. } | Mode::Paths => "same toplevel",
            };
            let path = c.new.as_ref().or(c.old.as_ref()).map_or("", |s| &s.path);
            let line = format!("no changes, {what} {path}");
            rows.push(Node::text(2, line.trim_end(), Style::DIM));
        }
        Status::Error { error } => rows.push(message("error: ", error)),
        Status::Timeout { error } => rows.push(message("", error)),
        Status::AbsentAtBase => {
            let rev = match mode {
                Mode::Revision { base, .. } => short(&base.rev),
                _ => "",
            };
            let line = format!("not present at base {rev}");
            rows.push(Node::text(2, line.trim_end(), Style::DIM));
        }
    }
    let mut nodes = vec![Node::new(
        0,
        vec![span(header, Style::BOLD)],
        Kind::Text,
        rows,
    )];
    nodes.extend(sections);
    nodes
}

/// A failure message in red, its continuation lines indented under its first.
fn message(prefix: &str, text: &str) -> Node {
    let mut lines = text.lines();
    let first = format!("{prefix}{}", lines.next().unwrap_or(""));
    let rest = lines.map(|l| Node::text(width(prefix), l, red())).collect();
    Node::new(2, vec![span(first, red())], Kind::Text, rest)
}

/// The rebuilt group's row: `412 rebuilt (same version, new hash, +3.1 MiB)`.
pub fn rebuilt_label(count: usize, size_delta: i64) -> String {
    format!(
        "{count} rebuilt (same version, new hash, {})",
        format_delta(size_delta)
    )
}

/// A churn group's row: `57 hash-only (store hash churn)`; with `hint`, text output's closed
/// form `57 hash-only (store hash churn; --expand-churn shows them)`.
pub fn churn_label(class: ChurnClass, count: usize, hint: bool) -> String {
    let (name, about) = match class {
        ChurnClass::HashOnly => ("hash-only", "store hash churn"),
        ChurnClass::VersionOnly => ("version-only", "store path version bumps"),
        ChurnClass::Real => ("real", "real changes"),
    };
    let hint = if hint {
        "; --expand-churn shows them"
    } else {
        ""
    };
    format!("{count} {name} ({about}{hint})")
}

fn kind_name(kind: ClosureKind) -> &'static str {
    match kind {
        ClosureKind::Nixos => "nixos",
        ClosureKind::NixDarwin => "nix-darwin",
        ClosureKind::HomeManager => "home-manager",
        ClosureKind::Unknown => "unknown",
    }
}

/// The old side's location; a configuration that failed before resolving it names its
/// target (ADR 0006), and no target means this machine.
fn location(mode: &Mode, c: &Configuration) -> String {
    match mode {
        Mode::Revision { base, .. } => format!("{} -> working tree", short(&base.rev)),
        Mode::Deployed | Mode::Paths => c
            .old
            .as_ref()
            .map(|side| side.location.as_str())
            .or(c.target.as_deref())
            .unwrap_or("local")
            .to_owned(),
    }
}

fn short(rev: &str) -> &str {
    rev.get(..7).unwrap_or(rev)
}

/// `system A -> B`, or `system A` when both sides run the same version.
fn system(c: &Configuration) -> Option<String> {
    let version = |side: &Option<Side>| side.as_ref().and_then(|s| s.system_version.clone());
    match (version(&c.old), version(&c.new)) {
        (None, None) => None,
        (Some(old), Some(new)) if old == new => Some(format!("system {old}")),
        (old, new) => Some(format!(
            "system {} -> {}",
            old.as_deref().unwrap_or("(none)"),
            new.as_deref().unwrap_or("(none)")
        )),
    }
}

fn red() -> Style {
    Style::fg(Some(Color::Red))
}

fn packages(p: &Packages, also: &AlsoIn) -> Node {
    let header = format!(
        "Packages  {} -> {} ({})",
        format_size(p.size_old),
        format_size(p.size_new),
        format_delta(delta(p.size_old, p.size_new))
    );
    let pname_width = p.changes.iter().map(|c| width(&c.pname)).max();
    let deltas: Vec<String> = p
        .changes
        .iter()
        .map(|c| format_delta(c.size_delta))
        .collect();
    let delta_width = deltas.iter().map(|d| width(d)).max().unwrap_or(0);
    let mut rows: Vec<Node> = p
        .changes
        .iter()
        .zip(&deltas)
        .map(|(c, delta)| {
            let versions = version_text(c.kind, &c.old_versions, &c.new_versions);
            let rest = columns(
                &c.pname,
                pname_width.unwrap_or(0),
                Some((delta, delta_width)),
                &versions,
            );
            let mut spans = package_row(c.kind, &c.pname, rest);
            push_also(&mut spans, also.package(c));
            Node::new(2, spans, Kind::Package(c.pname.clone()), Vec::new())
        })
        .collect();
    if p.rebuilt.count > 0 {
        rows.push(rebuilt(&p.rebuilt));
    }
    if rows.is_empty() {
        rows.push(Node::text(2, "no package changes", Style::PLAIN));
    }
    Node::new(0, vec![span(header, Style::BOLD)], Kind::Text, rows)
}

/// The collapsed rebuilt line; open, it lists each rebuilt pname with its size delta and
/// versions.
fn rebuilt(r: &Rebuilt) -> Node {
    let line = rebuilt_label(r.count, r.size_delta);
    let pname_width = r.items.iter().map(|i| width(&i.pname)).max().unwrap_or(0);
    let deltas: Vec<String> = r.items.iter().map(|i| format_delta(i.size_delta)).collect();
    let delta_width = deltas.iter().map(|d| width(d)).max().unwrap_or(0);
    let items = r
        .items
        .iter()
        .zip(&deltas)
        .map(|(item, delta)| {
            let versions = one_side(&item.versions);
            let rest = columns(
                &item.pname,
                pname_width,
                Some((delta, delta_width)),
                &versions,
            );
            let spans = vec![span(&item.pname, Style::PLAIN), span(rest, Style::PLAIN)];
            Node::new(2, spans, Kind::Package(item.pname.clone()), Vec::new())
        })
        .collect();
    Node::new(2, vec![span(line, Style::PLAIN)], Kind::Rebuilt, items)
}

fn delta(old: u64, new: u64) -> i64 {
    let delta = i128::from(new) - i128::from(old);
    i64::try_from(delta).unwrap_or(if delta < 0 { i64::MIN } else { i64::MAX })
}

/// `[U] openssl` in the kind's colour, then `rest`.
fn package_row(kind: PackageChangeKind, pname: &str, rest: String) -> Vec<Span> {
    let (tag, color) = match kind {
        PackageChangeKind::Upgraded => ("[U]", Some(Color::Cyan)),
        PackageChangeKind::Downgraded => ("[D]", Some(Color::Yellow)),
        PackageChangeKind::Changed => ("[C]", Some(Color::Magenta)),
        PackageChangeKind::Added => ("[A]", Some(Color::Green)),
        PackageChangeKind::Removed => ("[R]", Some(Color::Red)),
        PackageChangeKind::Resized => ("[S]", None),
    };
    let style = Style::fg(color);
    vec![
        span(tag, style),
        span(" ", Style::PLAIN),
        span(pname, style),
        span(rest, Style::PLAIN),
    ]
}

/// What follows a pname: padding to `name_width`, the delta padded to its column, the
/// versions; trailing spaces trimmed.
fn columns(name: &str, name_width: usize, delta: Option<(&str, usize)>, versions: &str) -> String {
    let mut rest = format!(
        "{:pad$}  ",
        "",
        pad = name_width.saturating_sub(width(name))
    );
    if let Some((delta, delta_width)) = delta {
        rest += &format!("{delta:<delta_width$} ");
    }
    rest += versions;
    rest.trim_end().to_owned()
}

/// Version text of a row (spec 7.2): added and resized rows show the new versions, removed
/// rows the old ones, the rest `old -> new` as a set difference, or the full sets in braces
/// when one side's difference is empty.
fn version_text(kind: PackageChangeKind, old: &[String], new: &[String]) -> String {
    match kind {
        PackageChangeKind::Added | PackageChangeKind::Resized => one_side(new),
        PackageChangeKind::Removed => one_side(old),
        _ => {
            let only = |a: &[String], b: &[String]| -> Vec<String> {
                a.iter().filter(|v| !b.contains(v)).cloned().collect()
            };
            let (old_only, new_only) = (only(old, new), only(new, old));
            if old_only.is_empty() || new_only.is_empty() {
                format!("{{{}}} -> {{{}}}", list(old), list(new))
            } else {
                format!("{} -> {}", list(&old_only), list(&new_only))
            }
        }
    }
}

/// A versionless package alone shows no version.
fn one_side(versions: &[String]) -> String {
    match versions {
        [only] if only.is_empty() => String::new(),
        _ => list(versions),
    }
}

fn list(versions: &[String]) -> String {
    versions
        .iter()
        .map(|v| if v.is_empty() { "(none)" } else { v })
        .collect::<Vec<_>>()
        .join(", ")
}

fn files(f: &Files, also: &AlsoIn) -> Option<Node> {
    if f.changes.is_empty() {
        return None;
    }
    let mut rows: Vec<Node> = f
        .changes
        .iter()
        .filter(|c| c.class == ChurnClass::Real)
        .map(|c| file(c, also.file(c)))
        .collect();
    for class in [ChurnClass::HashOnly, ChurnClass::VersionOnly] {
        let group: Vec<Node> = f
            .changes
            .iter()
            .filter(|c| c.class == class)
            .map(|c| file(c, None))
            .collect();
        if !group.is_empty() {
            let label = churn_label(class, group.len(), false);
            let spans = vec![span(label, Style::PLAIN)];
            rows.push(Node::new(2, spans, Kind::Churn(class), group));
        }
    }
    Some(Node::new(
        0,
        vec![span("Files", Style::BOLD)],
        Kind::Text,
        rows,
    ))
}

fn file(c: &FileChange, also: Option<usize>) -> Node {
    let mut spans = file_row(c, true);
    push_also(&mut spans, also);
    Node::new(2, spans, file_kind(c), hunk_lines(&c.hunks))
}

fn file_kind(c: &FileChange) -> Kind {
    Kind::File {
        path: c.path.clone(),
        via: c.via.clone(),
    }
}

/// `[M] path  details`, tag and path in the status colour. Shared rows leave sizes out, since
/// sizes are not compared (8.2).
fn file_row(c: &FileChange, sizes: bool) -> Vec<Span> {
    let (tag, color) = match c.status {
        FileStatus::Modified if c.binary => ("[B]", Color::Magenta),
        FileStatus::Modified => ("[M]", Color::Cyan),
        FileStatus::Added => ("[A]", Color::Green),
        FileStatus::Removed => ("[R]", Color::Red),
        FileStatus::LinkChanged => ("[L]", Color::Cyan),
        FileStatus::TypeChanged => ("[T]", Color::Magenta),
    };
    let style = Style::fg(Some(color));
    let mut spans = vec![
        span(tag, style),
        span(" ", Style::PLAIN),
        span(display_path(c), style),
    ];
    if let Some(details) = file_details(c, sizes) {
        spans.push(span(format!("  {details}"), Style::PLAIN));
    }
    spans
}

fn file_details(c: &FileChange, sizes: bool) -> Option<String> {
    let size = |s: Option<u64>| s.map_or_else(|| "(none)".to_owned(), format_size);
    let with_sizes = |what: &str| {
        if !sizes {
            return format!("({what})");
        }
        let sizes = match c.status {
            FileStatus::Added => size(c.size_new),
            FileStatus::Removed => size(c.size_old),
            _ => format!("{} -> {}", size(c.size_old), size(c.size_new)),
        };
        format!("({what}, {sizes})")
    };
    let present = match c.status {
        FileStatus::Added => c.type_new,
        FileStatus::Removed => c.type_old,
        FileStatus::Modified => Some(FileType::File),
        FileStatus::LinkChanged => {
            let link = |l: &Option<String>| l.clone().unwrap_or_default();
            return Some(format!("{} -> {}", link(&c.link_old), link(&c.link_new)));
        }
        FileStatus::TypeChanged => {
            return Some(format!(
                "{} -> {}",
                describe(c.type_old, &c.link_old),
                describe(c.type_new, &c.link_new)
            ));
        }
    };
    if present != Some(FileType::File) {
        let link = if c.status == FileStatus::Added {
            &c.link_new
        } else {
            &c.link_old
        };
        Some(describe(present, link))
    } else if !c.content_fetched {
        Some(with_sizes("content not fetched"))
    } else if c.binary {
        Some(with_sizes("binary"))
    } else {
        None
    }
}

/// `file`, `external link /dev/null`, as in a `[T]` row.
fn describe(file_type: Option<FileType>, link: &Option<String>) -> String {
    let link = link.as_deref().unwrap_or("");
    match file_type {
        None => "(none)".to_owned(),
        Some(FileType::File) => "file".to_owned(),
        Some(FileType::Directory) => "directory".to_owned(),
        Some(FileType::ExternalLink) => format!("external link {link}").trim_end().to_owned(),
        Some(FileType::DanglingLink) => format!("dangling link {link}").trim_end().to_owned(),
    }
}

fn hunk_lines(hunks: &[Hunk]) -> Vec<Node> {
    let mut rows = Vec::new();
    for h in hunks {
        let header = format!(
            "@@ -{},{} +{},{} @@",
            h.old_start, h.old_lines, h.new_start, h.new_lines
        );
        rows.push(Node::text(4, header, Style::DIM));
        for line in &h.lines {
            rows.push(Node::new(4, line_spans(line), Kind::Text, Vec::new()));
            if line.no_newline {
                rows.push(Node::text(4, "\\ No newline at end of file", Style::DIM));
            }
        }
    }
    rows
}

/// A hunk line in its op's colour, highlighted tokens bold; a hash-only pair dimmed, without
/// bold (spec 7.3). A span that does not fit the text leaves the rest plain.
fn line_spans(line: &Line) -> Vec<Span> {
    let (op, color) = match line.op {
        LineOp::Context => (" ", None),
        LineOp::Removed => ("-", Some(Color::Red)),
        LineOp::Added => ("+", Some(Color::Green)),
    };
    let plain = Style {
        color,
        dim: line.hash_only,
        bold: false,
    };
    let bold = Style {
        bold: true,
        ..plain
    };
    let mut spans = Vec::new();
    push(&mut spans, op, plain);
    let mut at = 0;
    if !line.hash_only {
        for &(start, end) in &line.spans {
            let (Some(before), Some(token)) = (line.text.get(at..start), line.text.get(start..end))
            else {
                break;
            };
            push(&mut spans, before, plain);
            push(&mut spans, token, bold);
            at = end;
        }
    }
    push(&mut spans, &line.text[at..], plain);
    spans
}

fn push(spans: &mut Vec<Span>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    match spans.last_mut() {
        Some(last) if last.style == style => last.text.push_str(&escape(text)),
        _ => spans.push(span(text, style)),
    }
}

fn boot(b: &Boot, also: &AlsoIn) -> Option<Node> {
    let rows: Vec<Node> = boot_rows(b)
        .iter()
        .map(|row| {
            let mut spans = boot_row(row);
            push_also(&mut spans, also.boot(row));
            Node::new(2, spans, Kind::Boot, Vec::new())
        })
        .collect();
    (!rows.is_empty()).then(|| Node::new(0, vec![span("Boot", Style::BOLD)], Kind::Text, rows))
}

/// One row of the table in spec 7.5.
fn boot_row(row: &BootChange) -> Vec<Span> {
    let link = |label: &str, l: &LinkChange| {
        let name = |n: &Option<String>| n.clone().unwrap_or_else(|| "(none)".to_owned());
        let text = if l.rebuilt {
            format!("{label}: {} (rebuilt)", name(&l.new))
        } else {
            format!("{label}: {} -> {}", name(&l.old), name(&l.new))
        };
        vec![span(text, Style::PLAIN)]
    };
    match row {
        BootChange::Kernel(l) => link("kernel", l),
        BootChange::Initrd(l) => link("initrd", l),
        BootChange::KernelModules(l) => link("kernel-modules", l),
        BootChange::KernelParams(p) => {
            let text = |t: &str| if t.is_empty() { "(none)" } else { t }.to_owned();
            let line = format!("kernel-params: {} -> {}", text(&p.old), text(&p.new));
            vec![span(line, Style::PLAIN)]
        }
        BootChange::Specialisation { name, status } => {
            let (tag, color, suffix) = match status {
                SpecialisationStatus::Added => ("[A]", Color::Green, ""),
                SpecialisationStatus::Removed => ("[R]", Color::Red, ""),
                SpecialisationStatus::Changed => ("[M]", Color::Cyan, " (changed)"),
            };
            let style = Style::fg(Some(color));
            vec![
                span(tag, style),
                span(" ", Style::PLAIN),
                span(format!("specialisation {name}{suffix}"), style),
            ]
        }
    }
}

fn push_also(spans: &mut Vec<Span>, also: Option<usize>) {
    if let Some(n) = also {
        spans.push(span(format!("  (also in {n})"), Style::PLAIN));
    }
}

/// Display columns of `text` once escaped; escaping escaped text changes nothing.
fn width(text: &str) -> usize {
    escape(text).width()
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| width(&s.text)).sum()
}
