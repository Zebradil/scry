---
title: Live demo
description: scry's interactive viewer in the browser, on a synthetic fleet.
---

The viewer below is scry's TUI compiled to WebAssembly. It shows the report of
`SCRY_FIXTURE=1 scry colmena`: five machines of a synthetic fleet, with no machine contacted.
Configurations arrive one by one, as in a live run.

Click the viewer, then use the keys from its bottom line: `j`/`k` to move, `Tab` to switch panes,
`Enter` to fold, `/` to filter, `t` for the tree view, `w` to wrap, `1`–`9` to pick a machine.

<iframe src="../viewer/" title="scry viewer" style="width: 100%; height: 36rem; border: 0;"></iframe>

Copying with `y`/`Y` works only in a terminal.
