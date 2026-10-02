//! File table layout and text truncation.

use super::*;

pub(super) struct Cell {
    pub(super) text: String,
    pub(super) right: bool,
    pub(super) color: egui::Color32,
    pub(super) full: Option<String>,
    pub(super) icon: Option<crate::icons::Icon>,
    pub(super) link: bool,
}

pub(super) struct Columns {
    size: f32,
    perms: f32,
    time: f32,
}

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

pub(super) const COLUMN_GAP: f32 = 10.0;

pub(super) fn column_offsets(columns: &Columns, width: f32) -> Vec<f32> {
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
        if index + 1 < count {
            x += COLUMN_GAP;
        }
        offsets.push(x);
    }
    offsets
}

pub(super) fn column_rect(row: egui::Rect, offsets: &[f32], index: usize) -> Option<egui::Rect> {
    let left = row.left() + *offsets.get(index)?;
    let right = row.left() + *offsets.get(index + 1)?;
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

pub(super) const MIN_TABLE_WIDTH: f32 = 420.0;

pub(super) const MIN_FIELD_WIDTH: f32 = 90.0;

pub(super) const ROW_HEIGHT: f32 = 18.0;

pub(super) fn table_row(
    ui: &mut egui::Ui,
    cells: &[Cell],
    columns: &Columns,
    selected: bool,
    width: f32,
) -> (egui::Response, Vec<egui::Rect>) {
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
        let mut hint: Option<String> = None;
        for (cell, cell_rect) in cells.iter().zip(&rects) {
            const PAD: f32 = 4.0;
            const ICON_GAP: f32 = 5.0;
            let mut text_left = cell_rect.left() + PAD;
            let mut budget = cell_rect.width() - PAD * 2.0;
            if let Some(icon) = cell.icon {
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
                text_left += side + ICON_GAP;
                budget -= side + ICON_GAP;
            }
            let (text, truncated) =
                truncate_to_width(ui, &cell.text, &font, cell.color, budget.max(0.0));
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
        if hint.is_none() {
            hint = cells.iter().find_map(|cell| cell.full.clone());
        }
        if let Some(full) = hint {
            return (response.on_hover_text(full), rects);
        }
    }
    (response, rects)
}

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
