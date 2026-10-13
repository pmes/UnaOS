//! A11YFACE (LEDGER SR124): Principia's accessibility rows on the GTK face.
//!
//! R104: the face reads, the shell applies. quartzite does not link the engine, so the rows cross bandy as
//! `SMessage::BrowserA11yPrefs`:
//!
//! - at start the three `system.accessibility.*` rows are read from Principia's store (`PrefStore::load`, the
//!   schema default through `PrefStore::effective` when a row is unset — the `devtools_view::stored_dock` shape);
//! - on change, two lanes re-read the store: a `PrincipiaCommand::PrefChanged` for namespace `system`, key
//!   `accessibility.*`, on this process's bus (the lane `workspace/sidebar.rs` and Vein's provider follow), and a
//!   `gio::FileMonitor` on the store's folder — Principia persists each domain atomically, and the Settings
//!   surface that writes a row usually runs in another process whose bus this one does not share. A re-read that
//!   changed the triple is sent again; an unchanged one is not.
//!
//! The shell's coordinator keeps the last triple and hands it to every tab, whose engine applies it
//! (`aether::dom::a11y::prefs::A11yPrefs::apply`): the `prefers-*` / `forced-colors` media queries flip, the page
//! restyles, and the focus ring takes its contrast width.

use std::cell::RefCell;
use std::rc::Rc;

use bandy::{PrefValue, PrincipiaCommand, SMessage, Synapse};
use gtk4::gio;
use gtk4::gio::prelude::*;

/// Principia's namespace and the three keys (prefs_core `schema.rs`, docs/dev/PREFS-SCHEMA.md).
pub const NS: &str = "system";
pub const KEYS: [&str; 3] = ["accessibility.reduce_motion", "accessibility.contrast", "accessibility.forced_colors"];

/// The three rows as the face sends them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A11yRows {
    pub reduce_motion: bool,
    pub contrast: String,
    pub forced_colors: bool,
}

impl A11yRows {
    /// From a lookup of a `system` key's value (None: unset — the schema defaults false / `no-preference`).
    pub fn from_values(get: impl Fn(&str) -> Option<PrefValue>) -> Self {
        let truthy = |k: &str| match get(k) {
            Some(PrefValue::Bool(b)) => b,
            Some(PrefValue::Str(s)) => matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on"),
            Some(PrefValue::Int(i)) => i != 0,
            _ => false,
        };
        let contrast = match get(KEYS[1]) {
            Some(PrefValue::Str(s)) if !s.trim().is_empty() => s.trim().to_ascii_lowercase(),
            _ => "no-preference".to_string(),
        };
        A11yRows { reduce_motion: truthy(KEYS[0]), contrast, forced_colors: truthy(KEYS[2]) }
    }

    pub fn message(&self) -> SMessage {
        SMessage::BrowserA11yPrefs { reduce_motion: self.reduce_motion, contrast: self.contrast.clone(), forced_colors: self.forced_colors }
    }
}

/// The rows in force now: Principia's store (a store that does not load reads as every row unset).
pub fn read_rows() -> A11yRows {
    let store = principia::prefs::PrefStore::load(principia::default_prefs_path()).ok();
    A11yRows::from_values(|k| {
        let s = store.as_ref()?;
        s.get(NS, k).or_else(|| s.effective(NS, k, |e| std::env::var(e).ok()).map(|(v, _)| v))
    })
}

/// A Principia change to one of the three rows.
pub fn is_a11y_change(msg: &SMessage) -> bool {
    matches!(msg, SMessage::Principia(PrincipiaCommand::PrefChanged { ns, key, .. }) if ns == NS && KEYS.contains(&key.as_str()))
}

thread_local! {
    /// The store folder's monitor, alive for the life of the face.
    static MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
}

/// Sends the rows now and whenever they change (the module doc's two lanes).
pub fn bootstrap_a11y_prefs(synapse: Synapse) {
    let last: Rc<RefCell<Option<A11yRows>>> = Rc::new(RefCell::new(None));
    let send: Rc<dyn Fn()> = {
        let syn = synapse.clone();
        Rc::new(move || {
            let rows = read_rows();
            if last.borrow().as_ref() != Some(&rows) {
                syn.fire(rows.message());
                *last.borrow_mut() = Some(rows);
            }
        })
    };
    send();
    {
        let send = send.clone();
        let mut rx = synapse.subscribe();
        gtk4::glib::spawn_future_local(async move {
            loop {
                match rx.recv().await {
                    Ok(m) if is_a11y_change(&m) => send(),
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }
    let dir = principia::default_prefs_path();
    match gio::File::for_path(&dir).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
        Ok(monitor) => {
            monitor.connect_changed(move |_, _, _, _| send());
            MONITOR.with(|m| *m.borrow_mut() = Some(monitor));
        }
        Err(e) => log::warn!("[aether] accessibility rows: no monitor on {} ({e}); changes arrive on the bus only", dir.display()),
    }
}
