pub use history::{history_segments, TerminalHistory, MAX_HISTORY_LINES};
pub use snapshot::{diff_snapshots, snapshot_from_term, TerminalDelta, TerminalSnapshot};

mod history;
mod runs;
mod snapshot;
