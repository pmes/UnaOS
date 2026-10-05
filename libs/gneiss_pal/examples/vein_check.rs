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

//! vein-check — one live round-trip through a Vein provider, streamed to the
//! terminal (CLAUDECODE, SR38).
//!
//! ```text
//! cargo run -p gneiss_pal --example vein_check -- --provider claudecode [--bin PATH] [--model M] \
//!     [--turns 2] [--system TEXT] ["prompt"]
//! cargo run -p gneiss_pal --example vein_check -- --provider claude  ["prompt"]   # ANTHROPIC_API_KEY
//! ```
//!
//! The provider is built exactly as Vein builds it (`ProviderConfig::from_prefs`
//! + `build_provider`), with the flags standing in for the `vein` preferences.
//! `--turns N` sends follow-ups in the same conversation, built the way Vein
//! builds it (VEINTURNS SR42: a `Thread` — every turn its own message). For
//! `claudecode` every turn after the first must report `resumed: true` and the
//! SAME session id (one CLI session); the run ends with a `session:` summary
//! and exits 1 if the id changed. Exit 0 = every turn streamed a reply; 1 = a
//! provider error (printed verbatim) or a broken session; 2 = usage.

use std::process::ExitCode;

use bandy::PrefValue;
use futures_util::StreamExt;
use gneiss_pal::api::{
    ChatDelta, ClaudeCodeProvider, ModelProvider, Part, ProviderConfig, ProviderKind, Thread, build_provider,
};

fn usage() -> ExitCode {
    eprintln!(
        "usage: vein_check --provider <claude|gemini|claudecode> [--model M] [--bin PATH] [--turns N] [--system TEXT] [prompt]"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut prefs: Vec<(String, String)> = Vec::new();
    let mut turns = 1usize;
    let mut system: Option<String> = None;
    let mut prompt: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |k: &str| args.next().map(|v| prefs.push((k.into(), v))).is_some();
        let ok = match a.as_str() {
            "--provider" => val("provider"),
            "--model" => val("model"),
            "--bin" => val("claudecode.bin"),
            "--turns" => match args.next().and_then(|n| n.parse().ok()) {
                Some(n) if n >= 1 => {
                    turns = n;
                    true
                }
                _ => false,
            },
            "--system" => {
                system = args.next();
                system.is_some()
            }
            "-h" | "--help" => false,
            p if !p.starts_with("--") && prompt.is_none() => {
                prompt = Some(p.to_string());
                true
            }
            _ => false,
        };
        if !ok {
            return usage();
        }
    }
    let get = |k: &str| prefs.iter().rev().find(|(pk, _)| pk == k).map(|(_, v)| PrefValue::Str(v.clone()));
    let cfg = match ProviderConfig::from_prefs(get) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("vein-check: {e}");
            return ExitCode::from(1);
        }
    };
    let cc = (cfg.kind == ProviderKind::ClaudeCode).then(|| ClaudeCodeProvider::new(cfg.claudecode_bin.clone(), cfg.model.clone()));
    let boxed;
    let provider: &dyn ModelProvider = match &cc {
        Some(p) => p,
        None => match build_provider(&cfg) {
            Ok(p) => {
                boxed = p;
                boxed.as_ref()
            }
            Err(e) => {
                eprintln!("vein-check: {e}");
                return ExitCode::from(1);
            }
        },
    };
    println!("vein-check: provider {} / model {}", provider.name(), provider.model());
    if let Some(p) = &cc {
        let path = std::env::var_os("PATH");
        match gneiss_pal::api::claudecode::resolve_bin(p.bin(), path.as_deref()) {
            Some(b) => println!("vein-check: CLI {}", b.display()),
            None => println!("vein-check: CLI `{}` not found on PATH (the first turn will say so)", p.bin()),
        }
    }
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("vein-check: no runtime: {e}");
            return ExitCode::from(1);
        }
    };
    let first = prompt.unwrap_or_else(|| "Reply with one short sentence: which model are you?".into());
    let mut thread = Thread::new();
    let mut ids: Vec<String> = Vec::new();
    for turn in 1..=turns {
        let text = if turn == 1 {
            first.clone()
        } else {
            format!("Repeat your previous reply word for word, then add: (turn {turn}).")
        };
        let user = vec![Part::text(text)];
        let mut req = thread.request(system.clone(), user.clone());
        req.max_tokens = cfg.max_tokens;
        req.temperature = cfg.temperature;
        println!("── turn {turn} ({} messages) ──", req.messages.len());
        let reply = rt.block_on(async {
            let mut s = provider.stream(&req).await?;
            let mut text = String::new();
            while let Some(d) = s.next().await {
                match d? {
                    ChatDelta::Text(t) => {
                        print!("{t}");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                        text.push_str(&t);
                    }
                    ChatDelta::Stop { stop, usage } => {
                        println!("\n[stop: {stop:?} · in {} / out {} tokens]", usage.input_tokens, usage.output_tokens);
                    }
                }
            }
            Ok::<_, gneiss_pal::api::ProviderError>(text)
        });
        match reply {
            Ok(text) => {
                if let Some(info) = cc.as_ref().and_then(ClaudeCodeProvider::last_run) {
                    println!(
                        "[claudecode: session {} · resumed: {} · model {} · CLI {} · {} ms · ${:.4}]",
                        info.session_id.as_deref().unwrap_or("?"),
                        info.resumed,
                        info.model.as_deref().unwrap_or("?"),
                        info.cli_version.as_deref().unwrap_or("?"),
                        info.duration_ms.unwrap_or(0),
                        info.total_cost_usd.unwrap_or(0.0)
                    );
                }
                if let Some(s) = provider.session() {
                    ids.push(s.id);
                }
                thread.record(user, &text);
            }
            Err(e) => {
                println!("\nvein-check: {} :: {e}", provider.name());
                return ExitCode::from(1);
            }
        }
    }
    if !ids.is_empty() {
        ids.dedup();
        if ids.len() == 1 {
            println!("session: one id across {turns} turns: {}", ids[0]);
        } else {
            println!("session: BROKEN — {} ids across {turns} turns: {}", ids.len(), ids.join(", "));
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}
