//! The detail pane's rows (spec 8.4): an entry's view nodes filtered, regrouped into
//! directories in tree view, flattened under the open state, then wrapped or cut to the pane.

use std::collections::HashMap;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::render::view::{self, FILE_PATH_SPAN, Kind, Node, Span, Style};

/// Lines kept between the cursor and the pane's edge where possible (spec 12).
pub const MARGIN: usize = 5;
const TAB: usize = 8;

/// A collapsible row: a view node by its kind, unique within one entry's tree (`render::view`),
/// or a tree-view directory by the kind of the node listing its files and its path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Node(Kind),
    Dir(Kind, String),
}

/// How an entry's nodes become rows.
pub struct Shape<'a> {
    /// Lowercase; empty for no filter.
    pub filter: &'a str,
    pub tree: bool,
    pub expand_churn: bool,
    /// Open state the reader set; other rows keep their default.
    pub open: &'a HashMap<Key, bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Columns from the pane's left edge.
    pub indent: usize,
    pub spans: Vec<Span>,
    /// Set on a collapsible row.
    pub key: Option<Key>,
    pub open: bool,
    pub parent: Option<usize>,
    /// The view node's kind; [`Kind::Text`] for a directory.
    pub kind: Kind,
    pub leaf: bool,
}

impl Row {
    /// Where `J`/`K` stop.
    pub fn actionable(&self) -> bool {
        self.key.is_some() || self.kind != Kind::Text
    }

    /// The row unwrapped, as `y` copies it.
    pub fn text(&self) -> String {
        let spans: String = self.spans.iter().map(|s| s.text.as_str()).collect();
        format!("{:indent$}{spans}", "", indent = self.indent)
    }
}

/// Default open state (spec 8.4): package groups open, rebuilt groups and file changes closed,
/// churn groups closed unless `--expand-churn`, directories open.
fn default_open(key: &Key, expand_churn: bool) -> bool {
    match key {
        Key::Node(Kind::PackageGroup(_)) => true,
        Key::Node(Kind::Churn(_)) => expand_churn,
        Key::Node(_) => false,
        Key::Dir(..) => true,
    }
}

pub fn rows(nodes: Vec<Node>, shape: &Shape) -> Vec<Row> {
    let nodes = if shape.filter.is_empty() {
        nodes
    } else {
        filter(nodes, shape.filter)
    };
    let mut rows = Vec::new();
    flatten(
        items(nodes, &Kind::Text, shape.tree),
        0,
        None,
        shape,
        &mut rows,
    );
    rows
}

/// Keeps package rows whose pname and file rows whose path or `via` contains `needle`, every
/// Boot row and every text row; drops a group left empty and recounts the rest (spec 8.4).
fn filter(nodes: Vec<Node>, needle: &str) -> Vec<Node> {
    let hit = |text: &str| text.to_lowercase().contains(needle);
    let mut kept = Vec::new();
    for mut node in nodes {
        let keep = match node.kind.clone() {
            Kind::Package(pname) => hit(&pname),
            Kind::File { path, via } => hit(&path) || via.iter().any(|v| hit(v)),
            Kind::Boot => true,
            _ if node.children.is_empty() => true,
            kind => {
                node.children = filter(std::mem::take(&mut node.children), needle);
                let count = node.children.len();
                let label = match kind {
                    Kind::PackageGroup(group) => Some(view::package_group_label(group, count)),
                    Kind::Rebuilt => Some(view::rebuilt_label(count)),
                    Kind::Churn(class) => Some(view::churn_label(class, count, false)),
                    _ => None,
                };
                if let Some(text) = label {
                    let style = node.spans.first().map(|s| s.style).unwrap_or_default();
                    node.spans = vec![Span { text, style }];
                }
                count > 0
            }
        };
        if keep {
            kept.push(node);
        }
    }
    kept
}

/// A node with the tree view applied to its children.
struct Item {
    indent: usize,
    spans: Vec<Span>,
    key: Option<Key>,
    kind: Kind,
    children: Vec<Item>,
}

