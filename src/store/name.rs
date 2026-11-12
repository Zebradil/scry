//! Store path names and the `builtins.parseDrvName` split (spec 7.1).

/// Every store path starts with this.
pub const STORE_PREFIX: &str = "/nix/store/";

/// The name part of a store path's file name: what follows the 32-character hash and its `-`.
/// A file name whose first `-` is not at position 32 has no hash and is returned whole.
pub fn store_name(file_name: &str) -> &str {
    match file_name.find('-') {
        Some(32) => &file_name[33..],
        _ => file_name,
    }
}

/// The store name of the store path that `path` is or lies below:
/// `/nix/store/<hash>-linux-6.12.48/bzImage` gives `linux-6.12.48`.
pub fn store_path_name(path: &str) -> &str {
    let rest = path.strip_prefix(STORE_PREFIX).unwrap_or(path);
    store_name(rest.split('/').next().unwrap_or(rest))
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

/// Output names Nix appends to a multi-output package's store name (`openssl-3.0.14-bin`).
const OUTPUT_SUFFIXES: [&str; 10] = [
    "-bin", "-dev", "-lib", "-man", "-doc", "-info", "-out", "-debug", "-static", "-devdoc",
];

/// The version without a trailing output suffix: `3.0.14-bin` gives `3.0.14`. Applied to a
/// version only, so a versionless name such as `foo-dev` keeps its suffix.
pub fn strip_output_suffix(version: &str) -> &str {
    OUTPUT_SUFFIXES
        .iter()
        .find_map(|suffix| version.strip_suffix(suffix))
        .unwrap_or(version)
}

/// The package identity of a store path or file name: pname and version, output suffix
/// stripped. `None` for a `.drv`, which is never a package.
pub fn package_id(path: &str) -> Option<(&str, &str)> {
    if path.ends_with(".drv") {
        return None;
    }
    let file_name = path.rsplit('/').next().unwrap_or(path);
    let (pname, version) = parse_drv_name(store_name(file_name));
    Some((pname, strip_output_suffix(version)))
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
    fn store_path_name_takes_the_store_path_below_the_prefix() {
        let h = "0123456789abcdfghijklmnpqrsvwxyz";
        assert_eq!(
            store_path_name(&format!("/nix/store/{h}-linux-6.12.48/bzImage")),
            "linux-6.12.48"
        );
        assert_eq!(
            store_path_name(&format!("/nix/store/{h}-app-start")),
            "app-start"
        );
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
            // Checked against `builtins.parseDrvName` of Nix 2.35.
            (
                "foo-0-unstable-2024-01-02",
                ("foo", "0-unstable-2024-01-02"),
            ),
            ("my-tool-1.2.0-1a2b3c4", ("my-tool", "1.2.0-1a2b3c4")),
            ("a--b", ("a", "-b")),
            ("-1.0", ("", "1.0")),
            ("foo-_bar", ("foo", "_bar")),
            ("foo-Bar-1", ("foo-Bar", "1")),
            ("home-manager-generation", ("home-manager-generation", "")),
            (
                "darwin-system-26.11.0a1b2c3",
                ("darwin-system", "26.11.0a1b2c3"),
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_drv_name(input), expected, "{input}");
        }
    }

    #[test]
    fn package_id_strips_hash_and_output_suffix() {
        let cases = [
            (
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-openssl-3.0.14-bin",
                Some(("openssl", "3.0.14")),
            ),
            (
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-glibc-2.40-66-dev",
                Some(("glibc", "2.40-66")),
            ),
            (
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-systemd-257.5-man",
                Some(("systemd", "257.5")),
            ),
            (
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-zlib-1.3-devdoc",
                Some(("zlib", "1.3")),
            ),
            (
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-python3-3.12.4",
                Some(("python3", "3.12.4")),
            ),
            (
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-libx11-1.8",
                Some(("libx11", "1.8")),
            ),
            (
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-foo-unstable-2024-01-02",
                Some(("foo-unstable", "2024-01-02")),
            ),
            (
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-my-tool-1.2.0-1a2b3c4",
                Some(("my-tool", "1.2.0-1a2b3c4")),
            ),
            (
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-foo-dev",
                Some(("foo-dev", "")),
            ),
            (
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-hello",
                Some(("hello", "")),
            ),
            (
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-nixos-system-web-1-25.11.1",
                Some(("nixos-system-web", "1-25.11.1")),
            ),
            ("python3-3.12.4", Some(("python3", "3.12.4"))),
            (
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-hello-2.12.drv",
                None,
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(package_id(input), expected, "{input}");
        }
    }
}
