// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use gneiss_pal::api::{ChatRequest, ModelProvider, Part, StopReason};

pub async fn compress_into_engram(
    provider: &dyn ModelProvider,
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

    // VEINPROV (B303): the compression rules travel as the system prompt, the
    // exchange as the one user turn — provider-neutral.
    let combined_text = format!(
        "[CONVERSATION HISTORY TO COMPRESS]:\nUser: {}\n\nAI: {}\n",
        user_prompt, ai_response
    );
    let req = ChatRequest::single(Some(system_instruction.to_string()), vec![Part::text(combined_text)]);
    let resp = provider.generate(&req).await.map_err(|e| e.to_string())?;
    match resp.stop {
        StopReason::EndTurn | StopReason::MaxTokens => Ok(resp.text),
        other => Err(format!("engram compression stopped: {other:?}")),
    }
}
