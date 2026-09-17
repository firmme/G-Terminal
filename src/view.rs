use crate::theme::{Palette, ansi_color};
use eframe::egui::{
    self, Align2, Color32, Event, FontId, ImeEvent, Key, Pos2, Rect, Sense, Stroke, StrokeKind,
    Vec2,
};
use g_terminal::{
    config::{search_engine_label, search_url},
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
    /// Consecutive clicks on one cell, and when the last landed. egui's own count
    /// stops at three, so a fourth click — the "select everything" gesture — has
    /// to be counted here.
    clicks: u8,
    click_time: f64,
    click_cell: (u16, u16),
    /// What the last copy or paste moved, handed to the app for the status bar.
    pub notice: Option<String>,
}

/// The message a copy or paste leaves behind.
fn moved_message(verb: &str, text: &str) -> String {
    format!("已{verb} {} 个字符", text.chars().count())
}

/// The bytes a wheel notch sends to a full-screen program. A program that has
/// switched the cursor keys to application mode — `less` does, through terminfo's
/// `smkx` — expects `SS3 A/B`; sending `CSI A/B` there scrolls nothing at all.
fn alternate_scroll_key(lines: i32, application_cursor: bool) -> &'static [u8] {
    match (lines > 0, application_cursor) {
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1b[A",
        (false, true) => b"\x1bOB",
        (false, false) => b"\x1b[B",
    }
}

/// What a run of consecutive clicks on the same cell asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Gesture {
    /// One click: place the caret, i.e. drop any selection.
    Point,
    Word,
    Line,
    Screen,
}

/// The gesture a run of `clicks` clicks adds up to. Anything past four stays at
/// "everything", so leaning on the button does not wrap round to something else.
fn click_gesture(clicks: u8) -> Gesture {
    match clicks {
        0 | 1 => Gesture::Point,
        2 => Gesture::Word,
        3 => Gesture::Line,
        _ => Gesture::Screen,
    }
}

/// Whether a click continues the previous run: the same cell, soon enough after.
fn continues_run(previous: (u16, u16), cell: (u16, u16), elapsed: f64, delay: f64) -> bool {
    previous == cell && elapsed >= 0.0 && elapsed <= delay
}

