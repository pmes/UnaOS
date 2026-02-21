use glib::subclass::prelude::*;
use glib::prelude::*;
use glib::Properties;
use std::cell::RefCell;
use serde::{Serialize, Deserialize};

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
        id: RefCell<String>,
        #[property(get, set)]
        sender: RefCell<String>,
        #[property(get, set)]
        subject: RefCell<String>,
        #[property(get, set)]
        timestamp: RefCell<String>,
        #[property(get, set)]
        content: RefCell<String>,
        #[property(get, set)]
        is_chat: RefCell<bool>,
        #[property(get, set)]
        is_expanded: RefCell<bool>,
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
            .property("is_chat", is_chat)
            .property("is_expanded", false)
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
