#![cfg(target_os = "windows")]

//! Windows 11+ Native Embassy (WinUI 3 / Win32)
//!
//! STUB: Awaiting future expansion.
//! This module will handle the FFI boundary for the Windows runtime,
//! ensuring UnaOS maintains its performance characteristics on Microsoft's host.

use crate::{NativeView, NativeWindow};

pub struct Backend;

impl Backend {
    pub fn new<F>(_app_id: &str, _bootstrap_fn: F) -> Self
    where
        F: FnOnce(&NativeWindow) -> NativeView + 'static,
    {
        // TODO: Ignite the Win32/WinUI application host.
        Self {}
    }

    pub fn run(&self) {
        // TODO: Engage the Windows message loop.
    }
}
