use alloc::string::String;
use alloc::vec::Vec;

pub struct UserSession {
    pub username: String,
    pub history: Vec<String>,
}

impl UserSession {
    pub fn new() -> Self {
        Self {
            username: String::from("architect"),
            history: Vec::new(),
        }
    }
}
