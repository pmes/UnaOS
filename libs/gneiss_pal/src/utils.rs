// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

/// Calculates the byte index where a string should be truncated based on a maximum
/// line count and character length limit.
/// Returns `Some(byte_index)` if truncation is needed, or `None` if the string
/// fits within the constraints.
pub fn calculate_truncation(content: &str, max_lines: usize, max_chars: usize) -> Option<usize> {
    let mut line_count = 0;

    for (idx, c) in content.char_indices() {
        if c == '\n' {
            line_count += 1;
        }

        if line_count >= max_lines || idx >= max_chars {
            return Some(idx);
        }
    }
    None
}
