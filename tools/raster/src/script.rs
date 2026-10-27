// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The `--do` parser (RASTERCLI, LEDGER SR233): script text -> [`Step`]s. One verb per line, `#` to end of line is a
//! comment, blank lines skipped, tokens split on whitespace. Parsing only: every value is handed to the cores as is.

use brush_core::BlendMode;

/// One parsed verb (the grammar is the table in `docs/dev/evidence/raster/RASTERCLI.md`).
#[derive(Debug, Clone)]
pub enum Op {
    New { w: u32, h: u32 },
    Open(String),
    Save(String),
    LayerAdd(String),
    LayerRemove(usize),
    LayerMove(usize, usize),
    LayerOpacity(usize, u8),
    LayerBlend(usize, BlendMode),
    LayerVisible(usize, bool),
    LayerOffset(usize, i32, i32),
    LayerRename(usize, String),
    SelectRect(i32, i32, i32, i32),
    SelectEllipse(i32, i32, i32, i32),
    SelectPoly(Vec<(i32, i32)>),
    SelectWand { layer: usize, x: i32, y: i32, tol: u8 },
    SelectNone,
    Fill(usize, [u8; 4]),
    Clear(usize),
    Brush(BrushArgs),
    Adjust(usize, Adjust),
    Undo,
    Redo,
    Export(String),
    Stat,
}

#[derive(Debug, Clone)]
pub struct BrushArgs {
    pub layer: usize,
    pub rgba: [u8; 4],
    pub diameter: f32,
    pub hardness: u8,
    pub spacing: f32,
    pub flow: u8,
    pub mode: BlendMode,
    /// `(x, y, pressure)` in layer coordinates.
    pub points: Vec<(f32, f32, u8)>,
}

#[derive(Debug, Clone)]
pub enum Adjust {
    Levels(adjust_core::Levels),
    Curves(Vec<(u8, u8)>),
    HueSat(adjust_core::HueSat),
    Invert,
    Threshold(u8),
    BoxBlur(u16),
    Gaussian(u16),
    Unsharp(adjust_core::Unsharp),
}

/// A parsed line: its 1-based number, the verb's name (for `"last"`), the op.
#[derive(Debug, Clone)]
pub struct Step {
    pub line: usize,
    pub name: String,
    pub op: Op,
}

/// A script error: the line and the verb name it stopped at.
#[derive(Debug)]
pub struct ParseError {
    pub line: usize,
    pub name: String,
}

fn num<T: std::str::FromStr>(t: Option<&&str>) -> Option<T> {
    t?.parse().ok()
}

fn rgba(t: Option<&&str>) -> Option<[u8; 4]> {
    let s = t?;
    if s.len() != 8 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 4];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn blend(t: Option<&&str>) -> Option<BlendMode> {
    Some(match *t? {
        "normal" => BlendMode::Normal,
        "multiply" => BlendMode::Multiply,
        "screen" => BlendMode::Screen,
        "overlay" => BlendMode::Overlay,
        "darken" => BlendMode::Darken,
        "lighten" => BlendMode::Lighten,
        "difference" => BlendMode::Difference,
        "erase" => BlendMode::Erase,
        _ => return None,
    })
}

