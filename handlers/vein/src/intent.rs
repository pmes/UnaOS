// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! CHARTER: Vein — handler (VUG-VEIN-BUS, LEDGER SR509): Vug's propose exchange over the bus.
//!
//! A face never links vein_core (VUG-FACE-PHOTO SR506): the key and the verified-TLS road are Vein's. A face fires
//! `SMessage::Intent(IntentEvent::Propose { .. })`; Vein resolves the provider the way chat does ([`ProviderSlot`], the
//! Principia `vein` namespace, Holocron's key first), encodes through `vein_core::propose::request`, sends with
//! [`SynapticRetry`], decodes the stream with vein_core's `StreamDecoder` (the decoder `client::exchange_dialect` drives
//! on ring 3), parses with `vein_core::propose::parse`, and answers `Proposals` or `Refused` by name with the request id
//! echoed. `vein.provider = "echo"` (vein_core's `prefs::provider_pref`) answers `propose::ECHO_ANSWER`, no proposals,
//! with no network (BP-vug-7). VUG-VEIN-STREAM (SR531): the body is read chunk by chunk ([`ProposeStream`] over
//! `Response::chunk`, vein_core's `propose::Incremental`); each proposal fires as `IntentEvent::Proposal { request, index,
//! proposal }` the moment its closing brace arrives, then ONE terminal: `Proposals` (exactly the whole-body parse, its
//! length the total) or `Refused` (`<Refusal> after N proposals` once N were streamed, the bare name otherwise).

use std::time::Duration;

use bandy::{IntentEvent, IntentProposal, PrefValue, SMessage, Synapse};
use gneiss_pal::api::http::{ClientBuilder, Response};
use gneiss_pal::api::{AuthMode, PREF_NS, ProviderConfig, ProviderKind};
use vein_core::claude::{self, Dialect, Event, Stop, StreamDecoder};
use vein_core::{gemini, prefs, propose};

use crate::provider::ProviderSlot;
use crate::synapse::SynapticRetry;

/// The decoder's line buffer: one `data:` line of a stream (a longer one is `Dropped`, refused by name).
const LINE_MAX: usize = 256 * 1024;

/// Where a propose request goes and with which credential. Built ONLY inside [`ProviderSlot::from_lookup_with`] from
/// the chat provider's config and Holocron-first key; the key is a private field and the type has no `Debug`.
#[derive(Clone)]
pub struct ProposeRoute {
    pub dialect: Dialect,
    pub model: String,
    url: String,
    auth: (&'static str, String),
}

impl ProposeRoute {
    /// The route for `cfg` with the key read through `env` (the slot's Holocron-overlaid environment). Gemini over gcloud
    /// and the Claude Code CLI are refused by name (VUG-VEIN-ADC owed).
    pub fn from_config(cfg: &ProviderConfig, env: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let key = |var: &str| env(var).map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).ok_or_else(|| format!("NoKey: set {var}"));
        match (&cfg.kind, &cfg.auth) {
            (ProviderKind::Claude, AuthMode::ApiKeyEnv(var)) => Ok(ProposeRoute {
                dialect: Dialect::Claude,
                model: cfg.model.clone(),
                url: format!("https://{}{}", claude::HOST, claude::PATH),
                auth: ("x-api-key", key(var.as_str())?),
            }),
            (ProviderKind::Gemini, AuthMode::ApiKeyEnv(var)) => {
                let mut buf = [0u8; gemini::PATH_CAP];
                let n = gemini::path(&cfg.model, &mut buf).ok_or_else(|| format!("BadModel: {}", cfg.model))?;
                let path = std::str::from_utf8(&buf[..n]).map_err(|_| format!("BadModel: {}", cfg.model))?;
                Ok(ProposeRoute {
                    dialect: Dialect::Gemini,
                    model: cfg.model.clone(),
                    url: format!("https://{}{path}", gemini::HOST),
                    auth: ("x-goog-api-key", key(var.as_str())?),
                })
            }
            (ProviderKind::Gemini, _) => {
                Err("Unsupported: Gemini over gcloud auth has no propose road; set vein.gemini.auth = \"api_key\"".to_string())
            }
            (ProviderKind::ClaudeCode, _) => {
                Err("Unsupported: the Claude Code CLI takes no image blocks; choose claude or gemini in Settings".to_string())
            }
            (ProviderKind::Claude, _) => Err("Unsupported: Claude without an API key".to_string()),
        }
    }

