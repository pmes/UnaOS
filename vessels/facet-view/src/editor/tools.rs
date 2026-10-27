// CHARTER: Facet (Images) — host face over the shared cores (RASTERFACE, LEDGER SR234)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The tool rail: the eight [`Tool`]s, the brush and adjust fields, and the button verbs the tetra tree fires.

/// One tool of the rail. A drag (or click) on the canvas runs the active tool's op.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    SelectRect,
    SelectEllipse,
    SelectLasso,
    Wand,
    Brush,
    Eraser,
    Fill,
    Move,
}

impl Tool {
    /// Every tool, in rail order.
    pub const ALL: [Tool; 8] =
        [Tool::SelectRect, Tool::SelectEllipse, Tool::SelectLasso, Tool::Wand, Tool::Brush, Tool::Eraser, Tool::Fill, Tool::Move];

    /// The tool's word (the button verb is `tool-<word>`, the witness names it).
    pub fn word(self) -> &'static str {
        match self {
            Tool::SelectRect => "select-rect",
            Tool::SelectEllipse => "select-ellipse",
            Tool::SelectLasso => "select-lasso",
            Tool::Wand => "wand",
            Tool::Brush => "brush",
            Tool::Eraser => "eraser",
            Tool::Fill => "fill",
            Tool::Move => "move",
        }
    }

    /// The rail button's label.
    pub fn label(self) -> &'static str {
        match self {
            Tool::SelectRect => "Rect",
            Tool::SelectEllipse => "Ellipse",
            Tool::SelectLasso => "Lasso",
            Tool::Wand => "Wand",
            Tool::Brush => "Brush",
            Tool::Eraser => "Eraser",
            Tool::Fill => "Fill",
            Tool::Move => "Move",
        }
    }

    pub fn from_word(w: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.word() == w)
    }
}

/// The brush fields (tools/raster's `brush` verb's arguments, less the layer and points).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushFields {
    pub diameter: f32,
    pub hardness: u8,
    pub spacing: f32,
    pub flow: u8,
}

impl Default for BrushFields {
    /// `brush_core::Brush::new`'s defaults at a 9 px tip.
    fn default() -> Self {
        BrushFields { diameter: 9.0, hardness: 255, spacing: 0.25, flow: 255 }
    }
}

/// The adjust menu's fields: one value set per adjust_core function (tools/raster's `adjust` arguments).
#[derive(Debug, Clone)]
pub struct AdjustFields {
    pub levels: adjust_core::Levels,
    pub curves: Vec<(u8, u8)>,
    pub huesat: adjust_core::HueSat,
    pub threshold: u8,
    pub boxblur: u16,
    pub gaussian: u16,
    pub unsharp: adjust_core::Unsharp,
}

impl Default for AdjustFields {
    fn default() -> Self {
        AdjustFields {
            levels: adjust_core::Levels { in_lo: 0, in_hi: 255, gamma_q8: 256, out_lo: 0, out_hi: 255 },
            curves: Vec::new(),
            huesat: adjust_core::HueSat { hue_deg: 0, sat_pct: 0, light_pct: 0 },
            threshold: 128,
            boxblur: 1,
            gaussian: 256,
            unsharp: adjust_core::Unsharp { amount_pct: 100, sigma_q8: 256, threshold: 0 },
        }
    }
}

/// The eight adjust kinds, in menu order (their words are tools/raster's).
pub const ADJUSTS: [(&str, &str); 8] = [
    ("levels", "Levels"),
    ("curves", "Curves"),
    ("huesat", "Hue/Sat"),
    ("invert", "Invert"),
    ("threshold", "Threshold"),
    ("boxblur", "Box blur"),
    ("gaussian", "Gaussian"),
    ("unsharp", "Unsharp"),
];

/// The top row's verbs after the tools: `(verb, label)`.
pub const VIEW_VERBS: [(&str, &str); 4] = [("zoom-", "-"), ("zoom+", "+"), ("undo", "Undo"), ("redo", "Redo")];

/// The bottom row's layer and file verbs: `(verb, label)`; the adjust verbs (`adjust-<kind>`) sit between them.
pub const LAYER_VERBS: [(&str, &str); 6] =
    [("layer-add", "Layer +"), ("layer-remove", "Layer -"), ("layer-up", "Up"), ("layer-down", "Down"), ("layer-blend", "Blend"), ("layer-eye", "Eye")];
pub const FILE_VERBS: [(&str, &str); 2] = [("save", "Save"), ("export", "Export")];

/// Is `v` a verb some button fires?
pub fn is_verb(v: &str) -> bool {
    v.strip_prefix("tool-").is_some_and(|t| Tool::from_word(t).is_some())
        || v.strip_prefix("adjust-").is_some_and(|k| ADJUSTS.iter().any(|(w, _)| *w == k))
        || VIEW_VERBS.iter().chain(LAYER_VERBS.iter()).chain(FILE_VERBS.iter()).any(|(w, _)| *w == v)
}

/// `rrggbbaa` -> RGBA (tools/raster's colour form).
pub fn parse_rgba(s: &str) -> Option<[u8; 4]> {
    if s.len() != 8 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 4];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// A blend mode's word (tools/raster's words, `BlendMode::ALL` order).
pub const BLEND_WORDS: [&str; 8] = ["normal", "multiply", "screen", "overlay", "darken", "lighten", "difference", "erase"];

pub fn blend_word(b: raster_core::BlendMode) -> &'static str {
    BLEND_WORDS[b.to_byte() as usize]
}

pub fn parse_blend(s: &str) -> Option<raster_core::BlendMode> {
    BLEND_WORDS.iter().position(|w| *w == s).and_then(|i| raster_core::BlendMode::from_byte(i as u8))
}
