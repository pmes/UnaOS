/// Commands sent from the UI thread to the Audio Engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AudioCommand {
    /// Update a specific parameter on a specific node.
    ///
    /// # Arguments
    /// * `node_id` - The index of the node in the graph.
    /// * `param_id` - The parameter ID (node-specific).
    /// * `value` - The new value.
    SetParam {
        node_id: usize,
        param_id: usize,
        value: f64,
    },

    /// Stop the audio engine immediately (panic button).
    Stop,

    /// Update the master frequency (assumes Node 0 is an oscillator).
    /// For the prototype: Just change the oscillator pitch.
    SetMasterFrequency(f64),
}
