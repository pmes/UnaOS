//! The zoom ladder (std-free): Chromium's preset page-zoom levels (`zoom::kPresetZoomFactors`), which
//! Ctrl+plus / Ctrl+minus step along and Ctrl+0 resets to 100 %.

/// The preset levels, in percent.
pub const LEVELS: [u32; 17] = [25, 33, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500];

/// One step from `pct`: `dir > 0` the next level up, `dir < 0` the next down, `0` back to 100. A
/// percentage between two levels steps to the neighbouring level in that direction; the ends stick.
pub fn step(pct: u32, dir: i32) -> u32 {
    match dir {
        0 => 100,
        d if d > 0 => LEVELS.iter().copied().find(|&l| l > pct).unwrap_or(LEVELS[LEVELS.len() - 1]),
        _ => LEVELS.iter().rev().copied().find(|&l| l < pct).unwrap_or(LEVELS[0]),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_ladder_steps_and_sticks_at_its_ends() {
        assert_eq!(super::step(100, 1), 110);
        assert_eq!(super::step(100, -1), 90);
        assert_eq!(super::step(500, 1), 500);
        assert_eq!(super::step(25, -1), 25);
        assert_eq!(super::step(105, -1), 100);
        assert_eq!(super::step(250, 0), 100);
    }
}
