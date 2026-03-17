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

// libs/quartzite/src/widgets/model.rs
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{glib, glib::Properties};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DispatchRecord {
    pub id: String,
    pub sender: String,
    pub subject: String,
    pub timestamp: String,
    pub content: String,
    pub is_chat: bool,
}

mod imp {
    use super::*;

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::HistoryObject)]
    pub struct HistoryObject {
        #[property(get, set)]
        pub id: RefCell<String>,
        #[property(get, set)]
        pub sender: RefCell<String>,
        #[property(get, set)]
        pub subject: RefCell<String>,
        #[property(get, set)]
        pub timestamp: RefCell<String>,
        #[property(get, set)]
        pub content: RefCell<String>,
        #[property(get, set)]
        pub is_chat: RefCell<bool>,
        #[property(get, set)]
        pub is_expanded: RefCell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HistoryObject {
        const NAME: &'static str = "HistoryObject";
        type Type = super::HistoryObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for HistoryObject {}
}

glib::wrapper! {
    pub struct HistoryObject(ObjectSubclass<imp::HistoryObject>);
}

impl HistoryObject {
    pub fn new(
        id: &str,
        sender: &str,
        subject: &str,
        timestamp: &str,
        content: &str,
        is_chat: bool,
    ) -> Self {
        glib::Object::builder()
            .property("id", id)
            .property("sender", sender)
            .property("subject", subject)
            .property("timestamp", timestamp)
            .property("content", content)
            .property("is-chat", is_chat)
            .property("is-expanded", false)
            .build()
    }

    pub fn from_record(record: &DispatchRecord) -> Self {
        Self::new(
            &record.id,
            &record.sender,
            &record.subject,
            &record.timestamp,
            &record.content,
            record.is_chat,
        )
    }

    pub fn to_record(&self) -> DispatchRecord {
        DispatchRecord {
            id: self.id(),
            sender: self.sender(),
            subject: self.subject(),
            timestamp: self.timestamp(),
            content: self.content(),
            is_chat: self.is_chat(),
        }
    }
}
