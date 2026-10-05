// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A minimal ONNX initializer reader (EMBED, B317).
//!
//! Reads the weight tensors (`GraphProto.initializer`) and the node list
//! (`GraphProto.node`: op type, inputs, outputs) out of an ONNX `ModelProto`
//! with a hand-written protobuf walk — no `protoc`, no generated code (the
//! reason: `candle-onnx` needs `protoc` at build time). A byte slice in,
//! owned values out. Moved here from gneiss_pal (EMBED, B317) by INFERCORE.
//!
//! Fields read (onnx.proto3): ModelProto.graph = 7; GraphProto.node = 1,
//! initializer = 5; NodeProto.input = 1, output = 2, op_type = 4;
//! TensorProto.dims = 1, data_type = 2 (1 = FLOAT), float_data = 4,
//! name = 8, raw_data = 9, data_location = 14 (1 = EXTERNAL: refused).

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::{format, vec};

/// A float tensor: row-major `data` with shape `dims`.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    pub dims: Vec<usize>,
    pub data: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub op_type: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Graph {
    /// FLOAT initializers by name (other element types are skipped).
    pub initializers: BTreeMap<String, Tensor>,
    pub nodes: Vec<Node>,
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

enum Field<'a> {
    Varint(u64),
    Fixed64,
    Len(&'a [u8]),
    Fixed32(u32),
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn varint(&mut self) -> Result<u64, String> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *self.buf.get(self.pos).ok_or("truncated varint")?;
            self.pos += 1;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err("varint too long".into())
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len()).ok_or("truncated field")?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn next(&mut self) -> Result<Option<(u64, Field<'a>)>, String> {
        if self.pos >= self.buf.len() {
            return Ok(None);
        }
        let key = self.varint()?;
        let f = match key & 7 {
            0 => Field::Varint(self.varint()?),
            1 => {
                self.take(8)?;
                Field::Fixed64
            }
            2 => {
                let n = self.varint()? as usize;
                Field::Len(self.take(n)?)
            }
            5 => {
                let b = self.take(4)?;
                Field::Fixed32(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            }
            w => return Err(format!("unsupported protobuf wire type {w}")),
        };
        Ok(Some((key >> 3, f)))
    }
}

fn utf8(b: &[u8]) -> Result<String, String> {
    core::str::from_utf8(b).map(str::to_string).map_err(|_| "non-UTF-8 string".to_string())
}

fn parse_node(b: &[u8]) -> Result<Node, String> {
    let mut n = Node { op_type: String::new(), inputs: vec![], outputs: vec![] };
    let mut r = Reader::new(b);
    while let Some((field, f)) = r.next()? {
        match (field, f) {
            (1, Field::Len(s)) => n.inputs.push(utf8(s)?),
            (2, Field::Len(s)) => n.outputs.push(utf8(s)?),
            (4, Field::Len(s)) => n.op_type = utf8(s)?,
            _ => {}
        }
    }
    Ok(n)
}

/// `Ok(None)` for a tensor that is not FLOAT (skipped).
fn parse_tensor(b: &[u8]) -> Result<Option<(String, Tensor)>, String> {
    let (mut dims, mut dtype, mut name) = (Vec::new(), 0u64, String::new());
    let (mut raw, mut floats): (Option<&[u8]>, Vec<f32>) = (None, Vec::new());
    let mut r = Reader::new(b);
    while let Some((field, f)) = r.next()? {
        match (field, f) {
            (1, Field::Varint(v)) => dims.push(v as usize),
            (1, Field::Len(s)) => {
                let mut p = Reader::new(s);
                while p.pos < s.len() {
                    dims.push(p.varint()? as usize);
                }
            }
            (2, Field::Varint(v)) => dtype = v,
            (4, Field::Fixed32(v)) => floats.push(f32::from_bits(v)),
            (4, Field::Len(s)) => floats.extend(s.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))),
            (8, Field::Len(s)) => name = utf8(s)?,
            (9, Field::Len(s)) => raw = Some(s),
            (14, Field::Varint(1)) => return Err(format!("tensor {name}: external data is not supported")),
            _ => {}
        }
    }
    if dtype != 1 {
        return Ok(None);
    }
    let data = match raw {
        Some(s) => s.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
        None => floats,
    };
    let want = dims.iter().try_fold(1usize, |a, &d| a.checked_mul(d));
    if want != Some(data.len()) {
        return Err(format!("tensor {name}: {} values for shape {dims:?}", data.len()));
    }
    Ok(Some((name, Tensor { dims, data })))
}

