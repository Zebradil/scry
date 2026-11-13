//! `--on` (spec 4.1): a comma list of patterns, each matching whole names with `*` and `?`, or
//! colmena tags as `@tag`. A configuration is selected when any pattern matches it; a pattern
//! that matches nothing is a usage error naming it, which catches typos.

use crate::exit::UsageError;

/// One configuration as selection sees it.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub name: &'a str,
    /// colmena `deployment.tags`; empty for a flake.
    pub tags: &'a [String],
}

/// The indices of `candidates` that any of `patterns` selects, in order. `tags` is false in
/// `scry flake`, where an `@` pattern is a usage error.
pub fn select(
    candidates: &[Candidate<'_>],
    patterns: &[String],
    tags: bool,
) -> Result<Vec<usize>, UsageError> {
    let mut selected = vec![false; candidates.len()];
    for pattern in patterns {
        let matches = |c: &Candidate<'_>| match pattern.strip_prefix('@') {
            Some(tag) => c.tags.iter().any(|t| glob(tag, t)),
            None => glob(pattern, c.name),
        };
        if pattern.starts_with('@') && !tags {
            return Err(UsageError(format!(
                "--on {pattern}: `@` selects colmena tags, and a flake has none"
            )));
        }
        let mut any = false;
        for (i, candidate) in candidates.iter().enumerate() {
            if matches(candidate) {
                selected[i] = true;
                any = true;
            }
        }
        if !any {
            return Err(UsageError(format!(
                "--on {pattern}: matches no configuration"
            )));
        }
    }
    Ok((0..candidates.len()).filter(|&i| selected[i]).collect())
}

/// Whether `pattern` matches the whole of `text`: `*` any run of characters, `?` one.
pub fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    // The classic greedy match with one backtrack point: the last `*` and where it resumed.
    let (mut i, mut j) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while j < t.len() {
        if i < p.len() && (p[i] == '?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some((i, j));
            i += 1;
        } else if let Some((si, sj)) = star {
            i = si + 1;
            j = sj + 1;
            star = Some((si, sj + 1));
        } else {
            return false;
        }
    }
    p[i..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_whole_names() {
        for (pattern, text, expected) in [
            ("web", "web", true),
            ("web", "web-1", false),
            ("web", "a-web", false),
            ("web-*", "web-1", true),
            ("web-*", "web-", true),
            ("web-*", "db-1", false),
            ("*", "", true),
            ("*", "anything", true),
            ("web-?", "web-1", true),
            ("web-?", "web-10", false),
            ("*-a", "web-a", true),
            ("*-a", "web-b", false),
            ("w*b*c", "wxbybzc", true),
            ("w*b*c", "wxbyc-", false),
            ("??", "é1", true),
            ("", "", true),
            ("", "a", false),
        ] {
            assert_eq!(glob(pattern, text), expected, "{pattern} on {text}");
        }
    }

    fn names(selected: &[usize], candidates: &[Candidate<'_>]) -> Vec<String> {
        selected
            .iter()
            .map(|&i| candidates[i].name.to_owned())
            .collect()
    }

    #[test]
    fn patterns_and_tags() {
        let web = vec!["web".to_owned()];
        let db = vec!["db-main".to_owned(), "eu".to_owned()];
        let candidates = [
            Candidate {
                name: "db-a",
                tags: &db,
            },
            Candidate {
                name: "web-a",
                tags: &web,
            },
            Candidate {
                name: "web-b",
                tags: &web,
            },
        ];
        let select = |patterns: &[&str], tags: bool| {
            let patterns: Vec<String> = patterns.iter().map(|&p| p.to_owned()).collect();
            select(&candidates, &patterns, tags)
        };
        let ok = |patterns: &[&str]| names(&select(patterns, true).unwrap(), &candidates);
        assert_eq!(ok(&["web-a"]), ["web-a"]);
        assert_eq!(ok(&["web-b", "db-a"]), ["db-a", "web-b"]);
        assert_eq!(ok(&["@web"]), ["web-a", "web-b"]);
        assert_eq!(ok(&["@db-*"]), ["db-a"]);
        assert_eq!(ok(&["@eu", "web-?"]), ["db-a", "web-a", "web-b"]);

        let err = |patterns: &[&str], tags: bool| select(patterns, tags).unwrap_err().0;
        assert_eq!(err(&["web"], true), "--on web: matches no configuration");
        assert_eq!(
            err(&["web-a", "@nope"], true),
            "--on @nope: matches no configuration"
        );
        assert_eq!(err(&[""], true), "--on : matches no configuration");
        assert_eq!(
            err(&["@web"], false),
            "--on @web: `@` selects colmena tags, and a flake has none"
        );
    }
}
