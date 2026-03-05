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

use chrono::Local;
use log::{Level, LevelFilter, Metadata, Record};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub struct UnaLogger {
    log_dir: PathBuf,
}

impl log::Log for UnaLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Debug // Unchoked from Info
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let target = record.target().split("::").next().unwrap_or("system");
            let safe_target = target.replace(|c: char| !c.is_alphanumeric(), "_");
            let log_file = self.log_dir.join(format!("{}.log", safe_target));

            let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            let msg = format!("[{}] [{}] {}\n", timestamp, record.level(), record.args());

            // Echo to stdout for the Architect
            print!("{}: {}", target.to_uppercase(), msg);

            // Route to specific subsystem log. Do not swallow errors silently.
            match OpenOptions::new().create(true).append(true).open(&log_file) {
                Ok(mut file) => {
                    if let Err(e) = file.write_all(msg.as_bytes()) {
                        eprintln!(
                            ">> [TELEMETRY FAULT] Failed to write to {}: {}",
                            log_file.display(),
                            e
                        );
                    }
                }
                Err(e) => {
                    eprintln!(
                        ">> [TELEMETRY FAULT] Failed to open {}: {}",
                        log_file.display(),
                        e
                    );
                }
            }
        }
    }

    fn flush(&self) {}
}

/// Ignites the autonomic telemetry routing system.
pub fn ignite(log_dir: PathBuf) {
    if !log_dir.exists() {
        if let Err(e) = fs::create_dir_all(&log_dir) {
            eprintln!(
                ">> [CRITICAL] Failed to construct telemetry vault at {}: {}",
                log_dir.display(),
                e
            );
            return;
        }
    }

    let logger = Box::new(UnaLogger { log_dir });
    log::set_boxed_logger(logger)
        .map(|()| log::set_max_level(LevelFilter::Debug)) // Unchoke the output
        .expect("Nervous system logging failed to ignite");
}
