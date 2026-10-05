use font_kit::family_name::FamilyName;
use font_kit::properties::Properties;
use font_kit::source::SystemSource;
use font_kit::font::Font;
use std::sync::Arc;

pub mod lines;
use taffy::prelude::*;

/// Cache key of one face: family class (0 sans, 1 serif, 2 mono) x bold x italic.
pub fn face_key(family: u8, bold: bool, italic: bool) -> u8 {
    family.min(2) * 4 + (bold as u8) * 2 + italic as u8
}

/// THE face for a text run — the text measurer (layout::remeasure) and the
/// painter (render::draw_node) both call this, so a run is wrapped with
/// exactly the advances it is drawn with. Loaded once per thread per face;
/// a missing bold/italic face falls back to the family's regular face, a
/// missing family to sans.
pub fn face(family: u8, bold: bool, italic: bool) -> Option<Arc<Font>> {
    use font_kit::properties::{Style, Weight};
    thread_local! {
        static FACES: std::cell::RefCell<[Option<Option<Arc<Font>>>; 12]> =
            const { std::cell::RefCell::new([const { None }; 12]) };
    }
    let key = face_key(family, bold, italic) as usize;
    if let Some(f) = FACES.with(|f| f.borrow()[key].clone()) {
        return f;
    }
    // Generic families resolve the way browsers resolve them: web content is
    // authored against the Times New Roman / Arial metrics, so those faces
    // (or their metric-compatible Liberation clones) come first, then the
    // system's own generic. Monospace keeps the system generic — Chromium's
    // default fixed font on Linux is "Monospace" too.
    let named = |names: &[&str]| -> Vec<FamilyName> {
        names.iter().map(|n| FamilyName::Title(n.to_string())).collect()
    };
    let (mut names, generic) = match family.min(2) {
        1 => (named(&["Times New Roman", "Liberation Serif", "Tinos"]), FamilyName::Serif),
        2 => (Vec::new(), FamilyName::Monospace),
        _ => (named(&["Arial", "Liberation Sans", "Arimo", "Helvetica"]), FamilyName::SansSerif),
    };
    names.push(generic);
    let name = names;
    let mut props = Properties::new();
    if bold {
        props.weight(Weight::BOLD);
    }
    if italic {
        props.style(Style::Italic);
    }
    let engine = FontEngine::new();
    let loaded = engine
        .load_font(&name, &props)
        .or_else(|| engine.load_font(&name, &Properties::new()))
        .or_else(|| engine.load_font(&[FamilyName::SansSerif], &Properties::new()));
    FACES.with(|f| f.borrow_mut()[key] = Some(loaded.clone()));
    loaded
}

/// Pixel line metrics of `font` at `size`: (ascent, descent, line gap),
/// each rounded to whole pixels the way Chromium's SimpleFontData rounds
/// them — so `line-height: normal` is ascent + descent + gap in integers
/// (Arial/Liberation Sans 16px: 14 + 3 + 1 = 18, not 18.4).
pub fn line_metrics(font: &Font, size: f32) -> (f32, f32, f32) {
    let m = font.metrics();
    let scale = size / m.units_per_em as f32;
    ((m.ascent * scale).round(), (-m.descent * scale).round(), (m.line_gap * scale).round())
}

/// The used line height: `mult` x size, `-mult` px when negative (a
/// length), or (0 = `normal`) the rounded metrics sum.
pub fn line_height(font: &Font, size: f32, mult: f32) -> f32 {
    if mult < 0.0 {
        -mult // a <length>: absolute px (layout::PaintStyle::line_height)
    } else if mult > 0.0 {
        size * mult
    } else {
        let (a, d, g) = line_metrics(font, size);
        a + d + g
    }
}

/// Offset from a line box's top to its baseline: the half-leading model
/// of CSS 2.2 §10.8.1 — the glyph area (A + D) is centred in the line
/// box, so (line-height - (A + D)) / 2 sits above the ascent.
pub fn baseline_offset(font: &Font, size: f32, mult: f32) -> f32 {
    let (a, d, _) = line_metrics(font, size);
    let lh = line_height(font, size, mult);
    ((lh - (a + d)) / 2.0).floor() + a
}

/// Advance of the U+0020 space in px (word spacing as the font draws it).
pub fn space_advance(font: &Font, size: f32) -> f32 {
    let scale = size / font.metrics().units_per_em as f32;
    font.glyph_for_char(' ')
        .and_then(|g| font.advance(g).ok())
        .map(|a| a.x() * scale)
        .filter(|w| *w > 0.0)
        .unwrap_or(size * 0.25)
}

pub struct FontEngine {
    source: SystemSource,
}

impl Default for FontEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl FontEngine {
    pub fn new() -> Self {
        Self {
            source: SystemSource::new(),
        }
    }

    pub fn load_font(&self, family: &[FamilyName], properties: &Properties) -> Option<Arc<Font>> {
        if let Ok(handle) = self.source.select_best_match(family, properties) {
            if let Ok(font) = handle.load() {
                return Some(Arc::new(font));
            }
        }
        None
    }

    pub fn create_measure_function(
        &self,
        font: Arc<Font>,
        text: String,
        font_size: f32,
    ) -> impl Fn(Size<Option<f32>>, Size<AvailableSpace>) -> Size<f32> + 'static {
        move |known_dimensions, _available_space| {
            if let (Some(width), Some(height)) = (known_dimensions.width, known_dimensions.height) {
                return Size { width, height };
            }

            let metrics = font.metrics();
            let units_per_em = metrics.units_per_em as f32;
            let scale = font_size / units_per_em;
            let line_height = (metrics.ascent - metrics.descent + metrics.line_gap) * scale;
            
            let mut width = 0.0;
            for c in text.chars() {
                if let Some(glyph_id) = font.glyph_for_char(c) {
                    if let Ok(advance) = font.advance(glyph_id) {
                        width += advance.x() * scale;
                    }
                }
            }

            let final_width = known_dimensions.width.unwrap_or(width);
            let final_height = known_dimensions.height.unwrap_or(line_height);

            Size {
                width: final_width,
                height: final_height,
            }
        }
    }
}
