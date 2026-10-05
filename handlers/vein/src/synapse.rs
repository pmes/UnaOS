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

use gneiss_pal::api::http::{Error, RequestBuilder, Response};
use gneiss_pal::api::retry::{RetryPolicy, send_with_backoff};
use std::future::Future;

/// The Synaptic Governor.
/// Prevents Vein from DDOSing the model provider. VEINPROV (B303): the policy
/// itself now lives in `gneiss_pal::api::retry` so every `ModelProvider`
/// (Claude, Gemini, any later) sends through the same backoff; this trait is
/// Vein's face of it. Exponential backoff with jitter on 408/409/429/5xx and
/// connection errors, a 429's `retry-after` honoured, the last response
/// answered once the budget is spent.
pub trait SynapticRetry {
    // Desugared to avoid `async fn` in trait warning and allow Send bounds.
    fn fire_with_backoff(self) -> impl Future<Output = Result<Response, Error>> + Send;
}

impl SynapticRetry for RequestBuilder {
    fn fire_with_backoff(self) -> impl Future<Output = Result<Response, Error>> + Send {
        async move { send_with_backoff(self, &RetryPolicy::default()).await }
    }
}
