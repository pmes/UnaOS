// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! THE SCHEMA — every preference key UnaOS declares, as ONE table both rings link (PRINCIPIA2, SR32;
//! AUDIT B287: Settings on the kernel and Principia on the host must not drift).
//!
//! Each [`Key`] row names its namespace and dotted key, its [`Kind`] (type plus range / enum / length),
//! its [`Default`] (a value, a derived [`Rule`](crate::rules::Rule), or "the consumer computes it"), who
//! writes it ([`Writer`]) and who reads it. Three things are derived from this table and from nothing
//! else:
//!
//! - **clamping** — [`check`] is the ONE validator both rings run on a write: an out-of-range integer or
//!   float is CLAMPED into its range and answered with `clamped = true` (Flight 19's dark-panel rule,
//!   [`crate::display::clamp_brightness`], is the `system.display.brightness` row of it); a wrong type, a
//!   value outside an enum, an over-long or unprintable string is REFUSED ([`Refusal`]). A key the table
//!   does not declare passes unchanged — namespaces stay open to every app's own preferences;
//! - **defaults** — [`default_of`] / [`effective`] answer the declared default (or evaluate the rule);
//!   the STORE still never holds a default (Principia's rule: a file contains only choices a user made);
//! - **the document** — [`render_markdown`] writes `docs/dev/PREFS-SCHEMA.md`; the gate
//!   (`tests/schema_gate.rs`) fails when the committed document is not the generated one, and runs
//!   `tools/prefs-schema-check.py`, which fails when any key referenced in the tree is absent here.

use alloc::string::String;
use core::fmt::Write;

use crate::rules::{Rule, RuleEnv};
use crate::PrefValue;

/// A key's type and its admissible values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// An integer in `min..=max`; outside it, CLAMPED.
    Int { min: i64, max: i64 },
    /// A finite float in `min..=max`; outside it, CLAMPED; NaN is refused.
    Float { min: f64, max: f64 },
    Bool,
    /// A string of at most `max_len` bytes; `printable` = ASCII 0x20..=0x7e only.
    Str { max_len: usize, printable: bool },
    /// A string that must be one of these spellings.
    Enum(&'static [&'static str]),
}

impl Kind {
    /// The [`PrefValue::type_name`] a value of this kind carries.
    pub const fn type_name(&self) -> &'static str {
        match self {
            Kind::Int { .. } => "int",
            Kind::Float { .. } => "float",
            Kind::Bool => "bool",
            Kind::Str { .. } | Kind::Enum(_) => "string",
        }
    }
}

/// A key's default when it is unset in the store.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Default {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(&'static str),
    /// Derived from other preferences and the environment.
    Rule(Rule),
    /// No preference-level default: the consumer computes it (the text says how).
    Consumer(&'static str),
}

/// Who writes a key. Every key may also be written by [`Writer::Operator`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Writer {
    /// The kernel Settings window (`video/settings.rs`, one key per change).
    Settings,
    /// The kernel dock's pin service pass (`video/dock.rs`).
    Dock,
    /// The kernel hot keys (F1/F2 brightness, F10–F12 volume), persisted by the settings pass.
    Keys,
    /// The kernel `wallpaper <path>|off` verb.
    WallpaperVerb,
    /// The operator: the kernel `pref set` verb, a session program's PREF_SET / host `PrefSet`, a hand
    /// edit of the file.
    Operator,
}

impl Writer {
    pub const fn name(self) -> &'static str {
        match self {
            Writer::Settings => "settings",
            Writer::Dock => "dock",
            Writer::Keys => "keys",
            Writer::WallpaperVerb => "wallpaper-verb",
            Writer::Operator => "operator",
        }
    }
}

/// One declared preference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub ns: &'static str,
    pub key: &'static str,
    pub kind: Kind,
    pub default: Default,
    /// The writers besides the operator (who may write every key).
    pub writers: &'static [Writer],
    /// Who reads it.
    pub reader: &'static str,
    pub doc: &'static str,
}

