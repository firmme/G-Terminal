//! The file table: its columns, the layout of a row's cells, truncation and
//! the header row.

use super::*;

/// One cell of a file-table row.
pub(super) struct Cell {
    pub(super) text: String,
    /// Right-align the metadata columns so their digits line up down the table.
    pub(super) right: bool,
    pub(super) color: egui::Color32,
    /// Shown on hover when `text` is a shortened form of something longer.
    pub(super) full: Option<String>,
    /// A glyph drawn before the text. The text's width budget makes room for it,
    /// so an icon can never push a name into the next column.
    pub(super) icon: Option<crate::icons::Icon>,
    /// Draw the shortcut badge over the icon.
    pub(super) link: bool,
}

/// Pixel widths for the metadata columns. The name takes whatever is left, rather
/// than every column taking a fraction: fractions squeeze the metadata into
/// truncation on a narrow pane even when the names would have fitted.
pub(super) struct Columns {
    size: f32,
    /// Zero when the table has no permissions to show: a column of width zero is
    /// dropped by [`column_offsets`].
    perms: f32,
    time: f32,
}

/// The local table carries permissions only where they exist. On Windows the
/// column's width is zero, so it drops out of the layout instead of showing a
/// fabricated mode.
#[cfg(unix)]
pub(super) const LOCAL_COLUMNS: Columns = Columns {
    size: 68.0,
    perms: 88.0,
    time: 96.0,
};
#[cfg(not(unix))]
pub(super) const LOCAL_COLUMNS: Columns = Columns {
    size: 68.0,
    perms: 0.0,
    time: 96.0,
};
pub(super) const REMOTE_COLUMNS: Columns = Columns {
    size: 68.0,
    perms: 88.0,
    time: 96.0,
};

#[cfg(unix)]
pub(super) const LOCAL_HEADERS: [&str; 4] = ["名称", "大小", "权限", "修改时间"];
#[cfg(not(unix))]
pub(super) const LOCAL_HEADERS: [&str; 3] = ["名称", "大小", "修改时间"];

/// Space held between columns, belonging to neither of them.
pub(super) const COLUMN_GAP: f32 = 10.0;

/// Left edge of every column, then the row's right edge.
///
/// The gap between columns is carried in these offsets but handed back by
/// [`column_rect`], so it ends up as space *between* the cells. Widening a cell
/// instead would not help: the text is free to fill whatever the cell is given.
pub(super) fn column_offsets(columns: &Columns, width: f32) -> Vec<f32> {
    /// Below this the names stop being readable, and a clipped name tells you far
    /// less than a clipped size or timestamp does.
    const MIN_NAME: f32 = 72.0;
    let mut trailing: Vec<f32> = [columns.size, columns.perms, columns.time]
        .into_iter()
        .filter(|width| *width > 0.0)
        .collect();
    let count = trailing.len();
    let gaps = COLUMN_GAP * count as f32;
    let total: f32 = trailing.iter().sum::<f32>() + gaps;
    let available = (width - MIN_NAME).max(1.0);
    if total > available && total > gaps {
        // Squeeze the metadata instead of the names.
        let scale = (available - gaps).max(1.0) / (total - gaps);
        for column in &mut trailing {
            *column *= scale;
        }
    }
    let name = (width - trailing.iter().sum::<f32>() - gaps).max(0.0);
    let mut offsets = vec![0.0, name + COLUMN_GAP];
    let mut x = name + COLUMN_GAP;
    for (index, column) in trailing.iter().enumerate() {
        x += column;
        // The last column ends the row; every other one hands its gap back.
        if index + 1 < count {
            x += COLUMN_GAP;
        }
        offsets.push(x);
    }
    offsets
}

/// The rect of column `index`, or None when the table has fewer columns.
pub(super) fn column_rect(row: egui::Rect, offsets: &[f32], index: usize) -> Option<egui::Rect> {
    let left = row.left() + *offsets.get(index)?;
    let right = row.left() + *offsets.get(index + 1)?;
    // Every column but the last gives the separator gap back, so it lands between
    // the cells rather than inside them.
    let right = if index + 2 < offsets.len() {
        right - COLUMN_GAP
    } else {
        right
    };
    Some(egui::Rect::from_min_max(
        egui::pos2(left, row.top()),
        egui::pos2(right, row.bottom()),
    ))
}

/// The narrowest a table may get before it scrolls sideways instead of squeezing
/// its columns. Below this the columns would clip, which tells the user less than
/// a scrollbar does.
pub(super) const MIN_TABLE_WIDTH: f32 = 420.0;

/// The least a path field may be squeezed to before its shortcut icons are given
/// up instead. A field narrower than this cannot show a path, and forcing the
/// width anyway would push the row outside its pane.
pub(super) const MIN_FIELD_WIDTH: f32 = 90.0;

