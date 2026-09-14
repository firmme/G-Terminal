use base64::Engine;
use std::sync::Arc;

pub enum Token {
    Text(Vec<u8>),
    Image(Vec<u8>),
}
#[derive(Default)]
pub struct OscScanner {
    escape: bool,
    osc: Option<Vec<u8>>,
    discard: bool,
}
impl OscScanner {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Token> {
        let mut tokens = vec![];
        let mut text = vec![];
        for &b in bytes {
            if let Some(osc) = &mut self.osc {
                let end = b == 7 || (b == b'\\' && osc.last() == Some(&27));
                if end {
                    let mut payload = self.osc.take().unwrap();
                    if payload.last() == Some(&27) {
                        payload.pop();
                    }
                    if !self.discard {
                        if payload.starts_with(b"1337;File=") {
                            if !text.is_empty() {
                                tokens.push(Token::Text(std::mem::take(&mut text)));
                            }
                            if let Some(colon) = payload.iter().position(|b| *b == b':')
                                && payload[..colon].windows(8).any(|w| w == b"inline=1")
                                && let Ok(data) = base64::engine::general_purpose::STANDARD
                                    .decode(&payload[colon + 1..])
                            {
                                tokens.push(Token::Image(data));
                            }
                        } else {
                            text.extend_from_slice(b"\x1b]");
                            text.extend(payload);
                            text.push(7);
                        }
                    }
                    self.discard = false;
                } else if !self.discard {
                    if osc.len() < 8 * 1024 * 1024 {
                        osc.push(b);
                    } else {
                        self.discard = true;
                        osc.clear();
                    }
                } else {
                    osc.clear();
                    if b == 27 {
                        osc.push(b);
                    }
                }
                continue;
            }
            if self.escape {
                self.escape = false;
                if b == b']' {
                    self.osc = Some(vec![]);
                    continue;
                }
                text.push(27);
            }
            if b == 27 {
                self.escape = true;
            } else {
                text.push(b);
            }
        }
        if !text.is_empty() {
            tokens.push(Token::Text(text));
        }
        tokens
    }
}

pub struct Graphic {
    pub id: u64,
    pub pixels: Arc<image::RgbaImage>,
    pub row: u16,
    pub col: u16,
    pub alternate: bool,
}
pub fn decode(bytes: &[u8]) -> Option<image::RgbaImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().ok().map(|i| i.to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn osc_split_packets_keep_titles_and_extract_images() {
        let mut scanner = OscScanner::default();
        let mut tokens = vec![];
        for b in b"a\x1b]2;title\x07b\x1b]1337;File=inline=1:aGk=\x1b\\c" {
            tokens.extend(scanner.feed(&[*b]));
        }
        let mut text = vec![];
        let mut pictures = vec![];
        for t in tokens {
            match t {
                Token::Text(v) => text.extend(v),
                Token::Image(v) => pictures.push(v),
            }
        }
        assert_eq!(text, b"a\x1b]2;title\x07bc");
        assert_eq!(pictures, vec![b"hi".to_vec()]);
    }
}