    /// Point at another endpoint (a test server).
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// POST the encoded body and hand back the 2xx response, read chunk by chunk by the caller (VUG-VEIN-STREAM SR531);
    /// a non-2xx status is `Http(<status>): <message>`.
    pub async fn open(&self, body: Vec<u8>) -> Result<Response, String> {
        let http = ClientBuilder::new().timeout(Duration::from_secs(600)).build().map_err(|e| format!("Client: {e}"))?;
        let mut rb = http.post(&self.url).header(self.auth.0, &self.auth.1).header("content-type", "application/json");
        if self.dialect == Dialect::Claude {
            rb = rb.header("anthropic-version", claude::API_VERSION);
            // vein_core's Params turns the body's `fallbacks` on by the same rule.
            if claude::wants_fallbacks(&self.model) {
                rb = rb.header("anthropic-beta", claude::FALLBACK_BETA);
            }
        }
        let resp = rb.body(body).fire_with_backoff().await.map_err(|e| format!("Connect: {e}"))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let mut bytes = resp.bytes().await.map_err(|e| format!("Recv: {e}"))?;
            let msg = claude::error_message(&mut bytes).unwrap_or("").to_string();
            return Err(format!("Http({status}): {msg}"));
        }
        Ok(resp)
    }
}

/// How a propose request is answered.
pub enum Road {
    /// `vein.provider = "echo"`: no proposals, no network.
    Echo,
    Live(ProposeRoute),
    /// No provider: the reason, by name.
    Refused(String),
}

impl Road {
    /// `pref` is Principia's raw `vein.provider`; Echo is vein_core's rule, everything else the chat slot's.
    pub fn resolve(pref: Option<&str>, slot: &ProviderSlot) -> Road {
        if prefs::provider_pref(pref.map(str::as_bytes)) == prefs::ProviderPref::Echo {
            return Road::Echo;
        }
        match slot.propose_route() {
            Ok(r) => Road::Live(r),
            Err(why) => Road::Refused(format!("NoProvider: {why}")),
        }
    }
}

/// Principia's `vein.provider`, raw (embed_batch_pref's shape).
pub fn provider_pref() -> Option<String> {
    principia::prefs::PrefStore::load(principia::default_prefs_path()).ok().and_then(|s| match s.get(PREF_NS, "provider") {
        Some(PrefValue::Str(v)) => Some(v),
        _ => None,
    })
}

/// The answer text of one event stream in `dialect`, or why there is none (`ProviderError: ..`, `Dropped`, `Declined`,
/// `Truncated`).
pub fn decode(dialect: Dialect, body: &[u8]) -> Result<String, String> {
    let mut line = vec![0u8; LINE_MAX];
    let mut dec = StreamDecoder::with_dialect(&mut line, dialect);
    let mut text = String::new();
    let mut fail: Option<String> = None;
    let mut on = |e: Event<'_>| match e {
        Event::Text(s) => text.push_str(s),
        Event::Error(m) => {
            fail.get_or_insert_with(|| format!("ProviderError: {m}"));
        }
        Event::Dropped => {
            fail.get_or_insert_with(|| "Dropped".to_string());
        }
        Event::Stop(_) | Event::Done => {}
    };
    dec.feed(body, &mut on);
    dec.finish(&mut on);
    let (done, stop) = (dec.done, dec.stop);
    verdict(fail, stop, done)?;
    Ok(text)
}

/// The stream's verdict once it ended: the first failure, a refusal stop (`Declined`), or no end (`Truncated`).
fn verdict(fail: Option<String>, stop: Option<Stop>, done: bool) -> Result<(), String> {
    if let Some(f) = fail {
        return Err(f);
    }
    if stop == Some(Stop::Refusal) {
        return Err("Declined".to_string());
    }
    if !done {
        return Err("Truncated".to_string());
    }
    Ok(())
}

// --- VUG-VEIN-STREAM (SR531): the stream parsed as it arrives (named hunk) ---
/// One propose stream in `dialect`, decoded and parsed as its chunks arrive: [`ProposeStream::feed`] returns the
/// proposals each chunk completed; [`ProposeStream::finish`] is the whole-body verdict ([`decode`]'s, then
/// `propose::parse` of the whole text).
pub struct ProposeStream<'b> {
    dec: StreamDecoder<'b>,
    inc: propose::Incremental,
    fail: Option<String>,
}

impl<'b> ProposeStream<'b> {
    /// `line` is the decoder's line buffer (one `data:` line; [`LINE_MAX`] in [`answer_with`]).
    pub fn new(line: &'b mut [u8], dialect: Dialect) -> Self {
        ProposeStream { dec: StreamDecoder::with_dialect(line, dialect), inc: propose::Incremental::new(), fail: None }
    }

    /// Feed the next chunk of the event stream; the proposals it completed, in order.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<propose::Proposal> {
        let mut out = Vec::new();
        let (inc, fail) = (&mut self.inc, &mut self.fail);
        let mut on = |e: Event<'_>| on_event(e, inc, fail, &mut out);
        self.dec.feed(chunk, &mut on);
        out
    }

    /// How many proposals the feeds have completed.
    pub fn yielded(&self) -> usize {
        self.inc.yielded()
    }

    /// The end of the stream: the whole-body parse, or why there is none (the stream's verdict, then the parse's
    /// refusal by name).
    pub fn finish(mut self) -> Result<Vec<propose::Proposal>, String> {
        let mut tail = Vec::new();
        {
            let (inc, fail) = (&mut self.inc, &mut self.fail);
            let mut on = |e: Event<'_>| on_event(e, inc, fail, &mut tail);
            self.dec.finish(&mut on);
        }
        // An item the last unterminated line completed is in the whole-body parse below; `tail` is not fired.
        let (done, stop) = (self.dec.done, self.dec.stop);
        verdict(self.fail, stop, done)?;
        self.inc.finish().map_err(|r| format!("{r:?}"))
    }
}

