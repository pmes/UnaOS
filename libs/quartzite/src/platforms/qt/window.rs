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

use std::sync::OnceLock;
use async_channel::{Receiver, Sender};

use gneiss_pal::{Event, GuiUpdate};
use tokio::runtime::Handle;
use crate::{NativeView, NativeWindow};

use super::ffi;

pub struct Backend {
    app: cxx::UniquePtr<ffi::LumenQApp>,
    main_window: cxx::UniquePtr<ffi::LumenMainWindow>,
}

pub static GLOBAL_TX: OnceLock<Sender<Event>> = OnceLock::new();
pub static GLOBAL_QT_THREAD: OnceLock<cxx_qt::CxxQtThread<qobject::LumenWindow>> = OnceLock::new();

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[cxx_name = "LumenWindow"]
        type LumenWindow = super::LumenWindowRust;

        #[qinvokable]
        #[cxx_name = "registerThread"]
        fn register_thread(self: Pin<&mut LumenWindow>);
    }

    impl cxx_qt::Threading for LumenWindow {}
}

#[derive(Default)]
pub struct LumenWindowRust {}

impl qobject::LumenWindow {
    pub fn register_thread(self: std::pin::Pin<&mut Self>) {
        use cxx_qt::Threading;
        let _ = GLOBAL_QT_THREAD.set(self.qt_thread());
    }
}

pub fn spawn_gui_listener(
    rx: Receiver<GuiUpdate>,
) {
    if let Ok(handle) = Handle::try_current() {
        handle.spawn(async move {
            while let Ok(update) = rx.recv().await {
                // Route the GUI update to specific handler facades
                match update {
                    GuiUpdate::HistoryBatch(items) => {
                        super::vein_bridge::route_history_batch(items);
                    }
                    GuiUpdate::ReviewPayload(payload) => {
                        super::vein_bridge::route_review_payload(payload);
                    }
                    GuiUpdate::ConsoleLog(log) => {
                        super::vein_bridge::route_console_log(log);
                    }
                    _ => {}
                }
            }
        });
    }
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
