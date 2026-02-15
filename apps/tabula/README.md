# 📜 Tabula: The Tablet

> *"Before the structure, there was the word."*

**Tabula** is the lightweight text and code editor of **UnaOS**. It is the "Notepad" evolved. It is designed for speed, instant startup, and raw text manipulation.

It is distinct from **Elessar**. Elessar is an Environment; Tabula is a Tool.

## ⚡ The Reflex (Usage)

Tabula is the default handler for:
*   **Single Code Files:** Opening a `.rs` or `.py` file outside of a project context.
*   **Configuration:** Viewing raw JSON/TOML/YAML files when a GUI is not required.
*   **Scripts:** Quick edits to shell scripts or batch files.
*   **Logs:** Streaming read-only views of system logs (piped from **Midden**).

## 🔮 The Mechanics

### 1. The "Magic" Detection
Tabula does not trust file extensions. It utilizes **Gneiss PAL** to sniff the "Magic Bytes" (file header). 
*   If a file is named `README` (no extension), Tabula detects the text content and highlights it as Markdown.
*   If a file contains a shebang (`#!/bin/una`), Tabula highlights it as scripting.

### 2. The Embed
Tabula is designed to be embedded.
*   **Inside Matrix:** Press `Space` on a text file. Tabula renders the preview.
*   **Inside Principia:** When viewing "Raw Config," Principia spawns a Tabula instance.

## 🛑 The Kill List
Tabula replaces:
*   **Notepad / TextEdit**
*   **Sublime Text** (for quick edits)
*   **less / cat** (Tabula can accept stdin pipes)
