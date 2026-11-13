# TUI: readings of the spec

Spec 8.4 and 8.5 leave some cases open. These readings fill them; the tests in
`src/tui/tests.rs` and `src/tui/rows.rs` pin them.

## Screen

- **Cursor and focus without colour**: 8.5 allows foreground colours, bold and dim only, so
  the cursor and the selected entry are marked with `>` in a gutter (bold in the focused pane,
  dim in the other) and the focused pane has a thick border. Reverse video would be a
  background colour, and both marks must also show with colour off.
- **A configuration still fetching** shows `fetching…` in the detail pane. **All** without
  shared changes shows `no shared changes` (`… yet` while fetching).
- **The left pane width** counts the status column as the widest status shown now, so the
  pane widens once when the first `done` row arrives.

## Keys

- **`g`/`G`** act on the detail pane from either pane, as 8.4's table says.
- **`l`** in the list only focuses the detail pane; in the detail pane it expands a closed
  node and does nothing else.
- **Enter on a leaf** collapses its parent and moves there only when the parent folds (a file
  change, a group, a directory); under a header such as `Packages` it does nothing.
- **`/`** starts from the current filter. The filter applies on Enter, not while typing.
- **The cursor** stays on its row when the filter, the tree view or an arrival changes the
  rows: the same collapsible node, else the same change (kind and tag), else the same text
  without its indent under the same parent node; among rows that look alike, the same
  occurrence, else the one nearest the old index. Otherwise it keeps its index.
- **A row taller than the pane** (a long hunk line, wrapped) fills the pane with the margin
  dropped. Ctrl-D/Ctrl-U and PgDn/PgUp scroll through it by half panes before the cursor
  moves on, so every line of it can be reached.

## Rows

- **Filter**: a file matches on its path or any `via` item. Text rows (headers, status
  lines) stay; a group or section left without rows is dropped; the rebuilt group's size
  delta is recounted from the matching packages.
- **Tree view**: a directory row shows the number of file rows below it. Only chains of
  single-child directories merge; a directory holding one file stays a directory. In All,
  the names column is realigned over the shown rows, since the tree view moves the labels.
- **Wrap** breaks at any character, by display width; continuation lines start at the row's
  indent. Tabs expand to stops of 8 for display; `y` copies them as tabs.

## Clipboard

- **`X KiB`** in `copied N lines (X KiB)` is the size of the OSC 52 escape, the same quantity
  the 100 KiB warning is measured on, so the number shown is the one terminals cap.
- **`nothing to copy yet`** applies to a configuration still fetching, and to All while it has
  no shared changes and some configuration is still fetching; All with no shared changes
  once all have arrived gives `nothing to copy`, for `y` and `Y` alike. Otherwise All copies
  what has arrived; `Y` copies the shared section as text output prints it.
- **Panics**: a panic on the viewer's thread restores the terminal before its message prints.
  Another thread's panic message is held until the terminal is restored, so it neither paints
  over the screen nor leaves the viewer running on a cooked terminal. The hook is removed
  when `tui::run` returns.
