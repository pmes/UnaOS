// libs/gneiss_pal/src/lib.rs (Logic Kernel)
#![allow(deprecated)]

pub mod api;
pub mod forge;
pub mod persistence;
pub mod paths;
pub mod shard;
pub mod types;

// Re-export types so consumers see them at the root
pub use shard::{Heartbeat, Shard, ShardRole, ShardStatus};
pub use types::*;

// --- LOGIC KERNEL ---
// No GTK, No Assets, No UI.
