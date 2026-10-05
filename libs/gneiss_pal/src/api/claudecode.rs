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

//! The Claude Code provider (CLAUDECODE, LEDGER SR38): Vein's route through a
//! Claude subscription.
//!
//! A subscription (Pro / Max) carries no API key. The sanctioned way a program
//! uses one is the Claude Code CLI logged into that account, driven headless:
//!
//! ```text
//! claude -p --output-format stream-json --verbose --include-partial-messages \
//!        --tools "" [--model M] [--system-prompt S | --system-prompt-file F] \
//!        [--resume <session_id>]          < prompt on stdin
//! ```
//!
//! The CLI is the client; Vein is its caller. Nothing here reads the CLI's
//! credential store, lifts an OAuth token, or talks to claude.ai: the binary
//! does all of that itself, and this provider only spawns it and parses what
//! it prints.
//!
//! What the CLI prints (recorded from Claude Code 2.1.289, fixtures in
//! `tests/fixtures/claudecode/`) is newline-delimited JSON:
//!
//! - `{"type":"system","subtype":"init","session_id":...,"model":...}` — the
//!   session this run belongs to;
//! - `{"type":"stream_event","event":{...}}` — the Messages-API stream events
//!   (`message_start`, `content_block_delta`/`text_delta`, `message_delta`,
//!   `message_stop`) when `--include-partial-messages` is on: the incremental
//!   text Vein streams;
//! - `{"type":"assistant","message":{...}}` — each complete assistant message
//!   (its text is used only when no partial deltas arrived for that message);
//! - `{"type":"system","subtype":"api_retry","error_status":401,...}` — the CLI
//!   retrying an API failure (kept as the context of a later error);
//! - `{"type":"result","is_error":...,"result":...,"stop_reason":...,
//!   "duration_ms":...,"total_cost_usd":...,"usage":{...}}` — the end of the run.
//!
//! Session continuity: a conversation stays ONE CLI session. After a turn
//! completes, the provider remembers the session id together with a
//! fingerprint of the conversation it has seen (the request's messages plus
//! the reply). A later request whose messages START with exactly that
//! conversation and continue with the person's new turn(s) is sent as
//! `--resume <id>` with only the new turn(s) on stdin. Any other request
//! (a different conversation, an edited history) starts a fresh session with
//! the whole transcript flattened into the prompt.
//!
//! Tools are OFF (`--tools ""`): Vein asks for a chat reply, never for the CLI
//! to run commands or edit files on the person's machine.
//!
//! Not carried: `max_tokens` and `temperature` (the CLI has no flags for
//! them; the plan's model defaults apply) and attachments (a
//! [`Part::FileData`] is a [`ProviderError::Unsupported`] refusal, never a
//! silent drop).
//!
//! Errors, all shown verbatim in-chat by Lumen:
//! - the binary is missing → [`ProviderError::Config`] naming the path and
//!   `vein.claudecode.bin`;
//! - the CLI is not logged in (its own line, quoted) → [`ProviderError::Config`];
//! - a `result` with `is_error` → classified by `api_error_status`
//!   (401/403 = the login, 429/5xx = [`ProviderError::Retryable`], other 4xx =
//!   [`ProviderError::Request`]) or [`ProviderError::Cli`];
//! - a non-zero exit without a result → [`ProviderError::Cli`] carrying the
//!   exit code and the CLI's stderr.

use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};

use super::Part;
use super::claude::map_stop;
use super::provider::{
    BoxFuture, ChatDelta, ChatMessage, ChatRequest, ChatResponse, DeltaStream, ModelProvider, ProviderError, Role,
    StopReason, Usage,
};

/// The binary looked up on `PATH` when `vein.claudecode.bin` is not set.
pub const CLAUDECODE_DEFAULT_BIN: &str = "claude";

/// The model value meaning "do not pass `--model`": the CLI's (the plan's)
/// own default.
pub const CLAUDECODE_DEFAULT_MODEL: &str = "default";

/// Variables a PARENT Claude Code session exports that bind a child `claude`
/// to the parent's session (observed: with them set, `-p` reports the
/// parent's `session_id`, and `--resume` would continue the parent's
/// transcript). The provider removes them from the child's environment so a
/// Vein conversation is always its own session. Nothing else is touched: the
/// CLI's own login (its config dir, or a host-managed credential) is inherited.
pub const INHERITED_SESSION_VARS: &[&str] =
    &["CLAUDE_CODE_SESSION_ID", "CLAUDE_CODE_REMOTE_SESSION_ID", "CLAUDE_CODE_CHILD_SESSION"];

/// System prompts longer than this go through `--system-prompt-file` (a
/// single argv string is capped at 128 KiB on Linux).
pub const SYSTEM_ARG_MAX: usize = 64 * 1024;

/// The facts of the last completed run (for a status line or a check tool).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunInfo {
    pub session_id: Option<String>,
    /// The model the CLI reported in its `init` event.
    pub model: Option<String>,
    pub cli_version: Option<String>,
    pub duration_ms: Option<u64>,
    pub total_cost_usd: Option<f64>,
    /// True when this run continued a session with `--resume`.
    pub resumed: bool,
}

/// The remembered CLI session: its id, and the conversation it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Session {
    id: String,
    /// [`fingerprint`] of the conversation (request messages + the reply).
    seen: u64,
    /// How many messages that conversation has.
    len: usize,
}

/// One planned CLI invocation (public so tests and a check tool can show it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// Arguments after the binary.
    pub args: Vec<String>,
    /// What is written to the CLI's stdin (the prompt).
    pub stdin: String,
    /// The session id passed to `--resume`, if this continues one.
    pub resume: Option<String>,
    /// A system prompt too long for argv, to be written to a file whose path
    /// replaces [`SYSTEM_FILE_PLACEHOLDER`] in `args`.
    pub system_file: Option<String>,
}

