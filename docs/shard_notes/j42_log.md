## 2024-03-03 - [MegaBar Abstracted - GTK Final Boss Defeated]
**Anomaly:** GTK layout in `spline.rs` conflated Pure-GTK fallback UI framing with structural GNOME Adwaita widgets, leading to complex `#cfg` pollution, unmanageable CSS classes within the UI tree, and lack of visual cohesion.
**Resolution:** Abstracted windowing boundaries into `MegaBarWindow` scoped separately across platforms. Implemented split-pane CSD titlebar alignment syncing natively in GTK `SizeGroup`s, passing tabs as references correctly to maintain the double stack layout in GTK and flat layout in Libadwaita.
