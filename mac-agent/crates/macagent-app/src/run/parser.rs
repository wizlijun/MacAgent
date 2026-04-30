//! Parser state: feeds raw PTY bytes into an alacritty `Term` and produces
//! `TerminalSnapshot` / diff events via macagent_core::terminal.

use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    term::{Config as TermConfig, TermMode},
    vte::ansi,
    Term,
};
use macagent_core::terminal::{
    diff_snapshots, history_segments, snapshot_from_term, TerminalHistory, TerminalSnapshot,
};

const SCROLLBACK: usize = 2000;

#[derive(Clone, Copy)]
struct TermSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows + SCROLLBACK
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Combined parser + history state for one PTY session.
pub struct ParserState {
    term: Term<VoidListener>,
    processor: ansi::Processor,
    history: TerminalHistory,
    revision: u64,
    pub last_snapshot: Option<TerminalSnapshot>,
}

impl ParserState {
    pub fn new(cols: u16, rows: u16) -> Self {
        let config = TermConfig {
            scrolling_history: SCROLLBACK,
            ..Default::default()
        };
        let size = TermSize {
            cols: cols as usize,
            rows: rows as usize,
        };
        Self {
            term: Term::new(config, &size, VoidListener),
            processor: ansi::Processor::new(),
            history: TerminalHistory::default(),
            revision: 0,
            last_snapshot: None,
        }
    }

    /// Feed raw bytes from the PTY into the terminal emulator.
    /// Returns any new history lines that were appended.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut history_lines = Vec::new();
        for segment in history_segments(bytes) {
            let was_alternate = self.term.mode().contains(TermMode::ALT_SCREEN);
            self.processor.advance(&mut self.term, segment);
            history_lines.extend(self.history.observe_term(&self.term, was_alternate));
        }
        self.revision += 1;
        history_lines
    }

    pub fn history_revision(&self) -> u64 {
        self.history.revision()
    }

    pub fn history_lines(&self) -> Vec<String> {
        self.history.lines()
    }

    /// Take a full snapshot of the current terminal state.
    pub fn snapshot(&self) -> TerminalSnapshot {
        snapshot_from_term(&self.term, self.revision)
    }

    /// Diff against last_snapshot, update last_snapshot, return delta if changed.
    /// Also returns the new snapshot for keyframe sends.
    pub fn diff(&mut self) -> Option<macagent_core::terminal::TerminalDelta> {
        let snap = snapshot_from_term(&self.term, self.revision);
        let delta = match &self.last_snapshot {
            None => {
                // First time — treat as full snapshot available via last_snapshot.
                self.last_snapshot = Some(snap);
                return None;
            }
            Some(prev) => diff_snapshots(prev, &snap),
        };
        self.last_snapshot = Some(snap);
        delta
    }

    /// Resize the terminal emulator (does not touch the real PTY).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let size = TermSize {
            cols: cols as usize,
            rows: rows as usize,
        };
        self.term.resize(size);
        self.history.sync_baseline(&self.term);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_simple_text_yields_snapshot() {
        let mut parser = ParserState::new(80, 24);
        parser.feed(b"hello");
        let snap = parser.snapshot();
        // At least one line should contain "hello".
        let found = snap
            .lines
            .iter()
            .any(|line| line.runs.iter().any(|run| run.text.contains("hello")));
        assert!(
            found,
            "snapshot should contain 'hello', lines: {:?}",
            snap.lines
        );
    }

    #[test]
    fn diff_returns_none_when_unchanged() {
        let mut parser = ParserState::new(80, 24);
        // Seed last_snapshot.
        parser.feed(b"abc");
        let _ = parser.diff(); // sets last_snapshot
                               // diff again without any new feed — revision unchanged so diff == None.
        let delta = parser.diff();
        assert!(delta.is_none(), "expected None delta when nothing changed");
    }
}
