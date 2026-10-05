// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The safetensors container, from its specification (huggingface/safetensors, "Format"):
//!
//! ```text
//! [ N: u64 little-endian ][ N bytes: UTF-8 JSON header ][ byte buffer ]
//! header = { "<name>": { "dtype": "F32", "shape": [d0, d1, ...], "data_offsets": [begin, end] },
//!            ..., "__metadata__": { "<key>": "<string>" } }   (metadata optional)
//! ```
//!
//! Offsets are relative to the start of the byte buffer; tensors are row-major, little-endian.
//! Validated as the reference loader does: the header starts with `{`, is at most 100 MB, every
//! tensor's byte length equals its element count × dtype size, and the tensors, sorted by offset,
//! tile the buffer exactly (no gap, no overlap, nothing left over). Any dtype is listed;
//! F32 / F16 / BF16 convert to f32 ([`TensorView::to_f32`]).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::half::{bf16_to_f32, f16_to_f32};
use crate::json::{self, Value};
use crate::{Result, err};

/// The reference loader's header cap.
pub const MAX_HEADER: usize = 100_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dtype {
    Bool,
    U8,
    I8,
    I16,
    U16,
    F16,
    BF16,
    I32,
    U32,
    F32,
    F64,
    I64,
    U64,
    /// The float8 variants (`F8_E4M3`, `F8_E5M2`): one byte, listed only.
    F8,
}

impl Dtype {
    fn parse(s: &str) -> Option<Dtype> {
        Some(match s {
            "BOOL" => Dtype::Bool,
            "U8" => Dtype::U8,
            "I8" => Dtype::I8,
            "I16" => Dtype::I16,
            "U16" => Dtype::U16,
            "F16" => Dtype::F16,
            "BF16" => Dtype::BF16,
            "I32" => Dtype::I32,
            "U32" => Dtype::U32,
            "F32" => Dtype::F32,
            "F64" => Dtype::F64,
            "I64" => Dtype::I64,
            "U64" => Dtype::U64,
            "F8_E4M3" | "F8_E5M2" => Dtype::F8,
            _ => return None,
        })
    }

