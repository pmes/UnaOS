// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `holocron` — the command line over Holocron (CODEX §2, The Key). HOLOCRON1, LEDGER SR33.
//!
//! ```text
//! holocron daemon [--idle <secs>]          run the keyring (bus socket + SSH agent socket)
//! holocron init                            create the ring (password twice on stdin)
//! holocron unlock | lock | status
//! holocron put <ns> <name> [--kind K] [--label L]   the secret is stdin (one trailing newline dropped)
//! holocron get <ns> <name>                 the secret to stdout
//! holocron list <ns>
//! holocron delete <ns> <name>
//! holocron keygen <name> [--label L]       mint an Ed25519 key in ssh/<name>
//! holocron sign <name>                     sign stdin with ssh/<name>; hex signature to stdout
//! holocron agent-env                       `SSH_AUTH_SOCK=…; export SSH_AUTH_SOCK`
//! ```
//!
//! The root is `$HOLOCRON_HOME`, else `$HOME/.holocron`; the bus socket `$HOLOCRON_SOCK`, else
//! `<root>/.bus.sock`. Example: `holocron put vein claude.api_key --kind api-key --label Claude`.

use holocron::client::{self, Client};
use holocron::daemon::{self, Shared};
use holocron::holocron_core::format;
use holocron::holocron_core::seal::KdfParams;
use holocron::holocron_core::service::Holocron;
use holocron::holocron_core::wire::{self, Request, RingState, status};
use holocron::store::{self, DirStore};
use holocron::{HostSealer, HostSigner, principal};
use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;
use std::time::Duration;

fn usage() -> ExitCode {
    eprintln!("usage: holocron daemon [--idle S] | init | unlock | lock | status | put <ns> <name> [--kind K] [--label L] | get <ns> <name> | list <ns> | delete <ns> <name> | keygen <name> [--label L] | sign <name> | agent-env");
    ExitCode::from(2)
}

fn status_name(s: i32) -> &'static str {
    match s {
        status::BAD_PASSWORD => "wrong password",
        status::NOT_FOUND => "not found",
        status::IO => "store I/O error",
        status::RATE_LIMITED => "too many attempts; wait and retry",
        status::DENIED => "denied: this user does not own the ring",
        status::EXISTS => "a ring already exists",
        status::NO_RING => "no ring: run `holocron init`",
        status::INVALID => "invalid request",
        status::TOO_BIG => "secret too large",
        status::LOCKED => "locked: run `holocron unlock`",
        status::CORRUPT => "stored file failed to authenticate",
        status::NO_VERB => "unknown verb",
        _ => "error",
    }
}

/// Split `args` into positionals and `--flag value` pairs.
fn parse(args: &[String]) -> Option<(Vec<&str>, Vec<(&str, &str)>)> {
    let mut pos = Vec::new();
    let mut flags = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if let Some(f) = args[i].strip_prefix("--") {
            flags.push((f, args.get(i + 1)?.as_str()));
            i += 2;
        } else {
            pos.push(args[i].as_str());
            i += 1;
        }
    }
    Some((pos, flags))
}

fn flag<'a>(flags: &[(&str, &'a str)], name: &str) -> Option<&'a str> {
    flags.iter().find(|(f, _)| *f == name).map(|(_, v)| *v)
}

fn stty(arg: &str) {
    let _ = std::process::Command::new("stty").arg(arg).stdin(std::process::Stdio::inherit()).status();
}

/// One line from stdin, echo off on a terminal.
fn read_password(prompt: &str) -> Vec<u8> {
    let tty = std::io::stdin().is_terminal();
    if tty {
        eprint!("{prompt}");
        stty("-echo");
    }
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    if tty {
        stty("echo");
        eprintln!();
    }
    let pw = line.trim_end_matches(['\n', '\r']).as_bytes().to_vec();
    let mut raw = line.into_bytes();
    holocron::holocron_core::zero::wipe(&mut raw);
    pw
}

fn connect() -> Result<Client, ExitCode> {
    let Some(sock) = client::default_socket() else {
        eprintln!("holocron: no $HOME or $HOLOCRON_HOME");
        return Err(ExitCode::from(1));
    };
    Client::connect(&sock).map_err(|e| {
        eprintln!("holocron: cannot reach {} ({e}); start it with `holocron daemon`", sock.display());
        ExitCode::from(1)
    })
}

fn call(req: Request) -> Result<wire::Reply, ExitCode> {
    let mut c = connect()?;
    let r = c.call(&req).map_err(|e| {
        eprintln!("holocron: {e}");
        ExitCode::from(1)
    })?;
    if r.status != status::OK {
        eprintln!("holocron: {} ({})", status_name(r.status), r.status);
        return Err(ExitCode::from(1));
    }
    Ok(r)
}

