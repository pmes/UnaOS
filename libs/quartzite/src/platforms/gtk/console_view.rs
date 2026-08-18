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

//! The Console app on glass — the GTK render of `TetraNode::Console`.
//!
//! A read-only, monospace, live view of the system log scrollback (the macOS
//! `Console.app` equivalent). It seeds from the `ConsoleTetra` snapshot the
//! tetra bridge built and then follows the live feed: it subscribes to the
//! Synapse and re-renders on every `SMessage::Logs(LogEvent::LogTail { .. })`
//! that `comscan` publishes. Every incoming record is run through Tabula's log
//! sanitizer (`crate::tetra::sanitize_line`) so a stray control byte off the
//! cable is *shown* (as a Control Picture), never obeyed.
//!
//! **Read-only by ownership, not app policy.** The pane has no input field and
//! the text view is not editable — on the shard the kernel log is root-owned
//! and the UnaFS ACL denies a user vessel any write, so a mutation was never
//! this view's to make. The pane only renders records.
//!
//! ## Summoning
//!
//! [`open_console_window`] presents the Console as its own top-level window,
//! sharing the process bus so it renders the live log. [`install_console_summon`]
//! wires that to a facade-native **gesture** (Ctrl+`) on a host window — never a
//! command-line flag. A shell tile/menu that fires the same summon lives in the
//! shell layer; this is the platform half.

use gtk4::prelude::*;
use gtk4::gdk::{Key, ModifierType};
use gtk4::{Box, Label, Orientation, ScrolledWindow, TextView, WrapMode};

use bandy::{LogEvent, SMessage, Synapse};

use crate::tetra::{ConsoleLine, ConsoleTetra};
use crate::NativeView;

/// One scrollback row as text: `source  text` (or just `text` when the record
/// carries no source tag). `text` is already sanitized by the bridge.
fn fmt_line(l: &ConsoleLine) -> String {
    if l.source.is_empty() {
        l.text.clone()
    } else {
        format!("{}  {}", l.source, l.text)
    }
}

/// The whole scrollback rendered as one buffer string, oldest→newest.
fn render_body(lines: &[ConsoleLine]) -> String {
    lines.iter().map(fmt_line).collect::<Vec<_>>().join("\n")
}

/// The header line: what the pane is, plus the honest bounded-ring state.
fn render_header(dropped: u64, paused: bool) -> String {
    let mut h = String::from(":: Console — read-only system log");
    if dropped > 0 {
        h.push_str(&format!(" · {dropped} evicted"));
    }
    if paused {
        h.push_str(" · PAUSED");
    }
    h.push_str(" ::");
    h
}

/// Build the read-only, live Console widget from a `ConsoleTetra` snapshot.
///
/// Seeds from the snapshot, then follows the bus: each `LogTail` rebuilds the
/// buffer (sanitizing incoming records) and the header. The returned view is a
/// vertical box — header label over a scrolled, non-editable monospace text
/// view.
pub fn bootstrap_console(console: &ConsoleTetra, synapse: Synapse) -> NativeView {
    let container = Box::new(Orientation::Vertical, 0);

    let header = Label::builder()
        .label(render_header(console.dropped, console.paused))
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    header.set_margin_start(8);
    header.set_margin_end(8);
    header.set_margin_top(4);
    header.set_margin_bottom(4);

    let text_view = TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(WrapMode::WordChar)
        .build();
    let buffer = text_view.buffer();
    buffer.set_text(&render_body(&console.lines));

    let scroller = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&text_view)
        .build();

    container.append(&header);
    container.append(&scroller);

    // Follow the live feed. `comscan` folds every `SMessage::Log` into its
    // bounded scrollback and publishes `SMessage::Logs(LogTail)`; we re-render
    // on each one. Runs on the GTK main thread via a local future.
    let header_live = header.clone();
    let buffer_live = buffer.clone();
    let mut rx = synapse.subscribe();
    gtk4::glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            if let SMessage::Logs(LogEvent::LogTail { lines, dropped, paused }) = msg {
                let rendered: Vec<ConsoleLine> = lines
                    .iter()
                    .map(|l| ConsoleLine {
                        level: l.level.clone(),
                        source: l.source.clone(),
                        text: crate::tetra::sanitize_line(&l.content),
                    })
                    .collect();
                buffer_live.set_text(&render_body(&rendered));
                header_live.set_label(&render_header(dropped, paused));
            }
        }
    });

    container.into()
}

/// Summon the Console app as its own top-level window, rendering the live log.
///
/// Shares the process Synapse, so the window follows the same log feed every
/// other lobe sees. Opening a fresh top-level (not an `ApplicationWindow`) means
/// a running vessel can summon it without a second application loop.
pub fn open_console_window(synapse: Synapse) {
    let window = gtk4::Window::builder()
        .title("Console — UnaOS system log")
        .default_width(900)
        .default_height(600)
        .build();

    let view = crate::platforms::gtk::tetra_eval::eval_tetra(
        crate::tetra::TetraNode::Console(ConsoleTetra::default()),
        synapse,
    );
    window.set_child(Some(&view));
    window.present();
}

/// Wire a facade-native summon gesture (Ctrl+`) onto `window`: pressing it opens
/// the live Console window. This is the gesture half of the summon — a shell
/// tile/menu that wants the same effect calls [`open_console_window`] directly.
pub fn install_console_summon(window: &crate::NativeWindow, synapse: Synapse) {
    let controller = gtk4::EventControllerKey::new();
    let syn = synapse.clone();
    controller.connect_key_pressed(move |_ctrl, key, _keycode, state| {
        let is_ctrl = state.contains(ModifierType::CONTROL_MASK);
        if is_ctrl && (key == Key::grave || key == Key::dead_grave) {
            open_console_window(syn.clone());
            return gtk4::glib::Propagation::Stop;
        }
        gtk4::glib::Propagation::Proceed
    });
    window.add_controller(controller);
}
