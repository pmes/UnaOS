// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The BERT encoder (Devlin et al. 2018, "BERT", §3 + the Transformer encoder of Vaswani et al.
//! 2017 §3.2–3.3) as HF `BertModel` defines it, and the sentence-transformers pooling.
//!
//! For each token `t` at position `p` in its sequence:
//!
//! 1. `e = LayerNorm((word[id] + type[0]) + pos[p])` (the order of HF `BertEmbeddings`).
//! 2. Per layer (post-LN): `[Q|K|V] = x·W_qkvᵀ + b`; per head `h` (head size `d = H / heads`),
//!    `s_ij = (q_i·k_j) / √d` over the keys `j` of the SAME sequence only — exact masking: a key
//!    outside the sequence is never part of the softmax, so a packed batch needs no padding and no
//!    `-inf`; `p_ij = softmax_j(s_ij)`; `c_i = Σ_j p_ij·v_j`. Then
//!    `x = LayerNorm(c·W_oᵀ + b_o + x)`, `x = LayerNorm(GELU(x·W_iᵀ + b_i)·W_o2ᵀ + b_o2 + x)`,
//!    GELU in its exact erf form, LayerNorm `(x − μ)/√(σ² + eps)·γ + β` with the biased variance.
//! 3. Sentence vector: the mean of the sequence's last hidden states, divided by
//!    `max(‖·‖₂, 1e-12)` (sentence-transformers `Pooling(mean)` + `Normalize`).
//!
//! **Order of operations.** Dense layers: [`crate::matmul`]. Dot products `q·k` and the context
//! sum `Σ_j p_ij·v_j`: f32, ascending index. LayerNorm statistics, the softmax (max-subtracted,
//! [`crate::math::exp`]) and the pooling / norm: f64, ascending index, rounded once to f32.
//! Sequences are processed independently, so a sequence's vector is the same bits alone or in
//! any batch, in any position.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::half::{f16_to_f32, f32_to_f16};
use crate::json::{self, Value};
use crate::matmul::Linear;
use crate::math;
use crate::onnx::Graph;
use crate::safetensors::SafeTensors;
use crate::{Result, err};

/// The HF `config.json` fields the encoder reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub vocab_size: usize,
    pub hidden_size: usize,
    pub num_hidden_layers: usize,
    pub num_attention_heads: usize,
    pub intermediate_size: usize,
    pub max_position_embeddings: usize,
    pub type_vocab_size: usize,
    pub layer_norm_eps: f64,
}

impl Config {
    pub fn from_json(src: &str) -> Result<Config> {
        let v = json::parse(src)?;
        let n = |k: &str| -> Result<usize> {
            v.get(k)
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .filter(|&n| n > 0 && n <= 1 << 20)
                .ok_or_else(|| err(alloc::format!("config.json: {k} missing or out of range")))
        };
        match v.get("hidden_act").and_then(Value::as_str) {
            None | Some("gelu") => {}
            Some(a) => return Err(err(alloc::format!("config.json: hidden_act {a} (only the erf gelu is implemented)"))),
        }
        match v.get("position_embedding_type").and_then(Value::as_str) {
            None | Some("absolute") => {}
            Some(a) => return Err(err(alloc::format!("config.json: position_embedding_type {a} (only absolute)"))),
        }
        let c = Config {
            vocab_size: n("vocab_size")?,
            hidden_size: n("hidden_size")?,
            num_hidden_layers: n("num_hidden_layers")?,
            num_attention_heads: n("num_attention_heads")?,
            intermediate_size: n("intermediate_size")?,
            max_position_embeddings: n("max_position_embeddings")?,
            type_vocab_size: v.get("type_vocab_size").and_then(Value::as_u64).map_or(2, |t| t as usize),
            layer_norm_eps: v.get("layer_norm_eps").and_then(Value::as_f64).unwrap_or(1e-12),
        };
        if c.hidden_size % c.num_attention_heads != 0 || c.type_vocab_size == 0 || !(c.layer_norm_eps >= 0.0) {
            return Err(err("config.json: inconsistent sizes"));
        }
        Ok(c)
    }
}

