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

use crate::tetra::ConsoleTetra;
// AETHERMACDEVTOOLS (SR312): the rows and text are `widgets::console_rows`' (both faces read them).
use crate::widgets::console_rows::{render_body, render_header, rows_from_tail};
use crate::NativeView;

/// Build the read-only, live Console widget from a `ConsoleTetra` snapshot.
///
/// Seeds from the snapshot, then follows the bus: each `LogTail` rebuilds the
/// buffer (sanitizing incoming records) and the header. The returned view is a
/// vertical box — header label over a scrolled, non-editable monospace text
/// view.
pub fn bootstrap_console(console: &ConsoleTetra, synapse: Synapse) -> NativeView {
    bootstrap_console_held(console, synapse).0
}

/// FACELIVE-SUBSCRIPTION-DROP (SR441): [`bootstrap_console`] with its bus subscriber's handle, which the tetra evaluator
/// keeps and aborts when the Console leaves the tree (dropping the handle only detaches the subscriber).
pub fn bootstrap_console_held(console: &ConsoleTetra, synapse: Synapse) -> (NativeView, gtk4::glib::JoinHandle<()>) {
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
    let pad = jog_pad(&container, synapse.clone()); // VUGCOMSCANJOGFACEGTK (SR616): the jog pad under the scrollback, painted from widgets::jog_rows

    // Follow the live feed. `comscan` folds every `SMessage::Log` into its
    // bounded scrollback and publishes `SMessage::Logs(LogTail)`; we re-render
    // on each one. Runs on the GTK main thread via a local future.
    let header_live = header.clone();
    let buffer_live = buffer.clone();
    let mut rx = synapse.subscribe();
    let task = gtk4::glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            pad.hear(&msg); // VUGCOMSCANJOGFACEGTK (SR616): the pad rides this one subscriber, so the evaluator's abort (SR441) ends both
            if let SMessage::Logs(LogEvent::LogTail { lines, dropped, paused }) = msg {
                let rendered = rows_from_tail(&lines);
                buffer_live.set_text(&render_body(&rendered));
                header_live.set_label(&render_header(dropped, paused));
            }
        }
    });

    (container.into(), task)
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

// --- VUGCOMSCANJOGFACEGTK (LEDGER SR616): the Console's jog pad, painted from widgets::jog_rows (named tail hunk) ---

/// The jog pad under the Console's scrollback: comscan's `PumpCard` (the pad's state, folded from the bus) and the widgets
/// painting `widgets::jog_rows` over its snapshot. A key fires the card's own request on the bus; the desk refuses by name
/// what it cannot do, so the pad only greys (and says why) while no link is up.
#[derive(Clone)] // VUGCOMSCANDECLARECONSOLEFACE (SR633): the declare entry's Enter holds a clone (Rc and GObject handles) to repaint
struct JogPad {
    card: std::rc::Rc<std::cell::RefCell<comscan::PumpCard>>,
    link: Label,
    travel: Label,
    position: Label,
    squawk: Label,
    why: Label,
    steps: Vec<gtk4::ToggleButton>,
    keys: Vec<gtk4::Button>,
    syn: Synapse, // VUGCOMSCANMDNSDIAL (SR638): a found row's press fires the card's PumpLink
    found: Box, // VUGCOMSCANMDNSDIAL (SR638): the found rows' buttons, rebuilt when the rows change
    painted: std::rc::Rc<std::cell::RefCell<Vec<crate::widgets::jog_rows::JogRow>>>, // VUGCOMSCANMDNSDIAL (SR638): the found rows the buttons show
}

fn pad_label() -> Label {
    Label::builder().xalign(0.0).wrap(true).build()
}