/// Parse a `ModelProto`'s graph: FLOAT initializers and nodes.
pub fn parse_model(bytes: &[u8]) -> Result<Graph, String> {
    let mut r = Reader::new(bytes);
    let mut graph = None;
    while let Some((field, f)) = r.next()? {
        if let (7, Field::Len(g)) = (field, f) {
            graph = Some(g);
        }
    }
    let g = graph.ok_or("no graph in the ONNX model")?;
    let mut out = Graph::default();
    let mut r = Reader::new(g);
    while let Some((field, f)) = r.next()? {
        match (field, f) {
            (1, Field::Len(s)) => out.nodes.push(parse_node(s)?),
            (5, Field::Len(s)) => {
                if let Some((name, t)) = parse_tensor(s)? {
                    out.initializers.insert(name, t);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

impl Tensor {
    /// The transpose of a 2-D tensor (`[r, c]` → `[c, r]`).
    pub fn transposed(&self) -> Result<Tensor, String> {
        let [r, c] = self.dims[..] else { return Err(format!("transpose of a {}-D tensor", self.dims.len())) };
        let mut data = vec![0.0; r * c];
        for i in 0..r {
            for j in 0..c {
                data[j * r + i] = self.data[i * c + j];
            }
        }
        Ok(Tensor { dims: vec![c, r], data })
    }
}

/// Give the BERT names back to the weights an exporter renamed: every
/// `MatMul(x, W)` whose output feeds `Add(.., <prefix>.bias)` has `W` (stored
/// `[in, out]`) re-inserted as `<prefix>.weight` transposed to `[out, in]`
/// (the `nn.Linear` layout). Answers how many weights were mapped.
pub fn name_linear_weights(g: &mut Graph) -> Result<usize, String> {
    let mut mapped = Vec::new();
    for mm in g.nodes.iter().filter(|n| n.op_type == "MatMul") {
        let (Some(w), Some(out)) = (mm.inputs.get(1), mm.outputs.first()) else { continue };
        if !g.initializers.contains_key(w) || w.ends_with(".weight") {
            continue;
        }
        let bias = g
            .nodes
            .iter()
            .filter(|n| n.op_type == "Add" && n.inputs.iter().any(|i| i == out))
            .flat_map(|n| n.inputs.iter())
            .find(|i| i.ends_with(".bias") && g.initializers.contains_key(*i));
        if let Some(b) = bias {
            mapped.push((w.clone(), format!("{}.weight", b.trim_end_matches(".bias"))));
        }
    }
    for (from, to) in &mapped {
        let t = g.initializers[from].transposed()?;
        g.initializers.insert(to.clone(), t);
    }
    Ok(mapped.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn key(field: u64, wire: u64) -> u8 {
        ((field << 3) | wire) as u8
    }
    fn len(field: u64, body: &[u8]) -> Vec<u8> {
        let mut v = vec![key(field, 2)];
        let mut n = body.len();
        while n >= 0x80 {
            v.push((n as u8 & 0x7f) | 0x80);
            n >>= 7;
        }
        v.push(n as u8);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn reads_a_tiny_model_and_renames_a_linear() {
        // W: 2x3 raw floats, bias: 3 floats, named so the Add carries the BERT prefix.
        let w: Vec<u8> = (1..=6).flat_map(|i| (i as f32).to_le_bytes()).collect();
        let mut tw = len(1, &[2, 3]); // packed dims
        tw.extend([key(2, 0), 1]);
        tw.extend(len(8, b"onnx::MatMul_1"));
        tw.extend(len(9, &w));
        let mut tb = vec![key(1, 0), 3, key(2, 0), 1];
        tb.extend(len(8, b"l.q.bias"));
        tb.extend(len(9, &[0u8; 12]));
        let mut mm = len(1, b"x");
        mm.extend(len(1, b"onnx::MatMul_1"));
        mm.extend(len(2, b"y"));
        mm.extend(len(4, b"MatMul"));
        let mut add = len(1, b"l.q.bias");
        add.extend(len(1, b"y"));
        add.extend(len(2, b"z"));
        add.extend(len(4, b"Add"));
        let mut graph = len(1, &mm);
        graph.extend(len(1, &add));
        graph.extend(len(5, &tw));
        graph.extend(len(5, &tb));
        let model = len(7, &graph);

        let mut g = parse_model(&model).unwrap();
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.initializers["onnx::MatMul_1"].dims, vec![2, 3]);
        assert_eq!(name_linear_weights(&mut g).unwrap(), 1);
        let t = &g.initializers["l.q.weight"];
        assert_eq!(t.dims, vec![3, 2]);
        assert_eq!(t.data, vec![1.0, 4.0, 2.0, 5.0, 3.0, 6.0]);
    }

    #[test]
    fn truncation_is_an_error_not_a_panic() {
        assert!(parse_model(&[key(7, 2), 50, 1, 2]).is_err());
    }
}
