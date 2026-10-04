// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The offline provider. With no key (the FAT card, a fresh install) Lumen still answers: `echo: ` + the
//! prompt reversed by characters, streamed as text pieces exactly like a model's deltas, so the window,
//! the transcript and the streaming path are exercised with no network. (The Relay provider and the chat
//! bus codec of VEINCORE are retired: R82, LUMENAPP B323.)

pub const ECHO_PREFIX: &str = "echo: ";
pub const ECHO_MODEL: &str = "reverse";

/// Stream the Echo answer to `text` through `piece`, at most `chunk` bytes a piece, never splitting a
/// character. Returns the bytes streamed.
pub fn echo(text: &str, chunk: usize, piece: &mut dyn FnMut(&str)) -> usize {
    let mut buf = [0u8; 64];
    let cap = chunk.clamp(4, buf.len());
    let mut n = 0;
    let mut total = 0;
    let emit = |b: &[u8], piece: &mut dyn FnMut(&str)| {
        if let Ok(s) = core::str::from_utf8(b) {
            piece(s);
        }
    };
    let mut u = [0u8; 4];
    for ch in ECHO_PREFIX.chars().chain(text.chars().rev()) {
        let e = ch.encode_utf8(&mut u).as_bytes();
        if n + e.len() > cap {
            emit(&buf[..n], piece);
            n = 0;
        }
        buf[n..n + e.len()].copy_from_slice(e);
        n += e.len();
        total += e.len();
    }
    if n > 0 {
        emit(&buf[..n], piece);
    }
    total
}

/// The whole Echo answer, as a string (host tests).
#[cfg(feature = "alloc")]
pub fn echo_answer(text: &str) -> alloc::string::String {
    let mut s = alloc::string::String::from(ECHO_PREFIX);
    s.extend(text.chars().rev());
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::string::String;

    #[test]
    fn echo_streams_whole_chars_in_order() {
        let text = "héllo wörld ✓ abc";
        let mut got = String::new();
        let mut pieces = 0;
        let n = echo(text, 8, &mut |p| {
            assert!(p.len() <= 8);
            pieces += 1;
            got.push_str(p);
        });
        assert_eq!(got, echo_answer(text));
        assert_eq!(n, got.len());
        assert!(pieces >= 3);
    }
}
