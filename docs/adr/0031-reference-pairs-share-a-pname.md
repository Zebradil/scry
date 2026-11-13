# Reference pairs share a pname

Spec 7.4 pairs the store references of a modified file's old and new lines: within a hunk, a
run of removed lines with the run of added lines after it, and the j-th reference of one line
with the j-th of the other. When a line's references change package, not just hash or version,
that pairs two different packages. On real data a `PATH=` line paired an `xdg-utils` reference
with `dbus` and `glib`, and comparing their `bin/` directories showed every script of each as
removed and added: noise that buried the real changes.

`files::refs::pairs` keeps a j-th pair only when both references have the same pname (the
`builtins.parseDrvName` split of the store name, `store::name::parse_drv_name`). A version bump
(`openssl-3.0.13` and `openssl-3.0.14`) and a rebuild (same name, new hash) still pair.

## Considered options

- **Matching references by pname within the line pair**, ignoring position: finds a package
  that moved within the line, but a line naming one pname twice would need a rule of its own.
  The positional rule with a pname check covers the cases seen.
- **Keeping the spec's pairing**: 7.4 states it without a *must*, and the noise it produced is
  measured, so the narrowing is recorded here instead.

## Consequences

- A reference whose package was replaced by another (`dbus` by `glib`) is not followed; the
  file's own diff still shows the change, and the Packages section the added and removed
  packages.
