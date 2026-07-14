# UnaIDE

**Context:** `Context::Code` (IDE Mode)
**Architecture:** Platinum (S60)
**Engine:** `libs/elessar`

## 📖 Overview

**UnaIDE** is the primary developer environment of the UnaOS ecosystem. It is **not** a monolithic IDE. It is an instance of the **Elessar Polymorphic Editor** explicitly frozen into a **Code-First Context**.

It does not contain editor logic. It does not contain terminal logic. It acts as the **Host**, dynamically loading the required **Handlers** to create a unified development environment.

## 🏗️ Architecture

Una sits at **Layer 3 (Vessels)** of the Trinity Architecture.

* **The Brain:** It initializes `libs/gneiss_pal` for headless state management.
* **The Window:** It uses `libs/quartzite` to render the GTK4 window frame.
* **The Logic:** It uses `libs/elessar` to manage the Spline (layout) and Context.

## 🧩 Capabilities (Handlers)

Una composes the following **Handlers** into a single workspace:

| Handler | Role | Screen Location |
| --- | --- | --- |
| **`handlers/tabula`** | **The Editor.** Syntax highlighting (TreeSitter), multi-cursor, LSP. | **Top Right** |
| **`handlers/midden`** | **The Terminal.** Shell emulation, process management. | **Bottom Right** |
| **`handlers/vairë`** | **The Version Control.** Git graph, diff view, commit interface. | **Left Panel** |
| **`handlers/matrix`** | **The Files.** Navigate Files. | **Left Panel** |
| **`handlers/aulë`** | **The Builder.** Cargo wrapper, task runner. | **Left Panel** |

## ⚠️ Status

**Pre-Alpha.**

* *Dependency:* Requires `libs/elessar` and `handlers/*` to be present in the workspace.
