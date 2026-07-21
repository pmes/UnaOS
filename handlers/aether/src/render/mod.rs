use crate::layout::LayoutTree;
use taffy::prelude::*;

pub fn render_frame(layout: &LayoutTree, surface: &mut [u8], width: u32, height: u32, scroll_x: f64, scroll_y: f64) {
    for chunk in surface.chunks_exact_mut(4) {
        chunk[0] = 255;
        chunk[1] = 255;
        chunk[2] = 255;
        chunk[3] = 255;
    }

    let sy = scroll_y as i32;
    let sx = scroll_x as i32;

    fn draw_node(
        node_id: NodeId,
        abs_x: f32,
        abs_y: f32,
        layout: &LayoutTree,
        surface: &mut [u8],
        width: u32,
        height: u32,
        sx: i32,
        sy: i32,
    ) {
        let Ok(layout_box) = layout.taffy.layout(node_id) else { return; };
        let current_x = abs_x + layout_box.location.x;
        let current_y = abs_y + layout_box.location.y;

        if let Some(dom_node) = layout.node_map.get(&node_id) {
            let mut bg_b = 200;
            let mut bg_g = 200;
            let mut bg_r = 200;
            
            if let Some(el) = dom_node.as_element() {
                if let Some(style) = el.attributes.borrow().get("style") {
                    if style.contains("background-color: red") || style.contains("background-color:red") {
                        bg_r = 255; bg_g = 0; bg_b = 0;
                    }
                }
            }

            let x_start = ((current_x as i32) - sx).max(0) as u32;
            let y_start = ((current_y as i32) - sy).max(0) as u32;
            let w = layout_box.size.width.max(0.0) as u32;
            let h = layout_box.size.height.max(0.0) as u32;
            
            let end_y = y_start.saturating_add(h).min(height);
            let end_x = x_start.saturating_add(w).min(width);
            
            for y in y_start..end_y {
                for x in x_start..end_x {
                    let idx = ((y * width + x) * 4) as usize;
                    if idx + 3 < surface.len() {
                        if x == x_start || x == end_x - 1 || y == y_start || y == end_y - 1 {
                            surface[idx] = 0;
                            surface[idx+1] = 0;
                            surface[idx+2] = 0;
                            surface[idx+3] = 255;
                        } else {
                            surface[idx] = bg_b;
                            surface[idx+1] = bg_g;
                            surface[idx+2] = bg_r;
                            surface[idx+3] = 255;
                        }
                    }
                }
            }
        }

        if let Ok(children) = layout.taffy.children(node_id) {
            for child in children {
                draw_node(child, current_x, current_y, layout, surface, width, height, sx, sy);
            }
        }
    }

    draw_node(layout.root_node, 0.0, 0.0, layout, surface, width, height, sx, sy);
}
