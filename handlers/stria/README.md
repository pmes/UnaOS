# Stria (The Studio)

**Layer:** Layer 2 (Capability)
**Role:** Audio/Video Studio & Media Engine
**Crate:** `handlers/stria`

## 🎬 Overview

**Stria** is the media engine of the UnaOS ecosystem. It is a powerful handler library that provides the logic and interface for playback, recording, and non-linear editing of audio and video content.

While **Facet** handles static imagery, **Stria** handles time-based media. It wraps complex multimedia frameworks (GStreamer/FFmpeg) into a clean, Rust-native API for the Trinity Architecture.

## 🏗️ Architecture

Stria sits at **Layer 2 (Handlers)** of the Trinity Architecture.

* **The Engine:** Built on top of **GStreamer** (via `gstreamer-rs`) for high-performance, hardware-accelerated pipeline management.
* **The View:** Provides GTK4 widgets for video surfaces, timelines, waveforms, and transport controls via `libs/quartzite`.
* **The State:** Manages the playback clock and media library in `libs/gneiss_pal`.

## 🎛️ Capabilities

Stria provides the following core services:

| Feature | Description |
| --- | --- |
| **Playback** | Hardware-accelerated video/audio playback (MP4, MKV, FLAC, WAV). |
| **Timeline** | Non-linear editing logic. Multi-track support for cutting and arranging clips. |
| **Capture** | Low-latency recording from microphones (`cpal`) and webcams (V4L2/PipeWire). |
| **Transcode** | Format conversion and rendering (e.g., export project to H.264). |
| **Analysis** | Real-time audio visualization (FFT) and waveform generation. |

## 🔌 Integration

**Used by `apps/una` (The Host):**
Stria powers the "Media Preview" and "Studio Mode."

1. **Asset Preview:** When you click a `.wav` or `.mp4` file in the file explorer, Stria renders the preview pane.
2. **Screencasting:** It handles the logic for recording the screen/window for bug reports or demos.
3. **Voice Notes:** Used by **Vein** to capture audio input for transcription.

**Usage Example (Rust):**

```rust
use stria::{Player, MediaSource};

// Simple Playback
let mut player = Player::new();
player.load(MediaSource::file("render.mp4"));
player.play();

// Accessing the view widget for GTK
let video_widget = player.widget();
container.append(&video_widget);

```

## ⚠️ Status

**Experimental.**

* *Requirement:* Requires `gstreamer` and `gst-plugins-base/good/bad/ugly` installed on the host OS (Fedora).
* *Audio Backend:* Uses **PipeWire** (via `cpal` or `gstreamer-pipewire`).
* *Edition:* **Rust 2024**.
