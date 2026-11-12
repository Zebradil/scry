# tokio for concurrency

A run waits on many child processes at once: up to `--jobs` configurations, each with commands on
up to two machines, up to `--eval-jobs` evaluations, at most 4 commands per SSH connection. Each
configuration's fetch phase has a timeout that must kill its children, Ctrl-C must kill all of
them, and the TUI shows each configuration's status while the rest still run.

We use tokio, multi-threaded runtime:

- One task per configuration. `--jobs`, `--eval-jobs` and the per-connection limit are
  `tokio::sync::Semaphore`s.
- Children are spawned with `kill_on_drop(true)`, so cancelling is dropping: the timeout is
  `tokio::time::timeout` around a configuration's fetch phase, and Ctrl-C (`tokio::signal::ctrl_c`)
  drops the whole run. No command needs a separate kill path.
- Progress is a `tokio::sync::mpsc::unbounded_channel` of finished `report::Configuration`s. The
  TUI runs its blocking crossterm loop on its own thread and drains the channel with `try_recv`
  between input polls; text and JSON ignore it and render the final report.
- Pure core work (diffs, highlight) runs inside the configuration's task; move it to
  `spawn_blocking` only if a measurement shows it starving the runtime.

## Considered options

- Threads with blocking `std::process`: every timeout needs a watchdog that finds and kills the
  right children, and Ctrl-C needs the same bookkeeping for all of them.
- smol or async-std: smaller, but tokio alone covers processes, signals, timers and channels.
