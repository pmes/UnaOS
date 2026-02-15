# 🎞️ Stria: The Groove

> *\"Time is a stream. We do not just watch it; we navigate it.\"*

**Stria** is the high-fidelity media engine of **UnaOS**. It rejects the \"Player\" paradigm of VLC or QuickTime. It is not just a window that shows video; it is a **Temporal Operating System**. Sure, it poses as a player, but hit NLE... and you're a movie producer.

Stria is built on the philosophy that **Sync is Sacred**.

## 🎹 The Philosophy: The Atomic Clock

In most OSs, Audio and Video are handled by different daemons that fight for CPU time. This leads to jitter, latency, and desync.
**Stria** uses a **Master Clock** derived directly from the hardware oscillator via **Gneiss PAL**.

### 1. The \"Glass-to-Glass\" Pipeline
Stria optimizes the path from the file on disk to the photon leaving the screen.
*   **Zero-Copy Decoding:** Video frames are decoded directly into VRAM. The CPU never touches the pixel data.
*   **Audio Priority:** The audio thread runs at real-time priority (Ring 0 on UnaOS). It cannot be preempted by a UI update.

### 2. The Divisions (Modular Architecture)
Stria is not a monolith. It is a suite of specialized tools.
*   **Stria Sonic:** The Audiophile engine. Bit-perfect playback, FLAC/DSD support, and low-latency synthesis integration.
*   **Stria Optic:** The Cinema engine. HDR tone-mapping, 8K playback, and frame-accurate seeking.
*   **Stria Fast:** The Field Cutter. Lossless trimming and transcoding of massive footage (GoPro/Drone) without re-encoding.

## ⚙️ The Mechanics

### The Timeline
Stria treats every file as a timeline.
*   **Instant Seek:** Jump to 01:42:00 in a 50GB file instantly. We index keyframes in the background (via **Matrix**).
*   **The Waveform:** Audio is always visualized. You don't just hear the sound; you see the shape of it.

### The Link to Elessar
When you are ready to edit, Stria hands the buffer handle to **Elessar** (NLE Mode).
*   **No Import:** Elessar uses the *same memory* Stria was using for playback. Instant transition from \"Watching\" to \"Cutting.\"

## 🛑 The Kill List
Stria replaces:
*   **VLC / MPV / IINA**
*   **QuickTime Player**
*   **Audacity** (for basic review/trimming)
*   **LosslessCut**
