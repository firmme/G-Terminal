//! The editing context menu egui's text fields do not have.
//!
//! `TextEdit` handles the keyboard shortcuts (copy, cut, paste, select-all) but
//! offers no menu of its own, so every field gets one attached here.

use eframe::egui;

/// A single-line field with the standard editing menu attached. `customize`
/// adjusts the builder — width, hint text and so on — before it is added.
pub(crate) fn field_with(
    ui: &mut egui::Ui,
    text: &mut String,
    customize: impl FnOnce(egui::TextEdit<'_>) -> egui::TextEdit<'_>,
) -> egui::Response {
    let response = ui.add(customize(egui::TextEdit::singleline(text)));
    context_menu(&response, text);
    response
}

/// Like [`field_with`], but the field is disabled while `enabled` is false.
pub(crate) fn field_enabled_with(
    ui: &mut egui::Ui,
    enabled: bool,
    text: &mut String,
    customize: impl FnOnce(egui::TextEdit<'_>) -> egui::TextEdit<'_>,
) -> egui::Response {
    let response = ui.add_enabled(enabled, customize(egui::TextEdit::singleline(text)));
    context_menu(&response, text);
    response
}

/// 复制 / 剪切 / 粘贴 / 全选. The selection lives in the widget's state, so the
/// text is edited directly and the state written back with it.
fn context_menu(response: &egui::Response, text: &mut String) {
    response.context_menu(|ui| {
        let id = response.id;
        let mut state = egui::TextEdit::load_state(ui.ctx(), id);
        let range = state.as_ref().and_then(|state| state.cursor.char_range());
        let selected = range
            .map(|range| range.slice_str(text).to_owned())
            .unwrap_or_default();
        let has_selection = !selected.is_empty();
        let mut touched = false;

        if ui
            .add_enabled(has_selection, egui::Button::new("复制"))
            .clicked()
        {
            ui.ctx().copy_text(selected.clone());
            touched = true;
            ui.close();
        }
        if ui
            .add_enabled(has_selection, egui::Button::new("剪切"))
            .clicked()
        {
            ui.ctx().copy_text(selected.clone());
            if let Some(range) = range {
                let chars = range.as_sorted_char_range();
                replace_chars(text, chars.clone(), "");
                set_range(&mut state, chars.start, chars.start);
            }
            touched = true;
            ui.close();
        }
        if ui.button("粘贴").clicked() {
            if let Ok(clipboard) =
                arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text())
            {
                let caret = match range.map(|range| range.as_sorted_char_range()) {
                    Some(chars) => {
                        replace_chars(text, chars.clone(), &clipboard);
                        chars.start + clipboard.chars().count()
                    }
                    None => {
                        text.push_str(&clipboard);
                        text.chars().count()
                    }
                };
                set_range(&mut state, caret, caret);
            }
            touched = true;
            ui.close();
        }
        if ui.button("全选").clicked() {
            let end = text.chars().count();
            set_range(&mut state, 0, end);
            touched = true;
            ui.close();
        }

        if touched {
            if let Some(state) = state {
                egui::TextEdit::store_state(ui.ctx(), id, state);
            }
            // Without focus the next keystroke would go nowhere, and the field
            // would not show the change until it is focused.
            response.request_focus();
            ui.ctx().request_repaint();
        }
    });
}

/// Selects characters `start..end`.
fn set_range(
    state: &mut Option<egui::widgets::text_edit::TextEditState>,
    start: usize,
    end: usize,
) {
    let state = state.get_or_insert_with(Default::default);
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(start),
            egui::text::CCursor::new(end),
        )));
}

/// Replaces a character (not byte) range.
fn replace_chars(text: &mut String, chars: std::ops::Range<usize>, replacement: &str) {
    let start = byte_index(text, chars.start);
    let end = byte_index(text, chars.end);
    text.replace_range(start..end, replacement);
}

fn byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The menu works in character offsets while `String` splices bytes; CJK
    /// makes the two differ, so the conversion is what has to be right.
    #[test]
    fn replacing_a_character_range_keeps_utf8_intact() {
        let mut text = "ab中文cd".to_string();
        replace_chars(&mut text, 2..4, "X");
        assert_eq!(text, "abXcd");
        // Insert at the front, where every following byte offset has shifted.
        replace_chars(&mut text, 0..0, "你");
        assert_eq!(text, "你abXcd");
        assert_eq!(byte_index("你ab", 1), 3);
    }
}
