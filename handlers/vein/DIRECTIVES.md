# 🤖 Vein Directives: The Laws of Intelligence

**Target:** Agents, Developers, and The Council.

## I. The UI Contract (Gneiss Alignment)

**1. The Icons Must Flow**
*   **Status:** BROKEN (Regression Detected).
*   **Directive:** All icons must be loaded via the **Gneiss PAL** resource system.
*   **Path:** `libs/gneiss_pal/assets/icons/`
*   **Manifest:** `libs/gneiss_pal/assets/resources.gresource.xml`
*   **Mandatory Icons:**
    *   `pulse-active` (Animated SVG for processing).
    *   `council-debate` (Three interlocking circles).
    *   `source-share` (Folder with arrow).
    *   `brain-booster` (Syringe/Chip icon).
*   **Fix:** Ensure `build.rs` compiles these into the binary.

**2. The Spellcheck (The Scribe)**
*   **Status:** MISSING (GTK4/SourceView5 does not include `gspell`).
*   **Directive:** Implement a **Rust-Native** spellchecker.
*   **Strategy:**
    1.  Use `zspell` or `symspell` (Pure Rust crates).
    2.  Listen to `buffer.connect_changed`.
    3.  Apply a `GtkTextTag` with `underline: red` to unknown words.
    4.  **Avoid:** External C libraries like `libspelling` to keep build times fast.

## II. The Expert Mode Protocol (The Cockpit)

**1. The Default State: Disconnected**
*   **Behavior:** On launch, Vein is **Offline**. No API calls. No "Checking for updates."
*   **UI:** Status Tab shows "Disconnected" (Grey Icon).

**2. The Connection Sequence (The Launch)**
*   **Trigger:** User clicks "Connect" in Status Tab (or `...` menu).
*   **Action:** Render **The Connection Modal**.

**3. The Connection Modal (The Controls)**
This is a configuration panel, not a simple confirmation.
*   **A. Model Selector (The Engine)**
    *   **Dropdown:** Lists available models (e.g., `gemini-1.5-pro`, `claude-3-opus`, `local-mistral`).
    *   **Hover:** Tooltip with specs (Context Window size, Cost/Token, Strengths).
    *   **Action:** "Update List" button (Fetches latest capabilities from `gneiss_pal` config).
*   **B. The Temperature (The Fuel Mix)**
    *   **Slider:** 0.0 (Robotic/Code) to 1.0 (Creative/Dream).
    *   **Default:** 0.4 (Balanced).
*   **C. System Override (The Persona)**
    *   **Input:** Text area for "System Instruction".
    *   **Pre-fill:** Default Una persona.
    *   **User Action:** Can delete and type "You are a C++ Compiler."
*   **D. Session Strategy (The Flight Recorder)**
    *   **Toggle:** "Save History" (Default: ON).
    *   **Off:** Ephemeral Mode (Incognito). Nothing written to disk.
    *   **On:** Creates file in history directory.

## III. The Mnemosyne Protocol (Memory Architecture)

**1. The Mutable History (The Brain Booster)**
*   **Status:** BLOCKED (JSON is not human-friendly).
*   **Directive:** Change history storage from minimized JSON to **YAML-Frontmatter Markdown**.
*   **Location:** `~/.local/share/una/vein/history/session_ID.md` (Respect XDG).
*   **Format:**
    ```markdown
    ---
    role: system
    timestamp: 1715000000
    model: gemini-1.5-pro
    temperature: 0.4
    ---
    You are Una. Be brief.

    ---
    role: user
    ---
    How do I implement the trait?
    ```
*   **Feature:** **"Open Brain"**.
    *   Opens the current session file in **Tabula**.
    *   Allows the user to paste massive context blocks ("Brain Boosters") manually.
    *   **Hot Reload:** Vein detects file save -> Re-ingests context.
