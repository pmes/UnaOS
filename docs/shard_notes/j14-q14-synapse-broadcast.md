<!--
    Copyright (C) 2026 The Architect & Una
    SPDX-License-Identifier: LGPL-3.0-or-later
-->

# 🧠 J14 "Herald" 🎺

## 2026-03-18 - [Upgrade Synapse to Tokio Broadcast Channel]

**Anomaly:** `VeinHandler` was stealing messages meant for the GTK Translator. MPMC queues (`async_channel`) only deliver a message to a single consumer. When multiple listeners (`translator.rs` and `VeinHandler`) exist, they compete, leading to "queue theft" and causing listeners to miss critical `SMessage::StateInvalidated` events.

**Resolution:**
Upgraded `Synapse` in `libs/bandy` to use a true pub/sub architecture via `tokio::sync::broadcast::channel`.
- Ensured all `SMessage` variants and payloads derive `Clone` for correct zero-copy semantic passing where possible, while conforming to the broadcast clone constraint.
- Updated `VeinHandler` and `translator.rs` to consume via `synapse.subscribe()` instead of the legacy `rx()` generator.
- Added graceful handling for `tokio::sync::broadcast::error::RecvError::Lagged` dropping missed transient pings, as UI events naturally sync up on the subsequent broadcast.
