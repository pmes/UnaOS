# 🏛️ Principia: The Law

> *"Order is not accidental. It is engineered."*

**Principia** is the unified configuration and policy engine for **UnaOS**. It rejects the chaotic "Settings Sprawl" of traditional operating systems, where every application reinvents its own preferences window.

In UnaOS, **Settings are a System Service.**

## ⚙️ The Philosophy: Schema is UI

Principia does not have hard-coded menus. It generates its GUI dynamically based on **Configuration Schemas** exposed by the Shards.

## 🌍 Host Mode: The Dotfile Sovereign

When running on **Linux, macOS, or Windows**, Principia acts as a universal configuration manager for your environment. It unifies the fragmented settings of the host OS.

### The "Meta-Config"
Principia can map its schemas to external config files.
*   **Unified Theme:** Toggle "Dark Mode" in Principia -> Updates GTK, Qt, macOS System Appearance, and VS Code simultaneously.
*   **Unified Typography:** Set "Monospace Font" -> Updates Terminal, Editor, and IDE.

### The Safety Net
Principia wraps your local config files (`~/.config`, `.bashrc`, `.gitconfig`) in a **Vairë** safety layer.
*   **Automatic Backups:** Before writing to an external file, Principia snapshots it.
*   **Drift Detection:** If an external app modifies a file Principia is watching, it alerts you to the conflict.

**Principia makes your Linux/Mac environment reproducible.**

### 1. The Universal Registry
When you install a Shard (e.g., **Facet**), it registers a `schema.toml` with Principia.
*   **The Facet Schema:** Defines "Brush Sensitivity" (Float: 0.0-1.0), "Dark Mode" (Bool), and "Cache Path" (Path).
*   **The Principia Action:** It instantly renders a standardized, accessible Settings Page for Facet within the main System Config.

### 2. The Authoritative Writer (Vairë Integration)
Principia is the only component allowed to write to the `/config` directory.
*   **Versioned Settings:** Every time you toggle a switch, Principia commits the change to **Vairë**.
*   **Time Travel:** Broken config? You can roll back your entire system's preferences (Kernel *and* Apps) to "Yesterday at 4:00 PM."

### 3. The Guard Rails (Vein Integration)
Principia validates inputs against the **Laws of Physics**.
*   **Static Validation:** It prevents you from setting a resolution your monitor cannot support.
*   **Semantic Validation (Vein):** Before applying a dangerous change, it consults the AI.
    *   *User:* Sets `voltage_offset = +1.5V` on the CPU.
    *   *Principia (via Vein):* **"ALERT: This setting exceeds thermal safety limits. You are likely to physically damage the hardware. Proceed?"**

## 🛑 The Kill List
Principia replaces:
*   **Windows Registry / macOS Defaults**
*   **Application-specific "Preferences" Windows** (Unified into one place)
*   **NVIDIA Control Panel**
*   **`ethtool` / `sysctl`** (GUI frontends)
