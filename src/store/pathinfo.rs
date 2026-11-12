//! `nix path-info --json` output, in every shape Nix has emitted:
//!
//! - an array of objects with `path`, an invalid path as `{"path": …, "valid": false}` (older Nix);
//! - an object keyed by full path, an invalid path as `null` (`--json` without `--json-format`,
//!   and `--json-format 1`, still the default in Nix 2.35);
//! - `{"info": {…}, "storeDir": …, "version": 2}` keyed by the path's file name
//!   (`--json-format 2`, Nix 2.35).

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// A closure as `nix path-info --json -r` reports it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PathInfo {
    /// Full store path to `narSize` in bytes.
    pub sizes: BTreeMap<String, u64>,
    /// Paths Nix reported as not in the store.
    pub invalid: Vec<String>,
}

pub fn parse(json: &[u8]) -> Result<PathInfo> {
    let value: Value = serde_json::from_slice(json).context("path-info output is not JSON")?;
    let entries: Vec<(String, &Value)> = match &value {
        Value::Array(items) => items
            .iter()
            .map(|item| {
                let path = item["path"]
                    .as_str()
                    .context("path-info entry without `path`")?;
                Ok((path.to_owned(), item))
            })
            .collect::<Result<_>>()?,
        Value::Object(object) => match object.get("info") {
            Some(Value::Object(info)) => {
                let store_dir = object["storeDir"].as_str().unwrap_or("/nix/store");
                info.iter()
                    .map(|(key, item)| (format!("{store_dir}/{key}"), item))
                    .collect()
            }
            _ => object
                .iter()
                .map(|(key, item)| (key.clone(), item))
                .collect(),
        },
        _ => bail!("path-info output is neither an array nor an object"),
    };
    let mut info = PathInfo::default();
    for (path, item) in entries {
        if item.is_null() || item["valid"] == Value::Bool(false) {
            info.invalid.push(path);
            continue;
        }
        let size = item["narSize"]
            .as_u64()
            .with_context(|| format!("path-info for {path} has no narSize"))?;
        info.sizes.insert(path, size);
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "/nix/store/0a1b2c3d4f5g6h7i8j9k0l1m2n3p4q5r-web-a";
    const B: &str = "/nix/store/s6v7w8x9y0z1a2b3c4d5f6g7h8i9j0k1-bash-5.2p37";
    const MISSING: &str = "/nix/store/zyxwvsrqpnmlkjihgfdcba9876543210-old-lib-0.9";

    fn expected(invalid: &[&str]) -> PathInfo {
        PathInfo {
            sizes: BTreeMap::from([(A.to_owned(), 57024), (B.to_owned(), 1_662_576)]),
            invalid: invalid.iter().map(|p| p.to_string()).collect(),
        }
    }

    #[test]
    fn array_of_objects() {
        let json = format!(
            r#"[{{"path":"{A}","narSize":57024,"references":["{B}"]}},
               {{"path":"{B}","narSize":1662576,"references":[]}},
               {{"path":"{MISSING}","valid":false}}]"#
        );
        assert_eq!(parse(json.as_bytes()).unwrap(), expected(&[MISSING]));
    }

    #[test]
    fn object_keyed_by_path() {
        let json = format!(
            r#"{{"{A}":{{"narSize":57024,"references":["{B}"],"version":1}},
               "{B}":{{"narSize":1662576,"references":[],"version":1}},
               "{MISSING}":null}}"#
        );
        assert_eq!(parse(json.as_bytes()).unwrap(), expected(&[MISSING]));
    }

    #[test]
    fn format_2_keyed_by_file_name() {
        let name = |p: &'static str| p.strip_prefix("/nix/store/").unwrap();
        let json = format!(
            r#"{{"info":{{"{}":{{"narSize":57024,"version":2}},
                          "{}":{{"narSize":1662576,"version":2}},
                          "{}":null}},
                 "storeDir":"/nix/store","version":2}}"#,
            name(A),
            name(B),
            name(MISSING)
        );
        assert_eq!(parse(json.as_bytes()).unwrap(), expected(&[MISSING]));
    }

    #[test]
    fn rejects_malformed_output() {
        for json in ["", "42", r#"[{"narSize":1}]"#, r#"{"/nix/store/x":{}}"#] {
            assert!(parse(json.as_bytes()).is_err(), "{json}");
        }
    }
}
