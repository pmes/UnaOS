// FIX: Use crate:: instead of unaos_kernel::
use crate::pal::TargetPal;
use gneiss_pal::GneissPal;

const METER_SEGMENTS: usize = 10;
const SEG_HEIGHT: u32 = 10;
const SEG_SPACING: u32 = 2;
const SEG_WIDTH: u32 = 40;

pub fn draw_vug_stats(pal: &mut TargetPal, tick: u64) {
    // FIX: Removed extra parentheses
    let total_height = METER_SEGMENTS as u32 * (SEG_HEIGHT + SEG_SPACING);
    let start_x = pal.width() - SEG_WIDTH - 20;
    let start_y = pal.height() - total_height - 20;

    // Draw a specialized "VU Meter"
    for i in 0..METER_SEGMENTS {
        let active = (tick / 10) % (METER_SEGMENTS as u64);
        let color = if (i as u64) <= active {
            0x00FF00 // Green
        } else {
            0x333333 // Dim Gray
        };

        let y_pos = start_y + (i as u32 * (SEG_HEIGHT + SEG_SPACING));
        pal.draw_rect(
            start_x as usize,
            y_pos as usize,
            SEG_WIDTH as usize,
            SEG_HEIGHT as usize,
            color,
        );
    }

    // Draw "VUG" heartbeat
    if (tick / 30) % 2 == 0 {
        pal.draw_rect((pal.width() / 2) as usize - 10, 20, 20, 20, 0xFF0000);
    }
}
