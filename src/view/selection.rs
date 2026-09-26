//! Turning a screen selection into text and finding the link under the pointer.

/// Rows are addressed relative to the live screen: zero is its first row,
/// negative rows are in scrollback. Changing the viewport does not move a
/// selection. The alternate screen has no scrollback and uses the same logic.
pub(super) fn history_point(row: u16, col: u16, scrollback: usize) -> (i64, u16) {
    (i64::from(row) - scrollback as i64, col)
}

pub(super) fn selection_text_history(
    screen: &mut vt100::Screen,
    a: (i64, u16),
    b: (i64, u16),
) -> String {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let original = screen.scrollback();
    let (height, width) = screen.size();
    let mut result = String::new();
    let mut previous_wrapped = false;
    let mut had_row = false;
    for absolute_row in a.0..=b.0.min(i64::from(height) - 1) {
        let requested = absolute_row.saturating_neg().max(0) as usize;
        screen.set_scrollback(requested);
        let row = absolute_row + screen.scrollback() as i64;
        if !(0..i64::from(height)).contains(&row) {
            continue; // Older history was evicted while the selection existed.
        }
        let row = row as u16;
        if had_row && !previous_wrapped {
            result.push('\n');
        }
        let start = if absolute_row == a.0 { a.1 } else { 0 };
        let end = if absolute_row == b.0 { b.1 } else { width - 1 };
        let mut line = String::new();
        for col in start..=end.min(width - 1) {
            if let Some(cell) = screen.cell(row, col)
                && !cell.is_wide_continuation()
            {
                line.push_str(if cell.contents().is_empty() {
                    " "
                } else {
                    cell.contents()
                });
            }
        }
        result.push_str(line.trim_end());
        previous_wrapped = screen.row_wrapped(row);
        had_row = true;
    }
    screen.set_scrollback(original);
    result
}

/// One screen row as text, plus the screen column each character came from, so
/// a link's character span can be mapped back to cells for an underline.
pub(super) fn row_line(screen: &vt100::Screen, row: u16, cols: u16) -> (String, Vec<u16>) {
    let mut text = String::new();
    let mut columns = Vec::new();
    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else {
            continue;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let contents = cell.contents();
        if contents.is_empty() {
            text.push(' ');
            columns.push(col);
        } else {
            for ch in contents.chars() {
                text.push(ch);
                columns.push(col);
            }
        }
    }
    (text, columns)
}

/// The URL covering character `index`, as `(url, start, end)` character
/// indices. Only explicit `http(s)://` and `www.` links are taken, so ordinary
/// words are not turned into links.
pub(super) fn link_at(text: &str, index: usize) -> Option<(String, usize, usize)> {
    for marker in ["http://", "https://", "www."] {
        let mut from = 0;
        while let Some(found) = text[from..].find(marker) {
            let start = from + found;
            let mut end = start;
            for (offset, ch) in text[start..].char_indices() {
                if ch.is_whitespace() {
                    break;
                }
                end = start + offset + ch.len_utf8();
            }
            // Trailing punctuation is usually the sentence's, not the URL's.
            while end > start {
                let last = text[start..end].chars().next_back().unwrap();
                if matches!(
                    last,
                    '.' | ',' | ';' | ':' | ')' | ']' | '}' | '\'' | '"' | '*'
                ) {
                    end -= last.len_utf8();
                } else {
                    break;
                }
            }
            let start_char = text[..start].chars().count();
            let end_char = text[..end].chars().count();
            if index >= start_char && index < end_char {
                let raw = &text[start..end];
                let url = if raw.starts_with("www.") {
                    format!("http://{raw}")
                } else {
                    raw.to_string()
                };
                return Some((url, start_char, end_char));
            }
            from = end.max(start + 1);
        }
    }
    None
}
