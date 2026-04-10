//! Table rendering for the Hypercolor CLI.
//!
//! Auto-aligned columns with ANSI-aware width calculation so that colored
//! cells don't break alignment.

use unicode_width::UnicodeWidthStr;

/// Strip ANSI escape sequences from a string for width measurement.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if c == '\x1b' {
            in_escape = true;
        } else {
            out.push(c);
        }
    }
    out
}

/// Measure the display width of a string, ignoring ANSI escape sequences.
fn display_width(s: &str) -> usize {
    strip_ansi(s).width()
}

/// Print a simple table with headers and rows.
///
/// Each row is a slice of column values. Columns are auto-aligned based on
/// the widest visible value in each column (ANSI escapes excluded from width
/// calculation).
pub fn print_table(headers: &[&str], rows: &[Vec<String>], quiet: bool) {
    if rows.is_empty() && quiet {
        return;
    }

    let col_count = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.width()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
    }

    // Header
    let header_line: String = headers
        .iter()
        .enumerate()
        .map(|(i, h)| format!("{h:<width$}", width = widths[i]))
        .collect::<Vec<_>>()
        .join("  ");
    println!("  {header_line}");

    // Separator
    let sep_width: usize = widths.iter().sum::<usize>() + (col_count.saturating_sub(1)) * 2;
    let separator = "\u{2500}".repeat(sep_width);
    println!("  {separator}");

    // Rows — pad based on display width, not byte length
    for row in rows {
        let line: String = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let w = widths
                    .get(i)
                    .copied()
                    .unwrap_or_else(|| display_width(cell));
                let visible = display_width(cell);
                let padding = w.saturating_sub(visible);
                format!("{cell}{:padding$}", "")
            })
            .collect::<Vec<_>>()
            .join("  ");
        println!("  {line}");
    }
}
