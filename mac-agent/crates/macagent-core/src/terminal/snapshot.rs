use alacritty_terminal::{
    event::EventListener, grid::Dimensions, index::Line, term::TermMode, Term,
};

use crate::ctrl_msg::TerminalLine;

use super::runs::{row_runs, row_wrapped};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSnapshot {
    pub revision: u64,
    pub cols: u16,
    pub rows: u16,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
    pub title: Option<String>,
    pub lines: Vec<TerminalLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalDelta {
    pub revision: u64,
    pub cols: u16,
    pub rows: u16,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
    pub title: Option<String>,
    pub lines: Vec<TerminalLine>,
}

pub fn snapshot_from_term<T: EventListener>(term: &Term<T>, revision: u64) -> TerminalSnapshot {
    let cols = term.columns();
    let rows = term.screen_lines();
    let cursor = term.grid().cursor.point;

    let lines = (0..rows)
        .map(|row| TerminalLine {
            index: row as u16,
            runs: row_runs(term.grid(), Line(row as i32), cols),
            wrapped: row_wrapped(term.grid(), Line(row as i32), cols),
        })
        .collect();

    TerminalSnapshot {
        revision,
        cols: cols as u16,
        rows: rows as u16,
        cursor_row: cursor.line.0.max(0) as u16,
        cursor_col: cursor.column.0 as u16,
        cursor_visible: term.mode().contains(TermMode::SHOW_CURSOR),
        title: None,
        lines,
    }
}

pub fn diff_snapshots(
    previous: &TerminalSnapshot,
    next: &TerminalSnapshot,
) -> Option<TerminalDelta> {
    let lines = next
        .lines
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| match previous.lines.get(idx) {
            Some(prev) if prev == line => None,
            _ => Some(line.clone()),
        })
        .collect::<Vec<_>>();
    if previous.cols == next.cols
        && previous.rows == next.rows
        && previous.cursor_row == next.cursor_row
        && previous.cursor_col == next.cursor_col
        && previous.cursor_visible == next.cursor_visible
        && previous.title == next.title
        && lines.is_empty()
    {
        return None;
    }
    Some(TerminalDelta {
        revision: next.revision,
        cols: next.cols,
        rows: next.rows,
        cursor_row: next.cursor_row,
        cursor_col: next.cursor_col,
        cursor_visible: next.cursor_visible,
        title: next.title.clone(),
        lines,
    })
}

impl TerminalSnapshot {
    pub fn apply_delta(&mut self, delta: &TerminalDelta) {
        self.revision = delta.revision;
        self.cols = delta.cols;
        self.rows = delta.rows;
        self.cursor_row = delta.cursor_row;
        self.cursor_col = delta.cursor_col;
        self.cursor_visible = delta.cursor_visible;
        self.title = delta.title.clone();
        let target_len = usize::from(delta.rows);
        if self.lines.len() < target_len {
            self.lines
                .extend((self.lines.len()..target_len).map(|idx| TerminalLine {
                    index: idx as u16,
                    runs: vec![],
                    wrapped: false,
                }));
        } else if self.lines.len() > target_len {
            self.lines.truncate(target_len);
        }
        for line in &delta.lines {
            let idx = usize::from(line.index);
            if idx < self.lines.len() {
                self.lines[idx] = line.clone();
            }
        }
    }
}