/// Stands in for the temp-file path in [`Invocation::args`].
pub const SYSTEM_FILE_PLACEHOLDER: &str = "<system-prompt-file>";

pub struct ClaudeCodeProvider {
    bin: String,
    model: String,
    env: Vec<(OsString, OsString)>,
    cwd: Option<PathBuf>,
    session: Arc<Mutex<Option<Session>>>,
    last: Arc<Mutex<Option<RunInfo>>>,
}

impl ClaudeCodeProvider {
    /// `bin`: a path, or a name looked up on `PATH` (blank = `claude`).
    /// `model`: a model id or alias the CLI accepts (blank or `default` = the
    /// CLI's own default).
    pub fn new(bin: impl Into<String>, model: impl Into<String>) -> Self {
        let bin = bin.into();
        let bin = if bin.trim().is_empty() { CLAUDECODE_DEFAULT_BIN.to_string() } else { bin.trim().to_string() };
        let model = model.into();
        let model = if model.trim().is_empty() { CLAUDECODE_DEFAULT_MODEL.to_string() } else { model.trim().to_string() };
        ClaudeCodeProvider {
            bin,
            model,
            env: Vec::new(),
            cwd: None,
            session: Arc::new(Mutex::new(None)),
            last: Arc::new(Mutex::new(None)),
        }
    }

    /// Set a variable in the child's environment (tests: a `PATH` holding a
    /// fake `claude`).
    pub fn with_env(mut self, k: impl Into<OsString>, v: impl Into<OsString>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }

    /// Run the CLI in this directory (default: the caller's). The CLI keys
    /// its saved sessions by working directory, so every run of one provider
    /// uses the same one.
    pub fn with_cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn bin(&self) -> &str {
        &self.bin
    }

    /// The CLI session the next continuing request would resume.
    pub fn session_id(&self) -> Option<String> {
        self.session.lock().ok().and_then(|s| s.as_ref().map(|s| s.id.clone()))
    }

    /// Forget the session: the next request starts a fresh one.
    pub fn reset_session(&self) {
        if let Ok(mut s) = self.session.lock() {
            *s = None;
        }
    }

    /// The last completed run's facts.
    pub fn last_run(&self) -> Option<RunInfo> {
        self.last.lock().ok().and_then(|l| l.clone())
    }

    /// The invocation `req` becomes, given the remembered session.
    pub fn plan(&self, req: &ChatRequest) -> Result<Invocation, ProviderError> {
        let session = self.session.lock().ok().and_then(|s| s.clone());
        plan_invocation(req, &self.model, session.as_ref())
    }

    async fn spawn(&self, req: &ChatRequest) -> Result<Run, ProviderError> {
        let inv = self.plan(req)?;
        let sysfile = match &inv.system_file {
            Some(text) => Some(TempFile::write(text)?),
            None => None,
        };
        let mut cmd = Command::new(&self.bin);
        for a in &inv.args {
            match (&sysfile, a.as_str()) {
                (Some(f), SYSTEM_FILE_PLACEHOLDER) => cmd.arg(&f.0),
                _ => cmd.arg(a),
            };
        }
        for v in INHERITED_SESSION_VARS {
            cmd.env_remove(v);
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        if let Some(d) = &self.cwd {
            cmd.current_dir(d);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| spawn_error(&self.bin, &e))?;
        // Feed the prompt and drain stderr off the read path: a child blocked
        // on a full pipe must never stall the stream.
        if let Some(mut stdin) = child.stdin.take() {
            let prompt = inv.stdin.clone().into_bytes();
            tokio::spawn(async move {
                let _ = stdin.write_all(&prompt).await;
                let _ = stdin.shutdown().await;
            });
        }
        let stderr = child.stderr.take().map(|mut e| {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let _ = e.read_to_end(&mut buf).await;
                String::from_utf8_lossy(&buf).into_owned()
            })
        });
        let stdout = child.stdout.take().ok_or_else(|| ProviderError::Malformed("no stdout from the CLI".into()))?;
        Ok(Run {
            child,
            lines: BufReader::new(stdout).lines(),
            stderr,
            parser: CliEvents { resumed: inv.resume.is_some(), ..Default::default() },
            pending: VecDeque::new(),
            done: false,
            text: String::new(),
            messages: req.messages.clone(),
            session: self.session.clone(),
            last: self.last.clone(),
            _sysfile: sysfile,
        })
    }
}

/// [`ClaudeCodeProvider::plan`], pure.
fn plan_invocation(req: &ChatRequest, model: &str, session: Option<&Session>) -> Result<Invocation, ProviderError> {
    if req.messages.last().map(|m| m.role) != Some(Role::User) {
        return Err(ProviderError::Unsupported(
            "a conversation that does not end with the person's turn (no prefill)".into(),
        ));
    }
    // Validate every part up front (an attachment anywhere is refused).
    let mut texts = Vec::with_capacity(req.messages.len());
    for m in &req.messages {
        texts.push(message_text(m)?);
    }
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--tools",
        "",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let m = model.trim();
    if !m.is_empty() && m != CLAUDECODE_DEFAULT_MODEL {
        args.push("--model".into());
        args.push(m.to_string());
    }
    let mut system_file = None;
    if let Some(sys) = req.system.as_ref().filter(|s| !s.trim().is_empty()) {
        if sys.len() > SYSTEM_ARG_MAX {
            args.push("--system-prompt-file".into());
            args.push(SYSTEM_FILE_PLACEHOLDER.into());
            system_file = Some(sys.clone());
        } else {
            args.push("--system-prompt".into());
            args.push(sys.clone());
        }
    }
    // Continuation: the remembered conversation is an exact prefix and only
    // the person's turns follow it.
    let resume = session.filter(|s| {
        s.len < req.messages.len()
            && fingerprint(&req.messages[..s.len]) == s.seen
            && req.messages[s.len..].iter().all(|m| m.role == Role::User)
    });
    let stdin = match resume {
        Some(s) => {
            args.push("--resume".into());
            args.push(s.id.clone());
            texts[s.len..].join("\n\n")
        }
        None => flatten(&req.messages, &texts),
    };
    if stdin.trim().is_empty() {
        return Err(ProviderError::Unsupported("an empty message (the CLI needs non-blank text)".into()));
    }
    Ok(Invocation { args, stdin, resume: resume.map(|s| s.id.clone()), system_file })
}

