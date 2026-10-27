//! CHARTER: Aether (CODEX §2 Web) — the developer-tools pane's pure rules and verbs, read from bandy's
//! `BrowserDevtools*` wire.
//!
//! AETHERMACDEVTOOLS (SR312): the pane's model is the shell's `aether::chrome::DevtoolsChrome` (open, dock, share;
//! `split` gives the page and pane rects) and its content the engine's `DevTools::render` lines (R104: the face paints,
//! the model decides). What a face derives from that wire and the verbs it fires back are written ONCE here,
//! platform-neutral (std + bandy + principia), and both faces read it: `platforms/gtk/devtools_view.rs` (which
//! re-exports the names it used to own, so `devtools_view::stored_dock` and friends keep their paths) and
//! `platforms/macos/devtools_view.rs`. The dock side is Principia's `aether.devtools.dock` (BP-handlers-5).

use bandy::{PrefValue, SMessage};

/// The pane's panels, as the engine labels them (`aether::devtools::Panel::label`, in `Panel::ALL`'s order; a host KAT
/// in aether-shell holds them equal); AETHERDEVTOOLS3 (SR128) added Sources.
pub const PANELS: [&str; 7] = ["Elements", "Console", "Sources", "Network", "Storage", "Performance", "Accessibility"];

/// AETHERDEVTOOLS3 (SR128): the (url, line) a pane text line names when it is a V8-shaped frame
/// (`    at fn (url:line:col)`, `    at url:line:col`) — the face's half of a frame click; the engine's own reading is
/// `aether::devtools::sources::frame_target`, the same grammar. The click fires `BrowserDevtoolsGoto`.
pub fn goto_target(line: &str) -> Option<(String, u32)> {
    let t = line.trim();
    let rest = t.strip_prefix("at ")?.trim_end();
    let loc = match (rest.strip_suffix(')'), rest.rfind(" (")) {
        (Some(r), Some(k)) => &r[k + 2..],
        _ => rest,
    };
    let mut it = loc.rsplitn(3, ':');
    let _col: u32 = it.next()?.parse().ok()?;
    let n: u32 = it.next()?.parse().ok()?;
    let file = it.next().filter(|f| !f.is_empty() && !f.contains(char::is_whitespace))?;
    Some((file.to_string(), n))
}

/// Principia's spelling of a dock side (`aether.devtools.dock`): true = under the page.
pub fn dock_from_pref(v: &str) -> bool {
    v.eq_ignore_ascii_case("bottom")
}

pub fn dock_pref(bottom: bool) -> &'static str {
    if bottom { "bottom" } else { "right" }
}

/// The pane's share (percent of the dock axis) for a splitter at `pos` of `total` px, rounded.
pub fn share_from_position(pos: i32, total: i32) -> u32 {
    if total <= 0 {
        return 0;
    }
    let pane = (total - pos.clamp(0, total)) as i64;
    ((pane * 100 + total as i64 / 2) / total as i64) as u32
}

/// The splitter position for a layout: the page rect's extent along the dock axis.
pub fn paned_position(bottom: bool, page: (u32, u32, u32, u32)) -> i32 {
    (if bottom { page.3 } else { page.2 }) as i32
}

/// Whether the splitter at `current` is already where a layout puts it (within 1 % of `total`): a drag's own echo is
/// not snapped back.
pub fn splitter_settled(current: i32, target: i32, total: i32) -> bool {
    (current - target).abs() <= (total / 100).max(1)
}

/// The dock side the person chose last (Principia `aether.devtools.dock`, else its schema default).
pub fn stored_dock() -> bool {
    let Ok(store) = principia::prefs::PrefStore::load(principia::default_prefs_path()) else { return false };
    if let Some(PrefValue::Str(s)) = store.get("aether", "devtools.dock") {
        return dock_from_pref(&s);
    }
    match store.effective("aether", "devtools.dock", |k| std::env::var(k).ok()) {
        Some((PrefValue::Str(s), _)) => dock_from_pref(&s),
        _ => false,
    }
}

/// Store the dock side in Principia (only into a store that loaded: a failed load never writes over the person's other
/// `aether` choices).
pub fn store_dock(bottom: bool) {
    match principia::prefs::PrefStore::load(principia::default_prefs_path()) {
        Ok(mut store) => {
            if let Err(e) = store.set("aether", "devtools.dock", PrefValue::Str(dock_pref(bottom).to_string())) {
                log::warn!("[aether] devtools dock not stored: {e}");
            }
        }
        Err(e) => log::warn!("[aether] devtools dock not stored (prefs did not load): {e}"),
    }
}

/// The toggle every face fires (its close button, AppKit's Cmd+Option+I / F12): F12 alone, which
/// `aether::chrome::is_devtools_toggle` takes.
pub fn toggle_key() -> SMessage {
    SMessage::BrowserDevtoolsKey { key: "F12".into(), ctrl: false, shift: false, alt: false }
}

/// The Console field's line as its verb; a blank line sends nothing.
pub fn eval_msg(line: &str) -> Option<SMessage> {
    if line.trim().is_empty() { None } else { Some(SMessage::BrowserDevtoolsEval(line.to_string())) }
}

