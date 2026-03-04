use gneiss_pal::api::{Content, Part, ResilientClient};

pub async fn compress_into_engram(
    client: &mut ResilientClient,
    user_prompt: &str,
    ai_response: &str,
) -> Result<String, String> {
    let system_instruction = r#"You are a highly efficient cognitive compression subroutine.
Your task is to compress the provided conversation history into a dense, token-efficient "Engram".

Rules:
1. Extract only the core intent, the specific technical constraints, and the final outcome or consensus.
2. Strip out all pleasantries, conversational filler, and redundant explanations.
3. Format the output as a concise, bulleted list.
4. Do not include introductory or concluding remarks. Output strictly the compiled facts.

Example Output format:
- User requested fix for Cortex amnesia.
- AI identified DiskManager semantic embeddings missing from Vertex payload.
- AI supplied Directive 065 to implement 'directive' memory class and Engram compression.
- User approved implementation."#;

    let mut request_contents = Vec::new();

    let combined_text = format!(
        "{}\n\n[CONVERSATION HISTORY TO COMPRESS]:\nUser: {}\n\nAI: {}\n",
        system_instruction, user_prompt, ai_response
    );

    request_contents.push(Content {
        role: "user".to_string(),
        parts: vec![Part::text(combined_text)],
    });

    let (response, _) = client.generate_content(&request_contents).await?;
    Ok(response)
}
