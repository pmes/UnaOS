#![cfg(target_os = "macos")]
use winit::{
    application::ApplicationHandler,
    event::{WindowEvent, KeyEvent, MouseScrollDelta, ElementState, MouseButton, Ime},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};
use softbuffer::{Context, Surface};
use std::rc::Rc;
use std::cell::RefCell;
use aether::AetherEngine;
use tokio::runtime::Runtime;

struct AetherApp {
    window: Option<Rc<Window>>,
    surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    engine: Rc<RefCell<AetherEngine>>,
    rt: Rc<Runtime>,
    cursor_x: f64,
    cursor_y: f64,
    url_bar_active: bool,
    current_url: String,
}

impl ApplicationHandler for AetherApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let window = Rc::new(event_loop.create_window(Window::default_attributes().with_title("Aether Browser").with_inner_size(winit::dpi::LogicalSize::new(800.0, 600.0))).unwrap());
            self.window = Some(window.clone());
            let context = Context::new(window.clone()).unwrap();
            let surface = Surface::new(&context, window.clone()).unwrap();
            self.surface = Some(surface);
            
            // Set IME allowed to receive Ime events
            window.set_ime_allowed(true);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let window = self.window.as_ref().unwrap();
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(surface) = &mut self.surface {
                    let _ = surface.resize(
                        core::num::NonZeroU32::new(size.width.max(1)).unwrap(),
                        core::num::NonZeroU32::new(size.height.max(1)).unwrap(),
                    );
                    let mut eng = self.engine.borrow_mut();
                    eng.handle_event(aether::api::events::Event::Resize(size.width, size.height));
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(surface) = &mut self.surface {
                    let mut eng = self.engine.borrow_mut();
                    let _damages = eng.render_frame();
                    
                    let mut buffer = surface.buffer_mut().unwrap();
                    let eng_surface = eng.surface();
                    
                    for (i, pixel) in buffer.iter_mut().enumerate() {
                        let idx = i * 4;
                        if idx + 3 < eng_surface.len() {
                            let b = eng_surface[idx] as u32;
                            let g = eng_surface[idx+1] as u32;
                            let r = eng_surface[idx+2] as u32;
                            *pixel = b | (g << 8) | (r << 16);
                        }
                    }
                    
                    // Synthetic URL Bar
                    if self.url_bar_active {
                        for y in 0..30 {
                            for x in 0..eng.width {
                                let idx = (y * eng.width + x) as usize;
                                if idx < buffer.len() {
                                    buffer[idx] = 0xDDDDDD; // Gray bar
                                }
                            }
                        }
                    }
                    
                    buffer.present().unwrap();
                    
                    let title = eng.title.clone();
                    if title != "Aether Browser" {
                        window.set_title(&title);
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_x = position.x;
                self.cursor_y = position.y;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left {
                    if self.cursor_y < 30.0 {
                        if state == ElementState::Pressed {
                            self.url_bar_active = true;
                            window.request_redraw();
                        }
                    } else {
                        self.url_bar_active = false;
                        let mut eng = self.engine.borrow_mut();
                        if state == ElementState::Pressed {
                            eng.handle_event(aether::api::events::Event::MouseDown(self.cursor_x, self.cursor_y));
                        } else {
                            eng.handle_event(aether::api::events::Event::MouseUp(self.cursor_x, self.cursor_y));
                        }
                        if eng.needs_repaint {
                            window.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let mut eng = self.engine.borrow_mut();
                match delta {
                    MouseScrollDelta::LineDelta(dx, dy) => {
                        eng.handle_event(aether::api::events::Event::Scroll(dx as f64 * 40.0, dy as f64 * 40.0));
                    }
                    MouseScrollDelta::PixelDelta(pos) => {
                        eng.handle_event(aether::api::events::Event::Scroll(pos.x, pos.y));
                    }
                }
                if eng.needs_repaint {
                    window.request_redraw();
                }
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                if self.url_bar_active {
                    self.current_url.push_str(&text);
                    window.request_redraw();
                } else {
                    let mut eng = self.engine.borrow_mut();
                    eng.handle_event(aether::api::events::Event::Text(text));
                    if eng.needs_repaint {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    if let Key::Named(NamedKey::Backspace) = event.logical_key {
                        if self.url_bar_active {
                            self.current_url.pop();
                            window.request_redraw();
                        } else {
                            let mut eng = self.engine.borrow_mut();
                            eng.handle_event(aether::api::events::Event::KeyDown("BackSpace".to_string()));
                            if eng.needs_repaint {
                                window.request_redraw();
                            }
                        }
                    } else if let Key::Named(NamedKey::Enter) = event.logical_key {
                        if self.url_bar_active {
                            self.url_bar_active = false;
                            let url = self.current_url.clone();
                            let engine = self.engine.clone();
                            let handle = self.rt.handle().clone();
                            let w = window.clone();
                            
                            std::thread::spawn(move || {
                                handle.block_on(async move {
                                    let _ = engine.borrow_mut().load_url(&url).await;
                                    w.request_redraw();
                                });
                            });
                        } else {
                            let mut eng = self.engine.borrow_mut();
                            eng.handle_event(aether::api::events::Event::KeyDown("Return".to_string()));
                            if eng.needs_repaint {
                                window.request_redraw();
                            }
                        }
                    } else if let Key::Character(c) = event.logical_key {
                        if c == "[" {
                            let mut eng = self.engine.borrow_mut();
                            let handle = self.rt.handle().clone();
                            let engine = self.engine.clone();
                            let w = window.clone();
                            std::thread::spawn(move || {
                                handle.block_on(async move {
                                    engine.borrow_mut().go_back().await;
                                    w.request_redraw();
                                });
                            });
                        } else if c == "]" {
                            let mut eng = self.engine.borrow_mut();
                            let handle = self.rt.handle().clone();
                            let engine = self.engine.clone();
                            let w = window.clone();
                            std::thread::spawn(move || {
                                handle.block_on(async move {
                                    engine.borrow_mut().go_forward().await;
                                    w.request_redraw();
                                });
                            });
                        }
                    }
                }
            }
            _ => (),
        }
    }
}

pub fn run() {
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Wait);
    
    let mut app = AetherApp {
        window: None,
        surface: None,
        engine: Rc::new(RefCell::new(AetherEngine::new())),
        rt: Rc::new(Runtime::new().unwrap()),
        cursor_x: 0.0,
        cursor_y: 0.0,
        url_bar_active: false,
        current_url: String::new(),
    };
    
    event_loop.run_app(&mut app).unwrap();
}
