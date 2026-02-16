use ab_glyph::{Font, FontRef, PxScale, point, ScaleFont};

// Embed the font data
const FONT_DATA: &[u8] = include_bytes!("../assets/Hack-Regular.ttf");

pub fn get_font() -> FontRef<'static> {
    FontRef::try_from_slice(FONT_DATA).expect("Error loading embedded font")
}

pub fn measure_text_height(width: u32, text: &str, font: &FontRef) -> i32 {
    let scale = PxScale { x: 24.0, y: 24.0 };
    let line_height = scale.y * 1.2;
    let start_x = 50;
    let max_x = width as f32;

    let mut caret = point(start_x as f32, 0.0);
    let scaled_font = font.as_scaled(scale);

    for (p_idx, paragraph) in text.split('\n').enumerate() {
        if p_idx > 0 {
            caret.x = start_x as f32;
            caret.y += line_height;
        }

        let words = paragraph.split(' ');
        for (i, word) in words.enumerate() {
            let prefix = if i > 0 { " " } else { "" };
            let full_word = format!("{}{}", prefix, word);
            let word_width = width_of_text(font, scale, &full_word);

            if caret.x + word_width <= max_x {
                 caret.x += word_width;
            } else if start_x as f32 + word_width <= max_x {
                 caret.x = start_x as f32 + word_width;
                 caret.y += line_height;
            } else {
                 if caret.x > start_x as f32 {
                     caret.x = start_x as f32;
                     caret.y += line_height;
                 }
                 let text_to_draw = if i > 0 && caret.x == start_x as f32 { word } else { &full_word };
                 for c in text_to_draw.chars() {
                     let glyph_id = font.glyph_id(c);
                     let advance = scaled_font.h_advance(glyph_id);
                     if caret.x + advance > max_x {
                         caret.x = start_x as f32;
                         caret.y += line_height;
                     }
                     caret.x += advance;
                 }
            }
        }
    }

    (caret.y + line_height) as i32
}

pub fn draw_text(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    text: &str,
    start_x: i32,
    start_y: i32,
    color: u32,
    font: &FontRef,
) -> i32 {
    let scale = PxScale { x: 24.0, y: 24.0 }; // 24px font size
    let scaled_font = font.as_scaled(scale);
    let line_height = scale.y * 1.2;

    let mut caret = point(start_x as f32, start_y as f32);
    let max_x = width as f32;

    for (p_idx, paragraph) in text.split('\n').enumerate() {
        if p_idx > 0 {
            caret.x = start_x as f32;
            caret.y += line_height;
        }

        let words = paragraph.split(' ');
        for (i, word) in words.enumerate() {
            let prefix = if i > 0 { " " } else { "" };
            let full_word = format!("{}{}", prefix, word);
            let word_width = width_of_text(font, scale, &full_word);

            // Case 1: Word fits
            if caret.x + word_width <= max_x {
                draw_str(buffer, width, height, font, scale, &mut caret, &full_word, color);
            }
            // Case 2: Fits on new line
            else if start_x as f32 + word_width <= max_x {
                caret.x = start_x as f32;
                caret.y += line_height;
                let trimmed = if i > 0 { word } else { &full_word };
                draw_str(buffer, width, height, font, scale, &mut caret, trimmed, color);
            }
            // Case 3: Giant word
            else {
                 if caret.x > start_x as f32 {
                     caret.x = start_x as f32;
                     caret.y += line_height;
                 }
                 let text_to_draw = if i > 0 && caret.x == start_x as f32 { word } else { &full_word };
                 for c in text_to_draw.chars() {
                     let glyph_id = font.glyph_id(c);
                     let advance = scaled_font.h_advance(glyph_id);
                     if caret.x + advance > max_x {
                         caret.x = start_x as f32;
                         caret.y += line_height;
                     }
                     draw_glyph(buffer, width, height, font, scale, caret, c, color);
                     caret.x += advance;
                 }
            }
        }
    }

    (caret.y + line_height) as i32
}

fn width_of_text(font: &FontRef, scale: PxScale, text: &str) -> f32 {
    let scaled_font = font.as_scaled(scale);
    let mut w = 0.0;
    for c in text.chars() {
        w += scaled_font.h_advance(font.glyph_id(c));
    }
    w
}

fn draw_str(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    font: &FontRef,
    scale: PxScale,
    caret: &mut ab_glyph::Point,
    text: &str,
    color: u32,
) {
    let scaled_font = font.as_scaled(scale);
    for c in text.chars() {
        draw_glyph(buffer, width, height, font, scale, *caret, c, color);
        caret.x += scaled_font.h_advance(font.glyph_id(c));
    }
}

fn draw_glyph(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    font: &FontRef,
    scale: PxScale,
    caret: ab_glyph::Point,
    c: char,
    color: u32,
) {
    let glyph_id = font.glyph_id(c);
    let glyph = glyph_id.with_scale_and_position(scale, caret);

    if let Some(outlined) = font.outline_glyph(glyph) {
        let bounds = outlined.px_bounds();
        outlined.draw(|x, y, v| {
            if v > 0.5 {
                // Calculate position in signed integers first to check bounds
                let px = x as i32 + bounds.min.x as i32;
                let py = y as i32 + bounds.min.y as i32;

                if px >= 0 && px < width as i32 && py >= 0 && py < height as i32 {
                    let idx = (py * width as i32 + px) as usize;
                    if idx < buffer.len() {
                        buffer[idx] = color;
                    }
                }
            }
        });
    }
}
