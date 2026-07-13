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

//! The bounded AUTO command channel (invariant **I8**).
//!
//! AUTO commands (the future autonomy feed) queue in a **fixed-capacity** ring
//! buffer — no allocation, never blocks the control tick. On overflow the queue
//! **drops the oldest** command (chosen over refuse: the freshest steering /
//! throttle intent is the safest one to keep, and a stale command must never
//! outlive a newer one). Every command still passes through the drive core's
//! full safety envelope (arm state, deadman, throttle cap) before it can reach
//! an actuator.

/// A single autonomy command: a target steering + throttle, both in µs. These
/// are *requests*; the drive core clamps and gates them (I3 throttle cap etc.)
/// before anything reaches the sink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    /// Requested steering pulse, µs.
    pub steering_us: u16,
    /// Requested throttle pulse, µs.
    pub throttle_us: u16,
}

impl Command {
    /// Construct a command.
    #[inline]
    pub const fn new(steering_us: u16, throttle_us: u16) -> Self {
        Self {
            steering_us,
            throttle_us,
        }
    }
}

/// Fixed-capacity, drop-oldest ring buffer of [`Command`]s. `CAP` slots, no
/// heap, no blocking. This is the concrete realisation of invariant **I8**.
#[derive(Clone, Debug)]
pub struct CmdQueue<const CAP: usize> {
    slots: [Command; CAP],
    head: usize, // index of the oldest element
    len: usize,  // number of live elements, 0..=CAP
    dropped: u32, // count of commands discarded on overflow (a witness stat)
}

impl<const CAP: usize> Default for CmdQueue<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAP: usize> CmdQueue<CAP> {
    /// An empty queue. `CAP` must be non-zero.
    pub const fn new() -> Self {
        assert!(CAP > 0, "CmdQueue capacity must be non-zero");
        Self {
            slots: [Command::new(0, 0); CAP],
            head: 0,
            len: 0,
            dropped: 0,
        }
    }

    /// Capacity (compile-time constant `CAP`).
    #[inline]
    pub const fn capacity(&self) -> usize {
        CAP
    }

    /// Number of queued commands.
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// `true` when no commands are queued.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Total commands dropped on overflow since construction.
    #[inline]
    pub const fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Enqueue a command. Never blocks. When the queue is full the **oldest**
    /// command is discarded to make room (and [`dropped`](Self::dropped) is
    /// bumped); the new command is always retained.
    pub fn push(&mut self, cmd: Command) {
        if self.len == CAP {
            // Drop oldest: advance head, overwrite the freed tail slot.
            self.head = (self.head + 1) % CAP;
            self.len -= 1;
            self.dropped = self.dropped.wrapping_add(1);
        }
        let tail = (self.head + self.len) % CAP;
        self.slots[tail] = cmd;
        self.len += 1;
    }

    /// Dequeue the oldest command, or `None` when empty.
    pub fn pop(&mut self) -> Option<Command> {
        if self.len == 0 {
            return None;
        }
        let cmd = self.slots[self.head];
        self.head = (self.head + 1) % CAP;
        self.len -= 1;
        Some(cmd)
    }

    /// Discard all queued commands (used on disarm / failsafe so stale autonomy
    /// intent cannot survive a re-arm).
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}