const OP: &[Writer] = &[];
/// `[A-Za-z0-9_]` env-var / model / region / path names are all printable ASCII.
const NAME: Kind = Kind::Str { max_len: 128, printable: true };

/// The local embedding model R81's rule looks for (`tools/una-models fetch all-MiniLM-L6-v2`; Vein's
/// `gneiss_pal::api::LOCAL_DEFAULT_EMBED_MODEL`).
pub const LOCAL_EMBED_MODEL: &str = "all-MiniLM-L6-v2";
/// The files that make a local model INSTALLED in `${XDG_CACHE_HOME:-$HOME/.cache}/unaos/models/<name>/`
/// (the `FILES` manifest of `tools/una-models`; `gneiss_pal::api::local::MODEL_FILES`).
pub const LOCAL_MODEL_FILES: [&str; 3] = ["model.onnx", "vocab.txt", "config.json"];
/// R81: Vein's default chat model per provider.
pub const CLAUDE_DEFAULT_MODEL: &str = "claude-opus-5-5";
pub const GEMINI_DEFAULT_MODEL: &str = "gemini-3.1-pro-preview";
/// CLAUDECODE (SR38): `default` = pass no `--model`, the Claude Code CLI's own (`gneiss_pal::api::CLAUDECODE_DEFAULT_MODEL`).
pub const CLAUDECODE_DEFAULT_MODEL: &str = "default";
pub const GEMINI_DEFAULT_KEY_ENV: &str = "GEMINI_API_KEY";

