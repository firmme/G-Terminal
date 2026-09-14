use crate::theme::{Palette, ansi_color};
use eframe::egui::{
    self, Align2, Color32, Event, FontId, ImeEvent, Key, Pos2, Rect, Sense, Stroke, StrokeKind,
    Vec2,
};
use g_terminal::{
    input::{encode_key, encode_mouse, encode_paste},
    session::Session,
};
use std::time::Duration;

pub struct Pane {
    pub id: u64,
    pub session: Session,
    selection: Option<((u16, u16), (u16, u16))>,
    revision: u64,
    preedit: String,
    composing: bool,
    scroll_fraction: f32,
    last_mouse: Option<(u16, u16)>,
    graphic: Option<(u64, egui::TextureHandle)>,
}

pub struct ViewOptions<'a> {
    pub active: bool,
    pub keyboard_enabled: bool,
    pub size: f32,
    pub palette: Palette,
    pub query: &'a str,
    pub copy_on_select: bool,
}

impl Pane {
    pub fn new(id: u64, session: Session) -> Self {
        Self {
            id,
            session,
            selection: None,
            revision: 0,
            preedit: String::new(),
            composing: false,
            scroll_fraction: 0.0,
            last_mouse: None,
            graphic: None,
        }
    }

    pub fn selected_text(&self) -> String {
        let terminal = self.session.terminal.lock().unwrap();
        let screen = terminal.parser.screen();
        let Some((start, end)) = self.selection else {
            return String::new();
        };
        selection_text(screen, start, end)
    }

