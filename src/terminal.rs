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
    /// Pending ZMODEM download offer (`rz` was run on the server).
    pub zmodem_offer: bool,
    scanner: crate::graphics::OscScanner,
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
            zmodem_offer: false,
            scanner: crate::graphics::OscScanner::default(),
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        // A bare ZMODEM header means the server started rz/sz without a pager.
        if bytes.windows(6).any(|w| w == b"**\x18B00")
            && !bytes.windows(2).any(|w| w == b"\x1b[")
        {
            self.zmodem_offer = true;
        }
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

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows.max(1), cols.max(1));
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
    fn cursor_queries_are_answered_and_titles_are_sanitized() {
        let mut t = Terminal::new(20, 80, 100);
        t.process(b"\x1b[4;9H\x1b[6n\x1b[5n\x1b[c");
        assert_eq!(t.parser.callbacks().replies, b"\x1b[4;9R\x1b[0n\x1b[?1;2c");
        t.process(b"\x1b]2;project-shell\x07");
        assert_eq!(t.parser.callbacks().title, "project-shell");
    }
}
