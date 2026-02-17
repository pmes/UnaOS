# Junct (The Communications Hub)

**Layer:** Layer 2 (Capability)
**Role:** Communications Aggregator & Protocol Bridge
**Crate:** `handlers/junct`

## 📡 Overview

**Junct** is the central nervous system for human-to-human interaction within UnaOS. It acts as a universal adapter for communication protocols, aggregating distinct streams (Matrix, Signal, IRC, SIP, WebRTC) into a single, unified state model.

Where **Vein** handles Human-AI interaction, **Junct** handles Human-Human interaction. It abstracts the complexity of connection management, encryption, and media negotiation so that Vessels can present a unified "Inbox" or "Conference Room."

## 🏗️ Architecture

Junct sits at **Layer 2 (Handlers)** of the Trinity Architecture.

* **The Core:** Maintains a unified `Contact` and `Message` model in `libs/gneiss_pal`.
* **The Bridges:** Implements or wraps protocol-specific clients (The "Spokes").
* **The View:** Provides GTK4 widgets for chat lists, video grids, and call controls via `libs/quartzite`.

## 🔗 Capabilities

Junct harmonizes the following protocols:

| Protocol | Implementation | Role |
| --- | --- | --- |
| **Matrix** | Native Rust SDK | The backbone of UnaOS messaging. End-to-End Encrypted. |
| **WebRTC** | Native / GStreamer | Video/Audio conferencing. Compatible with Jitsi/Zoom web wrappers. |
| **SIP** | VoIP Stack | Legacy telephony and standard voice calls. |
| **IRC** | Text Protocol | Developer chat and legacy channels. |
| **ActivityPub** | Fediverse | Social stream aggregation (Mastodon/Lemmy). |

## 🔌 Integration

**Used by `apps/una` (The Workspace):**
Junct powers the "Team View" or "Comms Panel."

1. **Unified Inbox:** Aggregates notifications from GitHub, Matrix, and Email (via `handlers/holocron`).
2. **Presence:** Broadcasts "Coding" or "Busy" status based on **Tabula** activity.
3. **Quick Calls:** Initiates WebRTC sessions directly from the editor context.

**Usage Example (Rust):**

```rust
use junct::{Client, Protocol, Message};

let client = Client::connect(Protocol::Matrix, credentials).await?;

// Sending a message
client.send(target_room, Message::text("Deploying S60 now.")).await?;

// Handling incoming streams
while let Some(event) = client.next_event().await {
    match event {
        Event::Message(msg) => println!("New msg from {}: {}", msg.sender, msg.content),
        Event::CallInvite(call) => junct::webrtc::accept(call),
    }
}

```

## ⚠️ Status

**Experimental.**

* *Requirement:* Heavy reliance on `async-std` / `tokio` for stream management.
* *Security:* Handles sensitive credentials. Must interface strictly with `handlers/holocron` for key storage.
* *Edition:* **Rust 2024**.
