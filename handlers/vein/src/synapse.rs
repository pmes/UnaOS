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

use reqwest::{RequestBuilder, Response, StatusCode};
use std::time::Duration;
use tokio::time::sleep;
use std::future::Future;
use rand::RngExt;

/// The Synaptic Governor.
/// Prevents Lumen from DDOSing the Vertex AI endpoint.
pub trait SynapticRetry {
    // Desugared to avoid `async fn` in trait warning and allow Send bounds.
    fn fire_with_backoff(self) -> impl Future<Output = reqwest::Result<Response>> + Send;
}

impl SynapticRetry for RequestBuilder {
    fn fire_with_backoff(self) -> impl Future<Output = reqwest::Result<Response>> + Send {
        // We move the template into the async block.
        // But since `try_clone()` borrows `self`, we need to ensure the async block owns `self`
        // and clones FROM it repeatedly.
        let template = self;

        async move {
            let mut attempt = 0;
            loop {
                // Try to create a fresh request from the template.
                let req = template
                    .try_clone()
                    .expect("CRITICAL: Uncloneable Cortex payload");

                match req.send().await {
                    Ok(res) => {
                        let status = res.status();
                        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                            if attempt >= 5 {
                                return Ok(res);
                            }
                            // Modern Rand API
                            let jitter: u64 = rand::rng().random_range(0..250);
                            let backoff = (1000 << attempt) + jitter;
                            sleep(Duration::from_millis(backoff)).await;
                            attempt += 1;
                            continue;
                        }
                        return Ok(res);
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
}
