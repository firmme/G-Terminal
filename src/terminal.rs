/// A bounded VT screen, independent of the window and transport.
#[derive(Default)]
pub struct TerminalCallbacks {
    pub title: String,
    pub replies: Vec<u8>,
}

impl vt100::Callbacks for TerminalCallbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title)
            .chars()
            .filter(|c| !c.is_control())
            .take(120)
            .collect();
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        if c == 'n' && i1.is_none() {
            match first {
                5 => self.replies.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let (row, col) = screen.cursor_position();
                    self.replies
                        .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
                }
                _ => {}
            }
        } else if c == 'c' && first == 0 && i1.is_none() {
            self.replies.extend_from_slice(b"\x1b[?1;2c");
        }
    }
}

pub struct Terminal {
    pub parser: vt100::Parser<TerminalCallbacks>,
    pub revision: u64,
    pub graphics: std::collections::VecDeque<crate::graphics::Graphic>,
    /// A ZMODEM offer from the server that nobody has answered yet. `Some(true)`
    /// means the server ran `rz` and is waiting to receive, i.e. an upload;
    /// `Some(false)` means it ran `sz` and is about to send.
    pub zmodem_offer: Option<bool>,
    scanner: crate::graphics::OscScanner,
    /// Tail of the last read, so a header split across reads is still seen.
    zmodem_probe: Vec<u8>,
    /// Scrollback capacity, kept so the screen can be rebuilt in place.
    scrollback: usize,
}

