#![cfg_attr(not(feature = "std"), no_std)]

pub const MOONSTONE_PURPLE: u32 = 0x2C003E;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Quit,
    Timer,
    Key(u8),
    Mouse { x: i32, y: i32 },
    None,
    Unknown,
}

// THE KERNEL INTERFACE
pub trait GneissPal {
    fn draw_pixel(&mut self, x: u32, y: u32, color: u32);
    fn poll_event(&mut self) -> Event;
    fn render(&mut self);

    // NEW: High-level helpers required by Console/Vug
    fn width(&self) -> u32;
    fn height(&self) -> u32;

    fn clear_screen(&mut self, color: u32) {
        for y in 0..self.height() {
            for x in 0..self.width() {
                self.draw_pixel(x, y, color);
            }
        }
    }

    fn draw_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        for row in 0..h {
            for col in 0..w {
                self.draw_pixel((x + col) as u32, (y + row) as u32, color);
            }
        }
    }

    // Placeholder for text - real implementation requires a font map
    fn draw_text(&mut self, _x: usize, _y: usize, _text: &str, _color: u32) {
        // No-op for now to satisfy the compiler
    }
}

// --- USERSPACE EXPORTS ---
#[cfg(feature = "std")]
pub mod backend;

#[cfg(feature = "std")]
pub use crate::backend::{KeyCode, WaylandApp, WindowEvent};

#[cfg(feature = "std")]
pub use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
