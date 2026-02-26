// apps/lumen/src/ui/model.rs
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{glib, glib::Properties};
use std::cell::RefCell;

// Define a local struct for UI dispatch records
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
}
