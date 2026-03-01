#![cfg(feature = "qt")]

//! Qt Native Embassy (*nix alternative)
//!
//! STUB: Awaiting future expansion.
//! This module will bridge UnaOS to the Qt ecosystem, providing a
//! high-performance alternative to GTK on Linux and BSD hosts.

use crate::{NativeView, NativeWindow};

pub struct Backend;

impl Backend {
    pub fn new<F>(_app_id: &str, _bootstrap_fn: F) -> Self
    where
        F: FnOnce(&NativeWindow) -> NativeView + 'static,
    {
        // TODO: Initialize QApplication.
        Self {}
    }

    pub fn run(&self) {
        // TODO: Engage the Qt event loop.
    }
}
