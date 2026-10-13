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
pub mod tabs;
pub mod zoom;

pub mod host;
