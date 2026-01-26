use gneiss_pal::{App as CoreApp, Platform, Plugin};
use std::pin::Pin;
use cxx_qt::CxxQtType;
use cxx_qt_lib::{QString, QGuiApplication};

#[cxx_qt::bridge]
mod bridge {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        type AppBridge = super::AppBridgeRust;

        #[qinvokable]
        fn init(self: Pin<&mut AppBridge>);
    }
}

pub struct AppBridgeRust {
    core: Option<CoreApp>,
}

impl Default for AppBridgeRust {
    fn default() -> Self {
        Self {
            core: Some(CoreApp::new()),
        }
    }
}

// Implement Platform for the Bridge?
// The bridge wrapper (AppBridge) is the QObject. The Rust struct (AppBridgeRust) is the data.
// We can implement Platform for AppBridgeRust if we want, but it can't easily access Qt methods directly
// without a pointer back to the C++ side or QObject.
// For this skeleton, we will implement a mock Platform or just print.

struct QtPlatform;
impl Platform for QtPlatform {
    fn set_title(&self, title: &str) {
        println!("Qt Platform set_title: {}", title);
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

struct HelloPlugin;
impl Plugin for HelloPlugin {
    fn on_init(&mut self, platform: &dyn Platform) {
        platform.set_title("Hello from Qt Skeleton");
        println!("Qt Plugin Initialized!");
    }
    fn on_update(&mut self, _platform: &dyn Platform) {}
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

impl bridge::AppBridge {
    pub fn init(self: Pin<&mut Self>) {
         println!("AppBridge Init called");

         let mut rust = self.rust_mut();
         if let Some(core) = &mut rust.core {
             core.register_plugin(HelloPlugin);

             let platform = QtPlatform;
             core.init(&platform);
         }
    }
}
