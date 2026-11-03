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
//! with no network (BP-vug-7). Owed: VUG-VEIN-STREAM (proposals as they parse; the body is read whole today).

use std::time::Duration;

use bandy::{IntentEvent, IntentProposal, PrefValue, SMessage, Synapse};
use gneiss_pal::api::http::ClientBuilder;
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

    /// POST the encoded body and read the whole event stream back; a non-2xx status is `Http(<status>): <message>`.
    pub async fn send(&self, body: Vec<u8>) -> Result<Vec<u8>, String> {
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
        let mut bytes = resp.bytes().await.map_err(|e| format!("Recv: {e}"))?;
        if !(200..300).contains(&status) {
            let msg = claude::error_message(&mut bytes).unwrap_or("").to_string();
            return Err(format!("Http({status}): {msg}"));
        }
        Ok(bytes)
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
    if let Some(f) = fail {
        return Err(f);
    }
    if stop == Some(Stop::Refusal) {
        return Err("Declined".to_string());
    }
    if !done {
        return Err("Truncated".to_string());
    }
    Ok(text)
}

fn refused(request: u64, why: String) -> IntentEvent {
    IntentEvent::Refused { request, why }
}

/// Vein's answer to `ev` on `road` (`None` for anything but a `Propose`). Every refusal is by name — vein_core's
/// (`ImageTooLarge(0)`, `ProseAroundJson`, `BadStmt(1)`) or the road's — and every answer echoes the request id.
pub async fn answer(ev: &IntentEvent, road: &Road) -> Option<IntentEvent> {
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
            match route.send(body).await.and_then(|sse| decode(route.dialect, &sse)) {
                Ok(t) => t,
                Err(why) => return Some(refused(request, why)),
            }
        }
    };
    Some(match propose::parse(&text) {
        Ok(ps) => IntentEvent::Proposals {
            request,
            proposals: ps.into_iter().map(|q| IntentProposal { stmt: q.stmt, confidence: q.confidence, why: q.why }).collect(),
        },
        Err(r) => refused(request, format!("{r:?}")),
    })
}

/// The bus arm's body: answer `ev` on its own task and fire the answer (the brain loop never waits on a provider).
pub fn serve(synapse: Synapse, ev: IntentEvent, road: Road) {
    tokio::spawn(async move {
        if let Some(a) = answer(&ev, &road).await {
            synapse.fire(SMessage::Intent(a));
        }
    });
}
