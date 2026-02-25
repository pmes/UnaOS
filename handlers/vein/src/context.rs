impl SMessage {
    /// Strips heavy payloads (images, massive files) for historical context.
    /// We only need the memory of the image, not the bytes themselves.
    pub fn prune_for_history(&self) -> Self {
        let mut pruned = self.clone();
        if let Some(payload) = &mut pruned.payload {
            if payload.is_image() {
                // Replace the 4MB base64 string with a 20-byte memory anchor.
                *payload = Payload::Text(format!("[System: User attached image '{}']", payload.filename()));
            }
        }
        pruned
    }
}

// When building the Vertex request:
pub fn build_prompt(history: &[SMessage], current: &SMessage) -> VertexRequest {
    let mut messages: Vec<VertexMessage> = history.iter()
        // Keep only the last 10 interactions to prevent context collapse
        .rev().take(10).rev()
        .map(|msg| msg.prune_for_history().into())
        .collect();

    // The current message keeps its full payload (the actual image)
    messages.push(current.clone().into());

    VertexRequest { messages }
}
