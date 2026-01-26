# UI Guidelines: Functional Beauty

## 1. The "16ms" Rule (60fps)
Responsiveness is the #1 feature.
* **The Promise:** If a user clicks, the screen MUST update within 16ms. Even if the app is frozen, the window manager must respond (move, minimize, close).
* **Implementation:** The Window Server runs on a dedicated high-priority thread (Real-Time Class). It never waits for an application to finish thinking.

## 2. Information Density (The "Data" Aesthetic)
We reject modern "white space" trends. We prefer **High Signal-to-Noise Ratio**.
* **Tabs:** Like BeOS, windows use distinctive tabs that are easy to grab.
* **Metadata First:** In the file browser, we don't just show icons. We show resolution, frame rate (for videos), and EXIF data (for photos) directly in the list view.
* **Typography:** We use a custom, high-legibility monospace font for system data (like `JetBrains Mono` or `Fira Code`) to emphasize precision.

## 3. The "Workspace" Metaphor
unaOS is a workbench, not a consumption device.
* **Spatial Organization:** Windows remember exactly where you put them. If you leave a text editor in the top-right corner, it stays there after reboot.
* **Virtual Desktops:** Deeply integrated. One workspace for "Kernel Dev," one for "Music," one for "Communication."

## 4. Dark Mode by Default (Ecology)
* **OLED Black:** The default theme uses true black (`#000000`) to turn off pixels on OLED screens (Pixel 10, modern laptops). This saves energy.
* **Accent Colors:** Used strictly to indicate status (Green = Good, Yellow = Busy, Red = Error). No decorative colors.
