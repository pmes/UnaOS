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

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QModelIndex, QString, QVariant};
use std::sync::OnceLock;

use crate::platforms::qt::window::GLOBAL_TX;
use gneiss_pal::Event;
use bandy::state::{HistoryItem, PreFlightPayload};

pub static VEIN_THREAD: OnceLock<cxx_qt::CxxQtThread<qobject::VeinBridge>> = OnceLock::new();
pub static HISTORY_MODEL_THREAD: OnceLock<cxx_qt::CxxQtThread<qobject::HistoryModel>> =
    OnceLock::new();
pub static PREFLIGHT_THREAD: OnceLock<cxx_qt::CxxQtThread<qobject::PreFlightPayloadQml>> =
    OnceLock::new();
pub static NETWORK_LOG_MODEL_THREAD: OnceLock<cxx_qt::CxxQtThread<qobject::NetworkLogModel>> =
    OnceLock::new();

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!(<QtCore/QAbstractListModel>);
        type QAbstractListModel;

        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, system)]
        #[qproperty(QString, directives)]
        #[qproperty(QString, engrams)]
        #[qproperty(QString, prompt)]
        type PreFlightPayloadQml = super::PreFlightPayloadQmlRust;

        #[qinvokable]
        #[cxx_name = "registerThread"]
        fn register_thread(self: Pin<&mut PreFlightPayloadQml>);
    }

    impl cxx_qt::Threading for PreFlightPayloadQml {}

    unsafe extern "RustQt" {
        #[qobject]
        #[base = QAbstractListModel]
        #[qml_element]
        type HistoryModel = super::HistoryModelRust;

        #[qinvokable(cxx_override)]
        #[cxx_name = "rowCount"]
        fn row_count(self: &HistoryModel, parent: &QModelIndex) -> i32;
        #[qinvokable(cxx_override)]
        fn data(self: &HistoryModel, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_name = "registerModelThread"]
        fn register_model_thread(self: Pin<&mut HistoryModel>);

        #[inherit]
        #[cxx_name = "beginResetModel"]
        fn begin_reset_model(self: Pin<&mut HistoryModel>);

        #[inherit]
        #[cxx_name = "endResetModel"]
        fn end_reset_model(self: Pin<&mut HistoryModel>);
    }

    impl cxx_qt::Threading for HistoryModel {}

    unsafe extern "RustQt" {
        #[qobject]
        #[base = QAbstractListModel]
        #[qml_element]
        type NetworkLogModel = super::NetworkLogModelRust;

        #[qinvokable(cxx_override)]
        #[cxx_name = "rowCount"]
        fn row_count(self: &NetworkLogModel, parent: &QModelIndex) -> i32;
        #[qinvokable(cxx_override)]
        fn data(self: &NetworkLogModel, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_name = "registerModelThread"]
        fn register_model_thread(self: Pin<&mut NetworkLogModel>);

        #[qinvokable]
        #[cxx_name = "appendLog"]
        fn append_log(self: Pin<&mut NetworkLogModel>, payload: QString);

        #[inherit]
        #[cxx_name = "beginResetModel"]
        fn begin_reset_model(self: Pin<&mut NetworkLogModel>);

        #[inherit]
        #[cxx_name = "endResetModel"]
        fn end_reset_model(self: Pin<&mut NetworkLogModel>);
    }

    impl cxx_qt::Threading for NetworkLogModel {}

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[cxx_name = "VeinBridge"]
        type VeinBridge = super::VeinBridgeRust;

        #[qinvokable]
        #[cxx_name = "sendMessage"]
        fn send_message(self: Pin<&mut VeinBridge>, text: QString);

        #[qinvokable]
        #[cxx_name = "requestHistory"]
        fn request_history(self: Pin<&mut VeinBridge>);

        #[qinvokable]
        #[cxx_name = "dispatchPayload"]
        fn dispatch_payload(
            self: Pin<&mut VeinBridge>,
            system: QString,
            directives: QString,
            engrams: QString,
            prompt: QString,
        );

        #[qinvokable]
        #[cxx_name = "registerThread"]
        fn register_thread(self: Pin<&mut VeinBridge>);

        #[qinvokable]
        #[cxx_name = "requestPreFlightReview"]
        fn request_pre_flight_review(self: Pin<&mut VeinBridge>, text: QString);

        #[qinvokable]
        #[cxx_name = "cancelPreFlight"]
        fn cancel_pre_flight(self: Pin<&mut VeinBridge>);

        #[qinvokable]
        #[cxx_name = "abortPreFlight"]
        fn abort_pre_flight(self: Pin<&mut VeinBridge>);

        #[qsignal]
        #[cxx_name = "networkPayloadDispatched"]
        fn network_payload_dispatched(self: Pin<&mut VeinBridge>, payload: QString);

        #[qsignal]
        #[cxx_name = "payloadReadyForReview"]
        fn payload_ready_for_review(
            self: Pin<&mut VeinBridge>,
            system: QString,
            directives: QString,
            engrams: QString,
            prompt: QString,
        );
    }

    impl cxx_qt::Threading for VeinBridge {}
}

