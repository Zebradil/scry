/// Churn classification: hash-only, version-only, real (spec 7.3).
pub mod churn;
/// File statuses from two dereferenced trees (spec 7.3).
pub mod compare;
/// Unified line diffs with 3 lines of context (spec 7.3).
pub mod diff;
/// Word highlight: line pairing and token LCS (spec 7.3).
pub mod highlight;
/// Store references in text and their old/new pairing (spec 7.4).
pub mod refs;
