#![cfg(feature = "qt")]
// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.


//! Qt Native Embassy (*nix alternative)
//!
//! STUB: Awaiting future expansion.
//! This module will bridge UnaOS to the Qt ecosystem, providing a
//! high-performance alternative to GTK on Linux and BSD hosts.

pub mod bridge;

use crate::{NativeView, NativeWindow};
use cxx_qt_lib::QGuiApplication;

#[cxx::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("main_window.h");

        type LumenMainWindow;
        fn create_main_window() -> UniquePtr<LumenMainWindow>;
        fn show_main_window(window: Pin<&mut LumenMainWindow>);

        // Define an opaque type for QApplication since cxx_qt_lib only exposes QGuiApplication
        type LumenQApp;
        fn create_qapplication() -> UniquePtr<LumenQApp>;
        fn exec_qapplication(app: Pin<&mut LumenQApp>) -> i32;
    }
}

pub struct Backend {
    app: cxx::UniquePtr<ffi::LumenQApp>,
    main_window: cxx::UniquePtr<ffi::LumenMainWindow>,
}

impl Backend {
    pub fn new<F>(_app_id: &str, _bootstrap_fn: F) -> Self
    where
        F: FnOnce(&NativeWindow) -> NativeView + 'static,
    {
        // Safe creation of QApplication via C++ stub to ensure Widgets are supported.
        let app = ffi::create_qapplication();

        // Provide the NativeWindow and invoke bootstrap logic here.
        let window = NativeWindow { ptr: std::ptr::null_mut() };
        let view = _bootstrap_fn(&window);

        Self { app, main_window: view.ptr }
    }

    pub fn run(&mut self) {
        if !self.main_window.is_null() {
            ffi::show_main_window(self.main_window.pin_mut());
        }

        if !self.app.is_null() {
            ffi::exec_qapplication(self.app.pin_mut());
        }
    }
}
