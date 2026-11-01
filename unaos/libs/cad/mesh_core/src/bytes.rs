//! Byte and text cursors for the readers: the `Option` readers in pixel_core's shape (facts.rs `le32(b, i)` via
//! `b.get(i)?`, cited, not linked) so a short buffer is a `None`, never a panic, and a whitespace token cursor that
//! keeps the line number for the text formats' named refusals.

/// `N` bytes at `i` in little-endian order (reversed when `big`), or `None` past the end.
pub(crate) fn take<const N: usize>(b: &[u8], i: usize, big: bool) -> Option<[u8; N]> {
    let s = b.get(i..i.checked_add(N)?)?;
    let mut out = [0u8; N];
    out.copy_from_slice(s);
    if big {
        out.reverse();
    }
    Some(out)
}

pub(crate) fn le_u16(b: &[u8], i: usize) -> Option<u16> {
    take::<2>(b, i, false).map(u16::from_le_bytes)
}
pub(crate) fn le_u32(b: &[u8], i: usize) -> Option<u32> {
    take::<4>(b, i, false).map(u32::from_le_bytes)
}
pub(crate) fn le_f32(b: &[u8], i: usize) -> Option<f32> {
    le_u32(b, i).map(f32::from_bits)
}

/// Whitespace-separated tokens with the 1-based line each starts on.
pub(crate) struct Tokens<'a> {
    s: &'a str,
    pos: usize,
    line: u32,
}

impl<'a> Tokens<'a> {
    pub(crate) fn new(s: &'a str, first_line: u32) -> Self {
        Tokens { s, pos: 0, line: first_line }
    }
    /// The line the cursor is on (the last token's line after `next`).
    pub(crate) fn line(&self) -> u32 {
        self.line
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = (&'a str, u32);
    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.s.as_bytes();
        while self.pos < bytes.len() && bytes[self.pos].is_ascii_whitespace() {
            if bytes[self.pos] == b'\n' {
                self.line += 1;
            }
            self.pos += 1;
        }
        if self.pos >= bytes.len() {
            return None;
        }
        let start = self.pos;
        while self.pos < bytes.len() && !bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
        Some((&self.s[start..self.pos], self.line))
    }
}
