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

use anyhow::Result;
use bandy::{BandyMember, SMessage};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(feature = "gtk")]
use gtk4::prelude::*;
#[cfg(feature = "gtk")]
use gtk4::{ScrolledWindow, TextBuffer, TextView, Widget};
#[allow(unused_imports)]
use unafs::FileSystem;

#[cfg(feature = "gtk")]
pub fn create_view() -> (Widget, TextBuffer) {
    let scroll = ScrolledWindow::new();
    scroll.set_vexpand(true);
    let view = TextView::builder().monospace(true).editable(false).build();
    view.add_css_class("console");

    let buffer = view.buffer();
    scroll.set_child(Some(&view));

    (scroll.upcast::<Widget>(), buffer)
}

/// The result of running one console command: captured streams plus the
/// process exit status. Console-ready: a view renders `stdout`/`stderr` and
/// keys off `exit_status` for prompt colouring. Built-ins (`cd`, `pwd`)
/// synthesize this same shape so callers never special-case them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    /// Process exit code; `0` for successful built-ins, `1` for their errors,
    /// and `-1` when a command could not be spawned at all.
    pub exit_status: i32,
}

impl CommandOutput {
    fn ok(stdout: impl Into<String>) -> Self {
        Self { stdout: stdout.into(), stderr: String::new(), exit_status: 0 }
    }

    fn err(stderr: impl Into<String>, code: i32) -> Self {
        Self { stdout: String::new(), stderr: stderr.into(), exit_status: code }
    }
}

pub struct Midden {
    // In a real scenario, this might be an Arc<Mutex<FileSystem>>
    // or a channel to the FS actor. For now, we simulate the connection.
    _fs_handle: String,
    current_path: PathBuf,
}

impl Default for Midden {
    fn default() -> Self {
        Self::new()
    }
}

impl Midden {
    /// New console rooted at the process's current working directory (falling
    /// back to `/` if that cannot be read).
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        Self::new_at(cwd)
    }

    /// New console rooted at an explicit directory (used by tests and by hosts
    /// that want to pin the shell somewhere other than the process cwd).
    pub fn new_at(path: impl Into<PathBuf>) -> Self {
        Self {
            _fs_handle: "mount:0".to_string(),
            current_path: path.into(),
        }
    }

    /// The current working directory of this console.
    pub fn cwd(&self) -> &Path {
        &self.current_path
    }

    /// The core loop: run one command line, cwd-relative. `cd` mutates this
    /// `Midden`'s held cwd; `pwd` reports it; everything else spawns the real
    /// program with its working directory set to the held cwd, so `ls`, `echo`,
    /// etc. resolve against the console's location.
    pub fn execute(&mut self, command: &str) -> CommandOutput {
        let parts: Vec<&str> = command.split_whitespace().collect();
        let Some(&program) = parts.first() else {
            return CommandOutput::ok("");
        };
        let args = &parts[1..];

        match program {
            "cd" => self.change_dir(args.first().copied()),
            "pwd" => CommandOutput::ok(format!("{}\n", self.current_path.display())),
            _ => self.run_external(program, args),
        }
    }

    /// `cd` built-in: resolve `dest` against the held cwd (or home-less bare
    /// `cd` → no-op stay), canonicalize, and adopt it iff it is a directory.
    fn change_dir(&mut self, dest: Option<&str>) -> CommandOutput {
        let target = match dest {
            None | Some("") => return CommandOutput::ok(""), // bare `cd`: stay put
            Some(d) => {
                let p = Path::new(d);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    self.current_path.join(p)
                }
            }
        };

        match target.canonicalize() {
            Ok(abs) if abs.is_dir() => {
                self.current_path = abs;
                CommandOutput::ok("")
            }
            Ok(_) => CommandOutput::err(
                format!("cd: not a directory: {}", target.display()),
                1,
            ),
            Err(_) => CommandOutput::err(
                format!("cd: no such file or directory: {}", target.display()),
                1,
            ),
        }
    }

    /// Spawn a real program with its working directory set to the held cwd and
    /// capture both streams and the exit code.
    fn run_external(&self, program: &str, args: &[&str]) -> CommandOutput {
        match Command::new(program)
            .args(args)
            .current_dir(&self.current_path)
            .output()
        {
            Ok(out) => CommandOutput {
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                exit_status: out.status.code().unwrap_or(-1),
            },
            Err(e) => CommandOutput::err(format!("{}: {}", program, e), -1),
        }
    }
}

// Ensure Midden behaves like a nervous system member
impl BandyMember for Midden {
    fn publish(&self, topic: &str, msg: SMessage) -> Result<()> {
        // In the future, Midden will broadcast shell events here
        println!("[MIDDEN] {} -> {:?}", topic, msg);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_writes_stdout_with_zero_exit() {
        let mut m = Midden::new();
        let out = m.execute("echo hello world");
        assert_eq!(out.stdout, "hello world\n");
        assert_eq!(out.stderr, "");
        assert_eq!(out.exit_status, 0);
    }

    #[test]
    fn ls_is_cwd_relative() {
        // Root the console at this crate's own directory (contains Cargo.toml).
        let mut m = Midden::new_at(env!("CARGO_MANIFEST_DIR"));
        let out = m.execute("ls");
        assert_eq!(out.exit_status, 0, "stderr: {}", out.stderr);
        assert!(
            out.stdout.contains("Cargo.toml"),
            "expected Cargo.toml in listing, got: {:?}",
            out.stdout
        );
    }

    #[test]
    fn cd_mutates_cwd_and_is_relative() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut m = Midden::new_at(&root);

        // `cd src` should move into the src/ subdirectory.
        let out = m.execute("cd src");
        assert_eq!(out.exit_status, 0, "stderr: {}", out.stderr);
        assert_eq!(m.cwd(), root.join("src").canonicalize().unwrap());

        // A bogus target leaves cwd unchanged and reports on stderr.
        let before = m.cwd().to_path_buf();
        let out = m.execute("cd definitely_not_a_real_dir_xyz");
        assert_eq!(out.exit_status, 1);
        assert!(out.stderr.contains("cd:"));
        assert_eq!(m.cwd(), before);
    }
}