/// THE TABLE. Sorted by namespace, then key (the gate asserts it), so the document diffs stably.
pub static SCHEMA: &[Key] = &[
    // ── system — the kernel's namespace (kernel `src/prefs.rs` `mod key`; PREFS B300) ─────────────────
    Key {
        ns: "system", key: "audio.amp_holdoff_ms", kind: Kind::Int { min: 0, max: 600_000 },
        default: Default::Consumer("`hda_amp::AMP_HOLDOFF_MS`, 5000 ms"),
        writers: OP, reader: "kernel HDA amp (`drivers/hda_amp.rs`)",
        doc: "Milliseconds of silence before the speaker amp powers down (PREFSKERNEL: declared from the kernel scan).",
    },
    Key {
        ns: "system", key: "audio.mute", kind: Kind::Bool, default: Default::Bool(false),
        writers: &[Writer::Settings, Writer::Keys], reader: "kernel settings (audio)",
        doc: "Output muted.",
    },
    Key {
        ns: "system", key: "audio.volume", kind: Kind::Int { min: 0, max: 16 }, default: Default::Int(12),
        writers: &[Writer::Settings, Writer::Keys], reader: "kernel settings (audio)",
        doc: "Output level in sixteenths.",
    },
    Key {
        ns: "system", key: "display.brightness",
        kind: Kind::Int { min: crate::display::BRIGHTNESS_MIN, max: crate::display::BRIGHTNESS_MAX },
        default: Default::Int(crate::display::BRIGHTNESS_DEFAULT),
        writers: &[Writer::Settings, Writer::Keys], reader: "kernel settings, backlight",
        doc: "Panel level in sixteenths; never 0 (BRIGHTFLOOR: the backlight's OFF belongs to the idle blank).",
    },
    Key {
        ns: "system", key: "display.font", kind: Kind::Enum(&["sans", "serif", "mono"]), default: Default::Str("sans"),
        writers: OP, reader: "kernel `video::text` (KERNELFONT)",
        doc: "The UI typeface family for captions, menus and running text: DejaVu Sans, Serif or Sans Mono (KERNELFONT B359; the console grid is always mono).",
    },
    Key {
        ns: "system", key: "display.font_size", kind: Kind::Int { min: 9, max: 32 }, default: Default::Int(13),
        writers: OP, reader: "kernel `video::text` (KERNELFONT)",
        doc: "UI text size in CSS px; device px = size x the panel's ppi (EDID) / 96, capped by the 16 px text cell; captions keep the bar-derived size.",
    },
    Key {
        ns: "system", key: "display.idle_min", kind: Kind::Int { min: 0, max: 1440 },
        default: Default::Int(crate::display::IDLE_MIN_DEFAULT),
        writers: &[Writer::Settings], reader: "kernel settings (DIMIDLE)",
        doc: "Minutes before the idle blank; 0 = never.",
    },
    Key {
        ns: "system", key: "display.mode", kind: Kind::Str { max_len: 16, printable: true },
        default: Default::Consumer("the panel's own density scale (UIMETRICS)"),
        writers: &[Writer::Settings], reader: "kernel settings (Display > Resolution), `video::dpi`",
        doc: "The looks-like size `WxH` of the chosen mode: the panel's native mode at a UI scale (`prefs_core::modes`); unset = the panel's default scale.",
    },
    Key {
        ns: "system", key: "display.wallpaper", kind: Kind::Str { max_len: 120, printable: true },
        default: Default::Str(""),
        writers: &[Writer::Settings, Writer::WallpaperVerb], reader: "kernel settings, wallpaper",
        doc: "Wallpaper image path; empty = off.",
    },
    Key {
        ns: "system", key: "dock.autohide", kind: Kind::Bool, default: Default::Bool(false),
        writers: &[Writer::Settings, Writer::Dock], reader: "kernel dock",
        doc: "Hide the dock until the pointer reaches its edge (DOCK2). Edited in Settings > General > Auto-hide dock.",
    },
    Key {
        ns: "system", key: "dock.pins", kind: Kind::Str { max_len: 256, printable: true },
        default: Default::Consumer("every pin the build carries (lumen only on a `lumen` build)"),
        writers: &[Writer::Dock], reader: "kernel dock",
        doc: "Comma-joined pinned app names (console, shell, quarry, activity, settings, editor, lumen); TOML arrays are outside the subset.",
    },
    Key {
        ns: "system", key: "dock.position", kind: Kind::Enum(&["bottom", "left", "right"]), default: Default::Str("bottom"),
        writers: &[Writer::Settings, Writer::Dock], reader: "kernel dock",
        doc: "The panel edge the dock sits on (DOCK2, MACPARITY row 25). Edited in Settings > General > Dock.",
    },
    Key {
        ns: "system", key: "login.items", kind: Kind::Str { max_len: crate::login::VALUE_MAX, printable: true },
        default: Default::Str(""),
        writers: &[Writer::Settings, Writer::Dock], reader: "kernel login items (`video/loginitems.rs`)",
        doc: "Comma-joined program names launched after the desktop is built at login, in order; empty = nothing opens itself (R88, R91). Edited in Settings > Login Items and the dock tile menu's Open at Login.",
    },
    Key {
        ns: "system", key: "pointer.speed", kind: Kind::Int { min: 0, max: 2 }, default: Default::Int(1),
        writers: &[Writer::Settings], reader: "kernel settings (pointer)",
        doc: "0 slow, 1 normal, 2 fast.",
    },
    Key {
        ns: "system", key: "power.lowbat_shutdown_pct", kind: Kind::Int { min: 0, max: 100 },
        default: Default::Consumer("the `UNAOS_LOWBAT_SHUTDOWN` build knob, 0 (off) when unset"),
        writers: OP, reader: "kernel POWERMENU",
        doc: "Battery percent at which the machine shuts down; 0 = off.",
    },
    Key {
        ns: "system", key: "settings.tab", kind: Kind::Int { min: 0, max: 5 }, default: Default::Int(0),
        writers: &[Writer::Settings], reader: "kernel settings",
        doc: "The Settings window's open tab (General, Users, Display, About, Login Items, File Types).",
    },
    // ── vein — the conversation handler's provider slot (VEINPROV B303, EMBED B317, R81) ───────────────
    Key {
        ns: "vein", key: "claude.api_key_env", kind: NAME, default: Default::Str("ANTHROPIC_API_KEY"),
        writers: OP, reader: "gneiss_pal ProviderConfig",
        doc: "NAME of the environment variable holding the Claude key (the key is never a preference).",
    },
    Key {
        ns: "vein", key: "claude.fallbacks", kind: Kind::Bool, default: Default::Bool(true),
        writers: OP, reader: "gneiss_pal ProviderConfig",
        doc: "Let the Claude client fall back to the next model on overload.",
    },
    Key {
        ns: "vein", key: "claudecode.bin", kind: Kind::Str { max_len: 4096, printable: true },
        default: Default::Str("claude"),
        writers: OP, reader: "gneiss_pal ProviderConfig",
        doc: "The Claude Code CLI binary for provider claudecode: a path, or a name looked up on PATH (CLAUDECODE, SR38).",
    },
    Key {
        ns: "vein", key: "embed.dims", kind: Kind::Int { min: 1, max: 65536 },
        default: Default::Consumer("the embedding model's known width"),
        writers: OP, reader: "gneiss_pal EmbedConfig",
        doc: "Embedding vector width.",
    },
    Key {
        ns: "vein", key: "embed.model", kind: NAME,
        default: Default::Consumer("`gemini.embed_model` for gemini, `all-MiniLM-L6-v2` for local"),
        writers: OP, reader: "gneiss_pal EmbedConfig",
        doc: "The embedding model.",
    },
    Key {
        ns: "vein", key: "embed.provider", kind: Kind::Enum(&["gemini", "local", "off"]),
        default: Default::Rule(Rule::Embedder),
        writers: OP, reader: "gneiss_pal EmbedConfig",
        doc: "The embedder, its own setting independent of the chat provider (R81); off = recall disabled, said in-chat.",
    },
    Key {
        ns: "vein", key: "embed.reembed_batch", kind: Kind::Int { min: 1, max: 100_000 }, default: Default::Int(16),
        writers: OP, reader: "vein reembed",
        doc: "Memories per re-embed pass.",
    },
    Key {
        ns: "vein", key: "endpoint", kind: Kind::Str { max_len: 160, printable: true },
        default: Default::Str("https://api.anthropic.com/v1/messages"),
        writers: OP, reader: "vein_ring3 prefs (LUMEN.ELF); kernel tests lumen",
        doc: "Where Vein's ring-3 client POSTs: an https:// URL (the key goes only over a verified TLS connection, VEINTLS) or an http:// relay that holds the key itself (never sent the key).",
    },
    Key {
        ns: "vein", key: "gemini.api_key_env", kind: NAME, default: Default::Str(GEMINI_DEFAULT_KEY_ENV),
        writers: OP, reader: "gneiss_pal ProviderConfig, EmbedConfig",
        doc: "NAME of the environment variable holding the Gemini key.",
    },
    Key {
        ns: "vein", key: "gemini.auth", kind: Kind::Enum(&["gcloud", "api_key", "apikey", "key", "adc", "gcloud_adc"]),
        default: Default::Str("gcloud"),
        writers: OP, reader: "gneiss_pal ProviderConfig, EmbedConfig",
        doc: "Gemini authentication: gcloud ADC or an API key from `gemini.api_key_env`.",
    },
    Key {
        ns: "vein", key: "gemini.embed_model", kind: NAME, default: Default::Str("text-embedding-004"),
        writers: OP, reader: "gneiss_pal ProviderConfig, EmbedConfig",
        doc: "Gemini's embedding model.",
    },
    Key {
        ns: "vein", key: "gemini.embed_region", kind: NAME, default: Default::Str("us-central1"),
        writers: OP, reader: "gneiss_pal ProviderConfig, EmbedConfig",
        doc: "Vertex region for embeddings.",
    },
    Key {
        ns: "vein", key: "gemini.project", kind: NAME,
        default: Default::Consumer("none: gcloud auth refuses without one"),
        writers: OP, reader: "gneiss_pal ProviderConfig, EmbedConfig",
        doc: "Google Cloud project for Vertex.",
    },
    Key {
        ns: "vein", key: "gemini.region", kind: NAME, default: Default::Str("global"),
        writers: OP, reader: "gneiss_pal ProviderConfig",
        doc: "Vertex region for chat.",
    },
    Key {
        ns: "vein", key: "key_file", kind: Kind::Str { max_len: 40, printable: true },
        default: Default::Consumer("unset: no key, the Echo provider answers"),
        writers: OP, reader: "vein_ring3 key (LUMEN.ELF); kernel tests lumen",
        doc: "Absolute path of the API key file on the UnaFS volume (read only when it stats with an inode id; refused on FAT). At most 40 bytes: ring 3's SYS_OPEN name bound.",
    },
    Key {
        ns: "vein", key: "max_tokens", kind: Kind::Int { min: 1, max: u32::MAX as i64 }, default: Default::Int(16000),
        writers: OP, reader: "gneiss_pal ProviderConfig",
        doc: "Output token cap per reply.",
    },
    Key {
        ns: "vein", key: "model", kind: NAME, default: Default::Rule(Rule::ChatModel),
        writers: OP, reader: "gneiss_pal ProviderConfig",
        doc: "The chat model.",
    },
    Key {
        ns: "vein", key: "provider", kind: Kind::Enum(&["claude", "gemini", "claudecode", "echo", "relay"]),
        default: Default::Str("claude"),
        writers: OP, reader: "gneiss_pal ProviderConfig; user-vein (metal: echo | relay)",
        doc: "The chat provider (R81: Claude is the default; Gemini is a preference, never hardwired; claudecode = the installed Claude Code CLI on a subscription, host only, SR38).",
    },
    Key {
        ns: "vein", key: "temperature", kind: Kind::Float { min: 0.0, max: 2.0 },
        default: Default::Consumer("the provider's own"),
        writers: OP, reader: "gneiss_pal ProviderConfig",
        doc: "Sampling temperature.",
    },
];

