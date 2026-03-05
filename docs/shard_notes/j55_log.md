## 2026-05-15 - [J55 "Kinesis"] UI/UX Flow Sculpting

**Anomaly:** The `quartzite` UI had multiple Can-Am infractions regarding fixed sizing and layout logic. Hardcoded constraints like `min-content-height` and `height-request` hindered smooth interactions. Additionally, counting pixel heights via GTK nodes for the expand/collapse threshold caused performance stuttering during ListView recycling.

**Resolution:**
- We completely decoupled expansion buttons from the chat content and moved them to the `meta_label` Header Box. This fixes DOM interference and keeps semantic alignment pure (left for user, right for AI).
- Expansion thresholds correctly query the string `line_count() > 11` directly from the `DispatchObject` rather than interrogating layout nodes.
- For Block Focus, we attached an `EventControllerKey` bound to Up/Down/PgUp/PgDn to intercept and explicitly invoke `grab_focus()` on the neighboring bubble `Box`es. The scroll view handles viewport shifting naturally.
- All predefined `height_request` limits on Pre-Flight staging input and the main chat input were replaced with `propagate_natural_height(true)` and `max_content_height(600)`.
- `Adjustment` values on `scrolled_window` are now monitored with a debounce to effortlessly dispatch `Event::LoadHistory`.