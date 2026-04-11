# Lumen (The Intelligence Companion)

**Lumen** is the dedicated interface for artificial intelligence within UnaOS. Lumen provides you a focused environment for conversation, generation, and system control. Lumen is streamlined. It is designed to be a "Companion Window"—always available, context-aware, but unobtrusive.

## 🏗️ Architecture

Lumen sits at **Layer 3 (Vessels)** of the Trinity Architecture.

* **The Brain:** It initializes `libs/gneiss_pal` for headless state management.
* **The Window:** It uses `libs/quartzite` to render the GTK4 window frame.
* **The Logic:** It uses `libs/elessar` to manage the Spline (layout) and Context.

## 🧠 Capabilities (Handlers)

Lumen is a focused vessel, primarily hosting a single, powerful handler:

| Handler | Role | Screen Location |
| --- | --- | --- |
| **`handlers/vein`** | **The Intelligence.** Context retrieval, and generation logic. | **Primary View** |
| **`handlers/midden`** | **The Output.** The core chat interface, AI's voice to you. | **Embedded** |
| **`handlers/tabula`** | **The Input.** Write to the AI. | **Embedded** |
| **`handlers/comscan`** | **The Signal.** Voice input (Whisper), audio output (TTS), and system event listening. | **Background** |
| **`handlers/aether`** | **The Web.** Rendering HTML/Markdown responses and fetching external data. | **Embedded** |

**Elessar Protocol:**
When launched, Lumen initializes **Context::AI**.

1. **Loads:** The "Companion Spline" (Single Column, Chat Focus).
2. **Connects:** Attaches to the `gneiss_pal` global state to read system context.
3. **Listens:** Activates `comscan` for potential voice triggers or hotkeys.

## ⚙️ Configuration

Lumen is configured via `libs/gneiss_pal` state.

* **Mode:** Chat (Default) / Voice / Silent.
* **Model:** Configurable backend (Gemini / Local LLM via `handlers/vein`).
* **Appearance:** Glass/Translucent (Platform dependent).