/// A message's text parts joined; an attachment is refused.
fn message_text(m: &ChatMessage) -> Result<String, ProviderError> {
    let mut out = String::new();
    for p in &m.parts {
        match p {
            Part::Text { text } => {
                if !out.is_empty() && !text.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
            Part::FileData { file_data } => {
                return Err(ProviderError::Unsupported(format!(
                    "attaching {} from {} — the Claude Code provider sends text only (choose Claude or Gemini in Settings to send it)",
                    file_data.mime_type, file_data.file_uri
                )));
            }
        }
    }
    Ok(out)
}

/// A fresh session's prompt: one turn is sent as is; a longer conversation is
/// the earlier turns as a labelled history, then the person's last turn(s).
fn flatten(messages: &[ChatMessage], texts: &[String]) -> String {
    let tail_start = messages.iter().rposition(|m| m.role != Role::User).map_or(0, |i| i + 1);
    let tail = texts[tail_start..].join("\n\n");
    if tail_start == 0 {
        return tail;
    }
    let mut s = String::from("<conversation-history>\n");
    for (m, t) in messages[..tail_start].iter().zip(texts) {
        s.push_str(match m.role {
            Role::User => "User: ",
            Role::Assistant => "Assistant: ",
        });
        s.push_str(t);
        s.push_str("\n\n");
    }
    s.push_str("</conversation-history>\n\n");
    s.push_str(&tail);
    s
}

/// FNV-1a over each message's role and text (deterministic across runs; the
/// boundaries are delimited so `["ab","c"]` and `["a","bc"]` differ).
fn fingerprint(messages: &[ChatMessage]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: &[u8]| {
        for &x in b {
            h ^= x as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for m in messages {
        eat(match m.role {
            Role::User => b"\x01U",
            Role::Assistant => b"\x01A",
        });
        for p in &m.parts {
            match p {
                Part::Text { text } => {
                    eat(b"\x02");
                    eat(&(text.len() as u64).to_le_bytes());
                    eat(text.as_bytes());
                }
                Part::FileData { file_data } => {
                    eat(b"\x03");
                    eat(file_data.file_uri.as_bytes());
                }
            }
        }
    }
    h
}

fn spawn_error(bin: &str, e: &std::io::Error) -> ProviderError {
    match e.kind() {
        std::io::ErrorKind::NotFound => ProviderError::Config(format!(
            "Claude Code CLI not found: `{bin}` — install Claude Code, or set vein.claudecode.bin to the claude binary's path"
        )),
        std::io::ErrorKind::PermissionDenied => ProviderError::Config(format!(
            "Claude Code CLI `{bin}` is not executable ({e}) — fix vein.claudecode.bin"
        )),
        _ => ProviderError::Cli { code: None, message: format!("could not start `{bin}`: {e}") },
    }
}

/// True for the CLI's "you are not logged in" lines.
pub fn is_login_line(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    [
        "not logged in",
        "please run /login",
        "run `claude login`",
        "invalid api key",
        "authentication_failed",
        "oauth token has expired",
        "oauth token revoked",
    ]
    .iter()
    .any(|p| l.contains(p))
}

/// The in-chat message for a logged-out CLI, quoting what it said.
pub fn not_logged_in(said: &str) -> ProviderError {
    ProviderError::Config(format!(
        "Claude Code is not logged in — the CLI said: \"{}\". Run `claude` in a terminal and /login with your subscription account, or choose another provider in Settings",
        said.trim()
    ))
}

/// The stream-json translation (pure: one line in, deltas out).
#[derive(Debug, Default)]
pub struct CliEvents {
    pub session_id: Option<String>,
    pub info: RunInfo,
    /// Messages whose text arrived as partial deltas (their complete
    /// `assistant` copy is then not re-emitted).
    streamed: HashSet<String>,
    current: Option<String>,
    usage: Usage,
    /// The last `api_retry` the CLI reported, e.g. `authentication_failed (401)`.
    retry_note: Option<(Option<u16>, String)>,
    /// Non-JSON stdout lines (kept for an error message).
    pub noise: Vec<String>,
    emitted_text: bool,
    /// A `result` (or a fatal line) was seen: the run's outcome is decided.
    pub finished: bool,
    resumed: bool,
}

impl CliEvents {
    pub fn feed(&mut self, line: &str) -> Vec<Result<ChatDelta, ProviderError>> {
        let line = line.trim();
        if line.is_empty() || self.finished {
            return vec![];
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                if is_login_line(line) {
                    self.finished = true;
                    return vec![Err(not_logged_in(line))];
                }
                if self.noise.len() < 20 {
                    self.noise.push(line.to_string());
                }
                return vec![];
            }
        };
        let s = |p: &str| v.pointer(p).and_then(Value::as_str);
        match s("/type").unwrap_or("") {
            "system" => {
                match s("/subtype") {
                    Some("init") => {
                        self.session_id = s("/session_id").map(str::to_string);
                        self.info.model = s("/model").map(str::to_string);
                        self.info.cli_version = s("/claude_code_version").map(str::to_string);
                    }
                    Some("api_retry") => {
                        let status = v.get("error_status").and_then(Value::as_u64).map(|n| n as u16);
                        let what = s("/error").unwrap_or("api error").to_string();
                        // The CLI retries a 401/403 ten times with backoff
                        // (about three minutes, observed): a credential that
                        // was refused once is not waited on.
                        if matches!(status, Some(401) | Some(403)) {
                            self.finished = true;
                            return vec![Err(not_logged_in(&format!("API Error: {} {what}", status.unwrap_or(0))))];
                        }
                        self.retry_note = Some((status, what));
                    }
                    _ => {}
                }
                vec![]
            }
            "stream_event" => match s("/event/type").unwrap_or("") {
                "message_start" => {
                    self.current = s("/event/message/id").map(str::to_string);
                    vec![]
                }
                "content_block_delta" if s("/event/delta/type") == Some("text_delta") => {
                    let t = s("/event/delta/text").unwrap_or("");
                    if let Some(id) = &self.current {
                        self.streamed.insert(id.clone());
                    }
                    if t.is_empty() {
                        return vec![];
                    }
                    self.emitted_text = true;
                    vec![Ok(ChatDelta::Text(t.to_string()))]
                }
                _ => vec![],
            },
            "assistant" => {
                // A message the CLI itself flags as an API error is never
                // shown as the model's text.
                if let Some(err) = s("/error") {
                    let text = assistant_text(&v);
                    self.retry_note = Some((None, if text.is_empty() { err.to_string() } else { text }));
                    return vec![];
                }
                let id = s("/message/id").unwrap_or("");
                if self.streamed.contains(id) {
                    return vec![];
                }
                let t = assistant_text(&v);
                if t.is_empty() {
                    return vec![];
                }
                self.emitted_text = true;
                vec![Ok(ChatDelta::Text(t))]
            }
            "result" => {
                self.finished = true;
                self.session_id = s("/session_id").map(str::to_string).or(self.session_id.take());
                self.info.session_id = self.session_id.clone();
                self.info.duration_ms = v.get("duration_ms").and_then(Value::as_u64);
                self.info.total_cost_usd = v.get("total_cost_usd").and_then(Value::as_f64);
                self.info.resumed = self.resumed;
                if v.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
                    return vec![Err(self.result_error(&v))];
                }
                let u = v.get("usage");
                let n = |k: &str| u.and_then(|u| u.get(k)).and_then(Value::as_u64).unwrap_or(0);
                self.usage = Usage {
                    input_tokens: n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens"),
                    output_tokens: n("output_tokens"),
                };
                let stop = match s("/stop_reason") {
                    Some(r) => map_stop(Some(r), v.get("stop_details")),
                    None => StopReason::EndTurn,
                };
                let mut out = Vec::new();
                // A result with text the stream never carried (no assistant
                // event at all) still delivers the reply.
                if !self.emitted_text {
                    if let Some(t) = s("/result").filter(|t| !t.is_empty()) {
                        self.emitted_text = true;
                        out.push(Ok(ChatDelta::Text(t.to_string())));
                    }
                }
                out.push(Ok(ChatDelta::Stop { stop, usage: self.usage }));
                out
            }
            _ => vec![],
        }
    }

    /// A `result` with `is_error: true` → a classified error.
    fn result_error(&self, v: &Value) -> ProviderError {
        let mut msg = v
            .get("result")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                v.get("errors").and_then(Value::as_array).map(|a| {
                    a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")
                })
            })
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| v.get("subtype").and_then(Value::as_str).unwrap_or("error").to_string());
        let status = v
            .get("api_error_status")
            .and_then(Value::as_u64)
            .map(|n| n as u16)
            .or(self.retry_note.as_ref().and_then(|r| r.0));
        if let Some((_, note)) = &self.retry_note {
            if !msg.contains(note.as_str()) {
                msg = format!("{msg} ({note})");
            }
        }
        classify(&msg, status)
    }

    /// The outcome when stdout ended without a `result`.
    pub fn end(&self, code: Option<i32>, stderr: &str) -> ProviderError {
        let lines = stderr.lines().chain(self.noise.iter().map(String::as_str));
        if let Some(l) = lines.clone().find(|l| is_login_line(l)) {
            return not_logged_in(l);
        }
        if let Some((st, note)) = &self.retry_note {
            if is_login_line(note) || matches!(st, Some(401)) {
                return not_logged_in(note);
            }
        }
        let said: Vec<&str> = lines.map(str::trim).filter(|l| !l.is_empty()).collect();
        let tail = said[said.len().saturating_sub(6)..].join(" | ");
        match code {
            Some(0) => ProviderError::Malformed(format!(
                "the CLI exited without a result event{}",
                if tail.is_empty() { String::new() } else { format!(": {tail}") }
            )),
            _ => ProviderError::Cli {
                code,
                message: if tail.is_empty() { "no output".into() } else { tail },
            },
        }
    }
}