    pub fn show(&mut self, ui: &mut egui::Ui, options: ViewOptions<'_>) -> (bool, Option<String>) {
        let ViewOptions {
            active,
            keyboard_enabled,
            size,
            palette,
            query,
            copy_on_select,
        } = options;
        let mut error = None;
        let font = FontId::monospace(size);
        let cell = Vec2::new(
            ui.fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0),
            (size * 1.30).ceil(),
        );
        let (rect, response) = ui.allocate_exact_size(
            ui.available_size().max(Vec2::splat(1.0)),
            Sense::click_and_drag(),
        );
        if active && keyboard_enabled {
            response.request_focus();
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
        }
        let content = rect.shrink(4.0);
        let rows = (content.height() / cell.y).floor().clamp(1.0, 500.0) as u16;
        let cols = (content.width() / cell.x).floor().clamp(1.0, 1000.0) as u16;
        if let Err(e) = self.session.resize(rows, cols) {
            error = Some(e.to_string());
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, palette.bg);
        painter.rect_stroke(
            rect,
            0,
            Stroke::new(
                1.0_f32,
                if active {
                    palette.accent.gamma_multiply(0.55)
                } else {
                    palette.line
                },
            ),
            StrokeKind::Inside,
        );
        let pointer_cell = |pos: Pos2| -> (u16, u16) {
            (
                ((pos.y - content.min.y) / cell.y)
                    .floor()
                    .clamp(0.0, (rows - 1) as f32) as u16,
                ((pos.x - content.min.x) / cell.x)
                    .floor()
                    .clamp(0.0, (cols - 1) as f32) as u16,
            )
        };
        let mut outgoing = Vec::<Vec<u8>>::new();
        let mut copied = None;
        let mut terminal = self.session.terminal.lock().unwrap();
        let mouse_mode = terminal.parser.screen().mouse_protocol_mode();
        let mouse_encoding = terminal.parser.screen().mouse_protocol_encoding();
        let mouse =
            mouse_mode != vt100::MouseProtocolMode::None && !ui.input(|i| i.modifiers.shift);
        if self.revision != terminal.revision {
            self.selection = None;
            self.revision = terminal.revision;
        }
        if response.hovered() {
            let delta = ui.input(|i| i.smooth_scroll_delta.y);
            self.scroll_fraction += delta / cell.y;
            let lines = self.scroll_fraction.trunc() as i32;
            if lines != 0 {
                if mouse {
                    if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                        let (row, col) = pointer_cell(pos);
                        for _ in 0..lines.unsigned_abs().min(32) {
                            outgoing.push(encode_mouse(
                                if lines > 0 { 64 } else { 65 },
                                col,
                                row,
                                false,
                                ui.input(|i| i.modifiers),
                                mouse_encoding,
                            ));
                        }
                    }
                } else {
                    terminal.scroll(lines);
                }
                self.scroll_fraction -= lines as f32;
                self.selection = None;
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        if response.drag_started() && !mouse {
            if let Some(pos) = ui.input(|i| i.pointer.press_origin()) {
                let cell = pointer_cell(pos);
                self.selection = Some((cell, cell));
            }
        } else if response.clicked() {
            self.selection = None;
        }
        if response.dragged()
            && !mouse
            && let (Some((start, _)), Some(pos)) = (self.selection, response.interact_pointer_pos())
        {
            self.selection = Some((start, pointer_cell(pos)));
        }
        if response.double_clicked()
            && !mouse
            && let Some(pos) = response.interact_pointer_pos()
        {
            let (row, col) = pointer_cell(pos);
            let screen = terminal.parser.screen();
            let width = cols.min(screen.size().1);
            let is_word = |c: Option<&vt100::Cell>| {
                c.is_some_and(|c| {
                    !c.is_wide_continuation() && c.contents().chars().next().is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
                })
            };
            if is_word(screen.cell(row, col)) {
                let mut start = col;
                while start > 0 && is_word(screen.cell(row, start - 1)) {
                    start -= 1;
                }
                let mut end = col;
                while end + 1 < width && is_word(screen.cell(row, end + 1)) {
                    end += 1;
                }
                self.selection = Some(((row, start), (row, end)));
            } else {
                // Double-click on whitespace selects the whole line.
                self.selection = Some(((row, 0), (row, cols - 1)));
            }
        }
        if copy_on_select
            && !mouse
            && (response.drag_stopped() || response.double_clicked())
            && let Some((a, b)) = self.selection
        {
            let text = selection_text(terminal.parser.screen(), a, b);
            if !text.is_empty() {
                copied = Some(text);
            }
        }
        if response.clicked_by(egui::PointerButton::Middle) {
            match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                Ok(text) => outgoing.push(encode_paste(
                    &text,
                    terminal.parser.screen().bracketed_paste(),
                )),
                Err(e) => error = Some(e.to_string()),
            }
        }
        if mouse && response.hovered() {
            for event in ui.input(|i| i.events.clone()) {
                match event {
                    Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers,
                    } if button != egui::PointerButton::Middle => {
                        if pressed || mouse_mode != vt100::MouseProtocolMode::Press {
                            let (row, col) = pointer_cell(pos);
                            let code = if button == egui::PointerButton::Primary {
                                0
                            } else {
                                2
                            };
                            outgoing.push(encode_mouse(
                                code,
                                col,
                                row,
                                !pressed,
                                modifiers,
                                mouse_encoding,
                            ));
                        }
                    }
                    Event::PointerMoved(pos) => {
                        let at = pointer_cell(pos);
                        let primary = ui.input(|i| i.pointer.primary_down());
                        let secondary = ui.input(|i| i.pointer.secondary_down());
                        if self.last_mouse != Some(at)
                            && (mouse_mode == vt100::MouseProtocolMode::AnyMotion
                                || (mouse_mode == vt100::MouseProtocolMode::ButtonMotion
                                    && (primary || secondary)))
                        {
                            outgoing.push(encode_mouse(
                                32 + if primary {
                                    0
                                } else if secondary {
                                    2
                                } else {
                                    3
                                },
                                at.1,
                                at.0,
                                false,
                                ui.input(|i| i.modifiers),
                                mouse_encoding,
                            ));
                        }
                        self.last_mouse = Some(at);
                    }
                    _ => {}
                }
            }
        }

        if active && keyboard_enabled {
            for event in ui.input(|i| i.events.clone()) {
                match event {
                    Event::Copy => {
                        // winit converts both Ctrl+C and Ctrl+Shift+C to Copy.
                        if ui.input(|i| i.modifiers.shift) {
                            if let Some((a, b)) = self.selection {
                                copied = Some(selection_text(terminal.parser.screen(), a, b));
                            }
                        } else {
                            outgoing.push(vec![3]);
                        }
                    }
                    Event::Cut => outgoing.push(vec![24]),
                    Event::Paste(text) => outgoing.push(encode_paste(
                        &text,
                        terminal.parser.screen().bracketed_paste(),
                    )),
                    Event::Text(text) if !self.composing => {
                        let alt = ui.input(|i| i.modifiers.alt && !i.modifiers.ctrl);
                        let mut bytes = text.into_bytes();
                        if alt {
                            bytes.insert(0, 27);
                        }
                        outgoing.push(bytes);
                    }
                    Event::Ime(ImeEvent::Preedit(text)) => {
                        self.composing = true;
                        self.preedit = text;
                    }
                    Event::Ime(ImeEvent::Commit(text)) => {
                        self.composing = false;
                        self.preedit.clear();
                        outgoing.push(text.into_bytes());
                    }
                    Event::Ime(ImeEvent::Disabled) => {
                        self.composing = false;
                        self.preedit.clear();
                    }
                    Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } if !self.composing => {
                        if modifiers.ctrl && modifiers.shift && key == Key::C {
                            if let Some((a, b)) = self.selection {
                                copied = Some(selection_text(terminal.parser.screen(), a, b));
                            }
                        } else if modifiers.ctrl && modifiers.shift && key == Key::V {
                            match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                                Ok(text) => outgoing.push(encode_paste(
                                    &text,
                                    terminal.parser.screen().bracketed_paste(),
                                )),
                                Err(e) => error = Some(format!("无法读取剪贴板：{e}")),
                            }
                        } else if modifiers.shift && matches!(key, Key::PageUp | Key::PageDown) {
                            terminal.scroll(if key == Key::PageUp {
                                rows as i32
                            } else {
                                -(rows as i32)
                            });
                            self.selection = None;
                        } else if let Some(bytes) = encode_key(
                            key,
                            modifiers,
                            terminal.parser.screen().application_cursor(),
                        ) {
                            outgoing.push(bytes);
                        }
                    }
                    _ => {}
                }
            }
        }
        if !outgoing.is_empty() {
            terminal.bottom();
            self.selection = None;
        }
        let screen = terminal.parser.screen();
        let selection = self
            .selection
            .map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        let text_painter = painter.with_clip_rect(content);
        for row in 0..rows.min(screen.size().0) {
            let row_text = if query.is_empty() {
                String::new()
            } else {
                (0..cols.min(screen.size().1))
                    .filter_map(|col| screen.cell(row, col))
                    .filter(|c| !c.is_wide_continuation())
                    .map(|c| {
                        if c.contents().is_empty() {
                            " "
                        } else {
                            c.contents()
                        }
                    })
                    .collect::<String>()
            };
            let matches: Vec<_> = if query.is_empty() {
                Vec::new()
            } else {
                row_text
                    .match_indices(query)
                    .map(|(i, s)| i..i + s.len())
                    .collect()
            };
            let mut byte = 0;
            let mut shaped_until = 0;
            let mut shaped_runs = Vec::new();
            for col in 0..cols.min(screen.size().1) {
                let Some(c) = screen.cell(row, col) else {
                    continue;
                };
                if c.is_wide_continuation() {
                    continue;
                }
                let pos = content.min + Vec2::new(col as f32 * cell.x, row as f32 * cell.y);
                let wide = if c.is_wide() { 2.0 } else { 1.0 };
                let cell_rect = Rect::from_min_size(pos, Vec2::new(cell.x * wide, cell.y));
                let mut fg = ansi_color(c.fgcolor(), palette.text, c.bold());
                let mut bg = ansi_color(c.bgcolor(), palette.bg, false);
                if c.inverse() {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let text = c.contents();
                let length = text.len().max(1);
                if matches
                    .iter()
                    .any(|r| byte < r.end && byte + length > r.start)
                {
                    bg = Color32::from_rgb(120, 93, 29);
                    fg = Color32::WHITE;
                }
                byte += length;
                if selection
                    .is_some_and(|(a, b)| (row, col) <= b && (row, col + (wide as u16 - 1)) >= a)
                {
                    bg = palette.accent.gamma_multiply(0.3);
                }
                if bg != palette.bg {
                    text_painter.rect_filled(cell_rect, 0, bg);
                }
                if col >= shaped_until && crate::shaping::needs_shaping(text) {
                    let mut run = String::new();
                    let mut end = col;
                    while end < cols.min(screen.size().1) {
                        let item = screen.cell(row, end).unwrap();
                        if item.fgcolor() != c.fgcolor()
                            || item.inverse() != c.inverse()
                            || item.bold() != c.bold()
                        {
                            break;
                        }
                        if !item.is_wide_continuation() {
                            run.push_str(if item.contents().is_empty() {
                                " "
                            } else {
                                item.contents()
                            });
                        }
                        end += 1;
                    }
                    shaped_until = end;
                    shaped_runs.push((
                        run,
                        Rect::from_min_size(pos, Vec2::new((end - col) as f32 * cell.x, cell.y)),
                        fg,
                    ));
                } else if col >= shaped_until && !text.is_empty() && text != " " {
                    text_painter.text(
                        pos + Vec2::new(0.0, (cell.y - size) * 0.35),
                        Align2::LEFT_TOP,
                        text,
                        font.clone(),
                        fg,
                    );
                }
                if c.underline() {
                    text_painter.line_segment(
                        [
                            cell_rect.left_bottom() - Vec2::new(0.0, 2.0),
                            cell_rect.right_bottom() - Vec2::new(0.0, 2.0),
                        ],
                        Stroke::new(1.0_f32, fg),
                    );
                }
            }
            for (text, rect, fg) in shaped_runs {
                crate::shaping::draw(&text_painter, text.trim_end(), rect, size, fg);
            }
        }
        let (row, col) = screen.cursor_position();
        if screen.scrollback() == 0
            && let Some(g) = terminal
                .graphics
                .back()
                .filter(|g| g.alternate == screen.alternate_screen())
        {
            if self.graphic.as_ref().is_none_or(|(id, _)| *id != g.id) {
                self.graphic = Some((
                    g.id,
                    ui.ctx().load_texture(
                        "terminal-image",
                        egui::ColorImage::from_rgba_unmultiplied(
                            [g.pixels.width() as usize, g.pixels.height() as usize],
                            g.pixels.as_raw(),
                        ),
                        egui::TextureOptions::LINEAR,
                    ),
                ));
            }
            if let Some((_, texture)) = &self.graphic {
                let pos = content.min + Vec2::new(g.col as f32 * cell.x, g.row as f32 * cell.y);
                let max = (content.right() - pos.x).max(1.0);
                let scale = (max / g.pixels.width() as f32).min(1.0);
                text_painter.image(
                    texture.id(),
                    Rect::from_min_size(
                        pos,
                        Vec2::new(
                            g.pixels.width() as f32 * scale,
                            g.pixels.height() as f32 * scale,
                        ),
                    ),
                    Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
        let cursor = Rect::from_min_size(
            content.min + Vec2::new(col as f32 * cell.x, row as f32 * cell.y),
            cell,
        );
        if active && keyboard_enabled {
            ui.ctx().output_mut(|o| {
                o.ime = Some(egui::output::IMEOutput {
                    rect: content,
                    cursor_rect: cursor,
                });
                o.mutable_text_under_cursor = true;
            });
        }
        if !screen.hide_cursor() && screen.scrollback() == 0 && content.intersects(cursor) {
            if active {
                if ui.input(|i| ((i.time * 2.0) as u64).is_multiple_of(2)) {
                    text_painter.rect_filled(
                        Rect::from_min_size(cursor.min, Vec2::new(2.0, cell.y)),
                        0,
                        palette.accent,
                    );
                }
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            } else {
                text_painter.rect_stroke(
                    cursor,
                    0,
                    Stroke::new(1.0_f32, palette.muted),
                    StrokeKind::Inside,
                );
            }
        }
        if !self.preedit.is_empty() && active {
            let galley = text_painter.layout_no_wrap(self.preedit.clone(), font, palette.text);
            text_painter.rect_filled(
                Rect::from_min_size(cursor.min, galley.size() + Vec2::new(4.0, 4.0)),
                2,
                palette.raised,
            );
            text_painter.galley(cursor.min, galley, palette.text);
        }
        if screen.scrollback() > 0 {
            text_painter.text(
                content.right_bottom(),
                Align2::RIGHT_BOTTOM,
                format!("↑ {} lines · 输入返回底部", screen.scrollback()),
                FontId::proportional(12.0),
                palette.accent,
            );
        }
        let visible_text = screen.contents();
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Label,
                true,
                format!("终端 {}\n{}", self.session.kind.label(), visible_text),
            )
        });
        drop(terminal);
        if let Some(text) = copied {
            ui.ctx().copy_text(text);
        }
        if !mouse {
            response.context_menu(|ui| {
                if ui.button("复制选中内容   Ctrl+Shift+C").clicked() {
                    ui.ctx().copy_text(self.selected_text());
                    ui.close();
                }
                if ui.button("复制可见屏幕").clicked() {
                    ui.ctx().copy_text(visible_text);
                    ui.close();
                }
                if ui.button("粘贴   Ctrl+Shift+V").clicked() {
                    match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                        Ok(text) => {
                            let bracketed = self
                                .session
                                .terminal
                                .lock()
                                .unwrap()
                                .parser
                                .screen()
                                .bracketed_paste();
                            outgoing.push(encode_paste(&text, bracketed));
                        }
                        Err(e) => error = Some(e.to_string()),
                    }
                    ui.close();
                }
                if ui.button("滚动到底部").clicked() {
                    self.session.terminal.lock().unwrap().bottom();
                    ui.close();
                }
            });
        }
        for bytes in outgoing {
            if let Err(e) = self.session.write(bytes) {
                error = Some(e.to_string());
            }
            if ui.button("清除内联图片").clicked() {
                self.session.terminal.lock().unwrap().graphics.clear();
                self.graphic = None;
                ui.close();
            }
        }
        (
            response.clicked()
                || response.drag_started()
                || response.secondary_clicked()
                || response.clicked_by(egui::PointerButton::Middle),
            error,
        )
    }
}

