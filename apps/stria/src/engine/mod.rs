pub struct MediaGraph {
    // The "Graph" holds the pipeline stages (Decode -> Filter -> Render)
    worker_threads: usize,
}

impl MediaGraph {
    pub fn new(cores: usize) -> Self {
        // High-Performance Strategy:
        // We will eventually pin specific threads to specific cores
        // using `core_affinity` to prevent context switching.
        Self {
            worker_threads: cores,
        }
    }

    pub fn cores(&self) -> usize {
        self.worker_threads
    }
}
