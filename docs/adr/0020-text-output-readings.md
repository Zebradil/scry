# Text output and shared changes: readings of the spec

Spec 8.2 and 8.3 leave some cases open. These readings fill them; the snapshot tests in
`src/render/tests.rs` pin them.

## Shared changes

- **File identity** is the whole `FileChange` with sizes dropped and hunk lines compared by
  text after `files::churn::normalize_hashes` (spans and `hash_only` ignored, since they follow
  the hashes). 8.2 names path, `via`, status and hunks; `[L]` and `[T]` rows have no hunks, so
  without their link targets and types two different link changes on one path would count as
  identical. Hunk positions count, as part of the hunk.
- **Binary and not-fetched files** have no content in the report, so path and status decide:
  two modified `etc/udev/hwdb.bin` count as one shared change. Comparing sizes instead would
  split text files whose hunks match but whose unchanged parts differ.
- **Order**: packages by kind then pname, files by path and `via`, Boot rows in section order;
  rows that tie keep the order of their first configuration.
- **Shared rows** show no sizes, which are not compared: `[B] path  (binary)`.

## Text output

- **Empty shared section** is omitted, like an empty Files or Boot section.
- **Several versions per side**: the set difference as in 7.2; when one side's difference is
  empty (`{1.0} -> {1.0, 2.0}`), both full sets in braces. A versionless package shows no
  version alone and `(none)` beside others.
- **`system`** reads `system A` when both sides run the same version, and is left out when
  neither side has one.
- **Failed configurations** show only the status row under the header, as the example does,
  even when a side resolved. The header names `old.location`, else `target` (ADR 0006), else
  `local`.
- **Short revision** is the first 7 characters of the base commit, in the header and in
  `not present at base <rev>`.
- **File order** follows the report (7.3: byte order); the 8.3 example lists `etc/mtab` after
  `etc/nix/nix.conf`, which 7.3 contradicts.
- **Colour**: a tagged row colours its tag and its name (pname, path, specialisation); deltas,
  versions, details and `(also in N)` stay plain. Configuration and section headers are bold.
  `\ No newline at end of file` is dimmed like a hunk header.
- **Control characters** in any text from the machines compared (file contents, paths, link
  targets, messages) are escaped before they reach a span: C0 except tab, and DEL, in caret
  form (`^[`, `^J`, `^?`), C1 as `\u{9b}`. Highlight spans slice the raw text first. A
  message's further lines are indented under its first. JSON writes DEL and C1 as `\u00XX`,
  as serde_json does for C0, so the data decodes unchanged.
- **Expanded churn group** drops the `--expand-churn` hint and lists its files, with hunks, one
  level deeper.

## The view model

Text output is built in two steps (`render::view`, then `render::text`) so the TUI renders the
same rows: a tree of nodes whose rows are styled spans, with a `Kind` for folding and
filtering, flattened with text output's open state. The module doc of `render::view` is the
contract for M7.
