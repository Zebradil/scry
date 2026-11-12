//! Store references in text, and churn: changes that only follow store hashes or package
//! versions (spec 7.3).

use std::ops::Range;

use crate::report::ChurnClass;
use crate::store::name::{STORE_PREFIX, parse_drv_name, strip_output_suffix};

const BASE32: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";
/// Stands in for every store hash. Outside Nix's base32 alphabet, so normalised text holds no
/// store reference and normalising twice changes nothing.
const HASH_PLACEHOLDER: &str = "<hash>";
const VERSION_PLACEHOLDER: &str = "<version>";

/// One store reference: `/nix/store/`, a 32-character hash, `-` and a name. A following
/// `/subpath` is not part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreRef {
    /// Byte range of the whole reference, which is the root store path.
    pub range: Range<usize>,
    pub hash: Range<usize>,
    pub name: Range<usize>,
}

fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"+._?=-".contains(&b)
}

/// Store references in `text`, in order.
pub fn store_refs(text: &str) -> Vec<StoreRef> {
    let bytes = text.as_bytes();
    let mut refs = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find(STORE_PREFIX) {
        let start = from + found;
        let hash = start + STORE_PREFIX.len()..start + STORE_PREFIX.len() + 32;
        from = start + 1;
        let hash_ok = bytes
            .get(hash.clone())
            .is_some_and(|h| h.iter().all(|b| BASE32.contains(b)));
        if !hash_ok || bytes.get(hash.end) != Some(&b'-') {
            continue;
        }
        let name_start = hash.end + 1;
        let name_len = bytes[name_start..]
            .iter()
            .take_while(|&&b| is_name_char(b))
            .count();
        if name_len == 0 {
            continue;
        }
        let end = name_start + name_len;
        refs.push(StoreRef {
            range: start..end,
            hash,
            name: name_start..end,
        });
        from = end;
    }
    refs
}

/// `text` with every store hash replaced by one placeholder.
pub fn normalize_hashes(text: &str) -> String {
    normalize(text, false)
}

/// `text` with every store hash and the version of every store name replaced (7.1 split).
pub fn normalize_versions(text: &str) -> String {
    normalize(text, true)
}

fn normalize(text: &str, versions: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for r in store_refs(text) {
        out.push_str(&text[last..r.hash.start]);
        out.push_str(HASH_PLACEHOLDER);
        out.push('-');
        let name = &text[r.name.clone()];
        let (pname, version) = parse_drv_name(name);
        if versions && !version.is_empty() {
            // The output suffix stays (spec 7.1), so `foo-1.0-bin` and `foo-2.0-dev` differ.
            let suffix = &version[strip_output_suffix(version).len()..];
            out.push_str(pname);
            out.push('-');
            out.push_str(VERSION_PLACEHOLDER);
            out.push_str(suffix);
        } else {
            out.push_str(name);
        }
        last = r.range.end;
    }
    out.push_str(&text[last..]);
    out
}

/// Classifies a modified text file (spec 7.3).
pub fn classify(old: &str, new: &str) -> ChurnClass {
    if normalize_hashes(old) == normalize_hashes(new) {
        ChurnClass::HashOnly
    } else if normalize_versions(old) == normalize_versions(new) {
        ChurnClass::VersionOnly
    } else {
        ChurnClass::Real
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: &str = "1b9p07z77phvv2hf6gm9f28syp39f1ag";
    const H2: &str = "9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g";

    fn refs(text: &str) -> Vec<&str> {
        store_refs(text)
            .into_iter()
            .map(|r| &text[r.range])
            .collect()
    }

    #[test]
    fn store_references_follow_the_spec_grammar() {
        let text = format!(
            "ExecStart=/nix/store/{H1}-app-start/bin/app-start \"/nix/store/{H2}-foo.conf\":x"
        );
        assert_eq!(
            refs(&text),
            [
                format!("/nix/store/{H1}-app-start"),
                format!("/nix/store/{H2}-foo.conf")
            ]
        );
        // `e`, `o`, `t`, `u` are outside Nix's base32 alphabet; a hash is exactly 32 characters
        // followed by `-` and a non-empty name.
        for not_a_ref in [
            "/nix/store/eb9p07z77phvv2hf6gm9f28syp39f1ag-x".to_owned(),
            format!("/nix/store/{}-x", &H1[1..]),
            format!("/nix/store/{H1}x"),
            format!("/nix/store/{H1}-"),
            format!("/nix/store/{H1}-/x"),
        ] {
            assert!(refs(&not_a_ref).is_empty(), "{not_a_ref}");
        }
        let name_chars = format!("/nix/store/{H1}-a+b._?=-Z9,rest");
        assert_eq!(refs(&name_chars), [format!("/nix/store/{H1}-a+b._?=-Z9")]);
    }

    #[test]
    fn hash_normalisation_touches_only_store_hashes() {
        let old = format!("x=/nix/store/{H1}-foo-1.0/bin sha256={H1}");
        let new = format!("x=/nix/store/{H2}-foo-1.0/bin sha256={H1}");
        assert_eq!(
            normalize_hashes(&old),
            "x=/nix/store/<hash>-foo-1.0/bin sha256=".to_owned() + H1
        );
        assert_eq!(normalize_hashes(&old), normalize_hashes(&new));
        assert_eq!(
            normalize_hashes(&normalize_hashes(&old)),
            normalize_hashes(&old)
        );
        // A checksum outside a store reference stays a real change.
        let new_checksum = format!("x=/nix/store/{H1}-foo-1.0/bin sha256={H2}");
        assert_eq!(classify(&old, &new_checksum), ChurnClass::Real);
    }

    #[test]
    fn version_normalisation_uses_the_parse_drv_name_split() {
        let cases = [
            ("single-file-1.26.4", "single-file-<version>"),
            ("python3-3.12.4", "python3-<version>"),
            ("openssl-3.0.14-bin", "openssl-<version>-bin"),
            ("hello", "hello"),
            ("foo-dev", "foo-dev"),
        ];
        for (name, expected) in cases {
            assert_eq!(
                normalize_versions(&format!("/nix/store/{H1}-{name}/x")),
                format!("/nix/store/<hash>-{expected}/x"),
                "{name}"
            );
        }
    }

    #[test]
    fn classification() {
        let file = |h: &str, v: &str| format!("ExecStart=/nix/store/{h}-single-file-{v}/bin/x\n");
        assert_eq!(
            classify(&file(H1, "1.0"), &file(H2, "1.0")),
            ChurnClass::HashOnly
        );
        assert_eq!(
            classify(&file(H1, "1.0"), &file(H2, "1.1")),
            ChurnClass::VersionOnly
        );
        assert_eq!(
            classify(&file(H1, "1.0"), &format!("{}x", file(H2, "1.0"))),
            ChurnClass::Real
        );
        // Different outputs of a bumped package are not a version-only change.
        assert_eq!(
            classify(
                &format!("/nix/store/{H1}-foo-1.0-bin"),
                &format!("/nix/store/{H2}-foo-2.0-dev")
            ),
            ChurnClass::Real
        );
    }
}
