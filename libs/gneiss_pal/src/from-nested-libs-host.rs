#[cfg(feature = "std")]
use crate::{GneissPal, Event};
#[cfg(feature = "std")]
use minifb::{Window, WindowOptions, Key};

// We need to import Vec and other std items manually because we are in a no_std crate
// even when the std feature is enabled, unless we modify the crate-level no_std attribute.
#[cfg(feature = "std")]
extern crate std;
#[cfg(feature = "std")]
use std::vec::Vec;

#[cfg(feature = "std")]
pub struct HostPal {
    window: Window,
    buffer: Vec<u32>, // Using a vector as our "Screen"
    width: usize,
    height: usize,
}

#[cfg(feature = "std")]
impl HostPal {
    pub fn new(width: usize, height: usize) -> Self {
        let mut window = Window::new(
            "Midden [HOST MODE]",
            width,
            height,
            WindowOptions::default(),
        ).unwrap_or_else(|e| {
            panic!("{}", e);
        });

        // Limit to 60 FPS so we don't melt the CPU
        window.limit_update_rate(Some(std::time::Duration::from_micros(16600)));

        Self {
            window,
            buffer: std::vec![0; width * height],
            width,
            height,
        }
    }
}

#[cfg(feature = "std")]
impl GneissPal for HostPal {
    fn draw_pixel(&mut self, x: u32, y: u32, color: u32) {
        if (x as usize) < self.width && (y as usize) < self.height {
            self.buffer[y as usize * self.width + x as usize] = color;
        }
    }

    fn poll_event(&mut self) -> Event {
        // If the X button is hit or Escape is pressed
        if !self.window.is_open() || self.window.is_key_down(Key::Escape) {
            return Event::Quit;
        }

        // Simulating the "None" event for now (we'll add keys later)
        Event::None
    }

    fn render(&mut self) {
        // FLUSH the buffer to the screen
        self.window
            .update_with_buffer(&self.buffer, self.width, self.height)
            .unwrap();
    }
}
