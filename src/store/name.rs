//! Store path names and the `builtins.parseDrvName` split (spec 7.1).

/// The name part of a store path's file name: what follows the 32-character hash and its `-`.
/// A file name whose first `-` is not at position 32 has no hash and is returned whole.
pub fn store_name(file_name: &str) -> &str {
    match file_name.find('-') {
        Some(32) => &file_name[33..],
        _ => file_name,
    }
}

/// Splits a name into pname and version like `builtins.parseDrvName`: the version starts after
/// the first `-` that is followed by a character other than an ASCII letter.
pub fn parse_drv_name(name: &str) -> (&str, &str) {
    let bytes = name.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'-' && bytes.get(i + 1).is_some_and(|c| !c.is_ascii_alphabetic()) {
            return (&name[..i], &name[i + 1..]);
        }
    }
    (name, "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_name_strips_hash_only_when_present() {
        assert_eq!(
            store_name("0123456789abcdfghijklmnpqrsvwxyz-python3-3.12.4"),
            "python3-3.12.4"
        );
        assert_eq!(store_name("etc-hosts"), "etc-hosts");
        assert_eq!(store_name("hello"), "hello");
    }

    #[test]
    fn parse_drv_name_matches_nix() {
        let cases = [
            ("python3-3.12.4", ("python3", "3.12.4")),
            ("libx11-1.8", ("libx11", "1.8")),
            ("hello", ("hello", "")),
            ("foo-dev", ("foo-dev", "")),
            ("openssl-3.0.14-bin", ("openssl", "3.0.14-bin")),
            (
                "nixos-system-web-1-25.11.20251001.9a8b7c6",
                ("nixos-system-web", "1-25.11.20251001.9a8b7c6"),
            ),
            ("foo-unstable-2024-01-02", ("foo-unstable", "2024-01-02")),
            ("trailing-", ("trailing-", "")),
            ("single-file-1.26.4", ("single-file", "1.26.4")),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_drv_name(input), expected, "{input}");
        }
    }
}