/// The declared row for `ns`/`key`.
pub fn lookup(ns: &str, key: &str) -> Option<&'static Key> {
    SCHEMA.iter().find(|k| k.ns == ns && k.key == key)
}

/// Every declared row of one namespace, in key order.
pub fn namespace(ns: &str) -> impl Iterator<Item = &'static Key> + '_ {
    SCHEMA.iter().filter(move |k| k.ns == ns)
}

// =====================================================================================================
// CLAMP
// =====================================================================================================

/// What a write stores.
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    /// The value stored (the written one, or its clamp).
    pub value: PrefValue,
    /// `true` when the written value was outside its range and [`Applied::value`] is the clamp.
    pub clamped: bool,
}

/// Why a write was refused by the schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The value's type is not the key's (`want` names the key's type).
    WrongType { want: &'static str },
    /// A string outside the key's enum.
    NotInEnum,
    /// A string longer than the key's `max_len`.
    TooLong,
    /// A non-printable byte in a printable-only string.
    NotPrintable,
    /// A NaN for a ranged float.
    NotANumber,
}

impl Refusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::WrongType { .. } => "wrong type for this key",
            Refusal::NotInEnum => "not one of the key's allowed values",
            Refusal::TooLong => "longer than the key allows",
            Refusal::NotPrintable => "only printable ASCII is allowed",
            Refusal::NotANumber => "NaN is not a value",
        }
    }
}

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Refusal::WrongType { want } => write!(f, "wrong type for this key: expected {}", want),
            other => f.write_str(other.as_str()),
        }
    }
}

