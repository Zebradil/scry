//! Followed references: pairing the store references of a modified file's old and new lines,
//! and naming the comparison results `referrer → name` (spec 7.4). Fetching them, and the depth
//! loop, belong to the fetch pipeline.

use crate::files::churn::{StoreRef, store_refs};
use crate::files::highlight;
use crate::report::{FileChange, LineOp};
use crate::store::name::parse_drv_name;

/// Store path pairs `(old, new)` to compare for a modified text file: on lines paired as for
/// word highlight, the j-th reference of the old line with the j-th of the new one when both
/// name the same pname (ADR 0031). Pairs whose paths differ, de-duplicated, in first-seen
/// order. Empty for files without hunks.
pub fn pairs(change: &FileChange) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for hunk in &change.hunks {
        for (r, a) in highlight::pairs(&hunk.lines) {
            let (old, new) = (&hunk.lines[r].text, &hunk.lines[a].text);
            for (o, n) in store_refs(old).into_iter().zip(store_refs(new)) {
                if pname(old, &o) != pname(new, &n) {
                    continue;
                }
                let pair = (old[o.range].to_owned(), new[n.range].to_owned());
                if pair.0 != pair.1 && !out.contains(&pair) {
                    out.push(pair);
                }
            }
        }
    }
    out
}

/// Store paths referenced on the lines of `op` in `change`'s hunks, de-duplicated, in
/// first-seen order.
pub fn on_lines(change: &FileChange, op: LineOp) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in change.hunks.iter().flat_map(|h| &h.lines) {
        if line.op != op {
            continue;
        }
        for r in store_refs(&line.text) {
            let path = &line.text[r.range];
            if !out.iter().any(|p| p == path) {
                out.push(path.to_owned());
            }
        }
    }
    out
}

fn pname<'a>(text: &'a str, reference: &StoreRef) -> &'a str {
    parse_drv_name(&text[reference.name.clone()]).0
}

/// Names the changes from comparing one reference pair. `name` is the store name
/// (`store::name::store_path_name`) of the new side's reference, or of the old side's when the
/// reference only exists there. `changes` come from comparing the two
/// references as trees: a regular file is the tree's root (path `""`); for `bin/` (`bin`
/// true) the paths are relative to `bin/`, and binaries are dropped. With a referrer the
/// result is `referrer → name[/bin/script]`: the referrer's path, and its `via` plus this
/// item. Without one (a small rebuild nothing points at) the path is `name/bin/script`.
pub fn label(
    changes: Vec<FileChange>,
    referrer: Option<&FileChange>,
    name: &str,
    bin: bool,
) -> Vec<FileChange> {
    changes
        .into_iter()
        .filter(|c| !(bin && c.binary))
        .map(|mut c| {
            let item = if bin {
                format!("{name}/bin/{}", c.path)
            } else {
                name.to_owned()
            };
            match referrer {
                Some(r) => {
                    c.path.clone_from(&r.path);
                    c.via = r.via.iter().cloned().chain([item]).collect();
                }
                None => {
                    c.path = item;
                    c.via = Vec::new();
                }
            }
            c
        })
        .collect()
}

/// The path as displayed: `path`, then each `via` item after ` → `.
pub fn display_path(change: &FileChange) -> String {
    std::iter::once(change.path.as_str())
        .chain(change.via.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" → ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::diff::hunks;
    use crate::report::{ChurnClass, FileStatus, FileType};

    const H1: &str = "1b9p07z77phvv2hf6gm9f28syp39f1ag";
    const H2: &str = "9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g";
    const H3: &str = "h4qa0r1sxv7lgm2c9dyk5nfzbw8p6ij3";

    fn modified(path: &str, old: &str, new: &str) -> FileChange {
        FileChange {
            path: path.into(),
            via: vec![],
            status: FileStatus::Modified,
            class: ChurnClass::Real,
            type_old: Some(FileType::File),
            type_new: Some(FileType::File),
            binary: false,
            content_fetched: true,
            size_old: Some(old.len() as u64),
            size_new: Some(new.len() as u64),
            link_old: None,
            link_new: None,
            hunks: hunks(old, new),
        }
    }

    fn p(hash: &str, name: &str) -> String {
        format!("/nix/store/{hash}-{name}")
    }

    #[test]
    fn pairs_jth_reference_with_jth_on_paired_lines() {
        let old = format!(
            "ExecStart={}/bin/app {}\nkeep\nEnv={}\n",
            p(H1, "app-start"),
            p(H1, "app.conf"),
            p(H1, "env"),
        );
        let new = format!(
            "ExecStart={}/bin/app {}\nkeep\nEnv={}\n",
            p(H2, "app-start"),
            p(H1, "app.conf"),
            p(H3, "env"),
        );
        let change = modified("etc/systemd/system/app.service", &old, &new);
        assert_eq!(
            pairs(&change),
            [
                (p(H1, "app-start"), p(H2, "app-start")),
                (p(H1, "env"), p(H3, "env")),
            ]
        );
    }

    #[test]
    fn pairs_only_references_of_one_pname() {
        // The second reference changed package: comparing `dbus` with `glib` only adds noise.
        let old = format!(
            "PATH={}/bin:{}/bin\n",
            p(H1, "xdg-utils-1.2"),
            p(H1, "dbus-1.14")
        );
        let new = format!(
            "PATH={}/bin:{}/bin\n",
            p(H2, "xdg-utils-1.3"),
            p(H2, "glib-2.80")
        );
        assert_eq!(
            pairs(&modified("f", &old, &new)),
            [(p(H1, "xdg-utils-1.2"), p(H2, "xdg-utils-1.3"))]
        );
    }

    #[test]
    fn pairs_are_deduplicated_and_unpaired_lines_ignored() {
        let old = format!("a={0}\nb={0}\n", p(H1, "x"));
        let new = format!("a={0}\nb={0}\nc={1}\n", p(H2, "x"), p(H3, "y"));
        assert_eq!(
            pairs(&modified("f", &old, &new)),
            [(p(H1, "x"), p(H2, "x"))]
        );
        // Added files have no removed lines to pair with.
        assert!(pairs(&modified("f", "", &new)).is_empty());
    }

    #[test]
    fn labels_and_chains() {
        let referrer = modified("etc/systemd/system/app.service", "a\n", "b\n");
        let script = modified("app-start", "x\n", "y\n");
        let mut binary = modified("app", "x\n", "y\n");
        binary.binary = true;

        let depth1 = label(
            vec![script.clone(), binary],
            Some(&referrer),
            "app-start",
            true,
        );
        assert_eq!(depth1.len(), 1);
        assert_eq!(depth1[0].path, "etc/systemd/system/app.service");
        assert_eq!(depth1[0].via, ["app-start/bin/app-start"]);

        let file = modified("", "x\n", "y\n");
        let depth2 = label(vec![file], Some(&depth1[0]), "app.conf", false);
        assert_eq!(
            display_path(&depth2[0]),
            "etc/systemd/system/app.service → app-start/bin/app-start → app.conf"
        );

        let small_rebuild = label(vec![script], None, "app-start", true);
        assert_eq!(small_rebuild[0].path, "app-start/bin/app-start");
        assert!(small_rebuild[0].via.is_empty());
        assert_eq!(display_path(&small_rebuild[0]), "app-start/bin/app-start");
    }
}