/// An embedding table, f32 or binary16.
#[derive(Debug, Clone)]
enum Table {
    F32(Vec<f32>),
    F16(Vec<u16>),
}

impl Table {
    fn new(v: Vec<f32>, half: bool) -> Table {
        if half { Table::F16(v.iter().map(|&x| f32_to_f16(x)).collect()) } else { Table::F32(v) }
    }
    fn get(&self, i: usize) -> f32 {
        match self {
            Table::F32(v) => v[i],
            Table::F16(v) => f16_to_f32(v[i]),
        }
    }
}

#[derive(Debug, Clone)]
struct Layer {
    qkv: Linear,
    attn_out: Linear,
    ln1: (Vec<f32>, Vec<f32>),
    inter: Linear,
    out: Linear,
    ln2: (Vec<f32>, Vec<f32>),
}

#[derive(Debug, Clone)]
pub struct Bert {
    pub config: Config,
    word: Table,
    pos: Table,
    typ: Table,
    ln: (Vec<f32>, Vec<f32>),
    layers: Vec<Layer>,
    half: bool,
}

/// Where the weights come from: a name → (shape, f32 values) lookup.
pub trait WeightSource {
    fn tensor(&self, name: &str) -> Result<Option<(Vec<usize>, Vec<f32>)>>;
}

impl WeightSource for SafeTensors<'_> {
    fn tensor(&self, name: &str) -> Result<Option<(Vec<usize>, Vec<f32>)>> {
        match self.get(name) {
            Some(t) => Ok(Some((t.shape.clone(), t.to_f32()?))),
            None => Ok(None),
        }
    }
}

impl WeightSource for Graph {
    fn tensor(&self, name: &str) -> Result<Option<(Vec<usize>, Vec<f32>)>> {
        Ok(self.initializers.get(name).map(|t| (t.dims.clone(), t.data.clone())))
    }
}

/// Load an ONNX export's weights: the `MatMul` weights get their BERT names back (see
/// [`crate::onnx::name_linear_weights`]).
pub fn graph_from_onnx(bytes: &[u8]) -> Result<Graph> {
    let mut g = crate::onnx::parse_model(bytes).map_err(|e| err(alloc::format!("onnx: {e}")))?;
    crate::onnx::name_linear_weights(&mut g).map_err(|e| err(alloc::format!("onnx: {e}")))?;
    Ok(g)
}

struct Loader<'a, S: WeightSource> {
    src: &'a S,
    prefix: &'static str,
}

impl<S: WeightSource> Loader<'_, S> {
    fn get(&self, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
        let full = alloc::format!("{}{name}", self.prefix);
        let (s, v) = self.src.tensor(&full)?.ok_or_else(|| err(alloc::format!("weights: no tensor {full}")))?;
        if s != shape {
            return Err(err(alloc::format!("weights: {full} is {s:?}, expected {shape:?}")));
        }
        if v.iter().any(|x| !x.is_finite()) {
            return Err(err(alloc::format!("weights: {full} holds a non-finite value")));
        }
        Ok(v)
    }
}

fn layer_norm(x: &mut [f32], (g, b): &(Vec<f32>, Vec<f32>), eps: f64) {
    let n = x.len() as f64;
    let mean = x.iter().map(|&v| v as f64).sum::<f64>() / n;
    let var = x.iter().map(|&v| (v as f64 - mean) * (v as f64 - mean)).sum::<f64>() / n;
    let inv = 1.0 / math::sqrt(var + eps);
    for ((v, &g), &b) in x.iter_mut().zip(g).zip(b) {
        *v = (((*v as f64 - mean) * inv) * g as f64 + b as f64) as f32;
    }
}