pub struct HistoryItemRust {
    pub sender: String,
    pub content: String,
    pub timestamp: String,
    pub is_chat: bool,
}

#[derive(Default)]
pub struct PreFlightPayloadQmlRust {
    pub system: QString,
    pub directives: QString,
    pub engrams: QString,
    pub prompt: QString,
}

impl qobject::PreFlightPayloadQml {
    pub fn register_thread(self: std::pin::Pin<&mut Self>) {
        use cxx_qt::Threading;
        let _ = PREFLIGHT_THREAD.set(self.qt_thread());
    }
}

#[derive(Default)]
pub struct VeinBridgeRust {}

impl qobject::VeinBridge {
    pub fn register_thread(self: std::pin::Pin<&mut Self>) {
        use cxx_qt::Threading;
        let _ = VEIN_THREAD.set(self.qt_thread());
    }

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

    pub fn dispatch_payload(
        mut self: std::pin::Pin<&mut Self>,
        system: QString,
        directives: QString,
        engrams: QString,
        prompt: QString,
    ) {
        // Construct the struct the kernel actually expects
        let payload = PreFlightPayload {
            system: system.to_string(),
            directives: directives.to_string(),
            engrams: engrams.to_string(),
            prompt: prompt.to_string(),
        };

        // Serialize to JSON for the core
        if let Ok(json_payload) = serde_json::to_string(&payload) {
            // Keep the formatted version for the Network Log visual
            let display_payload = format!(
                "System:\n{}\n\nDirectives:\n{}\n\nEngrams:\n{}\n\nPrompt:\n{}",
                system, directives, engrams, prompt
            );

            if let Some(tx) = GLOBAL_TX.get() {
                self.as_mut()
                    .network_payload_dispatched(QString::from(&display_payload));
                let _ = tx.try_send(Event::DispatchPayload(json_payload));
            }
        }
    }

    pub fn request_pre_flight_review(self: std::pin::Pin<&mut Self>, text: QString) {
        if let Some(tx) = GLOBAL_TX.get() {
            let event = Event::Input {
                target: "chat".to_string(),
                text: text.to_string(),
            };
            let _ = tx.try_send(event);
        }
    }

    pub fn cancel_pre_flight(self: std::pin::Pin<&mut Self>) {
        if let Some(tx) = GLOBAL_TX.get() {
            // Per the directive, fully discard Event::Input on cancel.
            // Sending ::CANCEL:: to the chat target will trigger the core to clear the state.
            let event = Event::Input {
                target: "chat".to_string(),
                text: "::CANCEL::".to_string(),
            };
            let _ = tx.try_send(event);
        }
    }

    pub fn abort_pre_flight(self: std::pin::Pin<&mut Self>) {
        if let Some(tx) = GLOBAL_TX.get() {
            let event = Event::Input {
                target: "chat".to_string(),
                text: "::CANCEL::".to_string(),
            };
            let _ = tx.try_send(event);
        }
    }
}

// QAbstractListModel implementation for HistoryModel
#[derive(Default)]
pub struct HistoryModelRust {
    pub rows: Vec<HistoryItemRust>,
}

impl qobject::HistoryModel {
    pub fn register_model_thread(self: std::pin::Pin<&mut Self>) {
        use cxx_qt::Threading;
        let _ = HISTORY_MODEL_THREAD.set(self.qt_thread());
    }

    pub fn add_items(mut self: std::pin::Pin<&mut Self>, new_items: Vec<HistoryItemRust>) {
        let count = new_items.len();
        if count == 0 {
            return;
        }
        self.as_mut().begin_reset_model();
        self.as_mut().rust_mut().rows.extend(new_items);
        self.as_mut().end_reset_model();
    }
    pub fn clear(mut self: std::pin::Pin<&mut Self>) {
        self.as_mut().begin_reset_model();
        self.as_mut().rust_mut().rows.clear();
        self.as_mut().end_reset_model();
    }