/// `nodes` as items; in tree view their file rows become a directory tree, placed where the
/// first file row was. `parent` is the kind of the node listing them.
fn items(nodes: Vec<Node>, parent: &Kind, tree: bool) -> Vec<Item> {
    let mut out = Vec::new();
    let mut files = Vec::new();
    let mut at = None;
    for node in nodes {
        if tree && matches!(node.kind, Kind::File { .. }) {
            at.get_or_insert(out.len());
            files.push(node);
        } else {
            out.push(item(node, tree));
        }
    }
    if let Some(at) = at {
        let indent = files.first().map_or(0, |f| f.indent);
        let root = directories(files);
        out.splice(at..at, dir_items(root.entries, "", indent, parent, tree));
    }
    out
}

fn item(node: Node, tree: bool) -> Item {
    let key = node.collapsible().then(|| Key::Node(node.kind.clone()));
    Item {
        indent: node.indent,
        children: items(node.children, &node.kind, tree),
        spans: node.spans,
        key,
        kind: node.kind,
    }
}

#[derive(Default)]
struct Dir {
    name: String,
    entries: Vec<Entry>,
}

enum Entry {
    Dir(Dir),
    File(Node),
}

impl Dir {
    fn child(&mut self, name: &str) -> &mut Dir {
        let found = self
            .entries
            .iter()
            .position(|e| matches!(e, Entry::Dir(d) if d.name == name));
        let at = found.unwrap_or_else(|| {
            self.entries.push(Entry::Dir(Dir {
                name: name.to_owned(),
                entries: Vec::new(),
            }));
            self.entries.len() - 1
        });
        match &mut self.entries[at] {
            Entry::Dir(d) => d,
            Entry::File(_) => unreachable!("position found a directory"),
        }
    }

    fn files(&self) -> usize {
        self.entries
            .iter()
            .map(|e| match e {
                Entry::Dir(d) => d.files(),
                Entry::File(_) => 1,
            })
            .sum()
    }
}

/// File rows by directory, in first-seen order; each row's display path cut to its name and
/// `via`, which never splits (spec 8.4).
fn directories(files: Vec<Node>) -> Dir {
    let mut root = Dir::default();
    for mut node in files {
        let Kind::File { path, via } = &node.kind else {
            unreachable!("only file rows are grouped")
        };
        let mut dirs: Vec<&str> = path.split('/').collect();
        let name = dirs.pop().unwrap_or_default();
        let label = std::iter::once(name)
            .chain(via.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" → ");
        let dir = dirs.iter().fold(&mut root, |dir, d| dir.child(d));
        if let Some(span) = node.spans.get_mut(FILE_PATH_SPAN) {
            span.text = view::escape(&label);
        }
        dir.entries.push(Entry::File(node));
    }
    root
}

/// Directory rows, a chain of single-child directories merged into one (`systemd/system/  (2)`).
fn dir_items(
    entries: Vec<Entry>,
    prefix: &str,
    indent: usize,
    parent: &Kind,
    tree: bool,
) -> Vec<Item> {
    entries
        .into_iter()
        .map(|entry| match entry {
            Entry::File(node) => item(node, tree),
            Entry::Dir(mut dir) => {
                let mut label = format!("{}/", dir.name);
                while let [Entry::Dir(_)] = dir.entries.as_slice() {
                    let Some(Entry::Dir(only)) = dir.entries.pop() else {
                        unreachable!("matched one directory")
                    };
                    label = format!("{label}{}/", only.name);
                    dir = only;
                }
                let path = format!("{prefix}{label}");
                let spans = vec![
                    Span {
                        text: view::escape(&label),
                        style: Style::default(),
                    },
                    Span {
                        text: format!("  ({})", dir.files()),
                        style: Style::default(),
                    },
                ];
                Item {
                    indent,
                    spans,
                    key: Some(Key::Dir(parent.clone(), path.clone())),
                    kind: Kind::Text,
                    children: dir_items(dir.entries, &path, indent, parent, tree),
                }
            }
        })
        .collect()
}

