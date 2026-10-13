// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The grammars, each written from its language's specification (the module docs cite the clauses).

pub mod c;
pub mod markdown;
pub mod rust;
pub mod shell;
pub mod toml;

/// Is the byte slice `w` one of `list`?
pub(crate) fn one_of(w: &[u8], list: &[&str]) -> bool {
    list.iter().any(|k| k.as_bytes() == w)
}