/// THE validator both rings run on every write: an undeclared key passes unchanged; a declared one is
/// type-checked, clamped (int/float range) or refused (type, enum, length, printable, NaN). Pure.
/// The kernel's `prefs::set` adopts it at the fold (today it applies only
/// [`crate::display::clamp_brightness`] at the settings call site).
pub fn check(ns: &str, key: &str, v: PrefValue) -> Result<Applied, Refusal> {
    match lookup(ns, key) {
        None => Ok(Applied { value: v, clamped: false }),
        Some(k) => clamp(&k.kind, v),
    }
}

/// [`check`] against a known kind.
pub fn clamp(kind: &Kind, v: PrefValue) -> Result<Applied, Refusal> {
    let want = kind.type_name();
    match (kind, v) {
        (Kind::Int { min, max }, PrefValue::Int(i)) => {
            let c = if i < *min { *min } else if i > *max { *max } else { i };
            Ok(Applied { value: PrefValue::Int(c), clamped: c != i })
        }
        (Kind::Float { min, max }, PrefValue::Float(f)) => {
            if f.is_nan() {
                return Err(Refusal::NotANumber);
            }
            let c = if f < *min { *min } else if f > *max { *max } else { f };
            Ok(Applied { value: PrefValue::Float(c), clamped: c != f })
        }
        (Kind::Bool, v @ PrefValue::Bool(_)) => Ok(Applied { value: v, clamped: false }),
        (Kind::Str { max_len, printable }, PrefValue::Str(s)) => {
            if s.len() > *max_len {
                Err(Refusal::TooLong)
            } else if *printable && !s.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
                Err(Refusal::NotPrintable)
            } else {
                Ok(Applied { value: PrefValue::Str(s), clamped: false })
            }
        }
        (Kind::Enum(allowed), PrefValue::Str(s)) => {
            if allowed.contains(&s.as_str()) {
                Ok(Applied { value: PrefValue::Str(s), clamped: false })
            } else {
                Err(Refusal::NotInEnum)
            }
        }
        _ => Err(Refusal::WrongType { want }),
    }
}