    pub fn size(self) -> usize {
        match self {
            Dtype::Bool | Dtype::U8 | Dtype::I8 | Dtype::F8 => 1,
            Dtype::I16 | Dtype::U16 | Dtype::F16 | Dtype::BF16 => 2,
            Dtype::I32 | Dtype::U32 | Dtype::F32 => 4,
            Dtype::F64 | Dtype::I64 | Dtype::U64 => 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TensorView<'a> {
    pub name: String,
    pub dtype: Dtype,
    pub shape: Vec<usize>,
    pub data: &'a [u8],
}

impl TensorView<'_> {
    pub fn numel(&self) -> usize {
        self.shape.iter().product()
    }

    /// The values as f32 (F32 / F16 / BF16; anything else is an error).
    pub fn to_f32(&self) -> Result<Vec<f32>> {
        let d = self.data;
        Ok(match self.dtype {
            Dtype::F32 => d.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
            Dtype::F16 => d.chunks_exact(2).map(|c| f16_to_f32(u16::from_le_bytes([c[0], c[1]]))).collect(),
            Dtype::BF16 => d.chunks_exact(2).map(|c| bf16_to_f32(u16::from_le_bytes([c[0], c[1]]))).collect(),
            other => return Err(err(alloc::format!("safetensors: tensor {} is {other:?}, not a float type", self.name))),
        })
    }

    /// The raw binary16 values of an F16 tensor (the f16 weight path keeps them as they are).
    pub fn f16_bits(&self) -> Option<Vec<u16>> {
        (self.dtype == Dtype::F16).then(|| self.data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
    }
}

#[derive(Debug, Clone)]
pub struct SafeTensors<'a> {
    /// In header order.
    pub tensors: Vec<TensorView<'a>>,
    pub metadata: Vec<(String, String)>,
}

impl<'a> SafeTensors<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        let n = bytes.get(..8).ok_or_else(|| err("safetensors: shorter than its 8-byte header length"))?;
        let n = u64::from_le_bytes([n[0], n[1], n[2], n[3], n[4], n[5], n[6], n[7]]);
        let n = usize::try_from(n).ok().filter(|&n| n <= MAX_HEADER).ok_or_else(|| err("safetensors: header too large"))?;
        let header = bytes.get(8..8 + n).ok_or_else(|| err("safetensors: header runs past the file"))?;
        if header.first() != Some(&b'{') {
            return Err(err("safetensors: header is not a JSON object"));
        }
        let header = core::str::from_utf8(header).map_err(|_| err("safetensors: header is not UTF-8"))?;
        let buf = &bytes[8 + n..];
        let doc = json::parse(header)?;
        let members = doc.as_object().ok_or_else(|| err("safetensors: header is not a JSON object"))?;
        let mut tensors = Vec::new();
        let mut metadata = Vec::new();
        let mut spans = Vec::new();
        for (name, v) in members {
            if name == "__metadata__" {
                for (k, s) in v.as_object().ok_or_else(|| err("safetensors: __metadata__ is not an object"))? {
                    let s = s.as_str().ok_or_else(|| err("safetensors: __metadata__ values must be strings"))?;
                    metadata.push((k.clone(), s.to_string()));
                }
                continue;
            }
            let bad = |what: &str| err(alloc::format!("safetensors: tensor {name}: {what}"));
            let dtype = v.get("dtype").and_then(Value::as_str).ok_or_else(|| bad("no dtype"))?;
            let dtype = Dtype::parse(dtype).ok_or_else(|| bad("unknown dtype"))?;
            let shape = v
                .get("shape")
                .and_then(Value::as_array)
                .ok_or_else(|| bad("no shape"))?
                .iter()
                .map(|d| d.as_u64().and_then(|d| usize::try_from(d).ok()))
                .collect::<Option<Vec<usize>>>()
                .ok_or_else(|| bad("bad shape"))?;
            let off = v.get("data_offsets").and_then(Value::as_array).ok_or_else(|| bad("no data_offsets"))?;
            let [b, e] = off else { return Err(bad("data_offsets is not [begin, end]")) };
            let (b, e) = b
                .as_u64()
                .zip(e.as_u64())
                .and_then(|(b, e)| usize::try_from(b).ok().zip(usize::try_from(e).ok()))
                .ok_or_else(|| bad("bad data_offsets"))?;
            if e < b || e > buf.len() {
                return Err(bad("data_offsets outside the buffer"));
            }
            let numel = shape.iter().try_fold(1usize, |a, &d| a.checked_mul(d)).ok_or_else(|| bad("shape overflows"))?;
            let len = numel.checked_mul(dtype.size()).ok_or_else(|| bad("shape overflows"))?;
            if len != e - b {
                return Err(bad("byte length does not match dtype × shape"));
            }
            spans.push((b, e));
            tensors.push(TensorView { name: name.clone(), dtype, shape, data: &buf[b..e] });
        }
        spans.sort_unstable();
        let mut at = 0;
        for (b, e) in spans {
            if b != at {
                return Err(err("safetensors: tensors overlap or leave a gap"));
            }
            at = e;
        }
        if at != buf.len() {
            return Err(err("safetensors: bytes after the last tensor"));
        }
        let mut names: Vec<&str> = tensors.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        if names.windows(2).any(|w| w[0] == w[1]) {
            return Err(err("safetensors: duplicate tensor name"));
        }
        Ok(SafeTensors { tensors, metadata })
    }

    pub fn get(&self, name: &str) -> Option<&TensorView<'a>> {
        self.tensors.iter().find(|t| t.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn file(header: &str, buf: &[u8]) -> Vec<u8> {
        let mut v = (header.len() as u64).to_le_bytes().to_vec();
        v.extend_from_slice(header.as_bytes());
        v.extend_from_slice(buf);
        v
    }

    #[test]
    fn reads_three_float_types() {
        let mut buf = Vec::new();
        buf.extend(1.5f32.to_le_bytes());
        buf.extend((-2.0f32).to_le_bytes());
        buf.extend(0x3c00u16.to_le_bytes());
        buf.extend(0xbfc0u16.to_le_bytes());
        let h = r#"{"a":{"dtype":"F32","shape":[2],"data_offsets":[0,8]},"h":{"dtype":"F16","shape":[1],"data_offsets":[8,10]},"b":{"dtype":"BF16","shape":[1,1],"data_offsets":[10,12]},"__metadata__":{"format":"pt"}}"#;
        let f = file(h, &buf);
        let st = SafeTensors::parse(&f).unwrap();
        assert_eq!(st.get("a").unwrap().to_f32().unwrap(), vec![1.5, -2.0]);
        assert_eq!(st.get("h").unwrap().to_f32().unwrap(), vec![1.0]);
        assert_eq!(st.get("b").unwrap().to_f32().unwrap(), vec![-1.5]);
        assert_eq!(st.get("b").unwrap().shape, vec![1, 1]);
        assert_eq!(st.metadata, vec![("format".to_string(), "pt".to_string())]);
    }

    #[test]
    fn rejects_bad_layouts() {
        let g = |h: &str, n: usize| SafeTensors::parse(&file(h, &vec![0u8; n])).is_err();
        assert!(g(r#"{"a":{"dtype":"F32","shape":[2],"data_offsets":[0,4]}}"#, 4)); // size mismatch
        assert!(g(r#"{"a":{"dtype":"F32","shape":[1],"data_offsets":[4,8]}}"#, 8)); // gap
        assert!(g(r#"{"a":{"dtype":"F32","shape":[1],"data_offsets":[0,4]}}"#, 8)); // trailing bytes
        assert!(g(r#"{"a":{"dtype":"F32","shape":[1],"data_offsets":[0,4]},"b":{"dtype":"F32","shape":[1],"data_offsets":[0,4]}}"#, 4));
        assert!(g(r#"{"a":{"dtype":"Q4","shape":[1],"data_offsets":[0,1]}}"#, 1));
        assert!(g(r#"{"a":{"dtype":"F32","shape":[4294967296,4294967296,16],"data_offsets":[0,4]}}"#, 4));
        assert!(g(r#"[]"#, 0));
        assert!(SafeTensors::parse(&[1, 2, 3]).is_err());
        assert!(SafeTensors::parse(&u64::MAX.to_le_bytes()).is_err());
        assert!(SafeTensors::parse(&file("{}", &[])).unwrap().tensors.is_empty());
    }
}
