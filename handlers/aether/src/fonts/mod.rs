use font_kit::family_name::FamilyName;
use font_kit::properties::Properties;
use font_kit::source::SystemSource;
use font_kit::font::Font;
use std::sync::Arc;
use taffy::prelude::*;

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
