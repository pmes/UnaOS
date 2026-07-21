use crate::layout::LayoutTree;
use image::{ImageBuffer, Rgba};
use bandy::signals::SMessage;

pub fn render_frame(layout: &LayoutTree, surface: &mut [u8], width: u32, height: u32) {
    // Fill background with white
    for chunk in surface.chunks_exact_mut(4) {
        chunk[0] = 255; // B
        chunk[1] = 255; // G
        chunk[2] = 255; // R
        chunk[3] = 255; // A
    }

    for (node_id, _) in &layout.node_map {
        if let Ok(layout_box) = layout.taffy.layout(*node_id) {
            let x_start = layout_box.location.x.max(0.0) as u32;
            let y_start = layout_box.location.y.max(0.0) as u32;
            let w = layout_box.size.width.max(0.0) as u32;
            let h = layout_box.size.height.max(0.0) as u32;
            
            let end_y = y_start.saturating_add(h).min(height);
            let end_x = x_start.saturating_add(w).min(width);
            
            for y in y_start..end_y {
                for x in x_start..end_x {
                    let idx = ((y * width + x) * 4) as usize;
                    if idx + 3 < surface.len() {
                        if x == x_start || x == end_x - 1 || y == y_start || y == end_y - 1 {
                            surface[idx] = 0;     // B
                            surface[idx+1] = 0;   // G
                            surface[idx+2] = 0;   // R
                            surface[idx+3] = 255; // A
                        } else {
                            surface[idx] = 200;
                            surface[idx+1] = 200;
                            surface[idx+2] = 200;
                            surface[idx+3] = 255;
                        }
                    }
                }
            }
        }
    }
}
