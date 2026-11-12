# Report fields beyond the spec

The JSON report is a public interface (spec 8.1); adding a field keeps `schema_version: 1`. These
additions and readings fill gaps between spec 8.1 and what 8.2 to 8.4 render. The spec example
still parses and serialises back unchanged (test in `report`).

- **`target`** on a configuration: the old side's machine in deployed and paths mode, absent for
  this machine and in revision mode. The text example of 8.3 heads a failed configuration
  `web-b · nixos (root@web-b)` while the JSON example of 8.1 has `"old": null` for it, so the
  location has to live outside the side.
- **`hash_only`** on a hunk line, serialised only when true: the line is paired with one that
  differs only in store hashes, shown dimmed without bold (7.3). Without it a renderer cannot tell
  such a line from an unpaired one, since both have no spans.
- **`no_newline`** on a hunk line, serialised only when true: the file's last line has no final
  newline, shown as `diff -u` does with `\ No newline at end of file` after it. Without it, a
  change that only adds or removes the final newline shows as a removed and an added line with
  the same text.
- **Shared boot items** (8.2 gives no JSON): `{"section": "boot", "change": {"item": …}}` with
  `item` one of `kernel`, `initrd`, `kernel-modules`, `kernel-params` (fields as in `boot`), or
  `specialisation` with `name` and `status` (`added`, `removed`, `changed`): one item per row of
  the Boot section.
- **Shared file items** carry the whole `FileChange` of the first configuration by name.
- **A small rebuild without a referrer** (7.4) has `path` `name/bin/script` and empty `via`.
- **Timeout**: `error` reads `timeout after N s`, the text line of 8.3.
- `status` and `mode` are enums carrying their fields (`error`, the sections, `base`/`head`), so
  the presence rules of 8.1 hold by construction.
