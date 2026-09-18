//! Mouse gestures: what a click run means and the byte sequences the
//! application gets for them.

/// The message a copy or paste leaves behind.
pub(super) fn moved_message(verb: &str, text: &str) -> String {
    format!("已{verb} {} 个字符", text.chars().count())
}

/// The bytes a wheel notch sends to a full-screen program. A program that has
/// switched the cursor keys to application mode — `less` does, through terminfo's
/// `smkx` — expects `SS3 A/B`; sending `CSI A/B` there scrolls nothing at all.
pub(super) fn alternate_scroll_key(lines: i32, application_cursor: bool) -> &'static [u8] {
    match (lines > 0, application_cursor) {
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1b[A",
        (false, true) => b"\x1bOB",
        (false, false) => b"\x1b[B",
    }
}

/// What a run of consecutive clicks on the same cell asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Gesture {
    /// One click: place the caret, i.e. drop any selection.
    Point,
    Word,
    Line,
    Screen,
}

/// The gesture a run of `clicks` clicks adds up to. Anything past four stays at
/// "everything", so leaning on the button does not wrap round to something else.
pub(super) fn click_gesture(clicks: u8) -> Gesture {
    match clicks {
        0 | 1 => Gesture::Point,
        2 => Gesture::Word,
        3 => Gesture::Line,
        _ => Gesture::Screen,
    }
}

/// Whether a click continues the previous run: the same cell, soon enough after.
pub(super) fn continues_run(
    previous: (u16, u16),
    cell: (u16, u16),
    elapsed: f64,
    delay: f64,
) -> bool {
    previous == cell && elapsed >= 0.0 && elapsed <= delay
}
