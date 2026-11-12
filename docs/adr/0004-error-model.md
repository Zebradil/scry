# Error model: anyhow chains, typed errors only where callers branch

Spec 4.7 and 9 distinguish three failures:

- **Usage error**, exit 2 with or without `--exit-code`. clap reports bad flags itself. Later
  checks (an unknown selection pattern, `--base` outside a git checkout) raise
  `exit::UsageError` as an `anyhow::Error`; `exit::Outcome::of_error` finds it under any added
  context.
- **Fatal error**, `scry: <message>` on stderr, exit as a failure. Any other `anyhow::Error` that
  reaches `run::main`.
- **Per-configuration error**: the configuration's task returns `anyhow::Result`; an `Err`
  becomes `Status::Error { error: format!("{err:#}") }`, and the run goes on. A timeout becomes
  `Status::Timeout`.

We use anyhow for propagation because its alternate format (`{:#}`) prints the context chain
joined by `": "`, the exact form spec 8.3 asks for, and `.context()` builds that chain where the
error passes a layer. thiserror defines the few errors a caller matches on: `exec::ExecError`
(spec 9 wordings; `NotFound` for `colmena` becomes `colmena not found on PATH`) and `UsageError`.
Both crates are MIT or Apache-2.0.

Full argv and stderr of every command go to stderr when `SCRY_DEBUG` is non-empty (spec 9 leaves
flag or variable open). A variable keeps the command line exactly that of spec 4 and reaches the
seam without threading a flag through every layer. `scry --help` documents it.
