//! Store references in text, and churn: changes that only follow store hashes, package
//! versions or the versions scry knows per side (spec 7.3).

use std::ops::Range;

use crate::report::ChurnClass;
use crate::store::name::{STORE_PREFIX, parse_drv_name, strip_output_suffix};

const BASE32: &[u8] = b"0123456789abcdfghijklmnpqrsvwxyz";
/// Stands in for every store hash. Outside Nix's base32 alphabet, so normalised text holds no
/// store reference and normalising twice changes nothing.
const HASH_PLACEHOLDER: &str = "<hash>";
const VERSION_PLACEHOLDER: &str = "<version>";
const SYSTEM_PLACEHOLDER: &str = "<system-version>";
const REVISION_PLACEHOLDER: &str = "<revision>";
const KERNEL_PLACEHOLDER: &str = "<kernel-version>";
/// Git's default abbreviation: a shorter tail, such as `11` in home-manager's `26.11`, is no
/// revision.
const SHORT_REVISION_MIN: usize = 7;
const REVISION_LEN: usize = 40;

/// Versions scry knows per side, masked outside store references in the version-only check
/// (spec 7.3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Masks {
    old: Vec<Mask>,
    new: Vec<Mask>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mask {
    placeholder: &'static str,
    prefix: String,
    /// Bytes replaced from the start of `prefix`; those past it must be hex.
    len: usize,
}

impl Masks {
    /// From each side's system version (the trimmed marker file) and kernel version, as
    /// `[old, new]`. A version is masked only when both sides know it and they differ.
    pub fn new(system: [Option<&str>; 2], kernel: [Option<&str>; 2]) -> Self {
        let mut masks = Self::default();
        masks.add(SYSTEM_PLACEHOLDER, system, |v| Some((v, v.len())));
        // A flake-built version ends in the nixpkgs short revision; `nixos-version` holds the
        // full one.
        masks.add(REVISION_PLACEHOLDER, system, |v| {
            let tail = v.rsplit_once('.')?.1;
            (tail.len() >= SHORT_REVISION_MIN && tail.bytes().all(|b| b.is_ascii_hexdigit()))
                .then_some((tail, REVISION_LEN))
        });
        masks.add(KERNEL_PLACEHOLDER, kernel, |v| Some((v, v.len())));
        masks
    }

    fn add<'a>(
        &mut self,
        placeholder: &'static str,
        [old, new]: [Option<&'a str>; 2],
        mask: impl Fn(&'a str) -> Option<(&'a str, usize)>,
    ) {
        let (Some(old), Some(new)) = (old.and_then(&mask), new.and_then(&mask)) else {
            return;
        };
        if old.0.is_empty() || new.0.is_empty() || old.0 == new.0 {
            return;
        }
        for ((prefix, len), side) in [(old, &mut self.old), (new, &mut self.new)] {
            side.push(Mask {
                placeholder,
                prefix: prefix.to_owned(),
                len,
            });
        }
    }
}

impl Mask {
    /// Replaces every match that no ASCII alphanumeric touches, so `6.1.5` stays in `6.1.50`.
    fn apply(&self, text: &str) -> String {
        let bytes = text.as_bytes();
        let alnum = |i: usize| bytes.get(i).is_some_and(u8::is_ascii_alphanumeric);
        let mut out = String::with_capacity(text.len());
        let (mut last, mut from) = (0, 0);
        while let Some(found) = text[from..].find(&self.prefix) {
            let start = from + found;
            let end = start + self.len;
            from = start + self.prefix.len();
            let hex_tail = bytes
                .get(from..end)
                .is_some_and(|tail| tail.iter().all(u8::is_ascii_hexdigit));
            if !hex_tail || (start > 0 && alnum(start - 1)) || alnum(end) {
                continue;
            }
            out.push_str(&text[last..start]);
            out.push_str(self.placeholder);
            (last, from) = (end, end);
        }
        out.push_str(&text[last..]);
        out
    }
}

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
    normalize(text, None)
}

/// `text` with every store hash and the version of every store name replaced (7.1 split), and
/// `masks` applied outside store references.
fn normalize_versions(text: &str, masks: &[Mask]) -> String {
    normalize(text, Some(masks))
}

