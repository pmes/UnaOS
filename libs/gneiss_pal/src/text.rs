use ab_glyph::{Font, FontRef, PxScale, point, ScaleFont};

// Embed the font data
const FONT_DATA: &[u8] = include_bytes!("../assets/Hack-Regular.ttf");

pub fn draw_text(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    text: &str,
    start_x: i32,
    start_y: i32,
    color: u32,
) {
    let font = FontRef::try_from_slice(FONT_DATA).expect("Error loading embedded font");
    let scale = PxScale { x: 24.0, y: 24.0 }; // 24px font size
    let mut caret = point(start_x as f32, start_y as f32);

    for c in text.chars() {
        if c == '\n' {
            caret.x = start_x as f32;
            caret.y += scale.y * 1.2;
            continue;
        }

        if c.is_control() {
            continue;
        }

        let glyph_id = font.glyph_id(c);
        let glyph = glyph_id.with_scale_and_position(scale, caret);

        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();

            outlined.draw(|x, y, v| {
                // v is coverage [0.0, 1.0]
                if v > 0.5 {
                    let px = x + bounds.min.x as u32;
                    let py = y + bounds.min.y as u32;

                    if px < width && py < height {
                        let idx = (py * width + px) as usize;
                        if idx < buffer.len() {
                            buffer[idx] = color;
                        }
                    }
                }
            });
        }

        caret.x += font.as_scaled(scale).h_advance(glyph_id);
    }
}