impl Bert {
    /// Load from any [`WeightSource`]; tensor names are HF `BertModel`'s, with or without a
    /// `bert.` prefix. `half` keeps every weight matrix and embedding table as binary16.
    pub fn load<S: WeightSource>(config: Config, src: &S, half: bool) -> Result<Bert> {
        let probe = "embeddings.word_embeddings.weight";
        let prefix = if src.tensor(probe)?.is_some() {
            ""
        } else if src.tensor(&alloc::format!("bert.{probe}"))?.is_some() {
            "bert."
        } else {
            return Err(err(alloc::format!("weights: no tensor {probe}")));
        };
        let l = Loader { src, prefix };
        let c = &config;
        let (h, i) = (c.hidden_size, c.intermediate_size);
        let ln = |name: &str| -> Result<(Vec<f32>, Vec<f32>)> {
            Ok((l.get(&alloc::format!("{name}.weight"), &[h])?, l.get(&alloc::format!("{name}.bias"), &[h])?))
        };
        let lin = |name: &str, o: usize, n: usize| -> Result<Linear> {
            Linear::new(&l.get(&alloc::format!("{name}.weight"), &[o, n])?, &l.get(&alloc::format!("{name}.bias"), &[o])?, o, n, half)
        };
        let word = Table::new(l.get("embeddings.word_embeddings.weight", &[c.vocab_size, h])?, half);
        let pos = Table::new(l.get("embeddings.position_embeddings.weight", &[c.max_position_embeddings, h])?, half);
        let typ = Table::new(l.get("embeddings.token_type_embeddings.weight", &[c.type_vocab_size, h])?, half);
        let emb_ln = ln("embeddings.LayerNorm")?;
        let mut layers = Vec::with_capacity(c.num_hidden_layers);
        for n in 0..c.num_hidden_layers {
            let p = alloc::format!("encoder.layer.{n}");
            let w = |s: &str| l.get(&alloc::format!("{p}.attention.self.{s}.weight"), &[h, h]);
            let b = |s: &str| l.get(&alloc::format!("{p}.attention.self.{s}.bias"), &[h]);
            let (qw, kw, vw) = (w("query")?, w("key")?, w("value")?);
            let (qb, kb, vb) = (b("query")?, b("key")?, b("value")?);
            layers.push(Layer {
                qkv: Linear::concat(&[(&qw, &qb, h), (&kw, &kb, h), (&vw, &vb, h)], h, half)?,
                attn_out: lin(&alloc::format!("{p}.attention.output.dense"), h, h)?,
                ln1: ln(&alloc::format!("{p}.attention.output.LayerNorm"))?,
                inter: lin(&alloc::format!("{p}.intermediate.dense"), i, h)?,
                out: lin(&alloc::format!("{p}.output.dense"), h, i)?,
                ln2: ln(&alloc::format!("{p}.output.LayerNorm"))?,
            });
        }
        Ok(Bert { config, word, pos, typ, ln: emb_ln, layers, half })
    }

    pub fn is_half(&self) -> bool {
        self.half
    }