/// The JavaScript console key (AppKit's Cmd+Option+J): show the Console panel, and open the pane when it is closed
/// (`open`, the last layout's); an open pane is never closed by it.
pub fn console_verbs(open: bool) -> Vec<SMessage> {
    let mut v = vec![SMessage::BrowserDevtoolsPanel(PANELS[1].to_string())];
    if !open {
        v.push(toggle_key());
    }
    v
}

/// Whether a layout opens the pane (`Some`), or `None` when `m` is not a `BrowserDevtoolsLayout`.
pub fn pane_open(m: &SMessage) -> Option<bool> {
    match m {
        SMessage::BrowserDevtoolsLayout { pane, .. } => Some(pane.is_some()),
        _ => None,
    }
}

/// The line of `text` holding the UTF-16 offset `at` (AppKit's selection location), without its newline; past the end
/// it is the last line.
pub fn line_at(text: &str, at: usize) -> &str {
    let mut units = 0usize;
    let mut byte = text.len();
    for (i, c) in text.char_indices() {
        if units >= at {
            byte = i;
            break;
        }
        units += c.len_utf16();
    }
    let start = text[..byte].rfind('\n').map(|k| k + 1).unwrap_or(0);
    let end = text[byte..].find('\n').map(|k| byte + k).unwrap_or(text.len());
    if start > end { "" } else { &text[start..end] }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The face's half of the split (moved from the GTK face, AETHERDEVTOOLS2 SR112): a layout's page rect places the
    /// splitter, a drag at that place reads back the share the model split by, a drag's echo within 1 % is not
    /// snapped, and the dock side round-trips through Principia's spelling.
    #[test]
    fn devtools_rows_splitter_kat() {
        assert_eq!(paned_position(false, (0, 0, 600, 800)), 600);
        assert_eq!(share_from_position(600, 1000), 40);
        assert_eq!(paned_position(true, (0, 0, 1000, 680)), 680);
        assert_eq!(share_from_position(680, 800), 15);
        assert_eq!(share_from_position(0, 0), 0);
        assert_eq!(share_from_position(-5, 100), 100);
        assert!(splitter_settled(605, 600, 1000));
        assert!(!splitter_settled(620, 600, 1000));
        assert!(dock_from_pref(dock_pref(true)) && !dock_from_pref(dock_pref(false)));
        assert!(!dock_from_pref("left"));
    }

    /// AETHERDEVTOOLS3 (SR128, moved from the GTK face): the Sources tab is the third, and a frame line reads back its
    /// url and line.
    #[test]
    fn devtools_rows_sources_kat() {
        assert_eq!(PANELS.len(), 7);
        assert_eq!(PANELS[2], "Sources");
        assert_eq!(goto_target("    at boom (https://a.test/index.html:3:9)"), Some(("https://a.test/index.html".to_string(), 3)));
        assert_eq!(goto_target("    at https://a.test/app.js:12:1"), Some(("https://a.test/app.js".to_string(), 12)));
        assert_eq!(goto_target("    at new C (https://a.test/c.js:4:20)"), Some(("https://a.test/c.js".to_string(), 4)));
        assert_eq!(goto_target("    at inner (t.js)"), None);
        assert_eq!(goto_target("x Uncaught Error: boom"), None);
    }

    /// AETHERMACDEVTOOLS (SR312): the verbs both faces fire and the click line AppKit reads.
    #[test]
    fn devtools_rows_verbs_kat() {
        let d = |m: SMessage| format!("{m:?}");
        assert_eq!(d(toggle_key()), "BrowserDevtoolsKey { key: \"F12\", ctrl: false, shift: false, alt: false }");
        assert!(eval_msg("  ").is_none());
        assert_eq!(eval_msg("1+1").map(d), Some("BrowserDevtoolsEval(\"1+1\")".to_string()));
        assert_eq!(console_verbs(true).into_iter().map(d).collect::<Vec<_>>(), ["BrowserDevtoolsPanel(\"Console\")"]);
        assert_eq!(console_verbs(false).len(), 2);
        assert_eq!(d(console_verbs(false).remove(1)), d(toggle_key()));
        let lay = |pane| SMessage::BrowserDevtoolsLayout { bottom: false, page: (0, 0, 600, 800), pane };
        assert_eq!(pane_open(&lay(Some((600, 0, 400, 800)))), Some(true));
        assert_eq!(pane_open(&lay(None)), Some(false));
        assert_eq!(pane_open(&SMessage::BrowserTabNew), None);
        let text = "x Uncaught Error: é\n    at boom (https://a.test/i.html:3:9)\nlast";
        assert_eq!(line_at(text, 0), "x Uncaught Error: é");
        // "x Uncaught Error: é\n" is 20 UTF-16 units; offset 25 is inside the frame line.
        assert_eq!(line_at(text, 25), "    at boom (https://a.test/i.html:3:9)");
        assert_eq!(goto_target(line_at(text, 25)), Some(("https://a.test/i.html".to_string(), 3)));
        assert_eq!(line_at(text, 999), "last");
        assert_eq!(line_at("", 3), "");
    }
}
