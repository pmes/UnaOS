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

use cxx_qt_lib::QString;
use async_channel::Sender;
use gneiss_pal::{Event, GuiUpdate};

// Wrap the sender and receiver so we can hold them in the QObject
pub struct Channels {
    pub tx: Sender<Event>,
}

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    // Wrap the HistoryItem to expose to QML
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, sender)]
        #[qproperty(QString, content)]
        #[qproperty(QString, timestamp)]
        #[qproperty(bool, is_chat)]
        type HistoryItemQml = super::HistoryItemQmlRust;
    }

    // Wrap the PreFlightPayload to expose to QML
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, system)]
        #[qproperty(QString, directives)]
        #[qproperty(QString, engrams)]
        #[qproperty(QString, prompt)]
        type PreFlightPayloadQml = super::PreFlightPayloadQmlRust;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[cxx_name = "LumenApp"]
        #[qproperty(QString, current_input)]
        type LumenApp = super::LumenAppRust;

        #[qinvokable]
        fn send_message(self: Pin<&mut LumenApp>, text: QString);

        #[qinvokable]
        fn request_history(self: Pin<&mut LumenApp>);

        #[qinvokable]
        fn dispatch_payload(self: Pin<&mut LumenApp>, text: QString);

        #[qinvokable]
        fn register_thread(self: Pin<&mut LumenApp>);
    }

    impl cxx_qt::Threading for LumenApp {}
}

// Rust structs backing the QObjects
#[derive(Default)]
pub struct HistoryItemQmlRust {
    pub sender: QString,
    pub content: QString,
    pub timestamp: QString,
    pub is_chat: bool,
}

#[derive(Default)]
pub struct PreFlightPayloadQmlRust {
    pub system: QString,
    pub directives: QString,
    pub engrams: QString,
    pub prompt: QString,
}

// Global channel hooks since QML instantiates the object
use std::sync::OnceLock;

pub static GLOBAL_TX: OnceLock<Sender<Event>> = OnceLock::new();
pub static GLOBAL_QT_THREAD: OnceLock<cxx_qt::CxxQtThread<qobject::LumenApp>> = OnceLock::new();

pub struct LumenAppRust {
    pub current_input: QString,
}

impl Default for LumenAppRust {
    fn default() -> Self {
        Self {
            current_input: QString::from(""),
        }
    }
}

// Background Task Spawner
// Takes ownership of the thread queue mechanism, listening asynchronously for GuiUpdates.
// Converts them safely into Qt loop closures.
pub fn spawn_gui_listener(
    rx: async_channel::Receiver<GuiUpdate>,
    qt_thread: cxx_qt::CxxQtThread<qobject::LumenApp>,
) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            while let Ok(update) = rx.recv().await {
                match update {
                    GuiUpdate::HistoryBatch(_items) => {
                        let qt_thread = qt_thread.clone();
                        qt_thread.queue(move |_qobj| {
                            // Note: To mutate, would use _qobj
                        }).unwrap();
                    }
                    GuiUpdate::ReviewPayload(_payload) => {
                         let qt_thread = qt_thread.clone();
                         qt_thread.queue(move |_qobj| {
                             // Note: To mutate, would use _qobj
                         }).unwrap();
                    }
                    _ => {}
                }
            }
        });
    }
}

// In cxx-qt 0.8, we can implement qobject::LumenApp methods
impl qobject::LumenApp {
    pub fn register_thread(self: std::pin::Pin<&mut Self>) {
        use cxx_qt::Threading;
        let _ = GLOBAL_QT_THREAD.set(self.qt_thread());
    }
}

impl qobject::LumenApp {
    pub fn send_message(self: std::pin::Pin<&mut Self>, text: QString) {
        if let Some(tx) = GLOBAL_TX.get() {
            let event = Event::Input {
                target: "chat".to_string(),
                text: text.to_string(),
            };
            let _ = tx.try_send(event);
        }
    }

    pub fn request_history(self: std::pin::Pin<&mut Self>) {
        if let Some(tx) = GLOBAL_TX.get() {
            let _ = tx.try_send(Event::LoadHistory);
        }
    }

    pub fn dispatch_payload(self: std::pin::Pin<&mut Self>, text: QString) {
         if let Some(tx) = GLOBAL_TX.get() {
             let _ = tx.try_send(Event::DispatchPayload(text.to_string()));
         }
    }
}
