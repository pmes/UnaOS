# Shell Philosophy: The "Midden" Integration

## 1. The Command Line is Primary
In unaOS, the GUI is just a visualizer for the shell. Anything you can click, you can type.
* **The Universal Shell:** All system settings are exposed as text commands.
* **Consistency:** We adhere to the POSIX standard where possible, but extend it with structured data (JSON/NuShell style) where Unix falls short.

## 2. Deep Integration of "Midden"
Your "Midden" project (Shell History Organizer) is not an app; it is the system's memory.
* **Contextual Recall:** The OS remembers not just *what* command you typed, but *where* (directory), *when* (time), and *why* (git branch context).
* **Predictive Assisstance:** Because `midden` understands context, the shell can autocomplete entire workflows, not just single words.
    * *User types:* `git p`
    * *Midden suggests:* `git push origin feature/new-kernel --force-with-lease` (because you just rebased).

## 3. The "Gneiss" Abstraction
The shell uses your "Gneiss PAL" to abstract differences between hardware.
* A script written on your Mac Hackintosh will run identically on a Pixel 10 because `gneiss` translates the underlying hardware calls (e.g., `wifi_up`) into the correct driver commands automatically.
