# 📊 Mica: The Crystalline Grid

> *"The world is data. Mica is the lens."*

**Mica** is the **High-Performance Data Grid** and **Spreadsheet** for the **UnaOS** ecosystem.

It rejects the bloated legacy of Excel and the limitations of CSV. Mica treats data as a **reactive surface**. It is not just rows and columns; it is a multi-dimensional array of logic.

---

## 💎 The Philosophy

### 1. The Infinite Sheet
Traditional spreadsheets choke on 100,000 rows.
**Mica** uses **Virtualization** and **Rust-native memory mapping**.
*   **Capacity:** 100 Million rows? No problem.
*   **Scrolling:** 120fps, always.
*   **Loading:** Instant. We map the file directly from disk; we don't load the whole thing into RAM until you look at it.

### 2. Logic, Not Just Math
Mica supports standard formulas (`=SUM(A1:B2)`), but it exposes the **Una Scripting Runtime**.
*   You can write Rust snippets directly in cells.
*   You can pipe data from **Matrix** (File System) directly into a grid.
    *   `=Matrix::list_files("/projects").filter(|f| f.size > 1GB)`
*   The grid updates in real-time as the file system changes.

### 3. The "Cleavage" (Views)
Mica allows you to "cleave" data into different views without duplicating it.
*   **Sheet 1:** Raw Data (The Source).
*   **Sheet 2:** A pivot table of Sheet 1 (The View).
*   **Sheet 3:** A visual graph of Sheet 2 (The Insight).

If you update a cell in Sheet 1, Sheet 3 updates instantly. No "Refresh Data" button required.

---

## 🛠️ Usage

Mica is designed for **Speed** and **Precision**.

```rust
// The Mica Data Model (Concept)
struct Cell {
    raw: String,        // "100" or "=A1+B1"
    computed: Value,    // 100 or 200
    dependencies: Vec<CellRef>, // Who do I listen to?
    dependents: Vec<CellRef>,   // Who listens to me?
}
```

### The Interface
*   **Visual:** A clean, minimal grid. No "Ribbon" toolbar taking up 20% of the screen. Contextual tools appear only when you select data.
*   **Headless:** You can run Mica from the CLI to process data without a UI.
    *   `mica process sales.csv --script "sum(col('Revenue'))"`

---

## 🔗 Integration (The Ecosystem)

Mica is not an island.
*   **Tabula:** You can embed a live Mica sheet inside a Tabula document. It isn't a screenshot; it's fully editable.
*   **Vug:** You can use Mica to sequence audio events (tracker style).
*   **Gneiss:** Mica uses the Gneiss file system traits to ensure data integrity, even if the power fails.

---

## 🤖 Directives for Agents

**ATTENTION:** If you are coding logic for Mica:

### 1. No Floating Point Errors
We do not use `f64` for currency or precise data.
*   **Rule:** Use `Decimal` types.
*   **Why:** `0.1 + 0.2 != 0.3` in standard float math. In Mica, it must equal `0.3`.

### 2. Dependency Graph
Every cell update must propagate through the **Directed Acyclic Graph (DAG)**.
*   Do not manually update dependent cells.
*   Update the *Source*, and let the *Engine* cascade the changes.

---

## 🔮 Roadmap

*   [ ] **Phase 1: The Grid** - Virtualized rendering of massive CSVs.
*   [ ] **Phase 2: The Formula** - Basic math engine (`+`, `-`, `*`, `/`, `SUM`).
*   [ ] **Phase 3: The Script** - Embedded Rust/Lua scripting for complex logic.
*   [ ] **Phase 4: The Graph** - Charts and visualization rendering.