impl Terminal {
    pub fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(
                rows,
                cols,
                scrollback,
                TerminalCallbacks::default(),
            ),
            revision: 0,
            graphics: std::collections::VecDeque::new(),
            zmodem_offer: None,
            scanner: crate::graphics::OscScanner::default(),
            zmodem_probe: Vec::new(),
            scrollback,
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        // A bare ZRQINIT header means the server started rz/sz without a pager.
        self.probe_zmodem(bytes);
        for token in self.scanner.feed(bytes) {
            match token {
                crate::graphics::Token::Text(bytes) => {
                    if bytes.windows(4).any(|w| w == b"\x1b[2J") {
                        self.graphics.clear();
                    }
                    self.parser.process(&bytes);
                }
                crate::graphics::Token::Image(bytes) => {
                    if let Some(pixels) = crate::graphics::decode(&bytes) {
                        self.graphics.clear();
                        let (row, col) = self.parser.screen().cursor_position();
                        self.graphics.push_back(crate::graphics::Graphic {
                            id: self.revision,
                            pixels: std::sync::Arc::new(pixels),
                            row,
                            col,
                            alternate: self.parser.screen().alternate_screen(),
                        });
                    }
                }
            }
        }
        self.revision = self.revision.wrapping_add(1);
    }

    /// Flags a ZMODEM hex header nobody has answered yet. The frame type says
    /// which way the transfer goes: `B00` is ZRQINIT, sent by the side that is
    /// about to *send* (so `sz`, a download); `B01` is ZRINIT, sent by the side
    /// waiting to *receive* (so `rz`, an upload).
    ///
    /// Probing for ZRQINIT alone is why `rz` never worked: the offer was never
    /// raised for it, so lrzsz's binary header went straight into the screen
    /// parser. The probe keeps a short tail so a header split across two reads is
    /// still recognised, and skips reads carrying escape sequences, which belong
    /// to screen output.
    fn probe_zmodem(&mut self, bytes: &[u8]) {
        const ZRQINIT: &[u8] = b"**\x18B00";
        const ZRINIT: &[u8] = b"**\x18B01";
        const TAIL: usize = ZRQINIT.len() - 1;
        if bytes.windows(2).any(|w| w == b"\x1b[") {
            self.zmodem_probe.clear();
            return;
        }
        let searched = self.zmodem_probe.len() + bytes.len();
        if searched < ZRQINIT.len() {
            self.zmodem_probe.extend_from_slice(bytes);
            return;
        }
        let mut window = std::mem::take(&mut self.zmodem_probe);
        window.extend_from_slice(bytes);
        if window.windows(ZRQINIT.len()).any(|w| w == ZRQINIT) {
            self.zmodem_offer = Some(false);
        } else if window.windows(ZRINIT.len()).any(|w| w == ZRINIT) {
            self.zmodem_offer = Some(true);
        }
        let keep = window.len().saturating_sub(TAIL);
        self.zmodem_probe = window[keep..].to_vec();
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows.max(1), cols.max(1));
        self.revision = self.revision.wrapping_add(1);
    }

    /// Prepares a screen that outlived its session for the next one. The
    /// scrollback is kept, so output from before a reconnect stays readable
    /// above the new output; only the transient state (alternate screen, pen,
    /// view offset) is reset. Nothing is printed here: the caller announces
    /// what it is about to do with [`Self::separator`]/[`Self::note`].
    pub fn recycle(&mut self, rows: u16, cols: u16) {
        if self.parser.screen().alternate_screen() {
            self.parser.process(b"\x1b[?1049l");
        }
        self.parser.process(b"\x1b[0m\x1b[?25h");
        self.parser.screen_mut().set_size(rows.max(1), cols.max(1));
        self.parser.screen_mut().set_scrollback(0);
        self.zmodem_offer = None;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Blank lines that keep a new run of diagnostics apart from older output.
    pub fn separator(&mut self) {
        self.parser.process(b"\r\n\r\n");
        self.parser.screen_mut().set_scrollback(0);
        self.revision = self.revision.wrapping_add(1);
    }

    /// Writes one informational line, e.g. a connect notice, and returns the
    /// view to the bottom so it is actually seen.
    pub fn note(&mut self, line: &str) {
        self.parser
            .process(format!("\r\n\x1b[0m{line}\x1b[0m\r\n").as_bytes());
        self.parser.screen_mut().set_scrollback(0);
        self.revision = self.revision.wrapping_add(1);
    }

    /// Like [`Self::note`], but coloured so a failure reads as one.
    pub fn note_error(&mut self, line: &str) {
        self.parser
            .process(format!("\r\n\x1b[31m{line}\x1b[0m\r\n").as_bytes());
        self.parser.screen_mut().set_scrollback(0);
        self.revision = self.revision.wrapping_add(1);
    }

    /// Blanks the visible screen but keeps the scrollback.
    pub fn clear_screen(&mut self) {
        self.parser.process(b"\x1b[H\x1b[2J");
        self.revision = self.revision.wrapping_add(1);
    }

    /// Drops everything this screen has ever shown, scrollback included.
    pub fn clear_all(&mut self) {
        let (rows, cols) = self.parser.screen().size();
        self.parser = vt100::Parser::new_with_callbacks(
            rows.max(1),
            cols.max(1),
            self.scrollback,
            TerminalCallbacks::default(),
        );
        self.graphics.clear();
        self.zmodem_probe.clear();
        self.zmodem_offer = None;
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn scroll(&mut self, lines: i32) {
        let screen = self.parser.screen_mut();
        let offset = screen.scrollback();
        screen.set_scrollback(offset.saturating_add_signed(lines as isize));
    }

    pub fn bottom(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    pub fn find_all(&mut self, query: &str) -> Vec<(usize, u16)> {
        if query.is_empty() {
            return vec![];
        }
        let screen = self.parser.screen_mut();
        let saved = screen.scrollback();
        screen.set_scrollback(usize::MAX);
        let total = screen.scrollback();
        let (rows, cols) = screen.size();
        let mut hits = vec![];
        let mut logical = String::new();
        let mut anchor = (total, 0);
        let mut visited = 0;
        let mut offset = total;
        loop {
            screen.set_scrollback(offset);
            for row in 0..rows {
                let absolute = total - offset + row as usize;
                if absolute < visited {
                    continue;
                }
                visited = absolute + 1;
                if logical.is_empty() {
                    anchor = (offset.saturating_sub(row as usize), 0);
                }
                let text = screen.rows(0, cols).nth(row as usize).unwrap_or_default();
                logical.push_str(&text);
                if !screen.row_wrapped(row) {
                    for _ in logical.match_indices(query).take(10_000 - hits.len()) {
                        hits.push(anchor);
                    }
                    logical.clear();
                }
                if hits.len() >= 10_000 {
                    break;
                }
            }
            if offset == 0 || hits.len() >= 10_000 {
                break;
            }
            offset = offset.saturating_sub(rows as usize);
        }
        screen.set_scrollback(saved);
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_finds_scrollback_and_wrapped_lines_without_changing_view() {
        let mut t = Terminal::new(3, 12, 100);
        t.process(b"unique-old\r\nabcdefghijklmnop\r\n");
        for i in 0..15 {
            t.process(format!("line {i}\r\n").as_bytes());
        }
        t.scroll(2);
        let before = t.parser.screen().scrollback();
        let hits = t.find_all("unique-old");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].0 > 3);
        assert_eq!(t.parser.screen().scrollback(), before);
        assert_eq!(t.find_all("jklmnop").len(), 1);
    }

    #[test]
    fn parses_color_cursor_and_unicode_across_reads() {
        let mut t = Terminal::new(10, 40, 100);
        let text = "\x1b[31m你好\x1b[0m".as_bytes();
        for byte in text {
            t.process(&[*byte]);
        }
        assert_eq!(t.parser.screen().cell(0, 0).unwrap().contents(), "你");
        assert!(t.parser.screen().cell(0, 0).unwrap().is_wide());
        assert_eq!(
            t.parser.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
        t.process(b"\x1b[2;3HX");
        assert_eq!(t.parser.screen().cell(1, 2).unwrap().contents(), "X");
    }

    #[test]
    fn alternate_screen_restores_primary_and_scrollback_is_bounded() {
        let mut t = Terminal::new(3, 20, 8);
        t.process(b"primary\x1b[?1049h\x1b[Hother\x1b[?1049l");
        assert!(t.parser.screen().contents().contains("primary"));
        for _ in 0..100 {
            t.process(b"\r\nline");
        }
        t.scroll(1000);
        assert!(t.parser.screen().scrollback() <= 8);
        t.bottom();
        assert_eq!(t.parser.screen().scrollback(), 0);
        t.resize(30, 100);
        assert_eq!(t.parser.screen().size(), (30, 100));
    }

    #[test]
    fn recycle_keeps_scrollback_and_leaves_the_alternate_screen() {
        let mut t = Terminal::new(3, 20, 100);
        for i in 0..10 {
            t.process(format!("old {i}\r\n").as_bytes());
        }
        t.process(b"\x1b[?1049h\x1b[Halternate");
        assert!(t.parser.screen().alternate_screen());
        t.recycle(3, 20);
        assert!(!t.parser.screen().alternate_screen());
        assert_eq!(t.parser.screen().scrollback(), 0);
        // Separating the runs is the caller's business; recycle is silent.
        assert!(!t.parser.screen().contents().contains("alternate"));
        assert!(!t.find_all("old 1").is_empty());
    }

    #[test]
    fn clear_screen_keeps_history_while_clear_all_drops_it() {
        let mut t = Terminal::new(3, 20, 100);
        for i in 0..10 {
            t.process(format!("line {i}\r\n").as_bytes());
        }
        t.clear_screen();
        assert_eq!(t.parser.screen().scrollback(), 0);
        assert!(!t.find_all("line 5").is_empty(), "清屏 should keep history");
        t.clear_all();
        assert!(t.find_all("line 5").is_empty(), "清空 should drop history");
        assert!(!t.parser.screen().contents().contains("line"));
    }

    #[test]
    fn notes_are_written_to_the_screen_and_return_the_view_to_the_bottom() {
        let mut t = Terminal::new(3, 20, 100);
        for i in 0..10 {
            t.process(format!("line {i}\r\n").as_bytes());
        }
        t.scroll(5);
        t.note("connect to COM3");
        assert_eq!(t.parser.screen().scrollback(), 0);
        assert!(t.parser.screen().contents().contains("connect to COM3"));
        t.note_error("connect failed: busy");
        assert!(
            t.parser
                .screen()
                .contents()
                .contains("connect failed: busy")
        );
    }

    /// `sz` announces itself with ZRQINIT (`B00`): the far side is about to send.
    #[test]
    fn zmodem_download_offer_is_detected_even_when_split_across_reads() {
        let mut t = Terminal::new(5, 40, 10);
        t.process(b"$ sz backup.tar.gz\r\n**\x18B");
        assert_eq!(t.zmodem_offer, None);
        t.process(b"0000000000000\r\n\x11");
        assert_eq!(t.zmodem_offer, Some(false));
    }

    /// `rz` announces itself with ZRINIT (`B01`): the far side is waiting to
    /// receive, so the offer is an upload. Probing for ZRQINIT alone is why typing
    /// `rz` used to do nothing at all — the payload here is the one a real server
    /// sends.
    #[test]
    fn zmodem_upload_offer_is_detected_and_read_as_an_upload() {
        let mut t = Terminal::new(5, 40, 10);
        t.process(b"$ rz\r\nrz waiting to receive.**\x18B");
        assert_eq!(t.zmodem_offer, None);
        t.process(b"0100000023be50\r\n\x11");
        assert_eq!(t.zmodem_offer, Some(true));
    }

    #[test]
    fn screen_output_with_escapes_is_not_mistaken_for_a_zmodem_offer() {
        let mut t = Terminal::new(5, 40, 10);
        for header in [
            &b"\x1b[31mred\x1b[0m **\x18B00 more"[..],
            b"\x1b[31mred\x1b[0m **\x18B01 more",
        ] {
            t.process(header);
            assert_eq!(t.zmodem_offer, None);
        }
    }

    #[test]
    fn cursor_queries_are_answered_and_titles_are_sanitized() {
        let mut t = Terminal::new(20, 80, 100);
        t.process(b"\x1b[4;9H\x1b[6n\x1b[5n\x1b[c");
        assert_eq!(t.parser.callbacks().replies, b"\x1b[4;9R\x1b[0n\x1b[?1;2c");
        t.process(b"\x1b]2;project-shell\x07");
        assert_eq!(t.parser.callbacks().title, "project-shell");
    }
}
