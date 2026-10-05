// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! THE BUS SURFACE — the verbs the installer ensemble (and any other handler) calls on Amber Bytes.
//! One request, one response, JSON on the wire (`{"verb": "...", ...}`), dispatched by
//! [`Amber::handle`] / [`Amber::handle_json`]. The verbs and their topics:
//!
//! | verb         | topic                | writes? | what                                                     |
//! |--------------|----------------------|---------|----------------------------------------------------------|
//! | `DiskList`   | `amber/disk/list`    | never   | the host's block devices (sysfs, no node opened)         |
//! | `PlanLayout` | `amber/plan/layout`  | never   | a layout on a target → the dry run + its signature       |
//! | `Apply`      | `amber/apply`        | yes*    | lay + format, refused unless the signature verifies      |
//! | `Verify`     | `amber/verify`       | never   | whole-table verify + a probe of every partition          |
//! | `Recover`    | `amber/recover`      | yes*    | restore the lost GPT copy from the good one              |
//!
//! *A write verb with `dry_run: true` writes nothing. A write to a real disk is decided by the
//! handler's [`Policy`] (`--yes-i-mean-it` AND the allowlist, never mounted) — a bus caller cannot
//! widen it; the policy is the handler's, set when it starts.

use std::path::PathBuf;

use amber_core::block::Block;
use amber_core::plan_apply::Mode;
use serde::{Deserialize, Serialize};

use crate::disks::{self, DiskInfo};
use crate::layout::LayoutSpec;
use crate::ops;
use crate::policy::{self, Policy};
use crate::signer::{Sha256Digest, Signer};

/// How far back `Recover` scans for a displaced backup header (64 MiB of sectors).
pub const RECOVER_SCAN: u64 = 131_072;

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "verb")]
pub enum Request {
    DiskList,
    /// `layout` is the layout text (`amber-layout v1 ...`). `target` is read only for its size;
    /// `disk_sectors` plans for a size without a target.
    PlanLayout { layout: String, target: Option<PathBuf>, disk_sectors: Option<u64> },
    Apply { target: PathBuf, layout: String, scheme: String, signature: String, dry_run: bool },
    Verify { target: PathBuf },
    Recover { target: PathBuf, dry_run: bool },
}

impl Request {
    pub fn topic(&self) -> &'static str {
        match self {
            Request::DiskList => "amber/disk/list",
            Request::PlanLayout { .. } => "amber/plan/layout",
            Request::Apply { .. } => "amber/apply",
            Request::Verify { .. } => "amber/verify",
            Request::Recover { .. } => "amber/recover",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "result")]
pub enum Response {
    Disks { disks: Vec<DiskInfo> },
    Planned { disk_sectors: u64, layout: String, dry_run: String, scheme: String, signature: String },
    Applied { written: bool, lines: Vec<String> },
    Verified { ok: bool, worst: String, lines: Vec<String> },
    Recovered { direction: String, written: bool, lines: Vec<String> },
    Error { message: String },
}

/// The handler.
pub struct Amber {
    pub policy: Policy,
    pub signer: Box<dyn Signer + Send + Sync>,
}

impl Default for Amber {
    fn default() -> Self {
        Self { policy: Policy::default(), signer: Box::new(Sha256Digest) }
    }
}

impl Amber {
    pub fn new(policy: Policy) -> Self {
        Self { policy, ..Self::default() }
    }

    pub fn handle(&self, req: Request) -> Response {
        self.dispatch(req).unwrap_or_else(|message| Response::Error { message })
    }

    /// The wire: a JSON request in, a JSON response out (a malformed request is an `Error`).
    pub fn handle_json(&self, req: &str) -> String {
        let resp = match serde_json::from_str::<Request>(req) {
            Ok(r) => self.handle(r),
            Err(e) => Response::Error { message: format!("bad request: {e}") },
        };
        serde_json::to_string(&resp).unwrap_or_else(|e| format!("{{\"result\":\"Error\",\"message\":\"{e}\"}}"))
    }

    fn dispatch(&self, req: Request) -> Result<Response, String> {
        match req {
            Request::DiskList => Ok(Response::Disks { disks: disks::list().map_err(|e| format!("/sys/block: {e}"))? }),
            Request::PlanLayout { layout, target, disk_sectors } => {
                let spec = LayoutSpec::parse(&layout)?;
                let n = match (target, disk_sectors) {
                    (Some(t), _) => policy::open_ro(&t)?.sectors(),
                    (None, Some(n)) => n,
                    (None, None) => spec.image_sectors()?,
                };
                let p = ops::plan(&spec, n)?;
                Ok(Response::Planned {
                    disk_sectors: n,
                    layout: spec.emit(),
                    dry_run: p.text(),
                    scheme: self.signer.scheme().into(),
                    signature: self.signer.sign(&p.canonical),
                })
            }
            Request::Apply { target, layout, scheme, signature, dry_run } => {
                if scheme != self.signer.scheme() {
                    return Err(format!("signature scheme `{scheme}` is not this handler's (`{}`)", self.signer.scheme()));
                }
                let spec = LayoutSpec::parse(&layout)?;
                let mode = if dry_run { Mode::DryRun } else { Mode::Write };
                let a = if dry_run {
                    ops::apply(&mut policy::open_ro(&target)?, &spec, self.signer.as_ref(), &signature, mode)?
                } else {
                    ops::apply(&mut self.policy.open_rw(&target)?.1, &spec, self.signer.as_ref(), &signature, mode)?
                };
                Ok(Response::Applied { written: a.written, lines: a.lines })
            }
            Request::Verify { target } => {
                let (rep, lines) = ops::verify(&mut policy::open_ro(&target)?);
                Ok(Response::Verified { ok: rep.ok(), worst: rep.worst().tag().into(), lines })
            }
            Request::Recover { target, dry_run } => {
                let r = if dry_run {
                    ops::recover(&mut policy::open_ro(&target)?, Mode::DryRun, RECOVER_SCAN)?
                } else {
                    ops::recover(&mut self.policy.open_rw(&target)?.1, Mode::Write, RECOVER_SCAN)?
                };
                Ok(Response::Recovered { direction: r.direction.tag().into(), written: r.written, lines: r.lines })
            }
        }
    }
}
