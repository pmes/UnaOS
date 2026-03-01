use crate::{NativeView, NativeWindow};
use gneiss_pal::{Event, GuiUpdate};

#[cfg(all(target_os = "linux", feature = "gtk"))]
use crate::platforms::gtk::spline::CommsSpline;

#[cfg(target_os = "macos")]
use crate::platforms::macos::spline::MacOSSpline;

pub struct Spline {
    #[cfg(all(target_os = "linux", feature = "gtk"))]
    inner: CommsSpline,

    #[cfg(target_os = "macos")]
    inner: MacOSSpline,
}

impl Spline {
    pub fn new() -> Self {
        #[cfg(all(target_os = "linux", feature = "gtk"))]
        return Self {
            inner: CommsSpline::new(),
        };

        #[cfg(target_os = "macos")]
        return Self {
            inner: MacOSSpline::new(),
        };

        #[cfg(not(any(all(target_os = "linux", feature = "gtk"), target_os = "macos")))]
        return Self {};
    }

    pub fn bootstrap(
        &self,
        _window: &NativeWindow,
        _tx_event: async_channel::Sender<Event>,
        _rx_gui: async_channel::Receiver<GuiUpdate>,
    ) -> NativeView {
        #[cfg(any(all(target_os = "linux", feature = "gtk"), target_os = "macos"))]
        return self.inner.bootstrap(_window, _tx_event, _rx_gui);

        #[cfg(not(any(all(target_os = "linux", feature = "gtk"), target_os = "macos")))]
        return (); // Fallback
    }
}