fn selection_text(screen: &vt100::Screen, a: (u16, u16), b: (u16, u16)) -> String {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };
    let mut result = String::new();
    for row in a.0..=b.0.min(screen.size().0 - 1) {
        let start = if row == a.0 { a.1 } else { 0 };
        let end = if row == b.0 { b.1 } else { screen.size().1 - 1 };
        let mut line = String::new();
        for col in start..=end.min(screen.size().1 - 1) {
            if let Some(c) = screen.cell(row, col)
                && !c.is_wide_continuation()
            {
                let text = c.contents();
                line.push_str(if text.is_empty() { " " } else { text });
            }
        }
        result.push_str(line.trim_end());
        if row < b.0 && !screen.row_wrapped(row) {
            result.push('\n');
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_selection_can_copy_automatically_without_touching_system_clipboard() {
        let ctx = egui::Context::default();
        let session =
            Session::disconnected(g_terminal::session::SessionKind::Local("cmd".into()), 100);
        session.terminal.lock().unwrap().process(b"hello world");
        let mut pane = Pane::new(900, session);
        let mut copy = String::new();
        let frames = vec![
            vec![],
            vec![
                Event::PointerMoved(egui::pos2(16.0, 20.0)),
                Event::PointerButton {
                    pos: egui::pos2(16.0, 20.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            vec![Event::PointerMoved(egui::pos2(75.0, 20.0))],
            vec![Event::PointerButton {
                pos: egui::pos2(75.0, 20.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        ];
        for events in frames {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 300.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        pane.show(ui, true_options());
                    });
                },
            );
            for command in output.platform_output.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    copy = text;
                }
            }
        }
        assert!(!copy.is_empty());
        assert!("hello world".contains(&copy));
    }
    fn true_options() -> ViewOptions<'static> {
        ViewOptions {
            active: true,
            keyboard_enabled: true,
            size: 15.0,
            palette: Palette::new(false),
            query: "",
            copy_on_select: true,
        }
    }
    #[test]
    fn selection_copies_wide_cells_once_and_preserves_wrapping() {
        let mut p = vt100::Parser::new(3, 6, 0);
        p.process("你好abcdef".as_bytes());
        assert_eq!(selection_text(p.screen(), (0, 0), (1, 3)), "你好abcdef");
        assert_eq!(selection_text(p.screen(), (1, 3), (0, 0)), "你好abcdef");
    }
}