/// `versions` holds the masks of a version normalisation, `None` for a hash normalisation.
fn normalize(text: &str, versions: Option<&[Mask]>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let outside = |out: &mut String, part: &str| match versions {
        Some(masks) => out.push_str(
            &masks
                .iter()
                .fold(part.to_owned(), |part, mask| mask.apply(&part)),
        ),
        None => out.push_str(part),
    };
    for r in store_refs(text) {
        outside(&mut out, &text[last..r.hash.start]);
        out.push_str(HASH_PLACEHOLDER);
        out.push('-');
        let name = &text[r.name.clone()];
        let (pname, version) = parse_drv_name(name);
        if versions.is_some() && !version.is_empty() {
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
    outside(&mut out, &text[last..]);
    out
}

/// Classifies a modified text file (spec 7.3).
pub fn classify(old: &str, new: &str, masks: &Masks) -> ChurnClass {
    if normalize_hashes(old) == normalize_hashes(new) {
        ChurnClass::HashOnly
    } else if normalize_versions(old, &masks.old) == normalize_versions(new, &masks.new) {
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
        assert_eq!(
            classify(&old, &new_checksum, &Masks::default()),
            ChurnClass::Real
        );
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
                normalize_versions(&format!("/nix/store/{H1}-{name}/x"), &[]),
                format!("/nix/store/<hash>-{expected}/x"),
                "{name}"
            );
        }
    }

    #[test]
    fn classification() {
        let classify = |old: &str, new: &str| classify(old, new, &Masks::default());
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

    const V1: &str = "26.05.20261102.7dcdd87";
    const V2: &str = "26.05.20261120.680275f";
    const R1: &str = "7dcdd87a866099a230ed5146a5bb7d16fd5c9b14";
    const R2: &str = "680275f0bb6afce84fbac3e75fc47c5f2272400b";

    fn masks() -> Masks {
        Masks::new([Some(V1), Some(V2)], [Some("6.12.40"), Some("6.12.48")])
    }

    #[test]
    fn known_versions_are_version_only() {
        let issue = |v: &str| format!("<<< Welcome to NixOS {v} (\\m) - \\l >>>\n");
        assert_eq!(
            classify(&issue(V1), &issue(V2), &masks()),
            ChurnClass::VersionOnly
        );
        let boot = |h: &str, v: &str, k: &str| {
            format!(
                "{{\"kernel\":\"/nix/store/{h}-linux-{k}/bzImage\",\"label\":\"NixOS Zokor {v} (Linux {k})\"}}"
            )
        };
        assert_eq!(
            classify(&boot(H1, V1, "6.12.40"), &boot(H2, V2, "6.12.48"), &masks()),
            ChurnClass::VersionOnly
        );
        let script = |v: &str, r: &str| {
            format!("echo \"{r}\"\n{{\"nixosVersion\":\"{v}\",\"nixpkgsRevision\":\"{r}\"}}\n")
        };
        assert_eq!(
            classify(&script(V1, R1), &script(V2, R2), &masks()),
            ChurnClass::VersionOnly
        );
        // The revision is masked only when it extends the version's own short revision.
        assert_eq!(
            classify(&script(V1, R2), &script(V2, R1), &masks()),
            ChurnClass::Real
        );
    }

    #[test]
    fn masks_need_two_known_different_versions() {
        let text = |v: &str| format!("BUILD_ID=\"{v}\"\n");
        for masks in [
            Masks::default(),
            Masks::new([Some(V1), Some(V1)], [None, None]),
            Masks::new([Some(V1), None], [None, None]),
            Masks::new([Some(""), Some(V2)], [None, None]),
        ] {
            assert_eq!(classify(&text(V1), &text(V2), &masks), ChurnClass::Real);
        }
        // No revision from a tail too short to be one.
        let hm = Masks::new([Some("26.05"), Some("26.11")], [None, None]);
        assert_eq!(hm.old.len(), 1);
    }

    #[test]
    fn a_real_change_next_to_a_version_stays_real() {
        let issue = |v: &str, tail: &str| format!("Welcome to NixOS {v}{tail}\n");
        assert_eq!(
            classify(&issue(V1, ""), &issue(V2, " (beta)"), &masks()),
            ChurnClass::Real
        );
        // A known version inside a longer word is not that version.
        assert_eq!(
            classify("v6.12.400", "v6.12.480", &masks()),
            ChurnClass::Real
        );
        assert_eq!(
            classify("x 6.12.40", "x 6.12.48", &masks()),
            ChurnClass::VersionOnly
        );
        // Masks never make a change hash-only.
        assert_eq!(classify(V1, V2, &masks()), ChurnClass::VersionOnly);
    }

    #[test]
    fn masks_stay_outside_store_references() {
        let masks = Masks::new([Some("1.0"), Some("2.0")], [None, None]);
        assert_eq!(
            normalize_versions(&format!("/nix/store/{H1}-foo-1.0 1.0"), &masks.old),
            "/nix/store/<hash>-foo-<version> <system-version>"
        );
    }
}