/// `a,b` -> `(a, b)`.
fn pair<T: std::str::FromStr>(s: &str) -> Option<(T, T)> {
    let (a, b) = s.split_once(',')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// `x,y[,p]` -> `(x, y, p)` (pressure 255 when absent).
fn point(s: &str) -> Option<(f32, f32, u8)> {
    let mut it = s.split(',');
    let (x, y) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    let p = match it.next() {
        Some(p) => p.parse().ok()?,
        None => 255,
    };
    if it.next().is_some() {
        return None;
    }
    Some((x, y, p))
}

/// `t` must hold exactly `n` tokens.
fn arity(t: &[&str], n: usize) -> Option<()> {
    (t.len() == n).then_some(())
}

fn layer_op(t: &[&str]) -> Option<Op> {
    let i = |k: usize| num::<usize>(t.get(k));
    Some(match *t.get(1)? {
        "add" => {
            arity(t, 3)?;
            Op::LayerAdd(t[2].to_string())
        }
        "remove" => {
            arity(t, 3)?;
            Op::LayerRemove(i(2)?)
        }
        "move" => {
            arity(t, 4)?;
            Op::LayerMove(i(2)?, i(3)?)
        }
        "opacity" => {
            arity(t, 4)?;
            Op::LayerOpacity(i(2)?, num(t.get(3))?)
        }
        "blend" => {
            arity(t, 4)?;
            Op::LayerBlend(i(2)?, blend(t.get(3))?)
        }
        "visible" => {
            arity(t, 4)?;
            Op::LayerVisible(
                i(2)?,
                match t[3] {
                    "0" => false,
                    "1" => true,
                    _ => return None,
                },
            )
        }
        "offset" => {
            arity(t, 5)?;
            Op::LayerOffset(i(2)?, num(t.get(3))?, num(t.get(4))?)
        }
        "rename" => {
            arity(t, 4)?;
            Op::LayerRename(i(2)?, t[3].to_string())
        }
        _ => return None,
    })
}

fn select_op(t: &[&str]) -> Option<Op> {
    let n = |k: usize| num::<i32>(t.get(k));
    Some(match *t.get(1)? {
        "rect" => {
            arity(t, 6)?;
            Op::SelectRect(n(2)?, n(3)?, n(4)?, n(5)?)
        }
        "ellipse" => {
            arity(t, 6)?;
            Op::SelectEllipse(n(2)?, n(3)?, n(4)?, n(5)?)
        }
        "poly" => Op::SelectPoly(t[2..].iter().map(|s| pair(s)).collect::<Option<Vec<_>>>()?),
        "wand" => {
            arity(t, 6)?;
            Op::SelectWand { layer: num(t.get(2))?, x: n(3)?, y: n(4)?, tol: num(t.get(5))? }
        }
        "none" => {
            arity(t, 2)?;
            Op::SelectNone
        }
        _ => return None,
    })
}

fn adjust_op(t: &[&str]) -> Option<Op> {
    let layer = num::<usize>(t.get(1))?;
    let a = match *t.get(2)? {
        "levels" => {
            arity(t, 8)?;
            Adjust::Levels(adjust_core::Levels {
                in_lo: num(t.get(3))?,
                in_hi: num(t.get(4))?,
                gamma_q8: num(t.get(5))?,
                out_lo: num(t.get(6))?,
                out_hi: num(t.get(7))?,
            })
        }
        "curves" => Adjust::Curves(t[3..].iter().map(|s| pair(s)).collect::<Option<Vec<_>>>()?),
        "huesat" => {
            arity(t, 6)?;
            Adjust::HueSat(adjust_core::HueSat {
                hue_deg: num(t.get(3))?,
                sat_pct: num(t.get(4))?,
                light_pct: num(t.get(5))?,
            })
        }
        "invert" => {
            arity(t, 3)?;
            Adjust::Invert
        }
        "threshold" => {
            arity(t, 4)?;
            Adjust::Threshold(num(t.get(3))?)
        }
        "boxblur" => {
            arity(t, 4)?;
            Adjust::BoxBlur(num(t.get(3))?)
        }
        "gaussian" => {
            arity(t, 4)?;
            Adjust::Gaussian(num(t.get(3))?)
        }
        "unsharp" => {
            arity(t, 6)?;
            Adjust::Unsharp(adjust_core::Unsharp {
                amount_pct: num(t.get(3))?,
                sigma_q8: num(t.get(4))?,
                threshold: num(t.get(5))?,
            })
        }
        _ => return None,
    };
    Some(Op::Adjust(layer, a))
}

fn brush_op(t: &[&str]) -> Option<Op> {
    if t.len() < 9 {
        return None;
    }
    let mode = match t[7] {
        "normal" => BlendMode::Normal,
        "erase" => BlendMode::Erase,
        _ => return None,
    };
    Some(Op::Brush(BrushArgs {
        layer: num(t.get(1))?,
        rgba: rgba(t.get(2))?,
        diameter: num(t.get(3))?,
        hardness: num(t.get(4))?,
        spacing: num(t.get(5))?,
        flow: num(t.get(6))?,
        mode,
        points: t[8..].iter().map(|s| point(s)).collect::<Option<Vec<_>>>()?,
    }))
}

/// The verb's name for `"last"`: `layer` / `select` name their sub-verb, `adjust` its kind.
fn name_of(t: &[&str]) -> String {
    match t[0] {
        "layer" | "select" => t.get(1).map_or(t[0].to_string(), |s| format!("{} {s}", t[0])),
        "adjust" => t.get(2).map_or(t[0].to_string(), |s| format!("adjust {s}")),
        v => v.to_string(),
    }
}

fn op_of(t: &[&str]) -> Option<Op> {
    let one = |op: Op| arity(t, 1).map(|_| op);
    let path = |f: fn(String) -> Op| arity(t, 2).map(|_| f(t[1].to_string()));
    match t[0] {
        "new" => {
            arity(t, 3)?;
            Some(Op::New { w: num(t.get(1))?, h: num(t.get(2))? })
        }
        "open" => path(Op::Open),
        "save" => path(Op::Save),
        "export" => path(Op::Export),
        "layer" => layer_op(t),
        "select" => select_op(t),
        "fill" => {
            arity(t, 3)?;
            Some(Op::Fill(num(t.get(1))?, rgba(t.get(2))?))
        }
        "clear" => {
            arity(t, 2)?;
            Some(Op::Clear(num(t.get(1))?))
        }
        "brush" => brush_op(t),
        "adjust" => adjust_op(t),
        "undo" => one(Op::Undo),
        "redo" => one(Op::Redo),
        "stat" => one(Op::Stat),
        _ => None,
    }
}

/// Parse a whole script. The first line that is not a verb of the grammar is the error.
pub fn parse(text: &str) -> Result<Vec<Step>, ParseError> {
    let mut steps = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let body = raw.split('#').next().unwrap_or("");
        let t: Vec<&str> = body.split_whitespace().collect();
        if t.is_empty() {
            continue;
        }
        let name = name_of(&t);
        match op_of(&t) {
            Some(op) => steps.push(Step { line: i + 1, name, op }),
            None => return Err(ParseError { line: i + 1, name }),
        }
    }
    Ok(steps)
}