pub struct ViewOptions<'a> {
    pub active: bool,
    pub keyboard_enabled: bool,
    pub size: f32,
    pub palette: Palette,
    pub query: &'a str,
    pub copy_on_select: bool,
    /// Which search engine the selection lookup opens.
    pub search_engine: &'a str,
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
            clicks: 0,
            click_time: 0.0,
            click_cell: (0, 0),
            notice: None,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, options: ViewOptions<'_>) -> (bool, Option<String>) {
        let ViewOptions {
            active,
            keyboard_enabled,
            size,
            palette,
            query,
            copy_on_select,
            search_engine,
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
        self.session.resize(rows, cols);
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
        let mut notice = None;
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
                } else if terminal.parser.screen().alternate_screen() {
                    // A full-screen program owns the screen and usually has no
                    // scrollback to move, so the wheel is handed to it as arrow
                    // keys — xterm's "alternate scroll", which is what makes
                    // `less` and friends follow the wheel.
                    let key =
                        alternate_scroll_key(lines, terminal.parser.screen().application_cursor());
                    for _ in 0..lines.unsigned_abs().min(32) {
                        outgoing.push(key.to_vec());
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
            // A drag is not part of a run of clicks.
            self.clicks = 0;
        } else if response.clicked() {
            if mouse {
                // The application owns the pointer; only drop any selection.
                self.selection = None;
            } else if let Some(pos) = response.interact_pointer_pos() {
                let cell = pointer_cell(pos);
                let now = ui.input(|i| i.time);
                // Taken from the options rather than hard-coded, so this agrees
                // with the interval egui itself uses for double clicks.
                let delay = ui.ctx().options(|o| o.input_options.max_double_click_delay);
                self.clicks = if continues_run(self.click_cell, cell, now - self.click_time, delay)
                {
                    self.clicks.saturating_add(1)
                } else {
                    1
                };
                self.click_time = now;
                self.click_cell = cell;
                let (row, col) = cell;
                match click_gesture(self.clicks) {
                    Gesture::Point => self.selection = None,
                    Gesture::Word => {
                        let screen = terminal.parser.screen();
                        let width = cols.min(screen.size().1);
                        let is_word = |c: Option<&vt100::Cell>| {
                            c.is_some_and(|c| {
                                !c.is_wide_continuation()
                                    && c.contents()
                                        .chars()
                                        .next()
                                        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
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
                            // A word click on whitespace takes the whole line.
                            self.selection = Some(((row, 0), (row, cols - 1)));
                        }
                    }
                    Gesture::Line => {
                        self.selection = Some(((row, 0), (row, cols.saturating_sub(1))));
                    }
                    Gesture::Screen => {
                        self.selection =
                            Some(((0, 0), (rows.saturating_sub(1), cols.saturating_sub(1))));
                    }
                }
            }
        }
        if response.dragged()
            && !mouse
            && let (Some((start, _)), Some(pos)) = (self.selection, response.interact_pointer_pos())
        {
            self.selection = Some((start, pointer_cell(pos)));
        }
        if copy_on_select
            && !mouse
            && (response.drag_stopped() || (response.clicked() && self.clicks >= 2))
            && let Some((a, b)) = self.selection
        {
            let text = selection_text(terminal.parser.screen(), a, b);
            if !text.is_empty() {
                copied = Some(text);
            }
        }
        if response.clicked_by(egui::PointerButton::Middle) {
            match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                Ok(text) => {
                    notice = Some(moved_message("粘贴", &text));
                    outgoing.push(encode_paste(
                        &text,
                        terminal.parser.screen().bracketed_paste(),
                    ));
                }
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
                        // On Windows both Ctrl+C and Ctrl+Shift+C arrive as Copy,
                        // and the unshifted one is the terminal interrupt. On
                        // macOS only Cmd+C arrives here — Ctrl+C stays an
                        // ordinary key event and is sent as the interrupt — so
                        // Copy always means copy.
                        let copy = cfg!(target_os = "macos") || ui.input(|i| i.modifiers.shift);
                        if copy {
                            if let Some((a, b)) = self.selection {
                                copied = Some(selection_text(terminal.parser.screen(), a, b));
                            }
                        } else {
                            outgoing.push(vec![3]);
                        }
                    }
                    // On Windows Ctrl+X reaches the shell as the control byte.
                    // On macOS this is Cmd+X, which must not turn into Ctrl+X.
                    Event::Cut => {
                        if !cfg!(target_os = "macos") {
                            outgoing.push(vec![24]);
                        }
                    }
                    Event::Paste(text) => {
                        notice = Some(moved_message("粘贴", &text));
                        outgoing.push(encode_paste(
                            &text,
                            terminal.parser.screen().bracketed_paste(),
                        ));
                    }
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
                                Ok(text) => {
                                    notice = Some(moved_message("粘贴", &text));
                                    outgoing.push(encode_paste(
                                        &text,
                                        terminal.parser.screen().bracketed_paste(),
                                    ));
                                }
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
            notice = Some(moved_message("复制", &text));
            ui.ctx().copy_text(text);
        }
        let selection = self.selection;
        if !mouse {
            response.context_menu(|ui| {
                if ui
                    .button(format!(
                        "复制选中内容   {}",
                        crate::app::accel("Ctrl+Shift+C")
                    ))
                    .clicked()
                {
                    if let Some((a, b)) = selection {
                        let text = {
                            let terminal = self.session.terminal.lock().unwrap();
                            selection_text(terminal.parser.screen(), a, b)
                        };
                        notice = Some(moved_message("复制", &text));
                        ui.ctx().copy_text(text);
                    }
                    ui.close();
                }
                if ui.button("复制可见屏幕").clicked() {
                    notice = Some(moved_message("复制", &visible_text));
                    ui.ctx().copy_text(visible_text);
                    ui.close();
                }
                if ui
                    .button(format!("粘贴   {}", crate::app::accel("Ctrl+Shift+V")))
                    .clicked()
                {
                    match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                        Ok(text) => {
                            notice = Some(moved_message("粘贴", &text));
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
                // A lookup of the selection, in the configured engine. It is the
                // one action here that leaves the app, so it lives on its own.
                if ui
                    .add_enabled(
                        selection.is_some(),
                        egui::Button::new(format!(
                            "浏览器搜索选中内容（{}）",
                            search_engine_label(search_engine)
                        )),
                    )
                    .clicked()
                {
                    if let Some((a, b)) = selection {
                        let text = {
                            let terminal = self.session.terminal.lock().unwrap();
                            selection_text(terminal.parser.screen(), a, b)
                        };
                        let query = text.trim();
                        if query.is_empty() {
                            error = Some("没有选中可搜索的内容".into());
                        } else {
                            match crate::remote_ui::open_url(&search_url(search_engine, query)) {
                                Ok(()) => {
                                    notice = Some(format!(
                                        "已用 {} 搜索",
                                        search_engine_label(search_engine)
                                    ));
                                }
                                Err(e) => error = Some(e),
                            }
                        }
                    }
                    ui.close();
                }
                if ui.button("滚动到底部").clicked() {
                    self.session.terminal.lock().unwrap().bottom();
                    ui.close();
                }
                ui.separator();
                // Ways to tidy up: 清屏 keeps the scrollback, 清空 throws the
                // whole transcript away, and images are dropped on their own.
                if ui.button("清屏").clicked() {
                    self.session.terminal.lock().unwrap().clear_screen();
                    ui.close();
                }
                if ui.button("清空所有控制台输出").clicked() {
                    self.session.terminal.lock().unwrap().clear_all();
                    ui.close();
                }
                if ui.button("清除内联图片").clicked() {
                    self.session.terminal.lock().unwrap().graphics.clear();
                    self.graphic = None;
                    ui.close();
                }
            });
        }
        self.notice = notice;
        for bytes in outgoing {
            if let Err(e) = self.session.write(bytes) {
                error = Some(e.to_string());
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

    /// `less` turns on application cursor keys, and then only the SS3 spelling
    /// of the arrows moves it — the CSI one scrolls nothing.
    #[test]
    fn alternate_scroll_respects_the_application_cursor_mode() {
        assert_eq!(alternate_scroll_key(1, false), b"\x1b[A");
        assert_eq!(alternate_scroll_key(-1, false), b"\x1b[B");
        assert_eq!(alternate_scroll_key(1, true), b"\x1bOA");
        assert_eq!(alternate_scroll_key(-1, true), b"\x1bOB");
    }

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
            search_engine: "google",
        }
    }
    #[test]
    fn selection_copies_wide_cells_once_and_preserves_wrapping() {
        let mut p = vt100::Parser::new(3, 6, 0);
        p.process("你好abcdef".as_bytes());
        assert_eq!(selection_text(p.screen(), (0, 0), (1, 3)), "你好abcdef");
        assert_eq!(selection_text(p.screen(), (1, 3), (0, 0)), "你好abcdef");
    }

    /// One click places the caret, two take a word, three take the line, four take
    /// the screen — and leaning on the button stays at "everything" rather than
    /// wrapping round to something else.
    #[test]
    fn click_runs_map_to_the_expected_gesture() {
        assert_eq!(click_gesture(0), Gesture::Point);
        assert_eq!(click_gesture(1), Gesture::Point);
        assert_eq!(click_gesture(2), Gesture::Word);
        assert_eq!(click_gesture(3), Gesture::Line);
        assert_eq!(click_gesture(4), Gesture::Screen);
        assert_eq!(click_gesture(5), Gesture::Screen);
        assert_eq!(click_gesture(u8::MAX), Gesture::Screen);
    }

    /// A run only continues on the same cell, soon enough after the last click.
    /// Getting this wrong would turn two separate clicks into a word selection.
    #[test]
    fn a_click_run_needs_the_same_cell_and_a_short_gap() {
        let here = (4, 10);
        assert!(continues_run(here, here, 0.05, 0.3));
        assert!(continues_run(here, here, 0.3, 0.3), "the boundary counts");
        assert!(!continues_run(here, here, 0.31, 0.3), "too slow");
        assert!(!continues_run(here, (4, 11), 0.05, 0.3), "another cell");
        assert!(!continues_run(here, (5, 10), 0.05, 0.3), "another row");
        // A clock that went backwards must not merge two clicks either.
        assert!(!continues_run(here, here, -1.0, 0.3));
    }
}
