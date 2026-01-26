use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use winit::event::{ElementState, Event, WindowEvent as WinitWindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowBuilder;
use crate::{AppHandler, Event as PalEvent, KeyCode as PalKeyCode};

// --- EXPORTS FOR LIB.RS ---
// We alias winit types so lib.rs doesn't break
pub use winit::event::WindowEvent;

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

    pub fn run_window(mut self, mut handler: impl AppHandler) -> Result<(), String> {
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

                            // Delegate drawing to the handler
                            handler.draw(&mut buffer, width, height);

                            buffer.present().unwrap();
                        }
                    }
                    Event::AboutToWait => {
                        handler.handle_event(PalEvent::Timer);
                        window.request_redraw();
                    }
                    Event::WindowEvent {
                        event: WinitWindowEvent::CloseRequested,
                        ..
                    } => {
                        elwt.exit();
                    }
                    Event::WindowEvent {
                        event: WinitWindowEvent::KeyboardInput { event: key_event, .. },
                        ..
                    } => {
                        if key_event.state == ElementState::Pressed {
                            match key_event.logical_key {
                                Key::Named(NamedKey::Enter) => {
                                    handler.handle_event(PalEvent::KeyDown(PalKeyCode::Enter));
                                }
                                Key::Named(NamedKey::Backspace) => {
                                    handler.handle_event(PalEvent::KeyDown(PalKeyCode::Backspace));
                                }
                                _ => {}
                            }

                            if let Some(text) = key_event.text {
                                for c in text.chars() {
                                    if !c.is_control() {
                                        handler.handle_event(PalEvent::Char(c));
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            })
            .map_err(|e| e.to_string())
    }
}