/// Build the jog pad, append it to `container` and paint its first rows. Keys fire on `synapse` as the local user.
fn jog_pad(container: &Box, synapse: Synapse) -> JogPad {
    use crate::widgets::jog_rows::{declare_verb, key_verb, DECLARE_PLACEHOLDER, KEYS, STEPS_MM}; // VUGCOMSCANDECLARECONSOLEFACE (SR633): declare_verb, DECLARE_PLACEHOLDER
    let who = bandy::Origin::LocalUser(std::env::var("USER").unwrap_or_else(|_| "una".into()));
    let card = std::rc::Rc::new(std::cell::RefCell::new(comscan::PumpCard::new(who)));

    let pad = Box::new(Orientation::Vertical, 4);
    pad.set_margin_start(8);
    pad.set_margin_end(8);
    pad.set_margin_top(4);
    pad.set_margin_bottom(8);
    let (link, travel, position, squawk, why) = (pad_label(), pad_label(), pad_label(), pad_label(), pad_label());
    why.add_css_class("dim-label");

    let step_row = Box::new(Orientation::Horizontal, 4);
    step_row.append(&Label::new(Some("step")));
    let mut steps: Vec<gtk4::ToggleButton> = Vec::with_capacity(STEPS_MM.len());
    for mm in STEPS_MM {
        let b = gtk4::ToggleButton::with_label(&format!("{mm} mm"));
        if let Some(first) = steps.first() {
            b.set_group(Some(first));
        }
        b.set_active(mm == card.borrow().step());
        let c = card.clone();
        b.connect_toggled(move |b| {
            if b.is_active() {
                c.borrow_mut().set_step(mm);
            }
        });
        step_row.append(&b);
        steps.push(b);
    }

    let declare = super::glyph::GlyphEntry::new(DECLARE_PLACEHOLDER); // VUGCOMSCANDECLARECONSOLEFACE (SR633): the house entry, under the squawk row

    let key_row = Box::new(Orientation::Horizontal, 4);
    let mut keys: Vec<gtk4::Button> = Vec::with_capacity(KEYS.len());
    for (k, label) in KEYS {
        let b = gtk4::Button::with_label(label);
        let (c, syn) = (card.clone(), synapse.clone());
        b.connect_clicked(move |_| syn.fire(key_verb(&c.borrow(), k)));
        key_row.append(&b);
        keys.push(b);
    }

    pad.append(&link);
    pad.append(&travel);
    pad.append(&position);
    pad.append(&squawk);
    let (declare_row, discover, found) = discover_row(&declare); pad.append(&declare_row); pad.append(&found); // VUGCOMSCANDECLARECONSOLEFACE (SR633); VUGCOMSCANMDNSDIAL (SR638): the discover button beside the entry, the found rows under it
    pad.append(&step_row);
    pad.append(&key_row);
    pad.append(&why);
    container.append(&pad);

    let p = JogPad { card, link, travel, position, squawk, why, steps, keys, syn: synapse.clone(), found, painted: Default::default() }; // VUGCOMSCANMDNSDIAL (SR638): + syn, found, painted
    p.render();
    p.wire_discover(&discover); // VUGCOMSCANMDNSDIAL (SR638)
    {
        // VUGCOMSCANDECLARECONSOLEFACE (SR633): Enter fires the card's declare when the parser takes the line and clears the
        // entry; a refused line stays for mending (the card says why in the squawk row); the pad repaints either way.
        let (pc, syn) = (p.clone(), synapse.clone());
        declare.connect_activate(move |e| {
            let sent = declare_verb(&mut pc.card.borrow_mut(), &e.text());
            if let Some(m) = sent {
                syn.fire(m);
                e.set_text("");
            }
            pc.render();
        });
    }
    p
}

impl JogPad {
    /// Fold a bus message into the card (pump answers only) and repaint.
    fn hear(&self, msg: &SMessage) {
        if let SMessage::Pump(ev) = msg {
            self.card.borrow_mut().hear(ev);
            self.render();
        }
    }

