// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! INFERCORE (LEDGER SR57): the inference UnaOS does itself — `no_std` + `alloc`, no
//! dependencies, no `unsafe`.
//!
//! - [`safetensors`] — the safetensors container (8-byte header length, JSON header, raw
//!   little-endian tensors), F32 / F16 / BF16 → f32 (other dtypes are listed, not converted).
//! - [`onnx`] — the initializers + node list of an ONNX `ModelProto` (a hand-written protobuf
//!   walk), and the `MatMul`→`Add(bias)` renaming that gives exported weights their BERT names.
//! - [`tokenizer`] — the BERT tokenizer: the `tokenizer.json` added (special) tokens matched in the
//!   raw text, the BERT normalizer (clean, CJK padding, NFD + Mn stripping, lowercase), the BERT
//!   pre-tokenizer (whitespace, punctuation) and greedy WordPiece — ids byte-equal to HF
//!   `tokenizers` (see [`unicode`] for why its Unicode data is 8.0 / 9.0).
//! - [`bert`] — the BERT encoder (embeddings, N post-LN layers of multi-head self-attention with
//!   exact per-sequence masking, erf GELU, LayerNorm), mean pooling + L2 normalisation as
//!   sentence-transformers does, over a packed batch; [`matmul`] is its cache-blocked f32 kernel
//!   (f32 or f16 weights).
//! - [`math`] — exp / erf / sqrt of our own, so results do not depend on the platform's libm.
//!
//! **Reproducibility.** Every reduction runs in a fixed, documented order (see [`matmul`] and
//! [`bert`]); Rust never contracts `a * b + c` into an FMA; the transcendental functions are ours.
//! The same inputs therefore give bit-identical outputs on every machine, and a sequence encoded
//! alone gives the same bits as the same sequence inside any batch.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

pub mod bert;
pub mod half;
pub mod json;
pub mod math;
pub mod matmul;
pub mod onnx;
pub mod safetensors;
pub mod tokenizer;
pub mod unicode;
mod unicode_tables;

use alloc::string::String;

/// Every failure: malformed input, a missing tensor, a shape that does not fit. Never a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;

/// `Err(Error(format!(...)))`.
#[macro_export]
macro_rules! bail {
    ($($t:tt)*) => { return Err($crate::Error(alloc::format!($($t)*))) };
}

pub(crate) fn err(s: impl Into<String>) -> Error {
    Error(s.into())
}