/// Draws one file-table row at `width`. Returns the response and the rect of every
/// column, so the geometry is observable rather than implicit in the painting.
pub(super) fn table_row(
    ui: &mut egui::Ui,
    cells: &[Cell],
    columns: &Columns,
    selected: bool,
    width: f32,
) -> (egui::Response, Vec<egui::Rect>) {
    const ROW_HEIGHT: f32 = 18.0;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
    let offsets = column_offsets(columns, rect.width());
    let rects: Vec<_> = (0..cells.len())
        .filter_map(|index| column_rect(rect, &offsets, index))
        .collect();
    if ui.is_rect_visible(rect) {
        if selected || response.hovered() {
            let visuals = ui.style().interact_selectable(&response, selected);
            ui.painter().rect_filled(rect, 0, visuals.bg_fill);
        }
        let font =
            egui::FontId::proportional(ui.style().text_styles[&egui::TextStyle::Button].size);
        // A long name would otherwise paint straight across the columns to its
        // right: the painter is clipped to the scroll area, not to the cell.
        let mut hint: Option<String> = None;
        for (cell, cell_rect) in cells.iter().zip(&rects) {
            const PAD: f32 = 4.0;
            /// Between a leading icon and the text it introduces.
            const ICON_GAP: f32 = 5.0;
            let mut text_left = cell_rect.left() + PAD;
            let mut budget = cell_rect.width() - PAD * 2.0;
            if let Some(icon) = cell.icon {
                // Square, and inset from the row so it does not touch the text
                // above or below.
                let side = (cell_rect.height() - 4.0).max(8.0);
                let icon_rect = egui::Rect::from_min_size(
                    egui::pos2(text_left, cell_rect.center().y - side * 0.5),
                    egui::Vec2::splat(side),
                );
                crate::icons::draw(
                    ui.painter(),
                    icon_rect,
                    icon,
                    cell.color,
                    crate::icons::Size::Row.stroke(),
                );
                if cell.link {
                    crate::icons::link_badge(
                        ui.painter(),
                        icon_rect,
                        cell.color,
                        crate::icons::Size::Row.stroke(),
                    );
                }
                // Taken out of the text's budget, not added to the cell's, so a
                // name can never spill into the column beside it.
                text_left += side + ICON_GAP;
                budget -= side + ICON_GAP;
            }
            let (text, truncated) =
                truncate_to_width(ui, &cell.text, &font, cell.color, budget.max(0.0));
            // A cell that had to be cut offers its own full text.
            if truncated && hint.is_none() {
                hint = Some(cell.text.clone());
            }
            let (pos, align) = if cell.right {
                (
                    egui::pos2(cell_rect.right() - PAD, cell_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                )
            } else {
                (
                    egui::pos2(text_left, cell_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                )
            };
            ui.painter()
                .text(pos, align, &text, font.clone(), cell.color);
        }
        // A shortened cell carries the full value for the tooltip even when it was
        // not itself cut — that is how the timestamp column works.
        if hint.is_none() {
            hint = cells.iter().find_map(|cell| cell.full.clone());
        }
        if let Some(full) = hint {
            return (response.on_hover_text(full), rects);
        }
    }
    (response, rects)
}

/// Trims `text` until it fits `max_width`, appending an ellipsis when it had to
/// cut. Splits on character boundaries, so multi-byte names survive intact.
pub(super) fn truncate_to_width(
    ui: &egui::Ui,
    text: &str,
    font: &egui::FontId,
    color: egui::Color32,
    max_width: f32,
) -> (String, bool) {
    let measure = |candidate: &str| {
        ui.painter()
            .layout_no_wrap(candidate.to_owned(), font.clone(), color)
            .rect
            .width()
    };
    if max_width <= 0.0 {
        return (String::new(), !text.is_empty());
    }
    if measure(text) <= max_width {
        return (text.to_owned(), false);
    }
    let budget = max_width - measure("…");
    if budget <= 0.0 {
        return ("…".to_owned(), true);
    }
    // Largest character prefix that still fits once the ellipsis is appended.
    // The midpoint must be taken of the remaining range: `low + high.div_ceil(2)`
    // can jump past `high` and leave the range unshrunk, spinning forever.
    let (mut low, mut high) = (0usize, text.chars().count());
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let candidate: String = text.chars().take(mid).collect();
        if measure(&candidate) <= budget {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let mut truncated: String = text.chars().take(low).collect();
    truncated.push('…');
    (truncated, true)
}

/// The header row, laid out with the same columns as the rows below it.
pub(super) fn table_header(
    ui: &mut egui::Ui,
    p: Palette,
    columns: &Columns,
    labels: &[&str],
    width: f32,
) {
    let cells: Vec<Cell> = labels
        .iter()
        .enumerate()
        .map(|(index, text)| Cell {
            text: (*text).to_string(),
            // Only the name column is left-aligned.
            right: index > 0,
            color: p.muted,
            full: None,
            icon: None,
            link: false,
        })
        .collect();
    table_row(ui, &cells, columns, false, width);
    ui.separator();
}
