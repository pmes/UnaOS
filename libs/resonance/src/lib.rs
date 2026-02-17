pub type Sample = f64;
pub const BLOCK_SIZE: usize = 64;

pub mod core;
pub mod graph;
pub mod nodes;

pub use core::{AudioNode, GraphContext};
pub use graph::{AudioGraph, NodeId};
pub use nodes::gain::Gain;
pub use nodes::mixer::Mixer;
pub use nodes::oscillators::SineOscillator;
