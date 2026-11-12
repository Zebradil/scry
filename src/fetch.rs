/// Changed files' contents in one `tar` stream per machine (spec 6.2 step 3).
pub mod content;
/// Tree listing, one round trip per link level (spec 6.2 step 2).
pub mod listing;
/// Toplevel resolution, markers, boot links, closure path-info (spec 6.2 step 1).
pub mod probe;
/// Followed references and small rebuilds, one round trip per depth (spec 6.2 step 4, 7.4).
pub mod references;
