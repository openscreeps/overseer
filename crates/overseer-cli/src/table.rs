//! The tables of `profiles` and `servers`.

use std::io::{self, Write};

/// Prints `rows` as a table with aligned columns. The first row is the
/// header. Each row has the same number of cells.
pub(crate) fn print(rows: &[Vec<String>], out: &mut dyn Write) -> io::Result<()> {
    let mut widths = vec![0; rows.first().map_or(0, Vec::len)];
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    for row in rows {
        let last = row.len().saturating_sub(1);
        for (at, (cell, width)) in row.iter().zip(&widths).enumerate() {
            if at == last {
                writeln!(out, "{cell}")?;
            } else {
                write!(out, "{cell:<width$}  ")?;
            }
        }
    }
    Ok(())
}
