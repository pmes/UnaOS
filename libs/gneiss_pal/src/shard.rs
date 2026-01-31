// libs/gneiss_pal/src/shard.rs

#[derive(Debug, Clone, PartialEq)]
pub enum ShardRole {
    Root,       // Una-Prime (The Command Deck)
    Builder,    // S9 (CI/CD)
    Storage,    // The Mule (Big Data)
    Kernel,     // Hardware Debugging
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShardStatus {
    Online,     // Green
    Offline,    // Grey
    Busy,       // Yellow/Orange
    Error,      // Red
}

#[derive(Debug, Clone)]
pub struct Shard {
    pub id: String,
    pub name: String,
    pub role: ShardRole,
    pub status: ShardStatus,
    pub cpu_load: u8, // Percentage 0-100
    pub children: Vec<Shard>,
}

#[derive(Debug, Clone)]
pub struct Heartbeat {
    pub id: String,
    pub status: ShardStatus,
    pub cpu_load: u8,
}

impl Shard {
    pub fn new(id: &str, name: &str, role: ShardRole) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            role,
            status: ShardStatus::Offline,
            cpu_load: 0,
            children: Vec::new(),
        }
    }
}
