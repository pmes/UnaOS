#[cxx_qt::bridge]
pub mod qml_bridge {
    unsafe extern "C++" {
        include!("aether-shell/src/cxxqt_hacks.h");
        type AetherViewBase;
    }
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = AetherViewBase]
        type AetherView = super::AetherViewRust;
        
        #[qinvokable]
        fn handle_mouse_press(self: Pin<&mut AetherView>, x: f64, y: f64);
        #[qinvokable]
        fn handle_key_press(self: Pin<&mut AetherView>, key: String);
        #[qinvokable]
        fn handle_wheel(self: Pin<&mut AetherView>, dx: f64, dy: f64);
        #[qinvokable]
        fn handle_text(self: Pin<&mut AetherView>, text: String);
    }
}
use std::pin::Pin;
#[derive(Default)]
pub struct AetherViewRust;

impl qml_bridge::AetherView {
    pub fn handle_mouse_press(self: Pin<&mut Self>, x: f64, y: f64) {}
    pub fn handle_key_press(self: Pin<&mut Self>, key: String) {}
    pub fn handle_wheel(self: Pin<&mut Self>, dx: f64, dy: f64) {}
    pub fn handle_text(self: Pin<&mut Self>, text: String) {}
}

pub fn run() {
    println!("Qt shell started");
}
