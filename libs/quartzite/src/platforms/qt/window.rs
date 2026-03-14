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

use async_channel::{Receiver, Sender};
use std::sync::OnceLock;

use crate::{NativeView, NativeWindow};
use gneiss_pal::Event;
use bandy::SMessage;
use bandy::state::AppState;
use std::sync::{Arc, RwLock};
use tokio::runtime::Handle;

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

pub fn spawn_state_listener(app_state: Arc<RwLock<AppState>>, rx: Receiver<SMessage>) {
    if let Ok(handle) = Handle::try_current() {
        handle.spawn(async move {
            while let Ok(update) = rx.recv().await {
                if let SMessage::StateInvalidated = update {
                    // Extract exactly what we need for the specific models using the read lock
                    let (history, payload, logs) = {
                        let state = app_state.read().unwrap();
                        let hist = state.history.clone();
                        let pay = state.review_payload.clone();
                        let ls = state.console_logs.clone();
                        (hist, pay, ls)
                    };

                    super::vein_bridge::route_history_batch(history);
                    if let Some(p) = payload {
                        super::vein_bridge::route_review_payload(p);
                    }

                    // We simply pass the entire vector of logs to the router to sync
                    super::vein_bridge::route_console_batch(logs);
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
        let window = NativeWindow {
            ptr: std::ptr::null_mut(),
        };
        let view = _bootstrap_fn(&window);

        Self {
            app,
            main_window: view.ptr,
        }
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