fn assistant_text(v: &Value) -> String {
    let mut t = String::new();
    if let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) {
        for b in blocks {
            if b.get("type").and_then(Value::as_str) == Some("text") {
                t.push_str(b.get("text").and_then(Value::as_str).unwrap_or(""));
            }
        }
    }
    t
}

/// An error message (+ the API status the CLI reported, if any) → a variant.
pub fn classify(msg: &str, status: Option<u16>) -> ProviderError {
    if is_login_line(msg) {
        return not_logged_in(msg);
    }
    match status {
        Some(401) | Some(403) => not_logged_in(msg),
        Some(s @ (408 | 409 | 429)) => ProviderError::Retryable { status: s, message: msg.to_string() },
        Some(s) if s >= 500 => ProviderError::Retryable { status: s, message: msg.to_string() },
        Some(s) if s >= 400 => ProviderError::Request { status: s, message: msg.to_string() },
        _ => ProviderError::Cli { code: None, message: msg.to_string() },
    }
}

/// A temp file removed on drop (a long system prompt).
struct TempFile(PathBuf);

impl TempFile {
    fn write(text: &str) -> Result<Self, ProviderError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "vein-claudecode-system-{}-{}.txt",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&p, text)
            .map_err(|e| ProviderError::Config(format!("could not write the system prompt file {}: {e}", p.display())))?;
        Ok(TempFile(p))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// One live CLI run, read as a stream.