/// PREFSKERNEL (B345): clamp every declared int / float in `tree` into its range, in place — the ONE
/// load-time pass (a hand-edited or pre-schema file holding `display.brightness = 0` loads as 1). A value
/// [`check`] would REFUSE (wrong type, enum miss, …) is left as the operator wrote it: a load never
/// deletes a choice. Returns how many values were clamped.
pub fn clamp_tree(tree: &mut crate::PrefTree) -> usize {
    let mut fix: alloc::vec::Vec<(&'static Key, PrefValue)> = alloc::vec::Vec::new();
    for k in SCHEMA {
        if let Some(v) = tree.get(k.ns, k.key) {
            if let Ok(a) = clamp(&k.kind, v.clone()) {
                if a.clamped {
                    fix.push((k, a.value));
                }
            }
        }
    }
    let n = fix.len();
    for (k, v) in fix {
        let _ = tree.set(k.ns, k.key, v);
    }
    n
}

// =====================================================================================================
// DEFAULTS
// =====================================================================================================

/// Where an effective value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The store holds it.
    Stored,
    /// The schema's static default.
    Default,
    /// A derived default ([`Rule`]).
    Rule(Rule),
}

/// The declared default of `ns`/`key` under `env` (a rule is evaluated). `None` = undeclared, or the
/// consumer computes it.
pub fn default_of(ns: &str, key: &str, env: &dyn RuleEnv) -> Option<(PrefValue, Source)> {
    let k = lookup(ns, key)?;
    let v = match k.default {
        Default::Int(i) => PrefValue::Int(i),
        Default::Float(f) => PrefValue::Float(f),
        Default::Bool(b) => PrefValue::Bool(b),
        Default::Str(s) => PrefValue::Str(String::from(s)),
        Default::Rule(r) => return r.eval(env).map(|v| (v, Source::Rule(r))),
        Default::Consumer(_) => return None,
    };
    Some((v, Source::Default))
}

/// The value in force: the stored one (`env.pref`), else the default.
pub fn effective(ns: &str, key: &str, env: &dyn RuleEnv) -> Option<(PrefValue, Source)> {
    match env.pref(ns, key) {
        Some(v) => Some((v, Source::Stored)),
        None => default_of(ns, key, env),
    }
}

// =====================================================================================================
// THE DOCUMENT
// =====================================================================================================

fn kind_text(k: &Kind, out: &mut String) {
    let _ = match k {
        Kind::Int { min, max } => write!(out, "int `{}..={}`", min, max),
        Kind::Float { min, max } => write!(out, "float `{:?}..={:?}`", min, max),
        Kind::Bool => write!(out, "bool"),
        Kind::Str { max_len, printable } => {
            write!(out, "string ≤{}{}", max_len, if *printable { " printable" } else { "" })
        }
        Kind::Enum(a) => {
            out.push_str("enum `");
            for (i, s) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(" \\| ");
                }
                out.push_str(s);
            }
            out.push('`');
            Ok(())
        }
    };
}

