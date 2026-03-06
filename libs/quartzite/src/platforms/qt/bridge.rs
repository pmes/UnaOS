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
use async_channel::{Sender, Receiver};
use gneiss_pal::{Event, GuiUpdate};
use cxx_qt::{CxxQtThread, CxxQtType};

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
        #[qproperty(QString, current_input)]
        type LumenApp = super::LumenAppRust;

        #[qinvokable]
        fn send_message(self: Pin<&mut LumenApp>, text: QString);

        #[qinvokable]
        fn request_history(self: Pin<&mut LumenApp>);

        #[qinvokable]
        fn dispatch_payload(self: Pin<&mut LumenApp>, text: QString);
    }
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

pub struct LumenAppRust {
    pub current_input: QString,
    pub tx: Option<Sender<Event>>,
}

impl Default for LumenAppRust {
    fn default() -> Self {
        Self {
            current_input: QString::from(""),
            tx: None,
        }
    }
}

impl qobject::LumenApp {
    pub fn send_message(self: std::pin::Pin<&mut Self>, text: QString) {
        if let Some(tx) = &self.rust().tx {
            let event = Event::Input {
                target: "chat".to_string(),
                text: text.to_string(),
            };
            let _ = tx.try_send(event);
        }
    }

    pub fn request_history(self: std::pin::Pin<&mut Self>) {
        if let Some(tx) = &self.rust().tx {
            let _ = tx.try_send(Event::LoadHistory);
        }
    }

    pub fn dispatch_payload(self: std::pin::Pin<&mut Self>, text: QString) {
         if let Some(tx) = &self.rust().tx {
             let _ = tx.try_send(Event::DispatchPayload(text.to_string()));
         }
    }
}

// Background Task Spawner
// Will be moved to LumenApp initialization context later
