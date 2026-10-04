// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The conversation model (feature `alloc`). The type NAMES mirror VEINPROV's `gneiss_pal::api`
//! (`ChatRequest`, `ChatResponse`, `ChatMessage`); `claude::encode_messages_request` encodes a
//! [`ChatRequest`] for the Messages API.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

pub use crate::role::Role;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: Role,
    pub text: String,
}

impl ChatMessage {
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        ChatMessage { role, text: text.into() }
    }
}

/// Why an answer stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatRequest {
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatResponse {
    pub text: String,
    pub stop: StopReason,
}

/// The default bound on a conversation's remembered turns (one turn = one message).
pub const MAX_TURNS: usize = 32;

/// One conversation: an id (the wire's `conv`) and a BOUNDED history — the oldest message is dropped
/// when the bound is reached, so neither ring can be grown without limit by a chatty caller.
#[derive(Debug, Clone)]
pub struct Conversation {
    pub id: u32,
    max_turns: usize,
    history: VecDeque<ChatMessage>,
}

impl Conversation {
    pub fn new(id: u32) -> Self {
        Self::with_bound(id, MAX_TURNS)
    }
    pub fn with_bound(id: u32, max_turns: usize) -> Self {
        Conversation { id, max_turns: max_turns.max(1), history: VecDeque::new() }
    }
    pub fn push(&mut self, role: Role, text: impl Into<String>) {
        if self.history.len() == self.max_turns {
            self.history.pop_front();
        }
        self.history.push_back(ChatMessage::new(role, text));
    }
    /// Record a finished exchange.
    pub fn record(&mut self, user: &str, resp: &ChatResponse) {
        self.push(Role::User, user);
        if resp.stop != StopReason::Error {
            self.push(Role::Assistant, resp.text.as_str());
        }
    }
    pub fn len(&self) -> usize {
        self.history.len()
    }
    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }
    pub fn messages(&self) -> impl Iterator<Item = &ChatMessage> {
        self.history.iter()
    }
    pub fn clear(&mut self) {
        self.history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_bounded_oldest_first_out() {
        let mut c = Conversation::with_bound(3, 4);
        for i in 0..10 {
            c.push(if i % 2 == 0 { Role::User } else { Role::Assistant }, alloc::format!("m{i}"));
        }
        assert_eq!(c.len(), 4);
        let t: Vec<&str> = c.messages().map(|m| m.text.as_str()).collect();
        assert_eq!(t, ["m6", "m7", "m8", "m9"]);
    }

    #[test]
    fn record_skips_a_failed_answer() {
        let mut c = Conversation::new(1);
        c.record("hi", &ChatResponse { text: "hello".into(), stop: StopReason::EndTurn });
        c.record("again", &ChatResponse { text: String::new(), stop: StopReason::Error });
        let r: Vec<Role> = c.messages().map(|m| m.role).collect();
        assert_eq!(r, [Role::User, Role::Assistant, Role::User]);
    }
}
