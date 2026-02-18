use crate::core::{AudioNode, GraphContext};
use crate::{BLOCK_SIZE, Sample};

/// Unique identifier for a node in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub usize);

/// The maximum number of input ports we track for a node.
const MAX_INPUTS: usize = 8;

/// The graph engine that owns nodes and manages signal flow.
pub struct AudioGraph {
    /// The processing nodes.
    nodes: Vec<Box<dyn AudioNode + Send>>,

    /// The connections (wires).
    /// connections[dst_node_id.0][input_port_index] = Some(src_node_id)
    connections: Vec<Vec<Option<NodeId>>>,

    /// The output buffers for each node.
    /// outputs[node_id.0] is the buffer written to by node_id.
    outputs: Vec<[Sample; BLOCK_SIZE]>,

    /// A silent buffer used for unconnected inputs or as a default reference.
    silence: [Sample; BLOCK_SIZE],

    /// The global context (sample rate, etc.).
    context: GraphContext,
}

impl AudioGraph {
    /// Creates a new audio graph with the specified sample rate.
    pub fn new(sample_rate: Sample) -> Self {
        Self {
            nodes: Vec::new(),
            connections: Vec::new(),
            outputs: Vec::new(),
            silence: [0.0; BLOCK_SIZE],
            context: GraphContext::new(sample_rate),
        }
    }

    /// Adds a node to the graph and returns its ID.
    ///
    /// The node is initialized with a blank output buffer and no input connections.
    pub fn add_node(&mut self, node: Box<dyn AudioNode + Send>) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(node);
        self.outputs.push([0.0; BLOCK_SIZE]);
        self.connections.push(Vec::new());
        id
    }

    /// Connects an output of a source node to an input of a destination node.
    ///
    /// # Arguments
    ///
    /// * `src` - The ID of the source node providing the signal.
    /// * `dst` - The ID of the destination node receiving the signal.
    /// * `input_index` - The input port index on the destination node (0 to MAX_INPUTS - 1).
    pub fn connect(&mut self, src: NodeId, dst: NodeId, input_index: usize) {
        if input_index >= MAX_INPUTS {
            panic!(
                "Input index {} exceeds MAX_INPUTS {}",
                input_index, MAX_INPUTS
            );
        }
        if src.0 >= self.nodes.len() || dst.0 >= self.nodes.len() {
            panic!("Invalid node ID");
        }

        let inputs = &mut self.connections[dst.0];
        if inputs.len() <= input_index {
            inputs.resize(input_index + 1, None);
        }
        inputs[input_index] = Some(src);
    }

    /// Sets a parameter on a specific node.
    ///
    /// This delegates the call to the node's `set_param` method.
    ///
    /// # Arguments
    /// * `node` - The ID of the target node.
    /// * `param_id` - The parameter ID.
    /// * `value` - The new value.
    pub fn set_node_param(&mut self, node: NodeId, param_id: usize, value: f64) {
        if let Some(n) = self.nodes.get_mut(node.0) {
            n.set_param(param_id, value);
        }
    }

    /// Processes one block of audio through the entire graph.
    ///
    /// Iterates through nodes in the order they were added (topological order is expected).
    /// Returns a reference to the output buffer of the last node in the chain.
    pub fn process(&mut self) -> &[Sample; BLOCK_SIZE] {
        // Iterate through every node by index
        for id in 0..self.nodes.len() {
            // Split outputs into past (inputs) and current (output).
            // This enables borrowing past outputs immutably while mutating current output.
            let (past_outputs, current_and_future) = self.outputs.split_at_mut(id);
            let (current_output, _future) = current_and_future.split_at_mut(1);
            let output_buffer = &mut current_output[0];

            // Build the input slice on the stack.
            // We use a fixed-size array of references, defaulting to silence.
            // This avoids any heap allocation (Vec) inside the audio thread.
            let mut input_refs: [&[Sample; BLOCK_SIZE]; MAX_INPUTS] = [&self.silence; MAX_INPUTS];

            // Resolve connections
            let input_map = &self.connections[id];

            // Determine the maximum input index used to slice correctly
            let mut max_input_index = 0;

            for (port_index, source_option) in input_map.iter().enumerate().take(MAX_INPUTS) {
                if let Some(src_id) = source_option {
                    if src_id.0 < id {
                        // Safe: src_id < id, so it's in past_outputs
                        input_refs[port_index] = &past_outputs[src_id.0];
                        if port_index + 1 > max_input_index {
                            max_input_index = port_index + 1;
                        }
                    } else {
                        // Connecting to a future node (or self) is not supported in this strict topological loop.
                        // We ignore it (effectively silence) for now.
                    }
                }
            }

            // Execute the node process
            // Only pass the slice up to the highest connected input port (or 0 if none)
            let inputs_slice = &input_refs[0..max_input_index];

            // We need to pass a slice of mutable outputs, even though we only have one.
            // The trait expects `&mut [&mut [Sample; BLOCK_SIZE]]`.
            // We construct a temporary array of mutable references on the stack.
            let mut output_refs = [output_buffer];

            self.nodes[id].process(inputs_slice, &mut output_refs, &self.context);
        }

        // Return the last buffer or silence if graph is empty
        if let Some(last) = self.outputs.last() {
            last
        } else {
            &self.silence
        }
    }
}
