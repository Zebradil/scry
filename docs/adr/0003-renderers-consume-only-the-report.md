# Renderers consume only the report

Text, JSON and the TUI take `report::Report` (the TUI also its configurations one by one as they
finish) and nothing else: no fetch results, trees or seam. Shared changes are computed from the
report's configurations (`shared`), by the run for the final report and by the TUI as
configurations arrive.

So the JSON is complete by construction: anything text or the TUI can show, a JSON consumer can
get. Renderers are tested from hand-written reports, without a pipeline. The cost is that the
report must carry every fact a renderer shows, which is why it has the fields of ADR 0006.
Renderers may call pure helpers (`size`, normalisation in `files::churn`) on report data.
