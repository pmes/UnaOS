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

use anyhow::{Context as AnyhowContext, Result};
use bandy::{BandyMember, SMessage};
use elessar::{Context, Spline};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

#[cfg(feature = "gtk")]
use async_channel::Sender;
#[cfg(feature = "gtk")]
use gtk4::prelude::*;
#[cfg(feature = "gtk")]
use gtk4::{Box, Button, Orientation, Widget};

#[cfg(feature = "gtk")]
pub fn create_view(tx: Sender<SMessage>) -> Widget {
    let aule_box = Box::new(Orientation::Vertical, 10);
    aule_box.set_margin_top(20);

    let ignite_btn = Button::with_label("Ignite");
    ignite_btn.set_icon_name("applications-engineering-symbolic");
    ignite_btn.add_css_class("suggested-action");

    let tx_clone = tx.clone();
    ignite_btn.connect_clicked(move |_| {
        let _ = tx_clone.send_blocking(SMessage::AuleIgnite);
    });

    aule_box.append(&ignite_btn);
    aule_box.upcast::<Widget>()
}

pub struct Aule {
    context: Context,
}

impl Aule {
    pub fn new(path: &std::path::Path) -> Self {
        let context = Context::new(path);
        Self { context }
    }

    /// The core function: Spawns a build process based on the Spline.
    /// Returns immediately; the process runs in a background thread.
    /// Output is streamed to stdout (legacy behavior, preserved).
    pub fn forge(&self) -> Result<()> {
        let (tx, rx) = mpsc::channel::<String>();
        self.forge_streamed(tx)?;
        // Drain the stream to stdout so the historical println-based API keeps
        // working for callers that have no channel of their own.
        thread::spawn(move || {
            for line in rx {
                println!("{}", line);
            }
        });
        Ok(())
    }

    /// Streaming variant of [`Aule::forge`]: spawns the same build process, but
    /// emits each stdout/stderr line (and the initial forge banner) as a plain
    /// `String` on `tx` instead of printing. Returns immediately; the process
    /// and its two reader threads run in the background. The vessel wraps these
    /// lines into whatever signal it routes.
    ///
    /// `std::sync::mpsc::Sender<String>` is used deliberately: it matches aule's
    /// existing `std::thread` reader structure with no async runtime. stdout and
    /// stderr lines are interleaved on the one channel in arrival order.
    pub fn forge_streamed(&self, tx: mpsc::Sender<String>) -> Result<()> {
        let (program, args) = match self.context.spline {
            Spline::UnaOS | Spline::Rust => ("cargo", vec!["build"]),
            Spline::Web => ("npm", vec!["run", "build"]),
            Spline::Python => ("python", vec!["setup.py", "build"]), // Or pip
            Spline::Void => return Ok(()),                           // Nothing to build
        };

        let _ = tx.send(format!("[AULE] Forging with: {} {:?}", program, args));

        // J15 SPECIALTY: Process Management
        let mut child = Command::new(program)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn build process")?;

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        // Spawn thread to stream STDOUT
        let tx_out = tx.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(|l| l.ok()) {
                let _ = tx_out.send(line);
            }
        });

        // Spawn thread to stream STDERR
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(|l| l.ok()) {
                let _ = tx.send(line);
            }
        });

        Ok(())
    }
}

impl BandyMember for Aule {
    fn publish(&self, topic: &str, msg: SMessage) -> Result<()> {
        println!("[AULE] {} -> {:?}", topic, msg);
        Ok(())
    }
}