    pub fn row_count(&self, parent: &QModelIndex) -> i32 {
        if parent.is_valid() {
            0
        } else {
            self.rust().rows.len() as i32
        }
    }

    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let row = index.row();
        if row < 0 || row >= self.rust().rows.len() as i32 {
            return QVariant::default();
        }

        let item = &self.rust().rows[row as usize];
        match role {
            0 => QVariant::from(&QString::from(&item.content)), // DisplayRole
            1 => QVariant::default(), // DecorationRole (Must be icon/pixmap, leaving empty)
            2 => QVariant::from(&QString::from(&item.sender)), // EditRole
            3 => QVariant::from(&item.is_chat), // ToolTipRole
            _ => QVariant::default(),
        }
    }
}

// QAbstractListModel implementation for NetworkLogModel
#[derive(Default)]
pub struct NetworkLogModelRust {
    pub rows: Vec<String>,
}

impl qobject::NetworkLogModel {
    pub fn register_model_thread(self: std::pin::Pin<&mut Self>) {
        use cxx_qt::Threading;
        let _ = NETWORK_LOG_MODEL_THREAD.set(self.qt_thread());
    }

    pub fn append_log(mut self: std::pin::Pin<&mut Self>, payload: QString) {
        self.as_mut().begin_reset_model();
        self.as_mut().rust_mut().rows.push(payload.to_string());
        self.as_mut().end_reset_model();
    }

    pub fn row_count(&self, parent: &QModelIndex) -> i32 {
        if parent.is_valid() {
            0
        } else {
            self.rust().rows.len() as i32
        }
    }

    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let row = index.row();
        if row < 0 || row >= self.rust().rows.len() as i32 {
            return QVariant::default();
        }

        let item = &self.rust().rows[row as usize];
        match role {
            0 => QVariant::from(&QString::from(item)), // DisplayRole
            _ => QVariant::default(),
        }
    }
}

pub fn route_history_batch(items: Vec<HistoryItem>) {
    let mut rust_items: Vec<HistoryItemRust> = items
        .into_iter()
        .map(|i| HistoryItemRust {
            sender: i.sender,
            content: i.content,
            timestamp: i.timestamp,
            is_chat: i.is_chat,
        })
        .collect();

    if let Some(thread) = HISTORY_MODEL_THREAD.get() {
        let thread = thread.clone();
        thread
            .queue(move |mut qobj| {
                // FORCE VISUAL CONFIRMATION IF VAULT IS EMPTY (prevent duplicates)
                if rust_items.is_empty() && qobj.as_ref().rust().rows.is_empty() {
                    rust_items.push(HistoryItemRust {
                        sender: "system".to_string(),
                        content: ":: UNAFS VAULT EMPTY. READY FOR TELEMETRY ::".to_string(),
                        timestamp: "".to_string(),
                        is_chat: false,
                    });
                }
                qobj.as_mut().add_items(rust_items);
            })
            .unwrap();
    } else {
        eprintln!("DROPPED: QML failed to register the HistoryModel thread.");
    }
}

pub fn route_review_payload(payload: PreFlightPayload) {
    // We emit the signal directly from VeinBridge rather than filling a model.
    if let Some(thread) = VEIN_THREAD.get() {
        let thread = thread.clone();
        thread
            .queue(move |mut qobj| {
                qobj.as_mut().payload_ready_for_review(
                    QString::from(&payload.system),
                    QString::from(&payload.directives),
                    QString::from(&payload.engrams),
                    QString::from(&payload.prompt),
                );
            })
            .unwrap();
    }
}

pub fn route_console_log(log: String) {
    if let Some(thread) = NETWORK_LOG_MODEL_THREAD.get() {
        let thread = thread.clone();
        thread
            .queue(move |mut qobj| {
                qobj.as_mut().append_log(QString::from(&log));
            })
            .unwrap();
    }
}

pub fn route_console_batch(logs: Vec<String>) {
    if let Some(thread) = NETWORK_LOG_MODEL_THREAD.get() {
        let thread = thread.clone();
        thread
            .queue(move |mut qobj| {
                qobj.as_mut().begin_reset_model();
                qobj.as_mut().rust_mut().rows = logs;
                qobj.as_mut().end_reset_model();
            })
            .unwrap();
    }
}
