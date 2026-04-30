use std::collections::VecDeque;

use alacritty_terminal::{
    event::EventListener,
    grid::Dimensions,
    index::{Column, Line},
    term::{
        cell::{Cell, Flags},
        TermMode,
    },
    Grid, Term,
};

pub const MAX_HISTORY_LINES: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
struct PlainRow {
    text: String,
    wrapped: bool,
}

#[derive(Debug, Default)]
pub struct TerminalHistory {
    revision: u64,
    history: VecDeque<String>,
    pending_fragment: String,
    previous_scrollback_rows: Vec<PlainRow>,
    previous_alt_rows: Option<Vec<PlainRow>>,
}

impl TerminalHistory {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn lines(&self) -> Vec<String> {
        self.history.iter().cloned().collect()
    }

    pub fn observe_term<T: EventListener>(
        &mut self,
        term: &Term<T>,
        was_alternate: bool,
    ) -> Vec<String> {
        let is_alternate = term.mode().contains(TermMode::ALT_SCREEN);
        let mut appended = Vec::new();

        if is_alternate {
            let rows = visible_plain_rows(term.grid(), term.columns());
            if let Some(previous) = &self.previous_alt_rows {
                let removed = detect_scrolled_rows(previous, &rows);
                appended.extend(self.append_rows(&removed));
            }
            self.previous_alt_rows = Some(rows);
            return appended;
        }

        // Leaving alternate screen; attempt to flush whatever we captured there.
        if was_alternate {
            if let Some(previous) = self.previous_alt_rows.take() {
                appended.extend(self.append_rows(&previous));
            }
            if !self.pending_fragment.is_empty() {
                let trailing = std::mem::take(&mut self.pending_fragment);
                appended.extend(self.extend_history([trailing]));
            }
        }

        let scrollback = scrollback_plain_rows(term.grid(), term.columns());
        let overlap = overlap_suffix_prefix(&self.previous_scrollback_rows, &scrollback);
        appended.extend(self.append_rows(&scrollback[overlap..]));
        self.previous_scrollback_rows = scrollback;

        appended
    }

    pub fn sync_baseline<T: EventListener>(&mut self, term: &Term<T>) {
        self.pending_fragment.clear();
        self.previous_scrollback_rows = scrollback_plain_rows(term.grid(), term.columns());
        self.previous_alt_rows = term
            .mode()
            .contains(TermMode::ALT_SCREEN)
            .then(|| visible_plain_rows(term.grid(), term.columns()));
    }

    fn append_rows(&mut self, rows: &[PlainRow]) -> Vec<String> {
        if rows.is_empty() {
            return Vec::new();
        }

        let mut lines = Vec::new();
        for row in rows {
            self.pending_fragment.push_str(&row.text);
            if !row.wrapped {
                lines.push(std::mem::take(&mut self.pending_fragment));
            }
        }
        self.extend_history(lines)
    }

    fn extend_history<I>(&mut self, lines: I) -> Vec<String>
    where
        I: IntoIterator<Item = String>,
    {
        let mut appended = Vec::new();
        let mut changed = false;

        for line in lines {
            let normalized = normalize_line(line);
            let skip_empty =
                normalized.is_empty() && self.history.back().is_some_and(|last| last.is_empty());
            let skip_duplicate = self.history.back().is_some_and(|last| last == &normalized);
            if skip_empty || skip_duplicate {
                continue;
            }
            self.history.push_back(normalized.clone());
            appended.push(normalized);
            changed = true;
        }

        while self.history.len() > MAX_HISTORY_LINES {
            self.history.pop_front();
            changed = true;
        }

        if changed {
            self.revision += 1;
        }

        appended
    }
}

pub fn history_segments(chunk: &[u8]) -> Vec<&[u8]> {
    if chunk.is_empty() {
        return Vec::new();
    }

    let mut segments = Vec::new();
    let mut start = 0;
    for (index, byte) in chunk.iter().enumerate() {
        if *byte == b'\n' {
            segments.push(&chunk[start..=index]);
            start = index + 1;
        }
    }
    if start < chunk.len() {
        segments.push(&chunk[start..]);
    }
    if segments.is_empty() {
        segments.push(chunk);
    }
    segments
}

fn scrollback_plain_rows(grid: &Grid<Cell>, cols: usize) -> Vec<PlainRow> {
    let history_size = grid.history_size();
    if history_size == 0 {
        return Vec::new();
    }
    let start = -(history_size as i32);
    (start..0)
        .map(|line| plain_row(grid, Line(line), cols))
        .collect()
}

fn visible_plain_rows(grid: &Grid<Cell>, cols: usize) -> Vec<PlainRow> {
    let rows = grid.screen_lines();
    (0..rows)
        .map(|row| plain_row(grid, Line(row as i32), cols))
        .collect()
}

fn plain_row(grid: &Grid<Cell>, line: Line, cols: usize) -> PlainRow {
    let mut text = String::new();
    for col in 0..cols {
        let cell = &grid[line][Column(col)];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        text.push(cell.c);
        if let Some(zerowidth) = cell.zerowidth() {
            text.extend(zerowidth.iter().copied());
        }
    }
    PlainRow {
        text: normalize_line(text),
        wrapped: row_wrapped(grid, line, cols),
    }
}

fn row_wrapped(grid: &Grid<Cell>, line: Line, cols: usize) -> bool {
    if cols == 0 {
        return false;
    }
    grid[line][Column(cols - 1)].flags.contains(Flags::WRAPLINE)
}

fn normalize_line(line: String) -> String {
    line.trim_end_matches(' ').to_string()
}

fn overlap_suffix_prefix<T: PartialEq>(previous: &[T], current: &[T]) -> usize {
    let max_overlap = previous.len().min(current.len());
    for overlap in (0..=max_overlap).rev() {
        if previous[previous.len() - overlap..] == current[..overlap] {
            return overlap;
        }
    }
    0
}

fn detect_scrolled_rows(previous: &[PlainRow], next: &[PlainRow]) -> Vec<PlainRow> {
    #[derive(Clone, Copy)]
    struct Candidate {
        start: usize,
        shift: usize,
        matched: usize,
        contentful: usize,
    }

    let mut best: Option<Candidate> = None;

    for start in 0..previous.len() {
        for shift in 1..previous.len().saturating_sub(start) {
            let overlap = previous
                .len()
                .saturating_sub(start + shift)
                .min(next.len().saturating_sub(start));
            if overlap < 2 {
                continue;
            }

            let mut matched = 0;
            let mut contentful = 0;
            while matched < overlap {
                let left = &previous[start + shift + matched];
                let right = &next[start + matched];
                if left.text != right.text {
                    break;
                }
                if !left.text.is_empty() {
                    contentful += 1;
                }
                matched += 1;
            }

            if matched < 2 || contentful == 0 {
                continue;
            }
            if previous[start..start + shift]
                .iter()
                .all(|row| row.text.is_empty())
            {
                continue;
            }

            let candidate = Candidate {
                start,
                shift,
                matched,
                contentful,
            };
            let replace = match best {
                None => true,
                Some(current) => {
                    candidate.matched > current.matched
                        || (candidate.matched == current.matched
                            && candidate.contentful > current.contentful)
                        || (candidate.matched == current.matched
                            && candidate.contentful == current.contentful
                            && candidate.start < current.start)
                }
            };
            if replace {
                best = Some(candidate);
            }
        }
    }

    best.map_or_else(Vec::new, |candidate| {
        previous[candidate.start..candidate.start + candidate.shift].to_vec()
    })
}
