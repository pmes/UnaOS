// libs/gneiss_pal/src/lib.rs (Logic Kernel)
#![allow(deprecated)]

pub mod persistence;
pub mod shard;
pub mod types;
pub mod api;
pub mod forge;

// Re-export types so consumers see them at the root
pub use types::*;
pub use shard::{Shard, ShardRole, ShardStatus, Heartbeat};

// --- LOGIC KERNEL ---
// No GTK, No Assets, No UI.
