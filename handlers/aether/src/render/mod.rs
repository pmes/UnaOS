use crate::layout::LayoutTree;
use image::{ImageBuffer, Rgba};
use bandy::signals::SMessage;

pub fn render_to_image(layout: &LayoutTree, width: u32, height: u32) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let mut img = ImageBuffer::from_pixel(width, height, Rgba([255, 255, 255, 255]));
    
    // Minimal mock rendering for M1: drawing layout boxes
    for (node_id, _) in &layout.node_map {
        if let Ok(layout_box) = layout.taffy.layout(*node_id) {
            let x_start = layout_box.location.x.max(0.0) as u32;
            let y_start = layout_box.location.y.max(0.0) as u32;
            let w = layout_box.size.width.max(0.0) as u32;
            let h = layout_box.size.height.max(0.0) as u32;
            
            let end_y = y_start.saturating_add(h);
            let end_x = x_start.saturating_add(w);
            
            for y in y_start..end_y {
                for x in x_start..end_x {
                    if x < width && y < height {
                        // Just drawing borders or a flat color for mock
                        if x == x_start || x == end_x - 1 || y == y_start || y == end_y - 1 {
                            img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
                        } else {
                            img.put_pixel(x, y, Rgba([200, 200, 200, 255]));
                        }
                    }
                }
            }
        }
    }
    
    img
}

pub fn create_surface_blit(url: &str, img: ImageBuffer<Rgba<u8>, Vec<u8>>) -> SMessage {
    let (width, height) = img.dimensions();
    SMessage::SurfaceBlit {
        url: url.to_string(),
        width,
        height,
        pixels: img.into_raw(),
    }
}
