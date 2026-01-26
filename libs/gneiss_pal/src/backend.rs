use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use winit::event::{Event, WindowEvent as WinitWindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::WindowBuilder;

// --- EXPORTS FOR LIB.RS ---
// We alias winit types so lib.rs doesn't break
pub use winit::event::WindowEvent;
pub use winit::keyboard::KeyCode;

pub struct WaylandApp {
    event_loop: Option<EventLoop<()>>,
}

impl WaylandApp {
    pub fn new() -> Result<Self, String> {
        let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
        Ok(Self {
            event_loop: Some(event_loop),
        })
    }

    pub fn run_window(mut self) -> Result<(), String> {
        let event_loop = self
            .event_loop
            .take()
            .ok_or("Event loop already consumed")?;

        let window = Rc::new(
            WindowBuilder::new()
                .with_title("UnaOS :: Vein")
                .with_inner_size(winit::dpi::LogicalSize::new(800.0, 600.0))
                .build(&event_loop)
                .map_err(|e| e.to_string())?,
        );

        let context = Context::new(window.clone()).map_err(|e| e.to_string())?;
        let mut surface = Surface::new(&context, window.clone()).map_err(|e| e.to_string())?;

        event_loop
            .run(move |event, elwt| {
                elwt.set_control_flow(ControlFlow::Wait);

                match event {
                    Event::WindowEvent {
                        window_id,
                        event: WinitWindowEvent::RedrawRequested,
                    } if window_id == window.id() => {
                        // FIX: inner_size() returns PhysicalSize, not Option
                        let size = window.inner_size();
                        let width = size.width;
                        let height = size.height;

                        if let (Some(w), Some(h)) =
                            (NonZeroU32::new(width), NonZeroU32::new(height))
                        {
                            surface.resize(w, h).unwrap();
                            let mut buffer = surface.buffer_mut().unwrap();

                            // FILL BACKGROUND (Dark Grey)
                            for index in 0..(width * height) {
                                buffer[index as usize] = 0x1a1a1a;
                            }

                            // DRAW A "CRYSTAL" (Blue Box)
                            let cx = width / 2;
                            let cy = height / 2;
                            // Safety check for bounds
                            for y in (cy.saturating_sub(20))..(cy.saturating_add(20)) {
                                for x in (cx.saturating_sub(20))..(cx.saturating_add(20)) {
                                    let i = y * width + x;
                                    if (i as usize) < buffer.len() {
                                        buffer[i as usize] = 0x00aaff; // Una Blue
                                    }
                                }
                            }

                            buffer.present().unwrap();
                        }
                    }
                    Event::WindowEvent {
                        event: WinitWindowEvent::CloseRequested,
                        ..
                    } => {
                        elwt.exit();
                    }
                    _ => {}
                }
            })
            .map_err(|e| e.to_string())
    }
}