fn flatten(
    items: Vec<Item>,
    base: usize,
    parent: Option<usize>,
    shape: &Shape,
    rows: &mut Vec<Row>,
) {
    for item in items {
        let indent = base + item.indent;
        let open = item.key.as_ref().is_none_or(|key| {
            shape
                .open
                .get(key)
                .copied()
                .unwrap_or_else(|| default_open(key, shape.expand_churn))
        });
        let at = rows.len();
        rows.push(Row {
            indent,
            spans: item.spans,
            key: item.key,
            open,
            parent,
            kind: item.kind,
            leaf: item.children.is_empty(),
        });
        if open {
            flatten(item.children, indent, Some(at), shape, rows);
        }
    }
}

/// Realigns the names column of shared change rows: `render::view::shared` pads it for full
/// paths, and the tree view moves and shortens the labels.
pub fn align_names(rows: &mut [Row]) {
    let shared = |r: &Row| r.kind != Kind::Text && r.spans.len() >= 2;
    let label = |r: &Row| {
        let label = &r.spans[..r.spans.len() - 1];
        r.indent + label.iter().map(|s| s.text.width()).sum::<usize>()
    };
    let Some(column) = rows.iter().filter(|r| shared(r)).map(label).max() else {
        return;
    };
    for row in rows.iter_mut().filter(|r| shared(r)) {
        let pad = column - label(row) + 2;
        if let Some(names) = row.spans.last_mut() {
            names.text = format!("{:pad$}{}", "", names.text.trim_start());
        }
    }
}

/// The screen lines of `row` in `width` columns, tabs expanded. With `wrap`, a row wraps by
/// display width and continues under its text, at its indent, unless that leaves less than
/// half the width; without, it is one line for the pane to cut (spec 8.4).
pub fn lines(row: &Row, width: usize, wrap: bool) -> Vec<Vec<Span>> {
    let hang = if width.saturating_sub(row.indent) * 2 < width {
        0
    } else {
        row.indent
    };
    let mut lines = vec![Vec::new()];
    push(&mut lines, &" ".repeat(row.indent), Style::default());
    let (mut col, mut start, mut logical) = (row.indent, 0, row.indent);
    for span in &row.spans {
        for c in span.text.chars() {
            let (c, repeat) = if c == '\t' {
                (' ', TAB - logical % TAB)
            } else {
                (c, 1)
            };
            let w = c.width().unwrap_or(0);
            for _ in 0..repeat {
                if wrap && col + w > width && col > start {
                    lines.push(Vec::new());
                    push(&mut lines, &" ".repeat(hang), Style::default());
                    (col, start) = (hang, hang);
                }
                push(&mut lines, c.encode_utf8(&mut [0; 4]), span.style);
                col += w;
                logical += w;
            }
        }
    }
    lines
}

fn push(lines: &mut [Vec<Span>], text: &str, style: Style) {
    let Some(line) = lines.last_mut() else { return };
    if text.is_empty() {
        return;
    }
    match line.last_mut() {
        Some(last) if last.style == style => last.text.push_str(text),
        _ => line.push(Span {
            text: text.to_owned(),
            style,
        }),
    }
}

/// What finds `rows[at]` again after the rows change around it: its key; else a change's
/// kind and tag (its other text moves with `(also in N)` and All's names); else its text
/// without the indent, which the tree view changes, under its parent's key.
pub fn id(rows: &[Row], at: usize) -> (Option<Key>, Option<Key>, Kind, String) {
    let row = &rows[at];
    match (&row.key, &row.kind) {
        (Some(key), kind) => (Some(key.clone()), None, kind.clone(), String::new()),
        (None, Kind::Text) => {
            let parent = row.parent.and_then(|p| rows[p].key.clone());
            let text = row.spans.iter().map(|s| s.text.as_str()).collect();
            (None, parent, Kind::Text, text)
        }
        (None, kind) => {
            let tag = row.spans.first().map(|s| s.text.clone());
            (None, None, kind.clone(), tag.unwrap_or_default())
        }
    }
}

