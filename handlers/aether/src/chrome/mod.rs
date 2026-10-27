//! AETHERCHROME (SR84) — the browser chrome's MODEL: tabs, the address bar's URL-or-search decision,
//! the per-tab session history, bookmarks, find-in-page state, the download list and the zoom ladder.
//!
//! R104: the host faces (GTK today, AppKit / Qt / the UnaOS compositor later) only PAINT the chrome;
//! everything a face would otherwise re-derive lives here, once. The model modules are std-free — they
//! use `core` and `alloc` only, never `std` — so the same model links into a ring-3 browser on the image.
//! The one exception is [`host`], the std adapter (files under the data directory, the Downloads folder,
//! the HEAD probe and the streaming download over gneiss_pal's HTTP client), kept apart so the model
//! stays portable.
//!
//! The wire between the model and a face is bandy's `SMessage::Browser*` vocabulary (signals.rs, the
//! AETHERCHROME block).

pub mod bookmarks;
pub mod downloads;
pub mod find;
pub mod history;
pub mod omnibox;
pub mod scale;
// AETHERSIDETABS (SR140, R118): the vertical side strip's layout pref, geometry and tab search (std-free).
pub mod sidetabs;
pub mod tabs;
pub mod zoom;

pub mod host;

// AETHERDEVTOOLS (SR112): the developer-tools pane. It READS the engine (std, thread-local page state), so
// like `host` it is not part of the std-free model: it is the crate's own `devtools` module (lib.rs), and
// this re-export keeps the `chrome::devtools` path the host frame call and the faces name.
pub use crate::devtools;

// ---- AETHERSIDETABS2 (SR143): the strip on the wire ---------------------------------------------------------------
// The model above is std-free; these two are the std seam between it and bandy (the shell publishes with
// `tabs_message` and routes the face's drag and group verbs through `apply_tab_verb`), here so the round trip is
// testable without a shell.

/// The strip as bandy's `BrowserTabs`: every tab's row (label, URL, group, letter tile) and every group.
pub fn tabs_message(strip: &tabs::TabStrip) -> bandy::SMessage {
    let tabs = strip
        .tabs()
        .iter()
        .map(|t| bandy::BrowserTabRow {
            id: t.id,
            label: t.label().to_string(),
            url: t.url.clone(),
            loading: t.loading,
            group: t.group,
            initial: t.initial().to_string(),
        })
        .collect();
    let groups = strip
        .groups()
        .iter()
        .map(|g| bandy::BrowserTabGroupRow {
            id: g.id,
            name: g.name.clone(),
            color: g.color.name().to_string(),
            rgb: g.color.rgb(),
            collapsed: g.collapsed,
        })
        .collect();
    bandy::SMessage::BrowserTabs { active: strip.active(), tabs, groups }
}

/// Apply a drag or group verb from the face to the strip. `None`: `m` is not one of them (the caller routes it);
/// `Some(changed)`: it was, and whether the strip moved (the caller republishes, and re-activates when the active
/// tab changed — collapsing the active tab's group moves it).
pub fn apply_tab_verb(strip: &mut tabs::TabStrip, m: &bandy::SMessage) -> Option<bool> {
    use bandy::SMessage as M;
    let color = |c: &Option<String>| c.as_deref().and_then(tabs::GroupColor::parse);
    Some(match m {
        M::BrowserTabMove { id, to } => strip.move_to(*id, *to as usize),
        M::BrowserTabGroupNew { tabs: ids, name, color: c } => {
            let col = color(c).unwrap_or_else(|| strip.next_color());
            strip.group_new(name, col, ids).is_some()
        }
        M::BrowserTabGroupAdd { id, group } => strip.group_add(*id, *group),
        M::BrowserTabUngroup(id) => strip.ungroup(*id),
        M::BrowserTabGroupToggle(g) => strip.group_toggle(*g).is_some(),
        M::BrowserTabGroupRename { group, name } => strip.group_rename(*group, name),
        M::BrowserTabGroupRecolor { group, color: c } => {
            let Some(cur) = strip.group(*group).map(|g| g.color) else { return Some(false) };
            strip.group_recolor(*group, color(c).unwrap_or(cur.next()))
        }
        M::BrowserTabGroupDissolve(g) => strip.group_dissolve(*g),
        _ => return None,
    })
}

/// Where the developer-tools pane sits in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DevtoolsDock {
    /// Beside the page, on the right (the pane takes a share of the width).
    #[default]
    Right,
    /// Under the page (the pane takes a share of the height).
    Bottom,
}

/// The pane's chrome state for one window: open or not, the dock side, and the pane's share of the dock
/// axis. std-free, like the rest of the model; the pane's content is [`devtools::DevTools`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DevtoolsChrome {
    pub open: bool,
    pub dock: DevtoolsDock,
    /// The pane's share of the window along the dock axis, in percent (clamped 15..=85).
    pub share_pct: u32,
}

impl Default for DevtoolsChrome {
    fn default() -> Self {
        Self { open: false, dock: DevtoolsDock::Right, share_pct: 40 }
    }
}

/// The pane's smallest and largest share of the dock axis, in percent.
pub const DEVTOOLS_SHARE_MIN: u32 = 15;
pub const DEVTOOLS_SHARE_MAX: u32 = 85;

/// Whether a key press is the developer-tools toggle: F12 alone, or Ctrl+Shift+I (`key` is the key's
/// name as the face reports it, `I` or `i` for the letter).
pub fn is_devtools_toggle(key: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
    match key {
        "F12" => !ctrl && !shift && !alt,
        "i" | "I" => ctrl && shift && !alt,
        _ => false,
    }
}

impl DevtoolsChrome {
    /// Opens or closes the pane; answers whether it is now open.
    pub fn toggle(&mut self) -> bool {
        self.open = !self.open;
        self.open
    }

    /// Handles a key press: the toggle shortcut flips the pane and answers true (the face swallows the
    /// key); any other key answers false and goes to the page.
    pub fn on_key(&mut self, key: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        if is_devtools_toggle(key, ctrl, shift, alt) {
            self.toggle();
            true
        } else {
            false
        }
    }

    /// Moves the pane to the other side.
    pub fn flip_dock(&mut self) -> DevtoolsDock {
        self.dock = match self.dock {
            DevtoolsDock::Right => DevtoolsDock::Bottom,
            DevtoolsDock::Bottom => DevtoolsDock::Right,
        };
        self.dock
    }

    /// Sets the pane's share (the splitter drag), clamped.
    pub fn set_share(&mut self, pct: u32) {
        self.share_pct = pct.clamp(DEVTOOLS_SHARE_MIN, DEVTOOLS_SHARE_MAX);
    }

    /// Splits a `w` x `h` content area into (page, pane) rects `(x, y, w, h)`; the pane is None when
    /// closed (the page keeps the whole area). The page rect is what the engine's device size follows.
    pub fn split(&self, w: u32, h: u32) -> ((u32, u32, u32, u32), Option<(u32, u32, u32, u32)>) {
        if !self.open {
            return ((0, 0, w, h), None);
        }
        let share = self.share_pct.clamp(DEVTOOLS_SHARE_MIN, DEVTOOLS_SHARE_MAX);
        match self.dock {
            DevtoolsDock::Right => {
                let pane = (w as u64 * share as u64 / 100) as u32;
                let page = w - pane;
                ((0, 0, page, h), Some((page, 0, pane, h)))
            }
            DevtoolsDock::Bottom => {
                let pane = (h as u64 * share as u64 / 100) as u32;
                let page = h - pane;
                ((0, 0, w, page), Some((0, page, w, pane)))
            }
        }
    }
}