struct Run {
    child: Child,
    lines: Lines<BufReader<ChildStdout>>,
    stderr: Option<tokio::task::JoinHandle<String>>,
    parser: CliEvents,
    pending: VecDeque<Result<ChatDelta, ProviderError>>,
    done: bool,
    text: String,
    messages: Vec<ChatMessage>,
    session: Arc<Mutex<Option<Session>>>,
    last: Arc<Mutex<Option<RunInfo>>>,
    _sysfile: Option<TempFile>,
}

impl Run {
    async fn next(&mut self) -> Option<Result<ChatDelta, ProviderError>> {
        loop {
            if let Some(d) = self.pending.pop_front() {
                match &d {
                    Ok(ChatDelta::Text(t)) => self.text.push_str(t),
                    Ok(ChatDelta::Stop { .. }) => {
                        self.done = true;
                        self.remember();
                    }
                    Err(_) => {
                        self.done = true;
                        self.forget();
                        let _ = self.child.start_kill();
                    }
                }
                return Some(d);
            }
            if self.done {
                return None;
            }
            match self.lines.next_line().await {
                Ok(Some(line)) => {
                    let out = self.parser.feed(&line);
                    self.pending.extend(out);
                }
                Ok(None) | Err(_) => {
                    let status = self.child.wait().await.ok();
                    let stderr = match self.stderr.take() {
                        Some(h) => h.await.unwrap_or_default(),
                        None => String::new(),
                    };
                    let code = status.and_then(|s| s.code());
                    self.pending.push_back(Err(self.parser.end(code, &stderr)));
                }
            }
        }
    }

    /// A completed turn: the session now holds this conversation + the reply.
    fn remember(&mut self) {
        if let Ok(mut l) = self.last.lock() {
            *l = Some(self.parser.info.clone());
        }
        let Some(id) = self.parser.session_id.clone() else { return };
        let mut convo = self.messages.clone();
        convo.push(ChatMessage { role: Role::Assistant, parts: vec![Part::text(self.text.clone())] });
        if let Ok(mut s) = self.session.lock() {
            *s = Some(Session { id, seen: fingerprint(&convo), len: convo.len() });
        }
    }

    /// A failed turn: the next request starts fresh (correctness over
    /// continuity — the CLI's copy of the session may now differ from ours).
    fn forget(&mut self) {
        if let Ok(mut s) = self.session.lock() {
            *s = None;
        }
    }
}

