// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Namespace and secret names. They become path components (`/home/<u>/.holocron/<ns>/<name>`), so the
//! alphabet is closed: no separator, no traversal, no hidden file, nothing a shell or a FAT 8.3 fallback
//! would reinterpret.

/// Longest namespace or name, in bytes.
pub const NAME_MAX: usize = 64;

/// `[A-Za-z0-9._-]{1,64}`, not starting with `.`, never containing `..`.
pub fn valid(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= NAME_MAX
        && b[0] != b'.'
        && !s.contains("..")
        && b.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || c == b'-')
}

#[cfg(test)]
mod tests {
    use super::valid;

    #[test]
    fn alphabet() {
        for ok in ["vein", "claude.api_key", "ssh", "id-ed25519", "A_b-9.z"] {
            assert!(valid(ok), "{ok}");
        }
        let long = "a".repeat(65);
        for bad in ["", ".ring", "a/b", "..", "a..b", "a b", "a\\b", "é", "a\0", long.as_str()] {
            assert!(!valid(bad), "{bad:?}");
        }
        assert!(valid(&"a".repeat(64)));
    }
}