fn run_daemon(idle: Option<Duration>) -> ExitCode {
    let Some(root) = store::default_root() else { return usage() };
    let Some(owner) = principal::my_principal() else {
        eprintln!("holocron: this uid has no valid passwd name; refusing to serve an unnamed principal");
        return ExitCode::from(1);
    };
    let Ok(rng) = holocron::host_entropy() else {
        eprintln!("holocron: /dev/urandom cannot seed the DRBG; refusing to start");
        return ExitCode::from(1);
    };
    let mut ds = DirStore::new(&root);
    if let Err(e) = holocron::holocron_core::service::Store::read_ring(&mut ds) {
        eprintln!("holocron: root {} refused ({e:?})", root.display());
        return ExitCode::from(1);
    }
    // Make the root now so the sockets can live in it (0700).
    if std::fs::symlink_metadata(&root).is_err() {
        let _ = std::fs::create_dir_all(&root);
        let _ = std::fs::set_permissions(&root, std::os::unix::fs::PermissionsExt::from_mode(0o700));
    }
    let svc = Holocron::new(HostSealer::default(), HostSigner::default(), ds, rng, owner.clone(), KdfParams::DEFAULT);
    let sh = Shared::new(svc, idle);
    let bus = daemon::bus_socket(&root);
    let agent = daemon::agent_socket(&root);
    eprintln!(":: HOLOCRON: suite=argon2id+chacha20poly1305 owner={owner} root={} bus={} agent={} ::", root.display(), bus.display(), agent.display());
    match daemon::run(sh, &bus, Some(&agent)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("holocron: {e}");
            ExitCode::from(1)
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((pos, flags)) = parse(&args) else { return usage() };
    let r: Result<(), ExitCode> = (|| match pos.as_slice() {
        ["daemon"] => {
            let idle = flag(&flags, "idle").and_then(|s| s.parse().ok()).map(Duration::from_secs);
            Err(run_daemon(idle))
        }
        ["init"] => {
            let a = read_password("new ring password: ");
            let b = read_password("again: ");
            if a != b || a.is_empty() {
                eprintln!("holocron: passwords differ or are empty");
                return Err(ExitCode::from(1));
            }
            call(Request::Unlock { create: true, password: a })?;
            eprintln!("holocron: ring created (unlocked)");
            Ok(())
        }
        ["unlock"] => call(Request::Unlock { create: false, password: read_password("ring password: ") }).map(|_| ()),
        ["lock"] => call(Request::Lock).map(|_| ()),
        ["status"] => {
            let r = call(Request::Status)?;
            let (st, suite, owner) = wire::decode_status(&r.body).ok_or(ExitCode::from(1))?;
            let st = match st {
                RingState::NoRing => "no ring",
                RingState::Locked => "locked",
                RingState::Unlocked => "unlocked",
            };
            let suite = if suite == format::SUITE_TEST_INSECURE { "TEST-INSECURE" } else { "argon2id+chacha20poly1305" };
            println!("{st} owner={owner} suite={suite}");
            Ok(())
        }
        ["put", ns, name] => {
            let mut data = Vec::new();
            std::io::stdin().read_to_end(&mut data).map_err(|_| ExitCode::from(1))?;
            if data.last() == Some(&b'\n') {
                data.pop();
                if data.last() == Some(&b'\r') {
                    data.pop();
                }
            }
            let kind = flag(&flags, "kind").unwrap_or("secret").to_string();
            let label = flag(&flags, "label").unwrap_or("").to_string();
            call(Request::Put { ns: ns.to_string(), name: name.to_string(), kind, label, data }).map(|_| ())
        }
        ["get", ns, name] => {
            let r = call(Request::Get { ns: ns.to_string(), name: name.to_string() })?;
            std::io::stdout().write_all(&r.body).map_err(|_| ExitCode::from(1))
        }
        ["list", ns] => {
            let r = call(Request::List { ns: ns.to_string() })?;
            for e in wire::decode_list(&r.body).ok_or(ExitCode::from(1))? {
                println!("{}\t{}\t{}\t{}", e.name, e.meta.kind, e.meta.created, e.meta.label);
            }
            Ok(())
        }
        ["delete", ns, name] => call(Request::Delete { ns: ns.to_string(), name: name.to_string() }).map(|_| ()),
        ["keygen", name] => {
            let label = flag(&flags, "label").unwrap_or("").to_string();
            call(Request::Put { ns: "ssh".into(), name: name.to_string(), kind: "ssh-ed25519".into(), label, data: vec![] }).map(|_| ())
        }
        ["sign", name] => {
            let mut data = Vec::new();
            std::io::stdin().read_to_end(&mut data).map_err(|_| ExitCode::from(1))?;
            let r = call(Request::Sign { key: name.to_string(), data })?;
            println!("{}", hex(&r.body));
            Ok(())
        }
        ["agent-env"] => {
            let root = store::default_root().ok_or(ExitCode::from(1))?;
            println!("SSH_AUTH_SOCK={}; export SSH_AUTH_SOCK;", daemon::agent_socket(&root).display());
            Ok(())
        }
        _ => Err(usage()),
    })();
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(c) => c,
    }
}
