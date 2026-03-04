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
    #[properties(wrapper_type = super::DispatchObject)]
    pub struct DispatchObject {
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

        // --- Added for J52 Staging Bubble ---
        // 0 = Standard, 1 = Staging, 2 = Pulse
        #[property(get, set)]
        pub message_type: RefCell<u32>,

        #[property(get, set)]
        pub is_locked: RefCell<bool>,

        #[property(get, set)]
        pub system_text: RefCell<String>,

        #[property(get, set)]
        pub directives_text: RefCell<String>,

        #[property(get, set)]
        pub engrams_text: RefCell<String>,

        #[property(get, set)]
        pub prompt_text: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DispatchObject {
        const NAME: &'static str = "DispatchObject";
        type Type = super::DispatchObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for DispatchObject {}
}

glib::wrapper! {
    pub struct DispatchObject(ObjectSubclass<imp::DispatchObject>);
}

impl DispatchObject {
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
            .property("message-type", 0u32)
            .property("is-locked", false)
            .property("system-text", "")
            .property("directives-text", "")
            .property("engrams-text", "")
            .property("prompt-text", "")
            .build()
    }

    pub fn new_staging(
        id: &str,
        system: &str,
        directives: &str,
        engrams: &str,
        prompt: &str,
    ) -> Self {
        glib::Object::builder()
            .property("id", id)
            .property("sender", "Architect")
            .property("subject", "Pre-Flight Payload")
            .property("timestamp", chrono::Local::now().format("%H:%M:%S").to_string())
            .property("content", "Staging Payload")
            .property("is-chat", true)
            .property("is-expanded", false)
            .property("message-type", 1u32)
            .property("is-locked", false)
            .property("system-text", system)
            .property("directives-text", directives)
            .property("engrams-text", engrams)
            .property("prompt-text", prompt)
            .build()
    }

    pub fn new_pulse(id: &str) -> Self {
        glib::Object::builder()
            .property("id", id)
            .property("sender", "Una-Prime")
            .property("subject", "Pulse")
            .property("timestamp", chrono::Local::now().format("%H:%M:%S").to_string())
            .property("content", "...")
            .property("is-chat", true)
            .property("is-expanded", false)
            .property("message-type", 2u32)
            .property("is-locked", true)
            .property("system-text", "")
            .property("directives-text", "")
            .property("engrams-text", "")
            .property("prompt-text", "")
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
