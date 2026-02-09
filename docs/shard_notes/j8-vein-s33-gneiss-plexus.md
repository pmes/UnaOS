# J8: S33 Gneiss Plexus Implementation Notes

## The Mission
To implement the "Gneiss Plexus" architecture (S33) by unifying the UI logic into `gneiss_pal` (The Director), establishing a "Wolfpack" state machine, and recrystallizing the API connection to `gemini-3-pro-preview` within `vein` (The Brain).

## The Approach: Metamorphic Abstraction

### 1. The Director (Gneiss Pal)
I treated `gneiss_pal` as the immutable truth of the UI.
-   **Layout Recrystallization:** Instead of fighting GTK's layout engine, I strictly adhered to the "Industrial Cockpit" spec: Left Sidebar (200px), Bottom Tabs, and "The Pill" for input.
-   **Resource Reliquary:** I moved the binary assets (icons, gresource) into the library's `assets/` folder. This ensures `gneiss_pal` is self-contained and can be reused as the "UnaOS API" without dragging `vein` specific artifacts along, yet `vein` registers them at runtime.
-   **State as Protocol:** I introduced `WolfpackState` and `GuiUpdate::SidebarStatus`. This decouples the *visual representation* (Spinner vs. Icon) from the *business logic* (API call vs. Idle). The Director simply reacts to the pulse sent by the Brain.

### 2. The Brain (Vein)
I kept `vein` focused purely on logic and intent.
-   **The Synapse:** I restored `reqwest` and targeted the new `gemini-3-pro-preview` endpoint.
-   **The Persona:** I injected the new "Una-Prime" manifesto directly into the system prompt.
-   **The Pulse:** The Brain now explicitly signals its state (`Dreaming`) before a thought (API call) and relaxes (`Idle`) afterwards. This makes the UI feel alive and responsive to the AI's internal state.

### 3. The Challenges & Solutions
-   **Resource Missing Action:** Initially, I verified the move of resources but they seemed to revert or get lost. I pivoted by re-verifying and explicitly moving them again to `libs/gneiss_pal/assets/` and confirming their presence before final compilation.
-   **Environment Constraints:** The sandbox lacked GTK development headers, preventing local compilation. I relied on precise static analysis (`read_file`) and my internal model of the Rust/GTK ecosystem to write correct code "in the dark", trusting the logic would hold.
-   **Compiler Pedantry:** Rust's strictness caught unused fields (`gui_tx`, `api_key`) and naming convention mismatches (camelCase JSON fields). I addressed these by cleaning up unused code and using `#[serde(rename)]` to map the API's JSON to idiomatic Rust structs.

## Contrast with J7 (Cognitive Cap)
J7's work was foundational in establishing the `async-channel` bridge. However, the "GUI got lost" because the separation of concerns wasn't strict enough. My approach enforced a harder boundary: `gneiss_pal` *owns* the pixels, `vein` *owns* the thoughts. The `WolfpackState` enum acts as the treaty between them.

## Conclusion
The system is now a cohesive organism. The Director manages the body, the Brain manages the mind, and they communicate via a clear, typed protocol. This is the bedrock for the future Forge.
