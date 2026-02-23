use reqwest::{RequestBuilder, Response, StatusCode};
use std::time::Duration;
use tokio::time::sleep;

/// The Synaptic Governor.
/// Prevents Lumen from DDOSing the Vertex AI endpoint.
pub trait SynapticRetry {
    async fn fire_with_backoff(self) -> reqwest::Result<Response>;
}

impl SynapticRetry for RequestBuilder {
    async fn fire_with_backoff(self) -> reqwest::Result<Response> {
        let mut attempt = 0;

        loop {
            // Clone the request. If it panics here, you passed a streaming body
            // that can't be cloned. Don't do that in the Cortex.
            let req = self.try_clone().expect("CRITICAL: Uncloneable Cortex payload");
            let res = req.send().await?;

            if res.status() == StatusCode::TOO_MANY_REQUESTS {
                if attempt >= 5 {
                    // The substrate is completely unresponsive. Bubble up the failure.
                    return Ok(res);
                }

                // Exponential backoff with jitter (bitwise shift for speed).
                // Base: 1s, 2s, 4s, 8s, 16s + up to 250ms of chaos.
                let jitter = fastrand::u64(0..250);
                let backoff = (1000 << attempt) + jitter;

                // Throttle the thread.
                sleep(Duration::from_millis(backoff)).await;
                attempt += 1;
                continue;
            }

            return Ok(res);
        }
    }
}
