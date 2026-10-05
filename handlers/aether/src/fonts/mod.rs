use font_kit::family_name::FamilyName;
use font_kit::properties::Properties;
use font_kit::source::SystemSource;
use font_kit::font::Font;
use std::sync::Arc;
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
