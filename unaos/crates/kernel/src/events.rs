// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use alloc::collections::VecDeque;
use spin::Mutex;
use gneiss_pal::Event;
use crate::pal::RAW_FRAMEBUFFER_PTR;

fn write_dye(offset: isize, color: u32) {
    unsafe {
        if RAW_FRAMEBUFFER_PTR != 0 {
            let ptr = RAW_FRAMEBUFFER_PTR as *mut u32;
            // Write to the specific pixel offset
            *ptr.offset(offset) = color;
        }
    }
}

// The Synapse: A global queue protected by a spinlock.
// We use a Mutex to ensure the Interrupt Handler (Writer) and Main Loop (Reader)
// don't fight over the memory.
pub static EVENT_QUEUE: Mutex<VecDeque<Event>> = Mutex::new(VecDeque::new());

/// Called by interrupts to fire a signal
pub fn push_event(event: Event) {
    // We try_lock to avoid deadlocks in interrupt context.
    // If we can't get the lock, we drop the event (better than crashing).
    if let Some(mut queue) = EVENT_QUEUE.try_lock() {
        if queue.len() < 100 { // Cap size to prevent memory leaks
            queue.push_back(event);
            // GREEN: Success PUSH
            write_dye(10, 0x00FF00);
        }
    } else {
        // RED: Lock Contention
        write_dye(15, 0xFF0000);
    }
}

/// Called by TargetPal to feel the signal
pub fn pop_event() -> Option<Event> {
    if let Some(mut queue) = EVENT_QUEUE.try_lock() {
        let item = queue.pop_front();
        if item.is_some() {
            // BLUE: Success POP
            write_dye(20, 0x0000FF);
        }
        item
    } else {
        None
    }
}