impl ModelProvider for ClaudeCodeProvider {
    fn name(&self) -> &str {
        "claudecode"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn generate<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, Result<ChatResponse, ProviderError>> {
        Box::pin(async move {
            let mut run = self.spawn(req).await?;
            let mut text = String::new();
            while let Some(d) = run.next().await {
                match d? {
                    ChatDelta::Text(t) => text.push_str(&t),
                    ChatDelta::Stop { stop, usage } => return Ok(ChatResponse { text, stop, usage }),
                }
            }
            Err(ProviderError::Malformed("the CLI stream ended without a stop".into()))
        })
    }

    fn stream<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, Result<DeltaStream<'a>, ProviderError>> {
        Box::pin(async move {
            let run = self.spawn(req).await?;
            let s = futures_util::stream::unfold(run, |mut run| async move { run.next().await.map(|d| (d, run)) });
            Ok(Box::pin(s) as DeltaStream<'a>)
        })
    }
}

/// Where the CLI keeps nothing of ours: a helper for a check tool to say
/// which binary would run (`PATH` lookup of a bare name).
pub fn resolve_bin(bin: &str, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let p = Path::new(bin);
    if p.components().count() > 1 || p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    std::env::split_paths(path_var?).map(|d| d.join(bin)).find(|c| c.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::os::unix::fs::PermissionsExt;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claudecode").join(name)
    }

    fn lines(name: &str) -> Vec<String> {
        std::fs::read_to_string(fixture(name)).unwrap().lines().map(str::to_string).collect()
    }

    /// The fixture's own `result` text: the CLI's concatenation of the reply
    /// (the oracle the streamed deltas must reproduce).
    fn result_text(name: &str) -> String {
        let last: Value = serde_json::from_str(lines(name).last().unwrap()).unwrap();
        last["result"].as_str().unwrap().to_string()
    }

    fn feed_all(p: &mut CliEvents, ls: &[String]) -> Vec<Result<ChatDelta, ProviderError>> {
        ls.iter().flat_map(|l| p.feed(l)).collect()
    }

    fn text_of(out: &[Result<ChatDelta, ProviderError>]) -> String {
        out.iter().filter_map(|d| match d {
            Ok(ChatDelta::Text(t)) => Some(t.as_str()),
            _ => None,
        })
        .collect()
    }

    fn req(msgs: &[(Role, &str)], system: Option<&str>) -> ChatRequest {
        ChatRequest {
            system: system.map(str::to_string),
            messages: msgs.iter().map(|(r, t)| ChatMessage { role: *r, parts: vec![Part::text(t.to_string())] }).collect(),
            max_tokens: 1024,
            temperature: None,
        }
    }

    // ---- the stream-json translation (recorded transcripts) ----------------

    #[test]
    fn recorded_turn_streams_its_partial_deltas_in_order() {
        let mut p = CliEvents::default();
        let out = feed_all(&mut p, &lines("turn1.jsonl"));
        let texts: Vec<&str> = out.iter().filter_map(|d| match d {
            Ok(ChatDelta::Text(t)) => Some(t.as_str()),
            _ => None,
        })
        .collect();
        assert_eq!(texts, ["one, two, three, four,", " five"], "deltas, and the complete assistant copy is not re-emitted");
        assert_eq!(texts.concat(), result_text("turn1.jsonl"));
        assert_eq!(
            out.last().unwrap(),
            &Ok(ChatDelta::Stop { stop: StopReason::EndTurn, usage: Usage { input_tokens: 2 + 668, output_tokens: 11 } })
        );
        assert_eq!(p.session_id.as_deref(), Some("00000000-0000-4000-8000-00000000cc01"));
        assert_eq!(p.info.cli_version.as_deref(), Some("2.1.289"));
        assert!(p.finished);
    }

    #[test]
    fn without_partials_the_assistant_message_carries_the_text() {
        let mut p = CliEvents::default();
        let out = feed_all(&mut p, &lines("nopartial.jsonl"));
        assert_eq!(text_of(&out), "Hello from turn one.");
        assert_eq!(text_of(&out), result_text("nopartial.jsonl"));
        assert!(matches!(out.last(), Some(Ok(ChatDelta::Stop { stop: StopReason::EndTurn, .. }))));
        assert!((p.info.total_cost_usd.unwrap() - 0.002756).abs() < 1e-9, "{:?}", p.info.total_cost_usd);
    }

    #[test]
    fn a_refused_credential_fails_on_the_first_401_retry() {
        let mut p = CliEvents::default();
        let out = feed_all(&mut p, &lines("authfail.jsonl"));
        assert_eq!(out.len(), 1, "{out:?}");
        let Err(ProviderError::Config(m)) = &out[0] else { panic!("{out:?}") };
        assert!(m.contains("not logged in") && m.contains("API Error: 401 authentication_failed"), "{m}");
        assert!(p.finished);
    }

    #[test]
    fn an_api_error_result_is_classified_and_never_shown_as_reply_text() {
        let ls = lines("authfail.jsonl");
        let mut p = CliEvents::default();
        // init, then the synthetic error message and the error result only.
        let out = feed_all(&mut p, &[ls[0].clone(), ls[3].clone(), ls[4].clone()]);
        assert_eq!(text_of(&out), "", "the CLI's synthetic error text is not the model's reply");
        assert_eq!(out.len(), 1);
        let Err(ProviderError::Config(m)) = &out[0] else { panic!("{out:?}") };
        assert!(m.contains("\"Failed to authenticate. API Error: 401 API key is invalid.\""), "{m}");
    }

    #[test]
    fn results_classify_by_api_status() {
        let mk = |status: u64, msg: &str| {
            let mut p = CliEvents::default();
            let l = format!(
                r#"{{"type":"result","subtype":"success","is_error":true,"api_error_status":{status},"result":"{msg}"}}"#
            );
            p.feed(&l).remove(0).unwrap_err()
        };
        assert_eq!(mk(529, "Overloaded"), ProviderError::Retryable { status: 529, message: "Overloaded".into() });
        assert_eq!(mk(429, "rate"), ProviderError::Retryable { status: 429, message: "rate".into() });
        assert_eq!(mk(400, "bad"), ProviderError::Request { status: 400, message: "bad".into() });
        assert!(matches!(mk(403, "forbidden"), ProviderError::Config(_)));
        let mut p = CliEvents::default();
        let e = p
            .feed(r#"{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["tool loop"]}"#)
            .remove(0)
            .unwrap_err();
        assert_eq!(e, ProviderError::Cli { code: None, message: "tool loop".into() });
    }

    #[test]
    fn a_plain_not_logged_in_line_is_quoted_verbatim() {
        let mut p = CliEvents::default();
        let out = p.feed("Not logged in · Please run /login");
        let Err(ProviderError::Config(m)) = &out[0] else { panic!("{out:?}") };
        assert!(m.contains("the CLI said: \"Not logged in · Please run /login\""), "{m}");
        // On stderr instead, found when the process ends.
        let p = CliEvents::default();
        assert!(matches!(p.end(Some(1), "Invalid API key · Please run /login\n"), ProviderError::Config(_)));
        assert_eq!(p.end(Some(3), "boom\n"), ProviderError::Cli { code: Some(3), message: "boom".into() });
        assert!(matches!(p.end(Some(0), ""), ProviderError::Malformed(_)));
        assert_eq!(p.end(None, ""), ProviderError::Cli { code: None, message: "no output".into() });
    }

    // ---- the invocation ------------------------------------------------------

    #[test]
    fn one_turn_is_the_prompt_on_stdin_with_tools_off() {
        let inv = plan_invocation(&req(&[(Role::User, "hi")], Some("be terse")), "default", None).unwrap();
        assert_eq!(
            inv.args,
            ["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--tools", "", "--system-prompt", "be terse"]
        );
        assert_eq!(inv.stdin, "hi");
        assert_eq!(inv.resume, None);
        let inv = plan_invocation(&req(&[(Role::User, "hi")], None), "opus", None).unwrap();
        assert!(inv.args.windows(2).any(|w| w == ["--model", "opus"]));
        assert!(!inv.args.iter().any(|a| a == "--system-prompt"));
    }

    #[test]
    fn a_long_system_prompt_goes_through_a_file() {
        let big = "x".repeat(SYSTEM_ARG_MAX + 1);
        let inv = plan_invocation(&req(&[(Role::User, "hi")], Some(&big)), "default", None).unwrap();
        assert!(inv.args.windows(2).any(|w| w == ["--system-prompt-file", SYSTEM_FILE_PLACEHOLDER]));
        assert_eq!(inv.system_file.as_deref(), Some(big.as_str()));
    }

    #[test]
    fn refusals_are_said_never_silent() {
        let mut r = req(&[(Role::User, "see this")], None);
        r.messages[0].parts.push(Part::file_data("application/pdf".into(), "gs://b/a.pdf".into()));
        assert!(matches!(plan_invocation(&r, "default", None), Err(ProviderError::Unsupported(ref m)) if m.contains("gs://b/a.pdf")));
        let r = req(&[(Role::User, "a"), (Role::Assistant, "b")], None);
        assert!(matches!(plan_invocation(&r, "default", None), Err(ProviderError::Unsupported(_))));
        let r = req(&[(Role::User, "  ")], None);
        assert!(matches!(plan_invocation(&r, "default", None), Err(ProviderError::Unsupported(_))));
    }

    #[test]
    fn a_fresh_multi_turn_request_flattens_the_history() {
        let r = req(&[(Role::User, "a"), (Role::Assistant, "b"), (Role::User, "c"), (Role::User, "d")], None);
        let inv = plan_invocation(&r, "default", None).unwrap();
        assert_eq!(inv.stdin, "<conversation-history>\nUser: a\n\nAssistant: b\n\n</conversation-history>\n\nc\n\nd");
    }

    #[test]
    fn a_continuation_resumes_and_an_edited_history_does_not() {
        let seen = req(&[(Role::User, "a"), (Role::Assistant, "b")], None);
        let s = Session { id: "S1".into(), seen: fingerprint(&seen.messages), len: 2 };
        let next = req(&[(Role::User, "a"), (Role::Assistant, "b"), (Role::User, "c")], None);
        let inv = plan_invocation(&next, "default", Some(&s)).unwrap();
        assert_eq!(inv.resume.as_deref(), Some("S1"));
        assert!(inv.args.windows(2).any(|w| w == ["--resume", "S1"]));
        assert_eq!(inv.stdin, "c");
        let edited = req(&[(Role::User, "a"), (Role::Assistant, "B"), (Role::User, "c")], None);
        let inv = plan_invocation(&edited, "default", Some(&s)).unwrap();
        assert_eq!(inv.resume, None);
        assert!(inv.stdin.starts_with("<conversation-history>"));
        // The same conversation again (nothing new after it) is not a resume.
        assert_eq!(plan_invocation(&req(&[(Role::User, "a")], None), "default", Some(&s)).unwrap().resume, None);
    }

    #[test]
    fn fingerprints_delimit_part_boundaries() {
        let a = req(&[(Role::User, "ab"), (Role::User, "c")], None);
        let b = req(&[(Role::User, "a"), (Role::User, "bc")], None);
        assert_ne!(fingerprint(&a.messages), fingerprint(&b.messages));
    }

    // ---- a fake `claude` on PATH replaying the recorded transcripts -----------

    struct Fake {
        dir: PathBuf,
    }

    impl Fake {
        /// A `claude` script in a fresh directory: records its argv, stdin and
        /// whether the parent-session variable reached it, then runs `body`.
        fn new(tag: &str, body: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("claudecode-fake-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let d = dir.display();
            let script = format!(
                "#!/bin/sh\nD='{d}'\nn=$(cat \"$D/n\" 2>/dev/null || echo 0); n=$((n+1)); echo $n > \"$D/n\"\n\
                 for a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$D/argv.$n\"\n\
                 cat > \"$D/stdin.$n\"\n\
                 printf '%s' \"${{CLAUDE_CODE_SESSION_ID-unset}}\" > \"$D/sessvar.$n\"\n{body}\n"
            );
            let bin = dir.join("claude");
            std::fs::write(&bin, script).unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            Fake { dir }
        }

        /// Replays turn1 for a fresh session and turn2 for a `--resume`.
        fn replaying(tag: &str) -> Self {
            let t1 = fixture("turn1.jsonl");
            let t2 = fixture("turn2.jsonl");
            Self::new(
                tag,
                &format!(
                    "case \" $* \" in *\" --resume \"*) cat '{}';; *) cat '{}';; esac",
                    t2.display(),
                    t1.display()
                ),
            )
        }

        /// A provider whose `claude` is found by a `PATH` lookup.
        fn provider(&self) -> ClaudeCodeProvider {
            let path = format!("{}:{}", self.dir.display(), std::env::var("PATH").unwrap_or_default());
            ClaudeCodeProvider::new("claude", "").with_env("PATH", path)
        }

        fn read(&self, f: &str) -> String {
            std::fs::read_to_string(self.dir.join(f)).unwrap_or_default()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn collect(p: &ClaudeCodeProvider, r: &ChatRequest) -> Vec<Result<ChatDelta, ProviderError>> {
        match p.stream(r).await {
            Ok(s) => s.collect().await,
            Err(e) => vec![Err(e)],
        }
    }

    #[tokio::test]
    async fn the_provider_streams_the_recorded_reply() {
        let fake = Fake::replaying("stream");
        let p = fake.provider();
        assert_eq!((p.name(), p.model()), ("claudecode", "default"));
        let r = req(&[(Role::User, "Count from one to five in words, comma separated, nothing else.")], Some("You are a terse test responder."));
        let out = collect(&p, &r).await;
        assert_eq!(text_of(&out), result_text("turn1.jsonl"), "{out:?}");
        assert_eq!(out.iter().filter(|d| matches!(d, Ok(ChatDelta::Text(_)))).count(), 2, "incremental, not one blob");
        assert!(matches!(out.last(), Some(Ok(ChatDelta::Stop { stop: StopReason::EndTurn, .. }))));
        let argv: Vec<String> = fake.read("argv.1").lines().map(str::to_string).collect();
        assert_eq!(&argv[..5], ["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
        assert!(argv.windows(2).any(|w| w == ["--system-prompt", "You are a terse test responder."]));
        assert!(!argv.iter().any(|a| a == "--resume"));
        assert_eq!(fake.read("stdin.1"), r.messages[0].parts.iter().map(|_| "Count from one to five in words, comma separated, nothing else.").collect::<String>());
        assert_eq!(fake.read("sessvar.1"), "unset", "a parent Claude Code session never leaks into the child");
        assert_eq!(p.session_id().as_deref(), Some("00000000-0000-4000-8000-00000000cc01"));
        let info = p.last_run().unwrap();
        assert_eq!((info.resumed, info.duration_ms), (false, Some(1733)));
    }

    #[tokio::test]
    async fn the_second_turn_resumes_the_same_cli_session() {
        let fake = Fake::replaying("resume");
        let p = fake.provider();
        let mut r = req(&[(Role::User, "Count from one to five in words, comma separated, nothing else.")], None);
        let first = p.generate(&r).await.unwrap();
        assert_eq!(first.text, result_text("turn1.jsonl"));
        r.messages.push(ChatMessage { role: Role::Assistant, parts: vec![Part::text(first.text.clone())] });
        r.messages.push(ChatMessage::user_text("Now continue from six to eight, same format."));
        let second = p.generate(&r).await.unwrap();
        assert_eq!(second.text, result_text("turn2.jsonl"));
        let argv: Vec<String> = fake.read("argv.2").lines().map(str::to_string).collect();
        assert!(argv.windows(2).any(|w| w == ["--resume", "00000000-0000-4000-8000-00000000cc01"]), "{argv:?}");
        assert_eq!(fake.read("stdin.2"), "Now continue from six to eight, same format.", "only the new turn is sent");
        assert!(p.last_run().unwrap().resumed);
        // A different conversation starts fresh.
        let other = req(&[(Role::User, "unrelated")], None);
        p.generate(&other).await.unwrap();
        assert!(!fake.read("argv.3").lines().any(|a| a == "--resume"));
    }

    #[tokio::test]
    async fn a_missing_binary_names_the_preference() {
        let p = ClaudeCodeProvider::new("/nonexistent/claudecode-test/claude", "");
        let e = p.generate(&req(&[(Role::User, "hi")], None)).await.unwrap_err();
        let ProviderError::Config(m) = &e else { panic!("{e:?}") };
        assert!(m.contains("/nonexistent/claudecode-test/claude") && m.contains("vein.claudecode.bin"), "{m}");
    }

    #[tokio::test]
    async fn a_logged_out_cli_is_reported_verbatim() {
        let fake = Fake::new("loggedout", "echo 'Not logged in · Please run /login'; exit 1");
        let e = fake.provider().generate(&req(&[(Role::User, "hi")], None)).await.unwrap_err();
        assert!(matches!(e, ProviderError::Config(ref m) if m.contains("\"Not logged in · Please run /login\"")), "{e:?}");
        let fake = Fake::new("loggedout-stderr", "echo 'Invalid API key · Please run /login' >&2; exit 1");
        let e = fake.provider().generate(&req(&[(Role::User, "hi")], None)).await.unwrap_err();
        assert!(matches!(e, ProviderError::Config(ref m) if m.contains("Invalid API key")), "{e:?}");
    }

    #[tokio::test]
    async fn a_refused_credential_does_not_wait_out_the_cli_retries() {
        // The recorded 401 run, then a sleep standing in for the CLI's ~3
        // minutes of retries: the provider answers at the first 401.
        let fake = Fake::new("authfail", &format!("head -n 2 '{}'; sleep 30", fixture("authfail.jsonl").display()));
        let t = std::time::Instant::now();
        let e = fake.provider().generate(&req(&[(Role::User, "hi")], None)).await.unwrap_err();
        assert!(matches!(e, ProviderError::Config(ref m) if m.contains("not logged in")), "{e:?}");
        assert!(t.elapsed() < std::time::Duration::from_secs(10), "{:?}", t.elapsed());
    }

    #[tokio::test]
    async fn a_non_zero_exit_carries_its_code_and_stderr_and_drops_the_session() {
        let fake = Fake::new("exit3", "if [ \"$(cat \"$D/n\")\" = 1 ]; then cat '__T1__'; else echo 'segfault in node' >&2; exit 3; fi"
            .replace("__T1__", &fixture("turn1.jsonl").display().to_string()).as_str());
        let p = fake.provider();
        let mut r = req(&[(Role::User, "one")], None);
        let first = p.generate(&r).await.unwrap();
        assert!(p.session_id().is_some());
        r.messages.push(ChatMessage { role: Role::Assistant, parts: vec![Part::text(first.text)] });
        r.messages.push(ChatMessage::user_text("two"));
        let e = p.generate(&r).await.unwrap_err();
        assert_eq!(e, ProviderError::Cli { code: Some(3), message: "segfault in node".into() });
        assert_eq!(e.to_string(), "Claude Code CLI exited with status 3: segfault in node");
        assert_eq!(p.session_id(), None, "after a failure the next turn starts fresh");
    }
}