    /// Paint `jog_rows` over the card's snapshot: the optional rows shown only when present, the picker and keys greyed
    /// while no link is up. The pump's words are sanitized like every Console line.
    fn render(&self) {
        use crate::widgets::jog_rows::{jog_rows, JogRow};
        let rows = jog_rows(&self.card.borrow().snapshot());
        self.position.set_visible(false);
        self.squawk.set_visible(false);
        self.why.set_visible(false);
        for row in rows {
            match row {
                JogRow::Link(t) => self.link.set_label(&t),
                JogRow::Travel(t) => self.travel.set_label(&t),
                JogRow::Position(t) => {
                    self.position.set_label(&t);
                    self.position.set_visible(true);
                }
                JogRow::Squawk(t) => {
                    self.squawk.set_label(&crate::tetra::sanitize_line(&t));
                    self.squawk.set_visible(true);
                }
                JogRow::Why(t) => {
                    self.why.set_label(&t);
                    self.why.set_visible(true);
                }
                JogRow::Steps { enabled, .. } => self.steps.iter().for_each(|b| b.set_sensitive(enabled)),
                JogRow::Pad { enabled, .. } => self.keys.iter().for_each(|b| b.set_sensitive(enabled)),
                JogRow::Found { .. } => {} // VUGCOMSCANMDNSDIAL (SR638): painted together by paint_found
            }
        }
        self.paint_found(); // VUGCOMSCANMDNSDIAL (SR638)
    }
}

// --- VUGCOMSCANMDNSDIAL (LEDGER SR638): the discover button and the found rows a press links (named tail hunk) ---

/// The declare entry with the discover button beside it, and the (empty) box the found rows' buttons go in.
fn discover_row(declare: &super::glyph::GlyphEntry) -> (Box, gtk4::Button, Box) {
    let row = Box::new(Orientation::Horizontal, 4);
    declare.set_hexpand(true);
    row.append(declare);
    let discover = gtk4::Button::with_label(crate::widgets::jog_rows::DISCOVER_LABEL);
    row.append(&discover);
    (row, discover, Box::new(Orientation::Vertical, 2))
}

impl JogPad {
    /// The discover button: the one mDNS question (`comscan::discovery::discover`, which blocks for its wait) on gio's
    /// blocking pool, the answers handed back to this main-loop future, said through the card's `say_found` (which keeps
    /// the telnet entries as rows), then the pad repainted. The card is touched on the main thread only.
    fn wire_discover(&self, discover: &gtk4::Button) {
        let pad = self.clone();
        discover.connect_clicked(move |_| {
            let pad = pad.clone();
            gtk4::glib::spawn_future_local(async move {
                let wait = comscan::discovery::DISCOVER_WAIT;
                let found = gtk4::gio::spawn_blocking(move || comscan::discovery::discover(wait)).await.unwrap_or_default();
                pad.card.borrow_mut().say_found(&found);
                pad.render();
            });
        });
    }

    /// The found rows as buttons, `found_label` each, rebuilt only when the rows changed (so a press never removes its own
    /// button); a press fires the card's PumpLink for that row (`dial_verb`) and the desk's `linked` comes back by `hear`.
    fn paint_found(&self) {
        use crate::widgets::jog_rows::{dial_verb, found_label, jog_rows, JogRow};
        let rows: Vec<JogRow> = jog_rows(&self.card.borrow().snapshot()).into_iter().filter(|r| matches!(r, JogRow::Found { .. })).collect();
        if *self.painted.borrow() == rows {
            return;
        }
        while let Some(child) = self.found.first_child() {
            self.found.remove(&child);
        }
        for (i, row) in rows.iter().enumerate() {
            let JogRow::Found { name, addr, port } = row else { continue };
            let b = gtk4::Button::with_label(&found_label(name, addr, *port));
            let (c, syn) = (self.card.clone(), self.syn.clone());
            b.connect_clicked(move |_| {
                if let Some(m) = dial_verb(&c.borrow(), i) {
                    syn.fire(m);
                }
            });
            self.found.append(&b);
        }
        *self.painted.borrow_mut() = rows;
    }
}
