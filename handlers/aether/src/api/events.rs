/// Input events the shell delivers to the engine (`AetherEngine::handle_event`).
pub enum Event {
    MouseMove(f64, f64),
    MouseDown(f64, f64),
    MouseUp(f64, f64),
    Scroll(f64, f64),
    KeyDown(String),
    Text(String),
    Resize(u32, u32),
}