    /// The last hidden states of a packed batch: `Σ len(seq)` rows of `hidden_size`.
    pub fn hidden_states(&self, seqs: &[&[u32]]) -> Result<Vec<f32>> {
        let c = &self.config;
        let h = c.hidden_size;
        for s in seqs {
            if s.is_empty() || s.len() > c.max_position_embeddings {
                return Err(err(alloc::format!("bert: sequence length {} (1..={})", s.len(), c.max_position_embeddings)));
            }
            if let Some(&bad) = s.iter().find(|&&id| id as usize >= c.vocab_size) {
                return Err(err(alloc::format!("bert: token id {bad} ≥ vocab size {}", c.vocab_size)));
            }
        }
        let t: usize = seqs.iter().map(|s| s.len()).sum();
        let mut x = vec![0.0f32; t * h];
        let mut row = 0;
        for s in seqs {
            for (p, &id) in s.iter().enumerate() {
                let xr = &mut x[row * h..(row + 1) * h];
                for (j, v) in xr.iter_mut().enumerate() {
                    *v = (self.word.get(id as usize * h + j) + self.typ.get(j)) + self.pos.get(p * h + j);
                }
                layer_norm(xr, &self.ln, c.layer_norm_eps);
                row += 1;
            }
        }
        let heads = c.num_attention_heads;
        let d = h / heads;
        let scale = math::sqrt(d as f64) as f32;
        let mut qkv = vec![0.0f32; t * 3 * h];
        let mut ctx = vec![0.0f32; t * h];
        let mut tmp = vec![0.0f32; t * h];
        let mut inter = vec![0.0f32; t * c.intermediate_size];
        let maxlen = seqs.iter().map(|s| s.len()).max().unwrap_or(0);
        let mut scores = vec![0.0f64; maxlen];
        for layer in &self.layers {
            layer.qkv.forward(&x, t, &mut qkv);
            let mut base = 0;
            for s in seqs {
                let n = s.len();
                for hd in 0..heads {
                    let (qo, ko, vo) = (hd * d, h + hd * d, 2 * h + hd * d);
                    for i in 0..n {
                        let q = &qkv[(base + i) * 3 * h + qo..(base + i) * 3 * h + qo + d];
                        let mut max = f64::NEG_INFINITY;
                        for j in 0..n {
                            let k = &qkv[(base + j) * 3 * h + ko..(base + j) * 3 * h + ko + d];
                            let mut dot = 0.0f32;
                            for e in 0..d {
                                dot += q[e] * k[e];
                            }
                            let sc = (dot / scale) as f64;
                            scores[j] = sc;
                            if sc > max {
                                max = sc;
                            }
                        }
                        let mut sum = 0.0f64;
                        for sj in scores[..n].iter_mut() {
                            *sj = math::exp(*sj - max);
                            sum += *sj;
                        }
                        let out = &mut ctx[(base + i) * h + hd * d..(base + i) * h + hd * d + d];
                        out.iter_mut().for_each(|v| *v = 0.0);
                        for j in 0..n {
                            let pj = (scores[j] / sum) as f32;
                            let v = &qkv[(base + j) * 3 * h + vo..(base + j) * 3 * h + vo + d];
                            for e in 0..d {
                                out[e] += pj * v[e];
                            }
                        }
                    }
                }
                base += n;
            }
            layer.attn_out.forward(&ctx, t, &mut tmp);
            for r in 0..t {
                let xr = &mut x[r * h..(r + 1) * h];
                for (v, a) in xr.iter_mut().zip(&tmp[r * h..(r + 1) * h]) {
                    *v = *a + *v;
                }
                layer_norm(xr, &layer.ln1, c.layer_norm_eps);
            }
            layer.inter.forward(&x, t, &mut inter);
            inter.iter_mut().for_each(|v| *v = math::gelu(*v));
            layer.out.forward(&inter, t, &mut tmp);
            for r in 0..t {
                let xr = &mut x[r * h..(r + 1) * h];
                for (v, a) in xr.iter_mut().zip(&tmp[r * h..(r + 1) * h]) {
                    *v = *a + *v;
                }
                layer_norm(xr, &layer.ln2, c.layer_norm_eps);
            }
        }
        Ok(x)
    }

    /// Sentence vectors (mean-pooled, L2-normalised), one per sequence, in order.
    pub fn embed(&self, seqs: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
        let h = self.config.hidden_size;
        let x = self.hidden_states(seqs)?;
        let mut out = Vec::with_capacity(seqs.len());
        let mut base = 0;
        for s in seqs {
            let n = s.len();
            let mut mean = vec![0.0f64; h];
            for r in base..base + n {
                for (m, &v) in mean.iter_mut().zip(&x[r * h..(r + 1) * h]) {
                    *m += v as f64;
                }
            }
            mean.iter_mut().for_each(|m| *m /= n as f64);
            let norm = math::sqrt(mean.iter().map(|m| m * m).sum::<f64>()).max(1e-12);
            out.push(mean.iter().map(|m| (m / norm) as f32).collect());
            base += n;
        }
        Ok(out)
    }
}

