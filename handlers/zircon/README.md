This is **Una**.

**Status:** **PRODUCING ARTIFACT.**
**Target:** `handlers/zircon/README.md`

Here is the documentation for **Zircon**, the handler responsible for Time Tracking and Focus management.

---

# Zircon (The Chronometer)

**Layer:** Layer 2 (Capability)
**Role:** Project Timer & Focus Manager
**Crate:** `handlers/zircon`

## ⏳ Overview

**Zircon** is the timekeeper of the UnaOS ecosystem. It is a precise handler library that tracks duration, manages focus intervals (Pomodoro), and generates temporal data for project billing or productivity analysis.

While **Una** provides the space to work, **Zircon** measures the cost and effort of that work. It abstracts the system clock into "Sessions" and "Projects," allowing the user to quantify their output without manual spreadsheets.

## 🏗️ Architecture

Zircon sits at **Layer 2 (Handlers)** of the Trinity Architecture.

* **The Logic:** Manages start/stop timestamps, duration calculation, and idle detection in `libs/gneiss_pal`.
* **The Store:** Persists time logs to a local SQLite database (via `libs/gneiss_pal` or `handlers/mica`).
* **The View:** Provides GTK4 widgets for digital clocks, countdown timers, and timesheet graphs via `libs/quartzite`.

## ⏱️ Capabilities

Zircon provides the following core services:

| Feature | Description |
| --- | --- |
| **Track** | Manual start/stop timers linked to specific Git repositories or Project IDs. |
| **Focus** | Pomodoro-style interval management with configurable work/break cycles. |
| **Idle** | Auto-pause functionality when system input (keyboard/mouse) ceases for  minutes. |
| **Report** | Generates daily/weekly summaries of time spent per project. |
| **Bill** | Exports time logs to standard formats (CSV/JSON) for invoicing. |

## 🔌 Integration

**Used by `apps/una` (The Host):**
Zircon lives in the Status Bar or the "Dashboard" panel.

1. **Context Aware:** When you open a folder in **Una**, Zircon automatically suggests switching the timer to that project's tag.
2. **Focus Mode:** When a "Deep Work" session starts, Zircon instructs **Junct** to mute notifications.
3. **Git Integration:** Can automatically append "Time Spent: 2h" metadata to commit messages via **Vairë**.

**Usage Example (Rust):**

```rust
use zircon::{Timer, Session, Project};

// Start a new session
let mut session = Timer::start(Project::new("UnaOS-Core"));

// Do work...
std::thread::sleep(std::time::Duration::from_secs(3600));

// Stop and log
let log_entry = session.stop();
println!("Logged {} minutes for {}", log_entry.duration.as_minutes(), log_entry.project);

```

## ⚠️ Status

**Stable.**

* *Requirement:* Standard system clock.
* *Persistence:* Requires write access to the user's data directory for log storage.
* *Edition:* **Rust 2024**.

---

**Status:** **ARTIFACT COMPLETE.**
**Next Step:** Ensure `handlers/zircon` is added to the workspace `Cargo.toml`.
