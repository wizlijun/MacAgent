use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    term::Config,
    vte::ansi::{self, StdSyncHandler},
    Term,
};
use macagent_core::terminal::{
    diff_snapshots, history_segments, snapshot_from_term, TerminalHistory,
};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
    total_lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.total_lines
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

fn make_term(cols: usize, rows: usize, scrollback: usize) -> Term<VoidListener> {
    Term::new(
        Config {
            scrolling_history: scrollback,
            ..Default::default()
        },
        &Size {
            cols,
            rows,
            total_lines: rows + scrollback,
        },
        VoidListener,
    )
}

fn feed(
    term: &mut Term<VoidListener>,
    processor: &mut ansi::Processor<StdSyncHandler>,
    data: &[u8],
) {
    for segment in history_segments(data) {
        processor.advance(term, segment);
    }
}

// ---------------------------------------------------------------------------
// snapshot tests
// ---------------------------------------------------------------------------

#[test]
fn snapshot_diff_returns_none_when_unchanged() {
    let term = make_term(20, 4, 128);
    let snap1 = snapshot_from_term(&term, 1);
    let snap2 = snapshot_from_term(&term, 1);
    // Same revision, same content → no delta.
    assert!(diff_snapshots(&snap1, &snap2).is_none());
}

#[test]
fn snapshot_diff_only_reports_changed_lines() {
    let mut processor = ansi::Processor::<StdSyncHandler>::new();
    let mut term = make_term(20, 4, 128);

    let snap1 = snapshot_from_term(&term, 1);

    // Write one line of text — only that row changes.
    feed(&mut term, &mut processor, b"hello");
    let snap2 = snapshot_from_term(&term, 2);

    let delta = diff_snapshots(&snap1, &snap2).expect("should have delta");
    // Only the cursor row (row 0) changed.
    assert_eq!(delta.lines.len(), 1);
    assert_eq!(delta.lines[0].index, 0);
}

#[test]
fn snapshot_apply_delta_round_trip() {
    let mut processor = ansi::Processor::<StdSyncHandler>::new();
    let mut term = make_term(20, 4, 128);

    let mut snap1 = snapshot_from_term(&term, 1);

    feed(&mut term, &mut processor, b"hello world\r\nnext line");
    let snap2 = snapshot_from_term(&term, 2);

    let delta = diff_snapshots(&snap1, &snap2).expect("should have delta");
    snap1.apply_delta(&delta);

    assert_eq!(snap1, snap2);
}

// ---------------------------------------------------------------------------
// runs tests
// ---------------------------------------------------------------------------

// We test run merging indirectly through snapshot: write text with a single
// default style and verify the first row has exactly 1 run containing all chars.
#[test]
fn runs_merge_combines_same_style_cells() {
    let mut processor = ansi::Processor::<StdSyncHandler>::new();
    let mut term = make_term(20, 4, 128);

    feed(&mut term, &mut processor, b"abc");
    let snap = snapshot_from_term(&term, 1);

    let row = &snap.lines[0];
    // All 3 chars have same default style → merged into the first run.
    assert_eq!(row.runs.len(), 1);
    assert!(row.runs[0].text.starts_with("abc"));
}

#[test]
fn runs_split_on_style_change() {
    let mut processor = ansi::Processor::<StdSyncHandler>::new();
    let mut term = make_term(20, 4, 128);

    // Write "abc" in red foreground, then "def" in green foreground.
    // ESC[31m = red, ESC[32m = green, ESC[m = reset
    feed(&mut term, &mut processor, b"\x1b[31mabc\x1b[32mdef\x1b[m");
    let snap = snapshot_from_term(&term, 1);

    let row = &snap.lines[0];
    // "abc" and "def" have different fg colors → at least 2 runs.
    assert!(
        row.runs.len() >= 2,
        "expected >=2 runs, got {}",
        row.runs.len()
    );
    let combined: String = row.runs.iter().map(|r| r.text.as_str()).collect();
    assert!(combined.starts_with("abcdef"));
}

// ---------------------------------------------------------------------------
// history tests
// ---------------------------------------------------------------------------

#[test]
fn history_appends_when_scrollback_overflows() {
    let mut processor = ansi::Processor::<StdSyncHandler>::new();
    // 4-row terminal with 128-line scrollback
    let mut term = make_term(20, 4, 128);
    let mut history = TerminalHistory::default();

    let mut observe = |term: &mut Term<VoidListener>, chunk: &[u8]| {
        let mut appended = Vec::new();
        for segment in history_segments(chunk) {
            let was_alt = term
                .mode()
                .contains(alacritty_terminal::term::TermMode::ALT_SCREEN);
            processor.advance(term, segment);
            appended.extend(history.observe_term(term, was_alt));
        }
        appended
    };

    // Fill the 4-row screen then push a 5th line to force scrollback.
    observe(&mut term, b"line1\r\nline2\r\nline3\r\nline4\r\n");
    // 5th line causes "line1" to scroll into scrollback.
    let appended = observe(&mut term, b"line5\r\n");

    // History should contain the scrolled-out line(s).
    assert!(
        !appended.is_empty() || !history.lines().is_empty(),
        "history should have captured scrolled content"
    );
}

#[test]
fn history_handles_alt_screen_switch() {
    let mut processor = ansi::Processor::<StdSyncHandler>::new();
    let mut term = make_term(20, 4, 128);
    let mut history = TerminalHistory::default();

    let mut observe = |term: &mut Term<VoidListener>, chunk: &[u8]| -> Vec<String> {
        let mut appended = Vec::new();
        for segment in history_segments(chunk) {
            let was_alt = term
                .mode()
                .contains(alacritty_terminal::term::TermMode::ALT_SCREEN);
            processor.advance(term, segment);
            appended.extend(history.observe_term(term, was_alt));
        }
        appended
    };

    // Write something on main screen.
    observe(&mut term, b"main line\r\n");
    // Enter alternate screen.
    observe(&mut term, b"\x1b[?1049h");
    assert!(
        term.mode()
            .contains(alacritty_terminal::term::TermMode::ALT_SCREEN),
        "should be in alt screen"
    );
    // Write something on alt screen.
    observe(&mut term, b"alt content\r\n");
    // Exit alternate screen.
    observe(&mut term, b"\x1b[?1049l");
    assert!(
        !term
            .mode()
            .contains(alacritty_terminal::term::TermMode::ALT_SCREEN),
        "should be back on main screen"
    );

    // After exiting alt screen, history revision should have advanced
    // (content was captured).
    assert!(history.revision() > 0, "history revision should advance");
}
