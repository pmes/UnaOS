use async_channel::Sender;
use elessar::gneiss_pal::Event;
use gtk4::prelude::*;
use gtk4::{Box, Button, Orientation, Widget};
use anyhow::{Context as AnyhowContext, Result};
use bandy::{SMessage, BandyMember};
use elessar::{Context, Spline};
use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader};
use std::thread;

pub fn create_view(tx: Sender<Event>) -> Widget {
    let aule_box = Box::new(Orientation::Vertical, 10);
    aule_box.set_margin_top(20);

    let ignite_btn = Button::with_label("Ignite");
    ignite_btn.set_icon_name("applications-engineering-symbolic");
    ignite_btn.add_css_class("suggested-action");

    let tx_clone = tx.clone();
    ignite_btn.connect_clicked(move |_| {
        let _ = tx_clone.send_blocking(Event::AuleIgnite);
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
    /// Output is streamed via Bandy.
    pub fn forge(&self) -> Result<()> {
        let (program, args) = match self.context.spline {
            Spline::UnaOS | Spline::Rust => ("cargo", vec!["build"]),
            Spline::Web => ("npm", vec!["run", "build"]),
            Spline::Python => ("python", vec!["setup.py", "build"]), // Or pip
            Spline::Void => return Ok(()), // Nothing to build
        };

        println!("[AULE] Forging with: {} {:?}", program, args);

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
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if let Ok(l) = line {
                    // In a real system, we'd emit this to Bandy
                    println!("[BUILD::OUT] {}", l);
                }
            }
        });

        // Spawn thread to stream STDERR
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(l) = line {
                    println!("[BUILD::ERR] {}", l);
                }
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
