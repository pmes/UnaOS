// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! End to end through the real binary: `holocron daemon`, then the CLI verbs over its bus socket, then
//! the SSH agent socket checked by an independent client (tests/agent_oracle.py: RFC 8032 §6's reference verifier) and
//! by `ssh-add -l` when OpenSSH is installed (skipped, and said so, when it is not).
//!
//! Every child gets explicit piped stdio: nothing here opens /dev/null.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_holocron");

fn home() -> PathBuf {
    // Unix socket paths are limited to 108 bytes; keep the root short.
    let p = std::env::temp_dir().join(format!("hcr-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

fn cli(root: &Path, args: &[&str], stdin: &[u8]) -> Output {
    let mut c = Command::new(BIN)
        .args(args)
        .env("HOLOCRON_HOME", root)
        .env_remove("HOLOCRON_SOCK")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(stdin).unwrap();
    c.wait_with_output().unwrap()
}

fn ok(o: &Output) -> String {
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout.clone()).unwrap()
}

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        // Child::kill is kill(pid, SIGKILL) on this child's own PID.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn which(tool: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?.to_str()?.split(':').map(|d| Path::new(d).join(tool)).find(|p| p.is_file())
}

#[test]
fn cli_daemon_bus_and_agent() {
    let root = home();
    let d = Daemon(
        Command::new(BIN)
            .args(["daemon", "--idle", "600"])
            .env("HOLOCRON_HOME", &root)
            .env_remove("HOLOCRON_SOCK")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let bus = root.join(".bus.sock");
    let agent = root.join(".agent.sock");
    let t0 = Instant::now();
    while !(bus.exists() && agent.exists()) {
        assert!(t0.elapsed() < Duration::from_secs(10), "daemon did not bind");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ok(&cli(&root, &["status"], b"")).starts_with("no ring owner=user:"));
    ok(&cli(&root, &["init"], b"pw\npw\n"));
    // THE brief's command: `holocron put vein claude.api_key`.
    ok(&cli(&root, &["put", "vein", "claude.api_key", "--kind", "api-key", "--label", "Claude"], b"sk-ant-cli\n"));
    assert_eq!(ok(&cli(&root, &["get", "vein", "claude.api_key"], b"")), "sk-ant-cli");
    let l = ok(&cli(&root, &["list", "vein"], b""));
    assert!(l.starts_with("claude.api_key\tapi-key\t") && l.trim_end().ends_with("\tClaude"), "{l}");
    // On disk: sealed, 0600, under <root>/vein/.
    let f = std::fs::read(root.join("vein/claude.api_key")).unwrap();
    assert!(!f.windows(10).any(|w| w == b"sk-ant-cli"));
    // An Ed25519 key, minted inside Holocron, and a signature over stdin.
    ok(&cli(&root, &["keygen", "id_ed25519", "--label", "peter@host"], b""));
    let sig = ok(&cli(&root, &["sign", "id_ed25519"], b"hello"));
    assert_eq!(sig.trim().len(), 128);
    let env = ok(&cli(&root, &["agent-env"], b""));
    assert!(env.contains(&agent.display().to_string()));

    // The agent socket, checked from outside Holocron's code.
    let py_ok = Command::new("python3")
        .args(["-c", "import hashlib, socket"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if py_ok {
        let o = Command::new("python3")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/agent_oracle.py"))
            .arg(&agent)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        let out = ok(&o);
        eprintln!("agent oracle:\n{out}");
        assert!(out.contains("peter@host") && out.trim_end().ends_with("identities=1"), "{out}");
    } else {
        eprintln!("agent oracle: SKIPPED (no python3)");
    }
    match which("ssh-add") {
        Some(ssh_add) => {
            let o = Command::new(ssh_add)
                .arg("-l")
                .env("SSH_AUTH_SOCK", &agent)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .unwrap();
            let out = ok(&o);
            eprintln!("ssh-add -l: {out}");
            assert!(out.contains("(ED25519)") && out.contains("peter@host"), "{out}");
        }
        None => eprintln!("ssh-add -l: SKIPPED (OpenSSH not installed in this container)"),
    }

    // Lock: the CLI names the fix; status says locked.
    ok(&cli(&root, &["lock"], b""));
    assert!(ok(&cli(&root, &["status"], b"")).starts_with("locked "));
    let g = cli(&root, &["get", "vein", "claude.api_key"], b"");
    assert!(!g.status.success() && String::from_utf8_lossy(&g.stderr).contains("holocron unlock"));
    let w = cli(&root, &["unlock"], b"nope\n");
    assert!(String::from_utf8_lossy(&w.stderr).contains("wrong password"));
    ok(&cli(&root, &["unlock"], b"pw\n"));
    assert_eq!(ok(&cli(&root, &["get", "vein", "claude.api_key"], b"")), "sk-ant-cli");
    ok(&cli(&root, &["delete", "vein", "claude.api_key"], b""));
    assert!(!cli(&root, &["get", "vein", "claude.api_key"], b"").status.success());
    drop(d);
    let _ = std::fs::remove_dir_all(&root);
}