fn default_text(d: &Default, out: &mut String) {
    let _ = match d {
        Default::Int(i) => write!(out, "`{}`", PrefValue::Int(*i)),
        Default::Float(f) => write!(out, "`{}`", PrefValue::Float(*f)),
        Default::Bool(b) => write!(out, "`{}`", PrefValue::Bool(*b)),
        Default::Str(s) => write!(out, "`{}`", PrefValue::Str(String::from(*s))),
        Default::Rule(r) => write!(out, "rule `{}`", r.name()),
        Default::Consumer(t) => write!(out, "consumer: {}", t),
    };
}

/// `docs/dev/PREFS-SCHEMA.md`, generated from [`SCHEMA`] and the rules. Pure; the gate compares it with
/// the committed file.
pub fn render_markdown() -> String {
    let mut o = String::new();
    o.push_str("# PREFS-SCHEMA — every preference key UnaOS declares\n\n");
    o.push_str("<!-- GENERATED by unaos/libs/sys/prefs_core (schema::render_markdown). Do not edit by hand:\n");
    o.push_str("     PREFS_SCHEMA_BLESS=1 cargo test -p prefs_core --test schema_gate regenerates it. -->\n\n");
    o.push_str("The table lives in `unaos/libs/sys/prefs_core/src/schema.rs` (`SCHEMA`), linked by the kernel and by\n");
    o.push_str("Principia. A write to a declared key goes through `prefs_core::schema::check`: an int or float outside\n");
    o.push_str("its range is CLAMPED and answered `clamped=true`; a wrong type, a string outside its enum, an over-long\n");
    o.push_str("or unprintable string is REFUSED. Undeclared keys pass unchanged (every app keeps its own namespace).\n");
    o.push_str("Defaults are answered by the schema; the store never holds one. Every key may also be written by the\n");
    o.push_str("operator (`pref set`, a session PREF_SET / host `PrefSet`, a hand edit).\n\n");
    let _ = write!(o, "Rows: {}.\n\n", SCHEMA.len());
    o.push_str("| key | type | default | writers | reader | meaning |\n");
    o.push_str("| :-- | :-- | :-- | :-- | :-- | :-- |\n");
    for k in SCHEMA {
        let _ = write!(o, "| `{}.{}` | ", k.ns, k.key);
        kind_text(&k.kind, &mut o);
        o.push_str(" | ");
        default_text(&k.default, &mut o);
        o.push_str(" | ");
        if k.writers.is_empty() {
            o.push_str("operator");
        } else {
            for (i, w) in k.writers.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
                o.push_str(w.name());
            }
        }
        let _ = write!(o, " | {} | {} |\n", k.reader, k.doc);
    }
    o.push_str("\n## Rules (derived defaults)\n\n");
    for r in Rule::ALL {
        let _ = write!(o, "- `{}` — {}\n", r.name(), r.describe());
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::tests::Env;

    #[test]
    fn the_table_is_sorted_unique_and_well_formed() {
        for w in SCHEMA.windows(2) {
            assert!((w[0].ns, w[0].key) < (w[1].ns, w[1].key), "{}.{} !< {}.{}", w[0].ns, w[0].key, w[1].ns, w[1].key);
        }
        for k in SCHEMA {
            assert!(crate::validate_ns(k.ns).is_ok() && crate::validate_key(k.key).is_ok(), "{}.{}", k.ns, k.key);
            // A key is a leaf: no declared key is a dotted prefix of another in its namespace.
            assert!(!namespace(k.ns).any(|o| crate::is_prefix_path(o.key, k.key)), "{}.{} collides", k.ns, k.key);
            // Every static default is itself admissible and unclamped.
            if let Some((v, Source::Default)) = default_of(k.ns, k.key, &Env::default()) {
                assert_eq!(clamp(&k.kind, v.clone()), Ok(Applied { value: v, clamped: false }), "{}.{}", k.ns, k.key);
            }
        }
    }

    #[test]
    fn brightness_clamp_is_the_flight19_rule() {
        for x in -40..60 {
            let a = check("system", "display.brightness", PrefValue::Int(x)).unwrap();
            assert_eq!(a.value, PrefValue::Int(crate::display::clamp_brightness(x)), "{x}");
            assert_eq!(a.clamped, crate::display::clamp_brightness(x) != x);
        }
    }

    #[test]
    fn out_of_range_clamps_and_says_so() {
        assert_eq!(check("system", "audio.volume", PrefValue::Int(99)), Ok(Applied { value: PrefValue::Int(16), clamped: true }));
        assert_eq!(check("system", "audio.volume", PrefValue::Int(-1)), Ok(Applied { value: PrefValue::Int(0), clamped: true }));
        assert_eq!(check("system", "audio.volume", PrefValue::Int(7)), Ok(Applied { value: PrefValue::Int(7), clamped: false }));
        assert_eq!(check("vein", "temperature", PrefValue::Float(3.5)), Ok(Applied { value: PrefValue::Float(2.0), clamped: true }));
        assert_eq!(check("vein", "temperature", PrefValue::Float(-0.5)), Ok(Applied { value: PrefValue::Float(0.0), clamped: true }));
        assert_eq!(check("vein", "temperature", PrefValue::Float(f64::NAN)), Err(Refusal::NotANumber));
        assert_eq!(check("system", "power.lowbat_shutdown_pct", PrefValue::Int(250)).unwrap().value, PrefValue::Int(100));
    }

    #[test]
    fn refusals() {
        assert_eq!(check("system", "audio.volume", PrefValue::Str("loud".into())), Err(Refusal::WrongType { want: "int" }));
        assert_eq!(check("system", "audio.mute", PrefValue::Int(1)), Err(Refusal::WrongType { want: "bool" }));
        assert_eq!(check("vein", "provider", PrefValue::Str("hal9000".into())), Err(Refusal::NotInEnum));
        assert!(check("vein", "provider", PrefValue::Str("gemini".into())).is_ok());
        let long = "x".repeat(121);
        assert_eq!(check("system", "display.wallpaper", PrefValue::Str(long)), Err(Refusal::TooLong));
        assert_eq!(check("system", "display.wallpaper", PrefValue::Str("A\tB".into())), Err(Refusal::NotPrintable));
    }

    /// PREFSKERNEL (B345): the load pass clamps declared ranges once, leaves refusals and undeclared keys.
    #[test]
    fn clamp_tree_clamps_ranges_once() {
        let mut t = crate::PrefTree::parse(
            "[system]\ndisplay.brightness = 0\naudio.volume = 40\naudio.mute = 1\npointer.speed = 1\n[aether]\nwidth = -9\n[vein]\ntemperature = 9.0\n",
        )
        .unwrap();
        assert_eq!(clamp_tree(&mut t), 3);
        assert_eq!(t.get("system", "display.brightness"), Some(&PrefValue::Int(1)));
        assert_eq!(t.get("system", "audio.volume"), Some(&PrefValue::Int(16)));
        assert_eq!(t.get("vein", "temperature"), Some(&PrefValue::Float(2.0)));
        assert_eq!(t.get("system", "audio.mute"), Some(&PrefValue::Int(1)), "a wrong type is not deleted");
        assert_eq!(t.get("aether", "width"), Some(&PrefValue::Int(-9)));
        assert_eq!(clamp_tree(&mut t), 0, "once");
    }

    #[test]
    fn undeclared_keys_pass_unchanged() {
        for v in [PrefValue::Int(-9_999), PrefValue::Str("\u{7}".into()), PrefValue::Float(f64::NAN)] {
            let a = check("aether", "window.width", v.clone()).unwrap();
            assert!(!a.clamped);
            assert_eq!(a.value.to_literal(), v.to_literal());
        }
    }
}
