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

use cxx_qt_lib::{QString, QVariant, QModelIndex};
use cxx_qt::CxxQtType;
use std::sync::OnceLock;
use std::pin::Pin;
use gneiss_pal::{Event, PreFlightPayload, HistoryItem};
use crate::platforms::qt::window::GLOBAL_TX;

pub static VEIN_THREAD: OnceLock<cxx_qt::CxxQtThread<qobject::VeinBridge>> = OnceLock::new();
pub static HISTORY_MODEL_THREAD: OnceLock<cxx_qt::CxxQtThread<qobject::HistoryModel>> = OnceLock::new();
pub static PREFLIGHT_THREAD: OnceLock<cxx_qt::CxxQtThread<qobject::PreFlightPayloadQml>> = OnceLock::new();

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
        fn rowCount(self: &HistoryModel, parent: &QModelIndex) -> i32;
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
        fn dispatch_payload(self: Pin<&mut VeinBridge>, text: QString);

        #[qinvokable]
        #[cxx_name = "registerThread"]
        fn register_thread(self: Pin<&mut VeinBridge>);
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

    pub fn dispatch_payload(self: std::pin::Pin<&mut Self>, text: QString) {
         if let Some(tx) = GLOBAL_TX.get() {
             let _ = tx.try_send(Event::DispatchPayload(text.to_string()));
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

    pub fn rowCount(&self, _parent: &QModelIndex) -> i32 {
        self.rust().rows.len() as i32
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

pub fn route_history_batch(items: Vec<HistoryItem>) {
    let rust_items: Vec<HistoryItemRust> = items.into_iter().map(|i| HistoryItemRust {
        sender: i.sender,
        content: i.content,
        timestamp: i.timestamp,
        is_chat: i.is_chat,
    }).collect();

    if let Some(thread) = HISTORY_MODEL_THREAD.get() {
        let thread = thread.clone();
        thread.queue(move |mut qobj| {
            qobj.add_items(rust_items);
        }).unwrap();
    } else {
        eprintln!(":: PLEXUS :: Payload dropped: HistoryModel thread not registered.");
    }
}

pub fn route_review_payload(payload: PreFlightPayload) {
    if let Some(thread) = PREFLIGHT_THREAD.get() {
        let thread = thread.clone();
        thread.queue(move |mut qobj| {
            qobj.as_mut().set_system(QString::from(&payload.system));
            qobj.as_mut().set_directives(QString::from(&payload.directives));
            qobj.as_mut().set_engrams(QString::from(&payload.engrams));
            qobj.as_mut().set_prompt(QString::from(&payload.prompt));
        }).unwrap();
    }
}
