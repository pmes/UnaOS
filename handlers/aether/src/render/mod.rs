use crate::layout::LayoutTree;
use taffy::prelude::*;

pub fn render_frame(layout: &LayoutTree, surface: &mut [u8], width: u32, height: u32, scroll_x: f64, scroll_y: f64, damage_rects: &[(u32, u32, u32, u32)]) {
    if damage_rects.is_empty() { return; }

    // Optional optimization: clear only damaged regions
    for &(dx, dy, dw, dh) in damage_rects {
        let ex = (dx + dw).min(width);
        let ey = (dy + dh).min(height);
        for y in dy..ey {
            for x in dx..ex {
                let idx = ((y * width + x) * 4) as usize;
                if idx + 3 < surface.len() {
                    surface[idx] = 255;
                    surface[idx+1] = 255;
                    surface[idx+2] = 255;
                    surface[idx+3] = 255;
                }
            }
        }
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
        damage_rects: &[(u32, u32, u32, u32)],
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
            
            // Check intersection with any damage rect
            let mut intersects = false;
            for &(dx, dy, dw, dh) in damage_rects {
                let dx2 = dx + dw;
                let dy2 = dy + dh;
                if x_start < dx2 && end_x > dx && y_start < dy2 && end_y > dy {
                    intersects = true;
                    break;
                }
            }
            
            if intersects {
                for y in y_start..end_y {
                    for x in x_start..end_x {
                        // Quick per-pixel clip
                        let mut in_damage = false;
                        for &(dx, dy, dw, dh) in damage_rects {
                            if x >= dx && x < dx + dw && y >= dy && y < dy + dh {
                                in_damage = true;
                                break;
                            }
                        }
                        if !in_damage { continue; }

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
        }

        if let Ok(children) = layout.taffy.children(node_id) {
            for child in children {
                draw_node(child, current_x, current_y, layout, surface, width, height, sx, sy, damage_rects);
            }
        }
    }

    draw_node(layout.root_node, 0.0, 0.0, layout, surface, width, height, sx, sy, damage_rects);
}
