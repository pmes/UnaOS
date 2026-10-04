// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The PURE half of Vein's context assembler (feature `alloc`): system prompt + bounded history → the
//! request, byte-identical on host and metal. Moved here out of `handlers/vein/src/context.rs`: the
//! engram compression prompt (`ENGRAM_SYSTEM` and the text `engram_prompt` builds — byte-for-byte the
//! `format!` that file carried). What stays host-side: the `ResilientClient` call itself, DiskManager
//! retrieval (cortex/vault), gravity scoring, uploads — all std/Tokio/network.

use alloc::string::String;
use alloc::vec::Vec;

use crate::model::{ChatMessage, ChatRequest, Conversation, Role};

/// The default answer budget a request carries.
pub const DEFAULT_MAX_TOKENS: u32 = 1024;

/// The engram compression subroutine's instruction (was a literal in `handlers/vein/src/context.rs`).
pub const ENGRAM_SYSTEM: &str = r#"You are a highly efficient cognitive compression subroutine.
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

/// The single user turn the engram compressor is sent (system instruction inlined, as before).
pub fn engram_prompt(user_prompt: &str, ai_response: &str) -> String {
    alloc::format!(
        "{}\n\n[CONVERSATION HISTORY TO COMPRESS]:\nUser: {}\n\nAI: {}\n",
        ENGRAM_SYSTEM, user_prompt, ai_response
    )
}

/// Assemble a request: the system prompt, then the conversation's bounded history in order, then the new
/// user turn. System-role messages inside the history are folded into `system` (one system prompt per
/// request, the shape both providers take).
pub fn assemble(system: &str, conv: &Conversation, user: &str, max_tokens: u32) -> ChatRequest {
    let mut sys = String::from(system);
    let mut messages: Vec<ChatMessage> = Vec::with_capacity(conv.len() + 1);
    for m in conv.messages() {
        if m.role == Role::System {
            if !sys.is_empty() {
                sys.push_str("\n\n");
            }
            sys.push_str(&m.text);
        } else {
            messages.push(m.clone());
        }
    }
    messages.push(ChatMessage::new(Role::User, user));
    ChatRequest { system: sys, messages, max_tokens }
}

/// The canonical plain-text rendering of a request (`System:` / `User:` / `Assistant:` paragraphs) —
/// what a text-only provider (a log, a line-protocol model) is given; identical bytes on
/// every ring.
pub fn render(req: &ChatRequest) -> String {
    let mut s = String::new();
    if !req.system.is_empty() {
        s.push_str("System: ");
        s.push_str(&req.system);
        s.push_str("\n\n");
    }
    for m in &req.messages {
        s.push_str(match m.role {
            Role::System => "System: ",
            Role::User => "User: ",
            Role::Assistant => "Assistant: ",
        });
        s.push_str(&m.text);
        s.push_str("\n\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engram_prompt_is_the_old_format_byte_for_byte() {
        let p = engram_prompt("u", "a");
        assert!(p.starts_with("You are a highly efficient cognitive compression subroutine.\n"));
        assert!(p.ends_with("- User approved implementation.\n\n[CONVERSATION HISTORY TO COMPRESS]:\nUser: u\n\nAI: a\n"));
    }

    #[test]
    fn assemble_orders_history_then_the_new_turn() {
        let mut c = Conversation::new(5);
        c.push(Role::System, "be brief");
        c.push(Role::User, "hi");
        c.push(Role::Assistant, "hello");
        let r = assemble("You are Vein.", &c, "and now?", DEFAULT_MAX_TOKENS);
        assert_eq!(r.system, "You are Vein.\n\nbe brief");
        let t: Vec<(Role, &str)> = r.messages.iter().map(|m| (m.role, m.text.as_str())).collect();
        assert_eq!(t, [(Role::User, "hi"), (Role::Assistant, "hello"), (Role::User, "and now?")]);
        assert_eq!(render(&r), "System: You are Vein.\n\nbe brief\n\nUser: hi\n\nAssistant: hello\n\nUser: and now?\n\n");
    }
}
