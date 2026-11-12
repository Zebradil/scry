//! Version ordering: `builtins.compareVersions` and the exceptions of spec 7.2.

use std::cmp::Ordering;

/// A version field this long or longer, all lowercase hex with at least one letter, is a git
/// revision (spec 12).
pub const GIT_REVISION_MIN_LEN: usize = 7;

/// `builtins.compareVersions`: components are digit runs or runs of other characters, split at
/// `.` and `-`. Numbers compare as integers, an empty component is less than a number, `pre` is
/// less than anything else, a string is less than a number, and strings compare bytewise.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    while !a.is_empty() || !b.is_empty() {
        let (ca, cb) = (next_component(&mut a), next_component(&mut b));
        if component_less(ca, cb) {
            return Ordering::Less;
        }
        if component_less(cb, ca) {
            return Ordering::Greater;
        }
    }
    Ordering::Equal
}

/// The order of two versions for a package change: `None` when they are unordered, so the change
/// is `changed`. That is the case for two different git revisions at the same field, and for two
/// different strings that compare equal (`1.0` and `1.00`).
pub fn version_order(old: &str, new: &str) -> Option<Ordering> {
    if old == new {
        return Some(Ordering::Equal);
    }
    // Empty fields are skipped, as Nix skips runs of separators.
    let fields = |v| str::split(v, ['.', '-']).filter(|f| !f.is_empty());
    let git_revisions_differ = fields(old)
        .zip(fields(new))
        .any(|(o, n)| o != n && is_git_revision(o) && is_git_revision(n));
    match compare_versions(old, new) {
        _ if git_revisions_differ => None,
        Ordering::Equal => None,
        order => Some(order),
    }
}

fn is_git_revision(field: &str) -> bool {
    field.len() >= GIT_REVISION_MIN_LEN
        && field
            .bytes()
            .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
        && field.bytes().any(|c| c.is_ascii_alphabetic())
}

fn next_component<'a>(rest: &mut &'a [u8]) -> &'a [u8] {
    let start = rest
        .iter()
        .position(|&c| c != b'.' && c != b'-')
        .unwrap_or(rest.len());
    *rest = &rest[start..];
    let digits = rest.first().is_some_and(u8::is_ascii_digit);
    let len = rest
        .iter()
        .position(|&c| {
            if digits {
                !c.is_ascii_digit()
            } else {
                c.is_ascii_digit() || c == b'.' || c == b'-'
            }
        })
        .unwrap_or(rest.len());
    let (component, tail) = rest.split_at(len);
    *rest = tail;
    component
}

/// Nix parses components with `string2Int<int>`: a digit run beyond `i32` is not a number.
fn as_number(component: &[u8]) -> Option<i32> {
    std::str::from_utf8(component).ok()?.parse().ok()
}

fn component_less(a: &[u8], b: &[u8]) -> bool {
    match (as_number(a), as_number(b)) {
        (Some(a), Some(b)) => a < b,
        (_, Some(_)) if a.is_empty() => true,
        _ if a == b"pre" && b != b"pre" => true,
        _ if b == b"pre" => false,
        (_, Some(_)) => true,
        (Some(_), _) => false,
        _ => a < b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `builtins.compareVersions a b` as Nix 2.35 evaluates it. The first twelve are the examples
    /// of the Nix manual (`nix-env --upgrade`, Versions).
    const NIX_PARITY: &[(&str, &str, i8)] = &[
        ("1.0", "2.3", -1),
        ("2.1", "2.3", -1),
        ("2.3", "2.3", 0),
        ("2.5", "2.3", 1),
        ("3.1", "2.3", 1),
        ("2.3.1", "2.3", 1),
        ("2.3.1", "2.3a", 1),
        ("2.3pre1", "2.3", -1),
        ("2.3pre3", "2.3pre12", -1),
        ("2.3a", "2.3c", -1),
        ("2.3pre1", "2.3c", -1),
        ("2.3pre1", "2.3q", -1),
        ("1.0pre", "1.0", -1),
        ("1.0", "1.0pre", 1),
        ("1.0", "1.00", 0),
        ("01", "1", 0),
        ("1.0", "1.0a", -1),
        ("1.0", "1.0.0", -1),
        ("1.0.0", "1-0-0", 0),
        ("1..0", "1.0", 0),
        ("1.0-", "1.0", 0),
        ("1.0", "1.0.", 0),
        ("2.10", "2.9", 1),
        ("3.0.14", "3.0.13", 1),
        ("1.2.0-1a2b3c4", "1.2.0-9f8e7d6", -1),
        ("5.2p37", "5.2p32", 1),
        ("5.2p37", "5.2", 1),
        ("0-unstable-2024-01-02", "0-unstable-2024-03-01", -1),
        ("unstable-2024-01-02", "unstable-2024-03-01", -1),
        ("25.05.20250801.6027c30", "25.11.20251001.9a8b7c6", -1),
        ("2147483647", "2147483648", 1),
        ("2147483648", "2147483649", -1),
        ("2147483647", "999", 1),
        ("2147483648", "999", -1),
        ("4294967296", "1", -1),
        ("1.99999999999", "1.100000000000", 1),
        ("1a", "1b", -1),
        ("1.0-rc1", "1.0", 1),
        ("1.0rc1", "1.0", 1),
        ("1.0-pre", "1.0", -1),
        ("1_0", "1.0", -1),
        ("1.x", "1.1", -1),
        ("a", "b", -1),
        ("A", "a", -1),
        ("pre", "pre", 0),
        ("pre", "a", -1),
        ("1", "", 1),
        ("a", "", 1),
        ("pre", "", -1),
        ("", "", 0),
    ];

    #[test]
    fn compare_versions_matches_nix() {
        for &(a, b, expected) in NIX_PARITY {
            assert_eq!(compare_versions(a, b) as i8, expected, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn version_order_adds_the_exceptions() {
        use Ordering::*;
        let cases = [
            ("3.0.13", "3.0.14", Some(Less)),
            ("258.1", "257.5", Some(Greater)),
            ("1.0", "1.0", Some(Equal)),
            ("1.0", "1.00", None),
            ("1.2.0-1a2b3c4", "1.2.0-9f8e7d6", None),
            ("25.05.20250801.6027c30", "25.11.20251001.9a8b7c6", None),
            ("0-unstable-2024-01-02", "0-unstable-2024-03-01", Some(Less)),
            // Not revisions: too short, digits only, uppercase.
            ("1.0-abc123", "1.0-abd123", Some(Less)),
            ("1.0-1234567", "1.0-1234568", Some(Less)),
            ("1.0-ABCDEF1", "1.0-ABCDEF2", Some(Less)),
            // Only one side is a revision at that field.
            ("1.0-1a2b3c4", "1.0-rc1", Some(Greater)),
            // The same revision on both sides leaves the rest ordered.
            ("1.0-1a2b3c4", "1.1-1a2b3c4", Some(Less)),
        ];
        for (old, new, expected) in cases {
            assert_eq!(version_order(old, new), expected, "{old} vs {new}");
        }
    }
}
