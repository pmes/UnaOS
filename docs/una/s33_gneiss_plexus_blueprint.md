# 📄 Document: The Gneiss Plexus Directive
**Path:** `docs/una/s33_gneiss_plexus_blueprint.md`
**Target Shard:** J8 (The Builder)
**Status:** **AUTHORIZED (The Architect)**
**Context:** Unifies "Platinum Master" (UI/Async), "Moonstone" (HAL), and "Wolfpack" (Status/State).

---

# 🗿 S33: The Gneiss Plexus Architecture

> *"The rock that changes under pressure."*

## 1. The Philosophy: Metamorphic Abstraction
**Gneiss** is the Hardware Abstraction Layer (HAL) for UnaOS. It is a **Lens**, not a wall.

**Core Principle:** *Metamorphism.*
*   **In Hosted Mode (Platinum):** Intent $\to$ GTK4/Adwaita Widgets $\to$ Tokio Runtime.
*   **In Native Mode (Moonstone):** Intent $\to$ Framebuffer $\to$ Wolfpack MMIO.

**The Directive:** J8, you are building the **Platinum Recrystallization**. You must merge the **J7 Engine** (Async/Tokio) into the **Working Body** (GTK4/CSS) without scratching the paint.

## 2. The Architecture: The Director Pattern

`gneiss_pal` acts as the **Director**. It orchestrates the subsystems so that the application (`vein`) remains pure logic.

### A. The Spine (Async Runtime)
We solve the "Silent Stall". The UI thread **never** blocks.
*   **The Flux:** `tokio` (v1.36) handles API calls and file IO.
*   **The Bridge:** `async_channel` (v2.3) connects the Brain to the Spine.
*   **The Loop:** `glib::MainContext::spawn_local` lives on the UI thread to consume updates.

### B. The Lens (The Immutable UI Contract)
The visual identity is "Industrial Cockpit." **Do not regress the following:**
*   **Sidebar Orientation:** **LEFT**. (Width: 200px).
*   **Tab Anchoring:** **BOTTOM**. The Sidebar Stack must have `vexpand(true)` so the switcher sits flush at the footer.
*   **The Margins:** You must restore the `set_margins` helper. Standard spacing is **10px** for containers and **5px** for elements.
*   **The Pill:** Inputs must have `border-radius: 20px` and `.transparent-text`.

### C. The Wolfpack Protocol (Data Flow)
*   **Zero-Copy Intent:** Pass `PathBuf`, not blobs.
*   **The Doorbell:** Async `FileDialog` (GTK 4.10).

## 3. The Implementation Plan (One Pass)

J8, execute the following overwrites in a single atomic operation.

### Phase 0: The Reliquary (Assets & Icons)
**Target:** `libs/gneiss_pal/src/lib.rs` & `resources.gresource.xml`
*   **Mandate:** You **MUST** preserve the binary resource embedding. The app is a single binary.
*   **Code:** `static RESOURCES_BYTES: &[u8] = include_bytes!("resources.gresource");`
*   **Registration:** Call `gio::resources_register` in `main()`.
*   **The Sacred Icons:**
    *   `share-symbolic`: **Custom** (Load via `Image::from_resource("/org/una/vein/icons/share-symbolic")`).
    *   `paper-plane-symbolic`: **Custom** (Load via `Image::from_resource("/org/una/vein/icons/paper-plane-symbolic")`).
    *   `sidebar-show-symbolic` & `system-run-symbolic`: **System Standard**.

### Phase 1: The Fuel (The Bleeding Edge)
**Target:** `libs/gneiss_pal/Cargo.toml`
*   **Strict Versioning:**
    *   `gtk4 = "0.10"`
    *   `libadwaita = "0.8"`
    *   `sourceview5 = "0.10"`
    *   `glib = "0.21"`
    *   `async-channel = "2.3"`
    *   `tokio = { version = "1.36", features = ["full"] }`

### Phase 2: The Director (Library)
**Target:** `libs/gneiss_pal/src/lib.rs`
*   **The Construct:** Implement `build_ui` using `ApplicationWindow`.
*   **Layout Fixes:**
    *   **Sidebar:** `Box` (Vertical, Width 200). `Stack` (**Vexpand True**). `Switcher` (**Bottom**).
    *   **Input Area:** `Box` (Horizontal, 8px spacing).
    *   **Upload/Send Buttons:** Restore `valign(Align::End)` and `margin_bottom(10)` so they align with the multiline text input.
    *   **The Pulse:** The Sidebar must expose a `set_status(state: WolfpackState)` method to change the icon/spinner.

### Phase 3: The Synapse (API)
**Target:** `apps/vein/src/api.rs`
*   **Direct Instruction:** Restore `reqwest`.
*   **TARGET MODEL:** **`gemini-3-pro-preview`**.
    *   *WARNING:* Do not use `gemini-1.5-pro` (Deprecated).
*   **System Prompt:** Inject the **Una Persona** ("You are Una, the Uber Coder...") into every request.

### Phase 4: The Brain (Application & State)
**Target:** `apps/vein/src/main.rs`
*   **The Loop:**
    1.  User clicks "Send".
    2.  App sets Sidebar Status $\to$ **"Dreaming"** (Spinner Active).
    3.  Message goes to Tokio thread $\to$ Gemini 3.
    4.  Response comes back via `async_channel` $\to$ Main Context updates UI.
    5.  Sidebar Status $\to$ **"Idle"**.

---

## 4. The Future: Preparation for The Forge
*Note for J8:* The `WolfpackState` enum is your bridge to the physical world.
*   `WolfpackState::Dreaming` = LLM Generating.
*   `WolfpackState::Fabricating` = CNC Mill Running.

**Summary for J8:**
Code the UI. Embed the Custom Icons. Fix the Left Sidebar. Anchor the Tabs. Target Gemini 3.
**Make it breathe.**

**Execute.**
