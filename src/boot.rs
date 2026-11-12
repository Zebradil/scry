//! Boot section of a NixOS configuration from both sides' probes (spec 7.5).

use std::collections::BTreeMap;

use crate::report::{Boot, LinkChange, ParamsChange, Specialisations};
use crate::store::name::store_path_name;

/// What the probe reads from one NixOS toplevel (spec 6.2 step 1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// Link targets of `kernel`, `initrd` and `kernel-modules`; `None` where the link is
    /// missing.
    pub kernel: Option<String>,
    pub initrd: Option<String>,
    pub kernel_modules: Option<String>,
    /// The text of `kernel-params`; `None` where the file is missing.
    pub kernel_params: Option<String>,
    /// Entries of `specialisation`, by name, with their link targets.
    pub specialisations: BTreeMap<String, String>,
}

/// The Boot section; every item `None` when nothing differs, which hides it.
pub fn compare(old: &Probe, new: &Probe) -> Boot {
    Boot {
        kernel: link(&old.kernel, &new.kernel),
        initrd: link(&old.initrd, &new.initrd),
        kernel_modules: link(&old.kernel_modules, &new.kernel_modules),
        kernel_params: params(&old.kernel_params, &new.kernel_params),
        specialisations: specialisations(&old.specialisations, &new.specialisations),
    }
}

/// Compared by target path, shown by the store name of the target's store path.
fn link(old: &Option<String>, new: &Option<String>) -> Option<LinkChange> {
    if old == new {
        return None;
    }
    let name = |target: &str| store_path_name(target).to_owned();
    let (old, new) = (old.as_deref().map(name), new.as_deref().map(name));
    Some(LinkChange {
        rebuilt: old.is_some() && old == new,
        old,
        new,
    })
}

/// Whitespace runs folded to one space, trimmed; a missing file is empty.
fn params(old: &Option<String>, new: &Option<String>) -> Option<ParamsChange> {
    let fold = |text: &Option<String>| {
        text.as_deref()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let (old, new) = (fold(old), fold(new));
    (old != new).then_some(ParamsChange { old, new })
}

fn specialisations(
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, String>,
) -> Option<Specialisations> {
    let only = |a: &BTreeMap<String, String>, b: &BTreeMap<String, String>| -> Vec<String> {
        a.keys().filter(|k| !b.contains_key(*k)).cloned().collect()
    };
    let changed: Vec<String> = old
        .iter()
        .filter(|(k, target)| new.get(*k).is_some_and(|t| t != *target))
        .map(|(k, _)| k.clone())
        .collect();
    let s = Specialisations {
        added: only(new, old),
        removed: only(old, new),
        changed,
    };
    (!s.added.is_empty() || !s.removed.is_empty() || !s.changed.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(hash: &str, name: &str) -> Option<String> {
        Some(format!("/nix/store/{hash}-{name}"))
    }

    const H1: &str = "1b9p07z77phvv2hf6gm9f28syp39f1ag";
    const H2: &str = "9w3ka6zcl5ns0fqyd8vkm2rhxbij4p7g";

    #[test]
    fn spec_example() {
        let old = Probe {
            kernel: p(H1, "linux-6.12.40/bzImage"),
            initrd: p(H1, "initrd-linux-6.12.40/initrd"),
            kernel_modules: p(H1, "linux-6.12.48-modules"),
            kernel_params: Some("quiet  loglevel=4\n".into()),
            specialisations: BTreeMap::from([
                ("old".into(), p(H1, "a").unwrap()),
                ("same".into(), p(H1, "b").unwrap()),
                ("moved".into(), p(H1, "c").unwrap()),
            ]),
        };
        let new = Probe {
            kernel: p(H2, "linux-6.12.48/bzImage"),
            initrd: p(H2, "initrd-linux-6.12.48/initrd"),
            kernel_modules: p(H2, "linux-6.12.48-modules"),
            kernel_params: Some("quiet loglevel=4 mitigations=auto".into()),
            specialisations: BTreeMap::from([
                ("debug".into(), p(H2, "d").unwrap()),
                ("same".into(), p(H1, "b").unwrap()),
                ("moved".into(), p(H2, "c").unwrap()),
            ]),
        };
        let boot = compare(&old, &new);
        let change = |old: &str, new: &str, rebuilt| {
            Some(LinkChange {
                old: Some(old.into()),
                new: Some(new.into()),
                rebuilt,
            })
        };
        assert_eq!(boot.kernel, change("linux-6.12.40", "linux-6.12.48", false));
        assert_eq!(
            boot.initrd,
            change("initrd-linux-6.12.40", "initrd-linux-6.12.48", false)
        );
        assert_eq!(
            boot.kernel_modules,
            change("linux-6.12.48-modules", "linux-6.12.48-modules", true)
        );
        assert_eq!(
            boot.kernel_params,
            Some(ParamsChange {
                old: "quiet loglevel=4".into(),
                new: "quiet loglevel=4 mitigations=auto".into(),
            })
        );
        assert_eq!(
            boot.specialisations,
            Some(Specialisations {
                added: vec!["debug".into()],
                removed: vec!["old".into()],
                changed: vec!["moved".into()],
            })
        );
    }

    #[test]
    fn missing_sides_and_no_change() {
        let old = Probe {
            kernel_params: Some(" a\tb ".into()),
            ..Probe::default()
        };
        let new = Probe {
            initrd: p(H2, "initrd-linux-6.12.48/initrd"),
            kernel_params: Some("a b".into()),
            ..Probe::default()
        };
        let boot = compare(&old, &new);
        assert_eq!(
            boot.initrd,
            Some(LinkChange {
                old: None,
                new: Some("initrd-linux-6.12.48".into()),
                rebuilt: false,
            })
        );
        assert_eq!(
            boot,
            Boot {
                initrd: boot.initrd.clone(),
                kernel: None,
                kernel_modules: None,
                kernel_params: None,
                specialisations: None,
            }
        );
        assert_eq!(
            compare(&new, &new),
            Boot {
                kernel: None,
                initrd: None,
                kernel_modules: None,
                kernel_params: None,
                specialisations: None,
            }
        );
    }
}
