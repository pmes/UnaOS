// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! A conversation as real turns (VEINTURNS, SR42).
//!
//! Vein used to send every message as a ONE-turn [`ChatRequest`] with whatever
//! history it had folded into the system prompt. A [`Thread`] keeps the
//! conversation as `(role, parts)` messages and builds each request as
//! `system` (the operator framing alone) + every earlier turn + the new user
//! turn — the shape the Messages API, Gemini's `contents` and the Claude Code
//! CLI's `--resume` all take. The assistant turn stored is the provider's
//! reply text byte for byte, so a provider that keeps its own session (the
//! CLI) recognises the conversation it already holds and continues it.
//!
//! Bounded: past [`THREAD_MAX`] messages the oldest are dropped down to
//! [`THREAD_KEEP`] in ONE step (never one exchange per turn), so the window
//! stays stable for many turns and a session-keeping provider re-anchors once
//! rather than starting fresh on every message.

use super::Part;
use super::provider::{ChatMessage, ChatRequest, DEFAULT_MAX_TOKENS, Role};

/// The most messages a thread holds before it trims.
pub const THREAD_MAX: usize = 64;
/// What a trim keeps (the newest messages, starting on a user turn).
pub const THREAD_KEEP: usize = 32;

#[derive(Debug, Clone, Default)]
pub struct Thread {
    messages: Vec<ChatMessage>,
    /// Bumped on every `clear`/`undo`/trim: a change that is not an append.
    epoch: u64,
}

impl Thread {
    pub fn new() -> Self {
        Self::default()
    }

    /// The recorded turns, oldest first.
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Completed exchanges (user + assistant pairs).
    pub fn exchanges(&self) -> usize {
        self.messages.len() / 2
    }

    /// How many times the history was rewritten (cleared, undone, trimmed).
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The request for a new user turn: `system` alone in `system`, the
    /// recorded turns, then `user` as the last message.
    pub fn request(&self, system: Option<String>, user: Vec<Part>) -> ChatRequest {
        let mut messages = Vec::with_capacity(self.messages.len() + 1);
        messages.extend(self.messages.iter().cloned());
        messages.push(ChatMessage { role: Role::User, parts: user });
        ChatRequest { system, messages, max_tokens: DEFAULT_MAX_TOKENS, temperature: None }
    }

    /// Record a completed exchange: the user turn exactly as sent and the
    /// reply text exactly as the provider streamed it. A failed turn is never
    /// recorded (the caller simply does not call this). Returns true when the
    /// record trimmed the history.
    pub fn record(&mut self, user: Vec<Part>, reply: &str) -> bool {
        self.messages.push(ChatMessage { role: Role::User, parts: user });
        self.messages.push(ChatMessage { role: Role::Assistant, parts: vec![Part::text(reply.to_string())] });
        if self.messages.len() > THREAD_MAX {
            let mut cut = self.messages.len() - THREAD_KEEP;
            while cut < self.messages.len() && self.messages[cut].role != Role::User {
                cut += 1;
            }
            self.messages.drain(..cut);
            self.epoch += 1;
            return true;
        }
        false
    }

    /// `/new`: forget every turn.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.epoch += 1;
    }

    /// `/undo`: drop the last exchange (an edit of the last message is
    /// `/undo` then the corrected message). Returns false when empty.
    pub fn undo(&mut self) -> bool {
        if self.messages.is_empty() {
            return false;
        }
        let keep = self.messages.iter().rposition(|m| m.role == Role::User).unwrap_or(0);
        self.messages.truncate(keep);
        self.epoch += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(m: &ChatMessage) -> String {
        m.parts
            .iter()
            .map(|p| match p {
                Part::Text { text } => text.clone(),
                Part::FileData { .. } => "<file>".into(),
            })
            .collect()
    }

    #[test]
    fn requests_carry_every_turn_with_its_role_and_the_system_alone() {
        let mut t = Thread::new();
        let r = t.request(Some("sys".into()), vec![Part::text("one".into())]);
        assert_eq!(r.messages.len(), 1);
        t.record(vec![Part::text("one".into())], "uno");
        t.record(vec![Part::text("two".into())], "dos");
        let r = t.request(Some("sys".into()), vec![Part::text("three".into())]);
        assert_eq!(r.system.as_deref(), Some("sys"));
        let got: Vec<(Role, String)> = r.messages.iter().map(|m| (m.role, text(m))).collect();
        assert_eq!(
            got,
            [
                (Role::User, "one".into()),
                (Role::Assistant, "uno".into()),
                (Role::User, "two".into()),
                (Role::Assistant, "dos".into()),
                (Role::User, "three".into()),
            ]
        );
        // Nothing of the history leaks into the system prompt.
        assert!(!r.system.unwrap().contains("uno"));
        assert_eq!(t.exchanges(), 2);
    }

    #[test]
    fn undo_drops_the_last_exchange_and_clear_drops_all() {
        let mut t = Thread::new();
        t.record(vec![Part::text("a".into())], "A");
        t.record(vec![Part::text("b".into())], "B");
        let e = t.epoch();
        assert!(t.undo());
        assert_eq!(t.len(), 2);
        assert_eq!(text(&t.messages()[1]), "A");
        assert!(t.epoch() > e);
        t.clear();
        assert!(t.is_empty());
        assert!(!t.undo());
    }

    #[test]
    fn the_trim_is_one_step_and_starts_on_a_user_turn() {
        let mut t = Thread::new();
        let mut trims = 0;
        for i in 0..40 {
            if t.record(vec![Part::text(format!("u{i}"))], &format!("a{i}")) {
                trims += 1;
            }
        }
        // 80 messages written; one trim at 66 → 32, then appends to 46.
        assert_eq!(trims, 1);
        assert_eq!(t.len(), 32 + 14);
        assert_eq!(t.messages()[0].role, Role::User);
        assert_eq!(text(t.messages().last().unwrap()), "a39");
    }
}