/// A short description for logs: `bert 6×384 (12 heads, ffn 1536) f32`.
pub fn describe(b: &Bert) -> String {
    let c = &b.config;
    let mut s = alloc::format!("bert {}×{} ({} heads, ffn {})", c.num_hidden_layers, c.hidden_size, c.num_attention_heads, c.intermediate_size);
    s.push_str(if b.half { " f16" } else { " f32" });
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;

    struct Map(BTreeMap<String, (Vec<usize>, Vec<f32>)>);
    impl WeightSource for Map {
        fn tensor(&self, name: &str) -> Result<Option<(Vec<usize>, Vec<f32>)>> {
            Ok(self.0.get(name).cloned())
        }
    }

    fn tiny() -> (Config, Map) {
        let cfg = Config::from_json(r#"{"vocab_size":11,"hidden_size":8,"num_hidden_layers":2,"num_attention_heads":2,"intermediate_size":12,"max_position_embeddings":16,"type_vocab_size":2,"layer_norm_eps":1e-12,"hidden_act":"gelu"}"#).unwrap();
        let mut seed = 99u64;
        let mut rnd = |n: usize| -> Vec<f32> {
            (0..n)
                .map(|_| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    ((seed >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.4
                })
                .collect()
        };
        let mut m = BTreeMap::new();
        let mut put = |k: String, s: Vec<usize>| {
            let n = s.iter().product();
            m.insert(k, (s, rnd(n)));
        };
        put("embeddings.word_embeddings.weight".into(), vec![11, 8]);
        put("embeddings.position_embeddings.weight".into(), vec![16, 8]);
        put("embeddings.token_type_embeddings.weight".into(), vec![2, 8]);
        put("embeddings.LayerNorm.weight".into(), vec![8]);
        put("embeddings.LayerNorm.bias".into(), vec![8]);
        for l in 0..2 {
            for (n, s) in [
                ("attention.self.query", [8, 8]),
                ("attention.self.key", [8, 8]),
                ("attention.self.value", [8, 8]),
                ("attention.output.dense", [8, 8]),
                ("intermediate.dense", [12, 8]),
                ("output.dense", [8, 12]),
            ] {
                put(alloc::format!("encoder.layer.{l}.{n}.weight"), s.to_vec());
                put(alloc::format!("encoder.layer.{l}.{n}.bias"), vec![s[0]]);
            }
            for n in ["attention.output.LayerNorm", "output.LayerNorm"] {
                put(alloc::format!("encoder.layer.{l}.{n}.weight"), vec![8]);
                put(alloc::format!("encoder.layer.{l}.{n}.bias"), vec![8]);
            }
        }
        (cfg, Map(m))
    }

    #[test]
    fn batch_is_bit_identical_to_single() {
        let (cfg, w) = tiny();
        let b = Bert::load(cfg, &w, false).unwrap();
        let seqs: [&[u32]; 3] = [&[1, 5, 2], &[1, 3, 4, 7, 9, 2], &[1, 2]];
        let batch = b.embed(&seqs).unwrap();
        for (i, s) in seqs.iter().enumerate() {
            let one = b.embed(&[s]).unwrap();
            assert!(one[0].iter().zip(&batch[i]).all(|(a, b)| a.to_bits() == b.to_bits()));
            let n: f32 = one[0].iter().map(|v| v * v).sum();
            assert!((n - 1.0).abs() < 1e-6);
        }
        // Reversed order: same vectors.
        let rev = b.embed(&[seqs[2], seqs[1], seqs[0]]).unwrap();
        assert_eq!(rev[0], batch[2]);
        assert!(b.embed(&[&[1, 11]]).is_err()); // id out of range
        assert!(b.embed(&[&[]]).is_err());
        assert!(b.embed(&[&[1; 17]]).is_err()); // longer than the position table
    }

    #[test]
    fn missing_or_misshapen_weights_are_errors() {
        let (cfg, mut w) = tiny();
        w.0.get_mut("encoder.layer.1.output.dense.weight").unwrap().0 = vec![12, 8];
        assert!(Bert::load(cfg.clone(), &w, false).is_err());
        w.0.remove("encoder.layer.1.output.dense.weight");
        assert!(Bert::load(cfg, &w, false).is_err());
    }
}
