//! The editing context menu egui's text fields do not have.
//!
//! `TextEdit` handles the keyboard shortcuts (copy, cut, paste, select-all) but
//! offers no menu of its own, so every field gets one attached here.

use eframe::egui;

#[derive(Clone, Copy, Default)]
struct FocusState {
    open: bool,
    pending: bool,
}

/// Retains an opening focus request across egui's invisible sizing pass.
#[derive(Clone, Copy)]
pub(crate) struct InitialFocus(egui::Id);

pub(crate) fn window_focus(
    ctx: &egui::Context,
    name: impl std::hash::Hash,
    open: bool,
) -> InitialFocus {
    let id = egui::Id::new(("initial-window-focus", name));
    ctx.data_mut(|data| {
        let mut state = data.get_temp::<FocusState>(id).unwrap_or_default();
        if open && !state.open {
            state.pending = true;
        }
        if !open {
            state.pending = false;
        }
        state.open = open;
        data.insert_temp(id, state);
    });
    InitialFocus(id)
}

impl InitialFocus {
    pub(crate) fn request(self, ui: &egui::Ui, response: &egui::Response) {
        let mut state = ui
            .ctx()
            .data(|data| data.get_temp::<FocusState>(self.0).unwrap_or_default());
        focus_once(ui, response, &mut state.pending);
        ui.ctx().data_mut(|data| data.insert_temp(self.0, state));
    }
}

pub(crate) fn focus_once(ui: &egui::Ui, response: &egui::Response, pending: &mut bool) {
    if *pending && ui.is_visible() && response.enabled() {
        response.request_focus();
        *pending = false;
    }
}

fn ime_key() -> egui::Id {
    egui::Id::new("g-terminal-ime-composing")
}

/// Whether an IME candidate list was open when this frame began.
///
/// Enter is how a composition is committed, and egui only filters a few keys
/// while one is active — Enter is not among them. A form that saves on Enter
/// therefore has to check this, or picking a pinyin candidate also submits the
/// dialog. The answer is the state from the *start* of the frame: the commit and
/// the Enter arrive together, so what matters is whether a list was up before
/// they were handled.
pub(crate) fn ime_composing(ctx: &egui::Context) -> bool {
    let remembered = ctx.data(|data| data.get_temp::<bool>(ime_key()).unwrap_or(false));
    // The same frame can already carry the composition — a key can arrive with
    // it — so this frame's events count too, not just the remembered state.
    remembered
        || ctx.input(|input| {
            input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Ime(egui::ImeEvent::Enabled | egui::ImeEvent::Preedit(_))
                )
            })
        })
}

/// Removes the keys an open candidate list owns.
///
/// Enter would otherwise make a single-line field surrender focus in the middle
/// of a composition, which discards the characters being typed. Only the key
/// events go: the commit arrives as [`egui::Event::Ime`] and still reaches the
/// field.
pub(crate) fn swallow_ime_keys(events: &mut Vec<egui::Event>) {
    events.retain(|event| {
        !matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Enter,
                pressed: true,
                ..
            }
        )
    });
}

/// Refreshes [`ime_composing`] from the frame's events. Call once per frame,
/// after everything that reads it has run.
pub(crate) fn track_ime(ctx: &egui::Context) {
    let mut composing = ime_composing(ctx);
    ctx.input(|input| {
        for event in &input.events {
            match event {
                egui::Event::Ime(egui::ImeEvent::Enabled | egui::ImeEvent::Preedit(_)) => {
                    composing = true;
                }
                egui::Event::Ime(egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled) => {
                    composing = false;
                }
                _ => {}
            }
        }
    });
    ctx.data_mut(|data| data.insert_temp(ime_key(), composing));
}

/// A single-line field with the standard editing menu attached. `customize`
/// adjusts the builder — width, hint text and so on — before it is added.
pub(crate) fn field_with(
    ui: &mut egui::Ui,
    text: &mut String,
    customize: impl FnOnce(egui::TextEdit<'_>) -> egui::TextEdit<'_>,
) -> egui::Response {
    let edit = egui::TextEdit::singleline(text);
    let edit = if ui.spacing().interact_size.y >= 28.0 {
        edit.margin(egui::Margin::symmetric(8, 5))
    } else {
        edit
    };
    let response = ui.add(customize(edit));
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
    let edit = egui::TextEdit::singleline(text);
    let edit = if ui.spacing().interact_size.y >= 28.0 {
        edit.margin(egui::Margin::symmetric(8, 5))
    } else {
        edit
    };
    let response = ui.add_enabled(enabled, customize(edit));
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

    #[test]
    fn opening_focus_survives_sizing_allows_switching_and_resets_on_reopen() {
        let ctx = egui::Context::default();
        let mut open = true;
        let first = egui::Id::new("initial-focus-first");
        let second = egui::Id::new("initial-focus-second");
        let mut first_text = String::new();
        let mut second_text = String::new();
        let mut render = |open: &mut bool| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                let focus = window_focus(ctx, "focus-test", *open);
                egui::Window::new("Focus test").open(open).show(ctx, |ui| {
                    let response = ui.add(egui::TextEdit::singleline(&mut first_text).id(first));
                    focus.request(ui, &response);
                    ui.add(egui::TextEdit::singleline(&mut second_text).id(second));
                });
            });
        };
        for _ in 0..3 {
            render(&mut open);
        }
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first));
        ctx.memory_mut(|memory| memory.request_focus(second));
        for _ in 0..3 {
            render(&mut open);
        }
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(second));
        open = false;
        render(&mut open);
        open = true;
        for _ in 0..3 {
            render(&mut open);
        }
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first));
    }

    /// Enter belongs to the candidate list while it is open; the text of the
    /// composition must be left alone.
    #[test]
    fn an_open_candidate_list_takes_enter_but_not_the_text() {
        let mut events = vec![
            egui::Event::Ime(egui::ImeEvent::Preedit("lianjie".into())),
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text("a".into()),
        ];
        swallow_ime_keys(&mut events);
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(matches!(events[1], egui::Event::Text(_)));
    }

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
