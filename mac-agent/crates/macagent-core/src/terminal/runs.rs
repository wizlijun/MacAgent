use alacritty_terminal::{
    index::{Column, Line},
    term::cell::{Cell, Flags},
    vte::ansi::{Color, NamedColor},
    Grid,
};

use crate::ctrl_msg::{TerminalColor, TerminalRun};

pub(super) fn row_runs(grid: &Grid<Cell>, line: Line, cols: usize) -> Vec<TerminalRun> {
    let mut runs = Vec::new();
    let mut current: Option<TerminalRun> = None;
    for col in 0..cols {
        let cell = &grid[line][Column(col)];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }

        let next_run = TerminalRun {
            text: cell_text(cell),
            fg: color_to_wire(cell.fg),
            bg: color_to_wire(cell.bg),
            bold: cell.flags.contains(Flags::BOLD),
            dim: cell.flags.contains(Flags::DIM),
            italic: cell.flags.contains(Flags::ITALIC),
            underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
            inverse: cell.flags.contains(Flags::INVERSE),
        };

        match current.as_mut() {
            Some(active) if same_style(active, &next_run) => active.text.push_str(&next_run.text),
            Some(active) => {
                runs.push(active.clone());
                current = Some(next_run);
            }
            None => current = Some(next_run),
        }
    }
    if let Some(active) = current {
        runs.push(active);
    }
    runs
}

pub(super) fn row_wrapped(grid: &Grid<Cell>, line: Line, cols: usize) -> bool {
    if cols == 0 {
        return false;
    }
    grid[line][Column(cols - 1)].flags.contains(Flags::WRAPLINE)
}

fn cell_text(cell: &Cell) -> String {
    let mut text = String::new();
    text.push(cell.c);
    if let Some(zerowidth) = cell.zerowidth() {
        text.extend(zerowidth.iter().copied());
    }
    text
}

fn same_style(left: &TerminalRun, right: &TerminalRun) -> bool {
    left.fg == right.fg
        && left.bg == right.bg
        && left.bold == right.bold
        && left.dim == right.dim
        && left.italic == right.italic
        && left.underline == right.underline
        && left.inverse == right.inverse
}

fn color_to_wire(color: Color) -> Option<TerminalColor> {
    match color {
        Color::Named(NamedColor::Foreground) | Color::Named(NamedColor::Background) => None,
        Color::Named(named) => named_to_wire(named),
        Color::Indexed(value) => Some(TerminalColor::Indexed { value }),
        Color::Spec(rgb) => Some(TerminalColor::Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        }),
    }
}

fn named_to_wire(color: NamedColor) -> Option<TerminalColor> {
    let indexed: u8 = match color {
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        NamedColor::BrightWhite => 15,
        NamedColor::DimBlack => 0,
        NamedColor::DimRed => 1,
        NamedColor::DimGreen => 2,
        NamedColor::DimYellow => 3,
        NamedColor::DimBlue => 4,
        NamedColor::DimMagenta => 5,
        NamedColor::DimCyan => 6,
        NamedColor::DimWhite => 7,
        NamedColor::DimForeground => return None,
        NamedColor::BrightForeground | NamedColor::Foreground => return None,
        NamedColor::Background => return None,
        NamedColor::Cursor => return None,
    };
    Some(TerminalColor::Indexed { value: indexed })
}