/// One decoder event into the incremental parse (text after a failure is not parsed: the verdict refuses it).
fn on_event(e: Event<'_>, inc: &mut propose::Incremental, fail: &mut Option<String>, out: &mut Vec<propose::Proposal>) {
    match e {
        Event::Text(s) => {
            if fail.is_none() {
                out.extend(inc.push(s));
            }
        }
        Event::Error(m) => {
            fail.get_or_insert_with(|| format!("ProviderError: {m}"));
        }
        Event::Dropped => {
            fail.get_or_insert_with(|| "Dropped".to_string());
        }
        Event::Stop(_) | Event::Done => {}
    }
}

/// vein_core's wire proposal as the bus carries it.
fn wire(q: propose::Proposal) -> IntentProposal {
    IntentProposal { stmt: q.stmt, confidence: q.confidence, why: q.why }
}

/// A refusal naming what arrived before it: `<why> after N proposals` once N were streamed, the bare `why` otherwise.
fn after(why: String, n: usize) -> String {
    if n == 0 { why } else { format!("{why} after {n} proposals") }
}

/// The live road, streamed: each completed proposal through `on` as `Proposal { request, index, proposal }`, then
/// the terminal answer.
async fn live<F: FnMut(IntentEvent) + Send>(request: u64, route: &ProposeRoute, body: Vec<u8>, on: &mut F) -> IntentEvent {
    let mut resp = match route.open(body).await {
        Ok(r) => r,
        Err(why) => return refused(request, why),
    };
    let mut line = vec![0u8; LINE_MAX];
    let mut st = ProposeStream::new(&mut line, route.dialect);
    let mut fired = 0usize;
    loop {
        let chunk = match resp.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => return refused(request, after(format!("Recv: {e}"), fired)),
        };
        for p in st.feed(&chunk) {
            on(IntentEvent::Proposal { request, index: fired as u32, proposal: wire(p) });
            fired += 1;
        }
    }
    match st.finish() {
        Ok(ps) => IntentEvent::Proposals { request, proposals: ps.into_iter().map(wire).collect() },
        Err(why) => refused(request, after(why, fired)),
    }
}

fn refused(request: u64, why: String) -> IntentEvent {
    IntentEvent::Refused { request, why }
}

/// Vein's terminal answer to `ev` on `road` (`None` for anything but a `Propose`). Every refusal is by name — vein_core's
/// (`ImageTooLarge(0)`, `ProseAroundJson`, `BadStmt(1)`) or the road's — and every answer echoes the request id.
pub async fn answer(ev: &IntentEvent, road: &Road) -> Option<IntentEvent> {
    answer_with(ev, road, |_| {}).await
}

/// [`answer`], streamed (VUG-VEIN-STREAM SR531): on the live road each proposal goes through `on` as
/// `IntentEvent::Proposal { request, index, proposal }` as soon as it parses; the return is the one terminal answer.
pub async fn answer_with<F: FnMut(IntentEvent) + Send>(ev: &IntentEvent, road: &Road, mut on: F) -> Option<IntentEvent> {
    let IntentEvent::Propose { context, measurements, images, request, .. } = ev else { return None };
    let request = *request;
    let refs: Vec<propose::ImageRef<'_>> =
        images.iter().map(|im| propose::ImageRef { media_type: im.media_type.as_str(), bytes: im.bytes.as_slice() }).collect();
    let p = propose::Propose { context: context.as_str(), measurements: measurements.as_str(), images: &refs };
    if let Err(r) = propose::check(&p) {
        return Some(refused(request, format!("{r:?}")));
    }
    let text = match road {
        Road::Echo => propose::ECHO_ANSWER.to_string(),
        Road::Refused(why) => return Some(refused(request, why.clone())),
        Road::Live(route) => {
            let body = match propose::request(&p, route.dialect, &route.model) {
                Ok(b) => b,
                Err(r) => return Some(refused(request, format!("{r:?}"))),
            };
            return Some(live(request, route, body, &mut on).await);
        }
    };
    Some(match propose::parse(&text) {
        Ok(ps) => IntentEvent::Proposals { request, proposals: ps.into_iter().map(wire).collect() },
        Err(r) => refused(request, format!("{r:?}")),
    })
}

/// The bus arm's body: answer `ev` on its own task, firing each streamed proposal and then the terminal answer (the
/// brain loop never waits on a provider).
pub fn serve(synapse: Synapse, ev: IntentEvent, road: Road) {
    tokio::spawn(async move {
        let each = synapse.clone();
        if let Some(a) = answer_with(&ev, &road, move |e| each.fire(SMessage::Intent(e))).await {
            synapse.fire(SMessage::Intent(a));
        }
    });
}