/// The first line to show of `total` in a pane `height` lines high, moved as little as
/// possible from `top` so that the cursor's lines `[start, end)` keep [`MARGIN`] lines from
/// both edges where possible. A cursor row taller than the pane fills it, scrolled within
/// the row.
pub fn scroll(top: usize, (start, end): (usize, usize), total: usize, height: usize) -> usize {
    let last = total.saturating_sub(height);
    if end - start > height {
        return top.clamp(start, end - height).min(last);
    }
    let margin = MARGIN.min((height - (end - start)) / 2);
    let mut top = top.min(last);
    if end + margin > top + height {
        top = end + margin - height;
    }
    if start < top + margin {
        top = start.saturating_sub(margin);
    }
    top.min(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::view::Color;
    use crate::report::ChurnClass;

    fn span(text: &str) -> Span {
        Span {
            text: text.into(),
            style: Style::default(),
        }
    }

    fn node(indent: usize, text: &str, kind: Kind, children: Vec<Node>) -> Node {
        Node {
            indent,
            spans: vec![span(text)],
            kind,
            children,
        }
    }

    fn text(indent: usize, text: &str) -> Node {
        node(indent, text, Kind::Text, vec![])
    }

    /// A file row as `render::view` builds it: tag, space, display path.
    fn file(path: &str, via: &[&str], hunk: bool) -> Node {
        let via: Vec<String> = via.iter().map(|v| v.to_string()).collect();
        let display = std::iter::once(path.to_owned())
            .chain(via.iter().cloned())
            .collect::<Vec<_>>()
            .join(" → ");
        let children = if hunk {
            vec![text(4, "@@ -1,1 +1,1 @@"), text(4, "-a"), text(4, "+b")]
        } else {
            vec![]
        };
        Node {
            indent: 2,
            spans: vec![span("[M]"), span(" "), span(&display)],
            kind: Kind::File {
                path: path.into(),
                via,
            },
            children,
        }
    }

    fn package(pname: &str) -> Node {
        node(2, pname, Kind::Package(pname.into()), vec![])
    }

    fn shape<'a>(open: &'a HashMap<Key, bool>, filter: &'a str, tree: bool) -> Shape<'a> {
        Shape {
            filter,
            tree,
            expand_churn: false,
            open,
        }
    }

    fn texts(rows: &[Row]) -> Vec<String> {
        rows.iter().map(Row::text).collect()
    }

    fn files_section(files: Vec<Node>) -> Vec<Node> {
        vec![node(0, "Files", Kind::Text, files)]
    }

    #[test]
    fn tree_view_groups_by_directory_in_first_seen_order_and_merges_chains() {
        let nodes = files_section(vec![
            file("etc/nix/nix.conf", &[], true),
            file("etc/mtab", &[], false),
            file("etc/systemd/system/debug-shell.service", &[], false),
            file(
                "etc/systemd/system/app.service",
                &["app-start/bin/app-start"],
                false,
            ),
            file("bin/switch", &[], false),
        ]);
        let open = HashMap::new();
        let rows = rows(nodes, &shape(&open, "", true));
        assert_eq!(
            texts(&rows),
            [
                "Files",
                "  etc/  (4)",
                "    nix/  (1)",
                "      [M] nix.conf",
                "    [M] mtab",
                "    systemd/system/  (2)",
                "      [M] debug-shell.service",
                "      [M] app.service → app-start/bin/app-start",
                "  bin/  (1)",
                "    [M] switch",
            ]
        );
        assert_eq!(
            rows[5].key,
            Some(Key::Dir(Kind::Text, "etc/systemd/system/".into()))
        );
        assert_eq!(rows[6].parent, Some(5));
        assert!(rows[3].key.is_some() && !rows[3].open, "file change closed");
    }

    #[test]
    fn via_never_splits_and_a_lone_chain_merges_from_the_top() {
        let nodes = files_section(vec![file("etc/a/b.conf", &["pkg/share/x/y"], false)]);
        let open = HashMap::new();
        assert_eq!(
            texts(&rows(nodes, &shape(&open, "", true))),
            ["Files", "  etc/a/  (1)", "    [M] b.conf → pkg/share/x/y"]
        );
    }

    #[test]
    fn tree_view_off_keeps_full_paths() {
        let nodes = files_section(vec![file("etc/a", &[], false), file("etc/b", &[], false)]);
        let open = HashMap::new();
        assert_eq!(
            texts(&rows(nodes, &shape(&open, "", false))),
            ["Files", "  [M] etc/a", "  [M] etc/b"]
        );
    }

    #[test]
    fn directories_in_churn_groups_key_by_their_group() {
        let churn = node(
            2,
            "2 hash-only (store hash churn)",
            Kind::Churn(ChurnClass::HashOnly),
            vec![file("etc/x/a", &[], false), file("etc/x/b", &[], false)],
        );
        let nodes = files_section(vec![file("etc/x/c", &[], false), churn]);
        let mut open = HashMap::new();
        open.insert(Key::Node(Kind::Churn(ChurnClass::HashOnly)), true);
        let rows = rows(nodes, &shape(&open, "", true));
        assert_eq!(
            texts(&rows),
            [
                "Files",
                "  etc/x/  (1)",
                "    [M] c",
                "  2 hash-only (store hash churn)",
                "    etc/x/  (2)",
                "      [M] a",
                "      [M] b",
            ]
        );
        assert_eq!(
            rows[4].key,
            Some(Key::Dir(Kind::Churn(ChurnClass::HashOnly), "etc/x/".into()))
        );
    }

    #[test]
    fn open_state_defaults_and_overrides() {
        let rebuilt = node(2, "1 rebuilt", Kind::Rebuilt, vec![package("bash")]);
        let churn = node(
            2,
            "1 hash-only",
            Kind::Churn(ChurnClass::HashOnly),
            vec![file("etc/b", &[], false)],
        );
        let nodes = vec![
            node(0, "Packages", Kind::Text, vec![rebuilt]),
            node(
                0,
                "Files",
                Kind::Text,
                vec![file("etc/a", &[], true), churn],
            ),
        ];
        let open = HashMap::new();
        let closed = texts(&rows(nodes.clone(), &shape(&open, "", false)));
        assert_eq!(
            closed,
            [
                "Packages",
                "  1 rebuilt",
                "Files",
                "  [M] etc/a",
                "  1 hash-only"
            ]
        );
        let expanded = Shape {
            expand_churn: true,
            ..shape(&open, "", false)
        };
        assert_eq!(
            texts(&rows(nodes.clone(), &expanded))[4..],
            ["  1 hash-only", "    [M] etc/b"]
        );
        let mut open = HashMap::new();
        open.insert(Key::Node(Kind::Rebuilt), true);
        open.insert(
            Key::Node(Kind::File {
                path: "etc/a".into(),
                via: vec![],
            }),
            true,
        );
        assert_eq!(
            texts(&rows(nodes, &shape(&open, "", false)))[..7],
            [
                "Packages",
                "  1 rebuilt",
                "    bash",
                "Files",
                "  [M] etc/a",
                "      @@ -1,1 +1,1 @@",
                "      -a"
            ]
        );
    }

    #[test]
    fn filter_matches_pnames_and_paths_case_insensitively_keeps_boot_and_recounts() {
        let nodes = vec![
            node(0, "web-a", Kind::Text, vec![text(2, "old /nix/store/x")]),
            node(
                0,
                "Packages",
                Kind::Text,
                vec![
                    package("OpenSSL"),
                    package("systemd"),
                    node(
                        2,
                        "3 rebuilt",
                        Kind::Rebuilt,
                        vec![package("bash"), package("openssh"), package("openssl")],
                    ),
                ],
            ),
            node(
                0,
                "Files",
                Kind::Text,
                vec![
                    file("etc/ssl/certs", &[], false),
                    file("etc/app", &["openssl-3/etc/x"], false),
                    file("etc/other", &[], false),
                    node(
                        2,
                        "2 hash-only",
                        Kind::Churn(ChurnClass::HashOnly),
                        vec![file("etc/a", &[], false), file("etc/ssl/b", &[], false)],
                    ),
                ],
            ),
            node(
                0,
                "Boot",
                Kind::Text,
                vec![node(2, "kernel", Kind::Boot, vec![])],
            ),
            node(0, "Other", Kind::Text, vec![package("zsh")]),
        ];
        let mut open = HashMap::new();
        open.insert(Key::Node(Kind::Rebuilt), true);
        let shape = shape(&open, "ssl", false);
        assert_eq!(
            texts(&rows(nodes, &shape)),
            [
                "web-a",
                "  old /nix/store/x",
                "Packages",
                "  OpenSSL",
                "  1 rebuilt (same version, new hash)",
                "    openssl",
                "Files",
                "  [M] etc/ssl/certs",
                "  [M] etc/app → openssl-3/etc/x",
                "  1 hash-only (store hash churn)",
                "Boot",
                "  kernel",
            ]
        );
    }

    #[test]
    fn rows_know_parents_leaves_and_actionable_rows() {
        let nodes = vec![node(
            0,
            "Packages",
            Kind::Text,
            vec![package("a"), text(2, "no package changes")],
        )];
        let open = HashMap::new();
        let rows = rows(nodes, &shape(&open, "", false));
        assert_eq!(rows[1].parent, Some(0));
        assert!(!rows[0].leaf && rows[1].leaf);
        assert!(!rows[0].actionable() && rows[1].actionable() && !rows[2].actionable());
    }

    fn row(indent: usize, text: &str) -> Row {
        Row {
            indent,
            spans: vec![span(text)],
            key: None,
            open: true,
            parent: None,
            kind: Kind::Text,
            leaf: true,
        }
    }

    fn wrapped(row: &Row, width: usize, wrap: bool) -> Vec<String> {
        lines(row, width, wrap)
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn wrap_hangs_under_the_rows_text() {
        assert_eq!(
            wrapped(&row(4, "abcdefghijkl"), 10, true),
            ["    abcdef", "    ghijkl"]
        );
        assert_eq!(wrapped(&row(2, "abc"), 10, true), ["  abc"]);
    }

    #[test]
    fn wrap_drops_the_indent_when_it_leaves_less_than_half() {
        assert_eq!(
            wrapped(&row(6, "abcdefgh"), 10, true),
            ["      abcd", "efgh"]
        );
        assert_eq!(
            wrapped(&row(5, "abcdefgh"), 10, true),
            ["     abcde", "     fgh"]
        );
    }

    #[test]
    fn wrap_counts_display_width() {
        // Each CJK character takes two columns; one that does not fit moves to the next line.
        assert_eq!(wrapped(&row(0, "日本語の"), 5, true), ["日本", "語の"]);
        assert_eq!(wrapped(&row(0, "a日"), 2, true), ["a", "日"]);
    }

    #[test]
    fn wrap_off_is_one_line_and_tabs_expand() {
        assert_eq!(
            wrapped(&row(2, "abcdefghijkl"), 5, false),
            ["  abcdefghijkl"]
        );
        assert_eq!(wrapped(&row(2, "a\tb"), 80, true), ["  a     b"]);
    }

    #[test]
    fn wrap_keeps_styles_per_span() {
        let red = Style {
            color: Some(Color::Red),
            ..Style::default()
        };
        let mut r = row(0, "ab");
        r.spans.push(Span {
            text: "cd".into(),
            style: red,
        });
        let lines = lines(&r, 3, true);
        assert_eq!(lines[0][1].text, "c");
        assert_eq!(lines[0][1].style, red);
        assert_eq!(lines[1][0].text, "d");
        assert_eq!(lines[1][0].style, red);
    }

    #[test]
    fn scroll_keeps_the_margin_where_possible() {
        // 100 lines in a 20-line pane.
        assert_eq!(scroll(0, (0, 1), 100, 20), 0);
        assert_eq!(scroll(0, (14, 15), 100, 20), 0);
        assert_eq!(scroll(0, (15, 16), 100, 20), 1);
        assert_eq!(scroll(50, (55, 56), 100, 20), 50);
        assert_eq!(scroll(50, (54, 55), 100, 20), 49);
        // At the ends the margin gives way.
        assert_eq!(scroll(0, (99, 100), 100, 20), 80);
        assert_eq!(scroll(80, (0, 1), 100, 20), 0);
        // Content shorter than the pane, and content that shrank.
        assert_eq!(scroll(0, (5, 6), 10, 20), 0);
        assert_eq!(scroll(90, (5, 6), 30, 20), 0);
        // A row taller than the pane fills it, scrolled within the row.
        assert_eq!(scroll(0, (30, 60), 100, 20), 30);
        assert_eq!(scroll(35, (30, 60), 100, 20), 35);
        assert_eq!(scroll(70, (30, 60), 100, 20), 40);
        // A small pane shrinks the margin.
        assert_eq!(scroll(0, (4, 5), 100, 5), 2);
    }
}
