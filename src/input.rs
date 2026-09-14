use eframe::egui::{Key, Modifiers};

pub fn encode_mouse(
    button: u8,
    col: u16,
    row: u16,
    release: bool,
    mods: Modifiers,
    encoding: vt100::MouseProtocolEncoding,
) -> Vec<u8> {
    let flags = 4 * u8::from(mods.shift) + 8 * u8::from(mods.alt) + 16 * u8::from(mods.ctrl);
    if encoding == vt100::MouseProtocolEncoding::Sgr {
        return format!(
            "\x1b[<{};{};{}{}",
            button + flags,
            col as u32 + 1,
            row as u32 + 1,
            if release { 'm' } else { 'M' }
        )
        .into_bytes();
    }
    let mut out = b"\x1b[M".to_vec();
    out.push(32 + if release { 3 + flags } else { button + flags });
    for coord in [col, row] {
        if encoding == vt100::MouseProtocolEncoding::Utf8 {
            let c = char::from_u32((coord as u32 + 33).min(2047)).unwrap();
            let mut b = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
        } else {
            out.push((coord.min(222) + 33) as u8);
        }
    }
    out
}

pub fn encode_key(key: Key, mods: Modifiers, application_cursor: bool) -> Option<Vec<u8>> {
    if mods.ctrl && !mods.alt {
        let control = match key {
            Key::A => 1,
            Key::B => 2,
            Key::C => 3,
            Key::D => 4,
            Key::E => 5,
            Key::F => 6,
            Key::G => 7,
            Key::H => 8,
            Key::I => 9,
            Key::J => 10,
            Key::K => 11,
            Key::L => 12,
            Key::M => 13,
            Key::N => 14,
            Key::O => 15,
            Key::P => 16,
            Key::Q => 17,
            Key::R => 18,
            Key::S => 19,
            Key::T => 20,
            Key::U => 21,
            Key::V => 22,
            Key::W => 23,
            Key::X => 24,
            Key::Y => 25,
            Key::Z => 26,
            Key::OpenBracket => 27,
            Key::Backslash => 28,
            Key::CloseBracket => 29,
            Key::Space => 0,
            _ => 255,
        };
        if control != 255 {
            return Some(vec![control]);
        }
    }
    let modifier = 1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl);
    let arrow = match key {
        Key::ArrowUp => Some('A'),
        Key::ArrowDown => Some('B'),
        Key::ArrowRight => Some('C'),
        Key::ArrowLeft => Some('D'),
        Key::Home => Some('H'),
        Key::End => Some('F'),
        _ => None,
    };
    if let Some(end) = arrow {
        return Some(
            if modifier > 1 {
                format!("\x1b[1;{modifier}{end}")
            } else if application_cursor {
                format!("\x1bO{end}")
            } else {
                format!("\x1b[{end}")
            }
            .into_bytes(),
        );
    }
    let tilde = match key {
        Key::Insert => Some(2),
        Key::Delete => Some(3),
        Key::PageUp => Some(5),
        Key::PageDown => Some(6),
        Key::F5 => Some(15),
        Key::F6 => Some(17),
        Key::F7 => Some(18),
        Key::F8 => Some(19),
        Key::F9 => Some(20),
        Key::F10 => Some(21),
        Key::F11 => Some(23),
        Key::F12 => Some(24),
        _ => None,
    };
    if let Some(n) = tilde {
        return Some(
            if modifier > 1 {
                format!("\x1b[{n};{modifier}~")
            } else {
                format!("\x1b[{n}~")
            }
            .into_bytes(),
        );
    }
    let sequence = match key {
        Key::Enter => "\r",
        Key::Backspace => "\x7f",
        Key::Escape => "\x1b",
        Key::Tab => {
            if mods.shift {
                "\x1b[Z"
            } else {
                "\t"
            }
        }
        Key::F1 => "\x1bOP",
        Key::F2 => "\x1bOQ",
        Key::F3 => "\x1bOR",
        Key::F4 => "\x1bOS",
        _ => return None,
    };
    let mut bytes = Vec::new();
    if mods.alt {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(sequence.as_bytes());
    Some(bytes)
}

pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    // Pasted ESC cannot escape bracketed-paste mode and become a terminal command.
    let text = text
        .replace('\x1b', "")
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_mouse_reports_coordinates_modifiers_and_release() {
        assert_eq!(
            encode_mouse(
                0,
                9,
                4,
                false,
                Modifiers::CTRL,
                vt100::MouseProtocolEncoding::Sgr
            ),
            b"\x1b[<16;10;5M"
        );
        assert_eq!(
            encode_mouse(
                0,
                9,
                4,
                true,
                Modifiers::NONE,
                vt100::MouseProtocolEncoding::Sgr
            ),
            b"\x1b[<0;10;5m"
        );
        assert_eq!(
            encode_mouse(
                64,
                0,
                0,
                false,
                Modifiers::NONE,
                vt100::MouseProtocolEncoding::Default
            ),
            vec![27, b'[', b'M', 96, 33, 33]
        );
    }
    #[test]
    fn keys_preserve_shell_controls_and_cursor_modes() {
        assert_eq!(encode_key(Key::C, Modifiers::CTRL, false), Some(vec![3]));
        assert_eq!(
            encode_key(Key::ArrowUp, Modifiers::NONE, true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode_key(Key::ArrowLeft, Modifiers::CTRL, false),
            Some(b"\x1b[1;5D".to_vec())
        );
    }
    #[test]
    fn paste_normalizes_line_endings_and_removes_escape() {
        assert_eq!(encode_paste("a\r\nb", false), b"a\rb");
        assert_eq!(
            encode_paste("a\x1b[201~", true),
            b"\x1b[200~a[201~\x1b[201~"
        );
    }
}
