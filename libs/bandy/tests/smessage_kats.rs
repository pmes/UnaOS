// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SMessage wire-shape KATs (Known-Answer Tests).
//
// `SMessage` is the single vocabulary every UnaOS handler and vessel exchanges,
// and ROADMAP §3b (bandy-on-metal) will carry it over a syscall transport.
// These tests FREEZE the serde_json wire shape: one golden JSON string per
// serializable variant. A failing KAT means the wire shape drifted — that is a
// protocol break, not a test to "fix up". If a change is deliberate, update the
// golden consciously and record the break in the arc's landing report.
//
// Equality strategy (tests-only by design): `SMessage` does not derive
// `PartialEq`, and this arc adds no production code. Round-trip fidelity is
// asserted two ways instead:
//   1. re-serialize identity — deserialize(golden) then serialize must
//      reproduce the golden byte-for-byte;
//   2. Debug-repr identity — derived `Debug` prints every field of every
//      payload type, so equal Debug strings are a faithful structural-equality
//      proxy for these data-only types.
//
// The one exception is `ContextTelemetry` / `WeightedSkeleton`: its `content`
// field is `#[serde(skip)]` BY DESIGN (in-process `Arc` only; see
// `bandy::ontology::WeightedSkeleton` and the bandy README). That variant is
// covered by dedicated lossy-wire proofs at the bottom of this file, not by a
// full-fidelity KAT.

use std::path::PathBuf;
use std::sync::Arc;

use bandy::ontology::WeightedSkeleton;
use bandy::signals::{
    A11yVerb, FacetCommand, FacetEdit, FacetFormat, FacetImageInfo, FacetView, FacetZoom, LogEvent, MatrixEvent, PrefValue,
    PrincipiaCommand, SMessage, TabulaCommand,
};
use bandy::state::{
    BrowseEntry, BrowseKind, BrowseListing, DispatchRecord, FsOutcome, FsVerb, LogLine, LogSource,
};
use bandy::Origin;

/// One golden KAT per serializable variant:
///   construct -> serialize == golden (the freeze)
///   golden -> deserialize -> Debug repr == original's (round-trip fidelity)
///   golden -> deserialize -> re-serialize == golden (wire idempotence)
macro_rules! kat {
    ($name:ident, $msg:expr, $golden:expr) => {
        #[test]
        fn $name() {
            let msg: SMessage = $msg;
            let json = serde_json::to_string(&msg).expect("serialize must succeed");
            assert_eq!(
                json, $golden,
                "WIRE SHAPE DRIFT in {}: serialized form no longer matches the frozen golden",
                stringify!($name)
            );
            let back: SMessage =
                serde_json::from_str($golden).expect("golden must deserialize");
            assert_eq!(
                format!("{back:?}"),
                format!("{msg:?}"),
                "ROUND-TRIP LOSS in {}: deserialized value differs from the original",
                stringify!($name)
            );
            let rejson = serde_json::to_string(&back).expect("re-serialize must succeed");
            assert_eq!(
                rejson, $golden,
                "WIRE IDEMPOTENCE BREAK in {}: re-serialized form differs from the golden",
                stringify!($name)
            );
        }
    };
}

// --- SYSTEM HEARTBEAT ---

kat!(kat_state_invalidated, SMessage::StateInvalidated, r#""StateInvalidated""#);
kat!(kat_ping, SMessage::Ping, r#""Ping""#);
kat!(
    kat_kill,
    SMessage::Kill("watchdog: vug unresponsive".to_string()),
    r#"{"Kill":"watchdog: vug unresponsive"}"#
);
kat!(
    kat_log,
    SMessage::Log {
        level: "INFO".to_string(),
        source: "vein".to_string(),
        content: "ignition sequence complete".to_string(),
    },
    r#"{"Log":{"level":"INFO","source":"vein","content":"ignition sequence complete"}}"#
);
kat!(
    kat_core_pulse,
    SMessage::CorePulse { loads: vec![0.5, 0.25, 1.0, 0.0] },
    r#"{"CorePulse":{"loads":[0.5,0.25,1.0,0.0]}}"#
);

// --- EUCLASE (The Visual Cortex) ---

kat!(
    kat_euclase_resize,
    SMessage::EuclaseResize(1920, 1080),
    r#"{"EuclaseResize":[1920,1080]}"#
);
kat!(kat_vug_pulse, SMessage::VugPulse, r#""VugPulse""#);

// --- RESONANCE (The Voice) ---

kat!(
    kat_audio_chunk,
    SMessage::AudioChunk {
        source_id: "resonance".to_string(),
        samples: vec![0.5, -0.5],
        sample_rate: 48000,
    },
    r#"{"AudioChunk":{"source_id":"resonance","samples":[0.5,-0.5],"sample_rate":48000}}"#
);
kat!(
    kat_spectrum,
    SMessage::Spectrum { magnitude: vec![0.0, 0.25] },
    r#"{"Spectrum":{"magnitude":[0.0,0.25]}}"#
);

// --- VEIN / LUMEN (The Mind) ---

kat!(
    kat_user_prompt,
    SMessage::UserPrompt("hello una".to_string()),
    r#"{"UserPrompt":"hello una"}"#
);
kat!(kat_ai_token, SMessage::AiToken("tok".to_string()), r#"{"AiToken":"tok"}"#);
kat!(
    kat_analyze_context,
    SMessage::AnalyzeContext {
        id: "ctx-1".to_string(),
        content: "fn main() {}".to_string(),
    },
    r#"{"AnalyzeContext":{"id":"ctx-1","content":"fn main() {}"}}"#
);
kat!(
    kat_network_log,
    SMessage::NetworkLog("GET /v1/models".to_string()),
    r#"{"NetworkLog":"GET /v1/models"}"#
);
kat!(
    kat_network_state,
    SMessage::NetworkState("online".to_string()),
    r#"{"NetworkState":"online"}"#
);
kat!(
    kat_get_diff,
    SMessage::GetDiff {
        commit_a: "7c75ead".to_string(),
        commit_b: "HEAD".to_string(),
    },
    r#"{"GetDiff":{"commit_a":"7c75ead","commit_b":"HEAD"}}"#
);
kat!(
    kat_diff_payload,
    SMessage::DiffPayload { diff: "+one line".to_string() },
    r#"{"DiffPayload":{"diff":"+one line"}}"#
);

// `ContextTelemetry` is deliberately NOT frozen with a full-fidelity KAT —
// see the "NON-WIRE EXCEPTION" tests at the bottom of this file.

// --- UNAFS / MATRIX (The Memory) ---

kat!(
    kat_file_event,
    SMessage::FileEvent {
        path: "/una/src/lib.rs".to_string(),
        event: "modified".to_string(),
    },
    r#"{"FileEvent":{"path":"/una/src/lib.rs","event":"modified"}}"#
);

// --- AMBER BYTES (The Storage Rune) ---

kat!(
    kat_storage_query,
    SMessage::StorageQuery { receipt_id: 7, embedding: vec![0.5, 0.25], embed_model: String::new() },
    r#"{"StorageQuery":{"receipt_id":7,"embedding":[0.5,0.25]}}"#
);
kat!(
    kat_storage_query_result,
    SMessage::StorageQueryResult {
        receipt_id: 7,
        memories: vec!["m1".to_string()],
        directives: vec![],
        engrams: vec!["e1".to_string()],
        chrono: vec![],
    },
    r#"{"StorageQueryResult":{"receipt_id":7,"memories":["m1"],"directives":[],"engrams":["e1"],"chrono":[]}}"#
);
// EMBED (B317): the model tag rides beside the vector; empty is omitted (older senders unchanged).
kat!(
    kat_storage_query_tagged,
    SMessage::StorageQuery { receipt_id: 7, embedding: vec![0.5], embed_model: "local/all-MiniLM-L6-v2".to_string() },
    r#"{"StorageQuery":{"receipt_id":7,"embedding":[0.5],"embed_model":"local/all-MiniLM-L6-v2"}}"#
);
kat!(
    kat_reembed,
    SMessage::ReEmbed { receipt_id: 3, embed_model: "gemini/text-embedding-004".to_string(), limit: 32 },
    r#"{"ReEmbed":{"receipt_id":3,"embed_model":"gemini/text-embedding-004","limit":32}}"#
);
kat!(
    kat_reembed_batch,
    SMessage::ReEmbedBatch { receipt_id: 3, items: vec![(5, "hi".to_string())], stale_total: 9 },
    r#"{"ReEmbedBatch":{"receipt_id":3,"items":[[5,"hi"]],"stale_total":9}}"#
);
kat!(
    kat_reembed_write,
    SMessage::ReEmbedWrite { receipt_id: 3, embed_model: "off/none".to_string(), vectors: vec![(5, vec![1.0])] },
    r#"{"ReEmbedWrite":{"receipt_id":3,"embed_model":"off/none","vectors":[[5,[1.0]]]}}"#
);
kat!(
    kat_reembed_done,
    SMessage::ReEmbedDone { receipt_id: 3, written: 1, remaining: 8, error: None },
    r#"{"ReEmbedDone":{"receipt_id":3,"written":1,"remaining":8,"error":null}}"#
);
kat!(
    kat_storage_save,
    SMessage::StorageSave {
        receipt_id: 8,
        sender: "vein".to_string(),
        content: "engram body".to_string(),
        timestamp: "2026-07-13T00:00:00Z".to_string(),
        embedding: vec![0.0],
        memory_type: "engram".to_string(),
        embed_model: String::new(),
    },
    r#"{"StorageSave":{"receipt_id":8,"sender":"vein","content":"engram body","timestamp":"2026-07-13T00:00:00Z","embedding":[0.0],"memory_type":"engram"}}"#
);
kat!(
    kat_storage_save_result_ok,
    SMessage::StorageSaveResult { receipt_id: 8, success: true, error: None },
    r#"{"StorageSaveResult":{"receipt_id":8,"success":true,"error":null}}"#
);
kat!(
    kat_storage_save_result_err,
    SMessage::StorageSaveResult {
        receipt_id: 8,
        success: false,
        error: Some("disk full".to_string()),
    },
    r#"{"StorageSaveResult":{"receipt_id":8,"success":false,"error":"disk full"}}"#
);
kat!(
    kat_storage_load_paged,
    SMessage::StorageLoadPaged { receipt_id: 9, offset: 0, limit: 32 },
    r#"{"StorageLoadPaged":{"receipt_id":9,"offset":0,"limit":32}}"#
);
kat!(
    kat_storage_load_paged_result,
    SMessage::StorageLoadPagedResult {
        receipt_id: 9,
        records: vec![DispatchRecord {
            id: "d-1".to_string(),
            origin: Origin::LocalUser("peter".to_string()),
            display_name: Some("Peter".to_string()),
            subject: "greeting".to_string(),
            timestamp: "2026-07-13T00:00:00Z".to_string(),
            content: "hello".to_string(),
            is_chat: true,
        }],
    },
    r#"{"StorageLoadPagedResult":{"receipt_id":9,"records":[{"id":"d-1","origin":{"LocalUser":"peter"},"display_name":"Peter","subject":"greeting","timestamp":"2026-07-13T00:00:00Z","content":"hello","is_chat":true}]}}"#
);

// --- AETHER (The Browser) ---

kat!(
    kat_open_document,
    SMessage::OpenDocument { url: "https://una.os/".to_string() },
    r#"{"OpenDocument":{"url":"https://una.os/"}}"#
);
kat!(
    kat_surface_blit,
    SMessage::SurfaceBlit {
        url: "https://una.os/".to_string(),
        width: 2,
        height: 1,
        pixels: vec![1, 2, 3, 4, 5, 6, 7, 8],
    },
    r#"{"SurfaceBlit":{"url":"https://una.os/","width":2,"height":1,"pixels":[1,2,3,4,5,6,7,8]}}"#
);
kat!(
    kat_play_media,
    SMessage::PlayMedia {
        url: "https://una.os/a.mp3".to_string(),
        title: "A".to_string(),
        mime: "audio/mpeg".to_string(),
    },
    r#"{"PlayMedia":{"url":"https://una.os/a.mp3","title":"A","mime":"audio/mpeg"}}"#
);
kat!(kat_browser_nav_back, SMessage::BrowserNavBack, r#""BrowserNavBack""#);
kat!(kat_browser_nav_forward, SMessage::BrowserNavForward, r#""BrowserNavForward""#);
kat!(kat_browser_nav_reload, SMessage::BrowserNavReload, r#""BrowserNavReload""#);
kat!(
    kat_browser_scroll,
    SMessage::BrowserScroll(0.0, 120.5),
    r#"{"BrowserScroll":[0.0,120.5]}"#
);
kat!(
    kat_browser_click,
    SMessage::BrowserClick(12.5, 40.0),
    r#"{"BrowserClick":[12.5,40.0]}"#
);
kat!(
    kat_browser_resize,
    SMessage::BrowserResize(800, 600),
    r#"{"BrowserResize":[800,600]}"#
);
kat!(
    kat_browser_key,
    SMessage::BrowserKey("Enter".to_string()),
    r#"{"BrowserKey":"Enter"}"#
);
kat!(
    kat_browser_text,
    SMessage::BrowserText("una".to_string()),
    r#"{"BrowserText":"una"}"#
);
kat!(
    kat_browser_paste,
    SMessage::BrowserPaste("una\nos".to_string()),
    r#"{"BrowserPaste":"una\nos"}"#
);
kat!(
    kat_browser_url_changed,
    SMessage::BrowserUrlChanged("https://una.os/next".to_string()),
    r#"{"BrowserUrlChanged":"https://una.os/next"}"#
);
kat!(
    kat_browser_title_changed,
    SMessage::BrowserTitleChanged("UnaOS".to_string()),
    r#"{"BrowserTitleChanged":"UnaOS"}"#
);
kat!(
    kat_browser_favicon_changed,
    SMessage::BrowserFaviconChanged {
        width: 1,
        height: 1,
        rgba: vec![255, 0, 0, 255],
    },
    r#"{"BrowserFaviconChanged":{"width":1,"height":1,"rgba":[255,0,0,255]}}"#
);
kat!(kat_browser_copy, SMessage::BrowserCopy, r#""BrowserCopy""#);
kat!(kat_browser_select_all, SMessage::BrowserSelectAll, r#""BrowserSelectAll""#);
kat!(
    kat_browser_clipboard_set,
    SMessage::BrowserClipboardSet("line one\nline two".to_string()),
    r#"{"BrowserClipboardSet":"line one\nline two"}"#
);

// --- AETHERCHROME (SR84): the browser chrome ---

kat!(
    kat_browser_go,
    SMessage::BrowserGo { input: "una os".to_string(), search: "https://html.duckduckgo.com/html/?q=%s".to_string() },
    r#"{"BrowserGo":{"input":"una os","search":"https://html.duckduckgo.com/html/?q=%s"}}"#
);
kat!(kat_browser_nav_stop, SMessage::BrowserNavStop, r#""BrowserNavStop""#);
kat!(kat_browser_tab_new, SMessage::BrowserTabNew, r#""BrowserTabNew""#);
kat!(kat_browser_tab_close_id, SMessage::BrowserTabClose(Some(3)), r#"{"BrowserTabClose":3}"#);
kat!(kat_browser_tab_close_active, SMessage::BrowserTabClose(None), r#"{"BrowserTabClose":null}"#);
kat!(kat_browser_tab_select, SMessage::BrowserTabSelect(2), r#"{"BrowserTabSelect":2}"#);
kat!(kat_browser_tab_step, SMessage::BrowserTabStep(-1), r#"{"BrowserTabStep":-1}"#);
kat!(kat_browser_zoom, SMessage::BrowserZoom(1), r#"{"BrowserZoom":1}"#);
kat!(
    kat_browser_find,
    SMessage::BrowserFind { query: "una".to_string(), forward: false },
    r#"{"BrowserFind":{"query":"una","forward":false}}"#
);
kat!(kat_browser_find_close, SMessage::BrowserFindClose, r#""BrowserFindClose""#);
kat!(kat_browser_bookmark_toggle, SMessage::BrowserBookmarkToggle, r#""BrowserBookmarkToggle""#);
kat!(kat_browser_page_info_ask, SMessage::BrowserPageInfoAsk, r#""BrowserPageInfoAsk""#);
kat!(
    kat_browser_tabs,
    SMessage::BrowserTabs { active: 2, tabs: vec![(1, "UnaOS".to_string(), false), (2, "New Tab".to_string(), true)] },
    r#"{"BrowserTabs":{"active":2,"tabs":[[1,"UnaOS",false],[2,"New Tab",true]]}}"#
);
kat!(
    kat_browser_nav_state,
    SMessage::BrowserNavState { can_back: true, can_forward: false, loading: true, zoom_pct: 110 },
    r#"{"BrowserNavState":{"can_back":true,"can_forward":false,"loading":true,"zoom_pct":110}}"#
);
kat!(
    kat_browser_find_result,
    SMessage::BrowserFindResult { query: "una".to_string(), matches: 4, current: 1 },
    r#"{"BrowserFindResult":{"query":"una","matches":4,"current":1}}"#
);
kat!(kat_browser_status, SMessage::BrowserStatus("https://una.os/a".to_string()), r#"{"BrowserStatus":"https://una.os/a"}"#);
kat!(
    kat_browser_bookmarks,
    SMessage::BrowserBookmarks { items: vec![("UnaOS".to_string(), "https://una.os/".to_string())], current: true },
    r#"{"BrowserBookmarks":{"items":[["UnaOS","https://una.os/"]],"current":true}}"#
);
kat!(
    kat_browser_page_info,
    SMessage::BrowserPageInfo { url: "https://una.os/".to_string(), secure: true, summary: "TLS 1.3".to_string() },
    r#"{"BrowserPageInfo":{"url":"https://una.os/","secure":true,"summary":"TLS 1.3"}}"#
);
kat!(
    kat_browser_download,
    SMessage::BrowserDownload { id: 7, name: "una.iso".to_string(), received: 512, total: Some(1024), done: false, error: None },
    r#"{"BrowserDownload":{"id":7,"name":"una.iso","received":512,"total":1024,"done":false,"error":null}}"#
);
// AETHERDEVTOOLS2 (SR112): the developer-tools pane's seven verbs.
kat!(
    kat_browser_devtools_key,
    SMessage::BrowserDevtoolsKey { key: "F12".to_string(), ctrl: false, shift: false, alt: false },
    r#"{"BrowserDevtoolsKey":{"key":"F12","ctrl":false,"shift":false,"alt":false}}"#
);
kat!(kat_browser_devtools_dock, SMessage::BrowserDevtoolsDock { bottom: true }, r#"{"BrowserDevtoolsDock":{"bottom":true}}"#);
kat!(kat_browser_devtools_share, SMessage::BrowserDevtoolsShare(40), r#"{"BrowserDevtoolsShare":40}"#);
kat!(kat_browser_devtools_panel, SMessage::BrowserDevtoolsPanel("Network".to_string()), r#"{"BrowserDevtoolsPanel":"Network"}"#);
kat!(kat_browser_devtools_eval, SMessage::BrowserDevtoolsEval("1+1".to_string()), r#"{"BrowserDevtoolsEval":"1+1"}"#);
kat!(kat_browser_devtools_goto, SMessage::BrowserDevtoolsGoto { url: "http://a/x.js".to_string(), line: 7 }, r#"{"BrowserDevtoolsGoto":{"url":"http://a/x.js","line":7}}"#);
kat!(
    kat_browser_devtools_layout,
    SMessage::BrowserDevtoolsLayout { bottom: false, page: (0, 0, 600, 800), pane: Some((600, 0, 400, 800)) },
    r#"{"BrowserDevtoolsLayout":{"bottom":false,"page":[0,0,600,800],"pane":[600,0,400,800]}}"#
);
kat!(
    kat_browser_devtools_layout_closed,
    SMessage::BrowserDevtoolsLayout { bottom: true, page: (0, 0, 1000, 800), pane: None },
    r#"{"BrowserDevtoolsLayout":{"bottom":true,"page":[0,0,1000,800],"pane":null}}"#
);
kat!(
    kat_browser_devtools_lines,
    SMessage::BrowserDevtoolsLines(vec!["[Elements]  Console".to_string(), " <html>".to_string()]),
    r#"{"BrowserDevtoolsLines":["[Elements]  Console"," <html>"]}"#
);

// A11YFACE (SR124): the accessibility verbs and Principia's accessibility rows.
kat!(kat_browser_a11y_tree, SMessage::BrowserA11y(A11yVerb::Tree), r#"{"BrowserA11y":"Tree"}"#);
kat!(kat_browser_a11y_skip, SMessage::BrowserA11y(A11yVerb::Skip), r#"{"BrowserA11y":"Skip"}"#);
kat!(
    kat_browser_a11y_heading,
    SMessage::BrowserA11y(A11yVerb::NextHeading { backward: true, level: Some(2) }),
    r#"{"BrowserA11y":{"NextHeading":{"backward":true,"level":2}}}"#
);
kat!(
    kat_browser_a11y_landmark,
    SMessage::BrowserA11y(A11yVerb::NextLandmark { backward: false }),
    r#"{"BrowserA11y":{"NextLandmark":{"backward":false}}}"#
);
kat!(
    kat_browser_a11y_lines,
    SMessage::BrowserA11yLines(vec!["RootWebArea".to_string(), "  main".to_string()]),
    r#"{"BrowserA11yLines":["RootWebArea","  main"]}"#
);
kat!(
    kat_browser_a11y_prefs,
    SMessage::BrowserA11yPrefs { reduce_motion: true, contrast: "more".to_string(), forced_colors: false },
    r#"{"BrowserA11yPrefs":{"reduce_motion":true,"contrast":"more","forced_colors":false}}"#
);

/// A11YFACE (SR124): the `a11y` verb table reads every spelling back to its verb, a heading level, and nothing else.
#[test]
fn a11y_verb_table_round_trips() {
    for (spelling, verb) in A11yVerb::TABLE {
        assert_eq!(A11yVerb::parse(spelling), Some(verb), "{spelling}");
        assert_eq!(verb.spelling(), spelling);
    }
    let h2 = A11yVerb::NextHeading { backward: false, level: Some(2) };
    assert_eq!(A11yVerb::parse("  A11Y next   heading 2 "), Some(h2));
    assert_eq!(h2.spelling(), "a11y next heading 2");
    assert_eq!(A11yVerb::parse("a11y next heading 7"), None);
    assert_eq!(A11yVerb::parse("a11y skip 2"), None);
    assert_eq!(A11yVerb::parse("a11y"), None);
    assert_eq!(A11yVerb::parse(""), None);
}

// --- STRIA (A/V playback) ---

kat!(kat_media_poster, SMessage::MediaPoster { url: "file:///v.webm".to_string() }, r#"{"MediaPoster":{"url":"file:///v.webm"}}"#);
kat!(kat_media_pause, SMessage::MediaPause { url: "u".to_string() }, r#"{"MediaPause":{"url":"u"}}"#);
kat!(kat_media_resume, SMessage::MediaResume { url: "u".to_string() }, r#"{"MediaResume":{"url":"u"}}"#);
kat!(
    kat_media_seek,
    SMessage::MediaSeek { url: "u".to_string(), position_ns: 1_500_000_000 },
    r#"{"MediaSeek":{"url":"u","position_ns":1500000000}}"#
);
kat!(kat_media_stop, SMessage::MediaStop { url: "u".to_string() }, r#"{"MediaStop":{"url":"u"}}"#);
kat!(kat_media_mute, SMessage::MediaMute { url: "u".to_string(), muted: true }, r#"{"MediaMute":{"url":"u","muted":true}}"#);
kat!(kat_media_volume, SMessage::MediaVolume { url: "u".to_string(), volume: 0.5 }, r#"{"MediaVolume":{"url":"u","volume":0.5}}"#);
kat!(kat_media_rate, SMessage::MediaRate { url: "u".to_string(), rate: 1.5 }, r#"{"MediaRate":{"url":"u","rate":1.5}}"#);
kat!(
    kat_media_opened,
    SMessage::MediaOpened {
        url: "u".to_string(),
        duration_ns: 1_000_000_000,
        width: 320,
        height: 240,
        video: "av01".to_string(),
        audio: "".to_string(),
        real_video: false,
        audio_clock: false,
    },
    r#"{"MediaOpened":{"url":"u","duration_ns":1000000000,"width":320,"height":240,"video":"av01","audio":"","real_video":false,"audio_clock":false}}"#
);
kat!(
    kat_media_frame,
    SMessage::MediaFrame { url: "u".to_string(), pts_ns: 40_000_000, width: 1, height: 1, rgba: vec![1, 2, 3, 255], levels: vec![] },
    r#"{"MediaFrame":{"url":"u","pts_ns":40000000,"width":1,"height":1,"rgba":[1,2,3,255]}}"#
);
// AUDIOTRACK (SR45): an audio-only session's meter frame — no pixels, peak L/R per 50 ms.
kat!(
    kat_media_frame_levels,
    SMessage::MediaFrame { url: "u".to_string(), pts_ns: 50_000_000, width: 0, height: 0, rgba: vec![], levels: vec![[0.5, 0.25], [1.0, 0.0]] },
    r#"{"MediaFrame":{"url":"u","pts_ns":50000000,"width":0,"height":0,"rgba":[],"levels":[[0.5,0.25],[1.0,0.0]]}}"#
);
kat!(
    kat_media_ended,
    SMessage::MediaEnded { url: "u".to_string(), presented: 10, dropped: 0 },
    r#"{"MediaEnded":{"url":"u","presented":10,"dropped":0}}"#
);
kat!(
    kat_media_error,
    SMessage::MediaError { url: "u".to_string(), error: "no such file".to_string() },
    r#"{"MediaError":{"url":"u","error":"no such file"}}"#
);

// --- SONDE (Probes; REALSENSE, LEDGER SR92) ---
// The four probe verbs, frozen. ProbeList's ask is the empty list; ProbeClose is both the request and
// Sonde's "stopped" answer. ProbeFrame's floats are chosen exactly representable or shortest-printing
// (0.001 prints as 0.001 through serde_json's f32 path), so the golden is the wire, not a rounding.

kat!(kat_probe_list_ask, SMessage::ProbeList { probes: vec![] }, r#"{"ProbeList":{"probes":[]}}"#);
kat!(
    kat_probe_list,
    SMessage::ProbeList {
        probes: vec![(
            "replay:d435i".to_string(),
            "Intel RealSense D435i (replay)".to_string(),
            vec!["depth".to_string(), "ir".to_string(), "color".to_string()],
        )],
    },
    r#"{"ProbeList":{"probes":[["replay:d435i","Intel RealSense D435i (replay)",["depth","ir","color"]]]}}"#
);
kat!(
    kat_probe_open,
    SMessage::ProbeOpen { id: "replay:d435i".to_string(), stream: "depth".to_string(), width: 848, height: 480, fps: 30 },
    r#"{"ProbeOpen":{"id":"replay:d435i","stream":"depth","width":848,"height":480,"fps":30}}"#
);
kat!(
    kat_probe_frame_depth,
    SMessage::ProbeFrame {
        id: "replay:d435i".to_string(),
        stream: "depth".to_string(),
        format: "Z16".to_string(),
        seq: 7,
        timestamp_us: 233_331,
        width: 2,
        height: 1,
        stride: 4,
        depth_scale: 0.001,
        intrinsics: Some([3.0, 3.0, 1.0, 0.5]),
        data: vec![0xE8, 0x03, 0x00, 0x00],
    },
    r#"{"ProbeFrame":{"id":"replay:d435i","stream":"depth","format":"Z16","seq":7,"timestamp_us":233331,"width":2,"height":1,"stride":4,"depth_scale":0.001,"intrinsics":[3.0,3.0,1.0,0.5],"data":[232,3,0,0]}}"#
);
kat!(
    kat_probe_frame_color,
    SMessage::ProbeFrame {
        id: "replay:d435i".to_string(),
        stream: "color".to_string(),
        format: "YUY2".to_string(),
        seq: 0,
        timestamp_us: 0,
        width: 2,
        height: 1,
        stride: 4,
        depth_scale: 0.0,
        intrinsics: None,
        data: vec![16, 128, 235, 128],
    },
    r#"{"ProbeFrame":{"id":"replay:d435i","stream":"color","format":"YUY2","seq":0,"timestamp_us":0,"width":2,"height":1,"stride":4,"depth_scale":0.0,"intrinsics":null,"data":[16,128,235,128]}}"#
);
kat!(
    kat_probe_close,
    SMessage::ProbeClose { id: "replay:d435i".to_string(), stream: "depth".to_string() },
    r#"{"ProbeClose":{"id":"replay:d435i","stream":"depth"}}"#
);
kat!(
    kat_probe_close_all,
    SMessage::ProbeClose { id: "replay:d435i".to_string(), stream: String::new() },
    r#"{"ProbeClose":{"id":"replay:d435i","stream":""}}"#
);

// --- EDITOR (The Code Pane) ---

kat!(
    kat_editor_load,
    SMessage::EditorLoad {
        path: Some("/una/x.rs".to_string()),
        content: "fn main(){}".to_string(),
        language: "rust".to_string(),
    },
    r#"{"EditorLoad":{"path":"/una/x.rs","content":"fn main(){}","language":"rust"}}"#
);
kat!(
    kat_editor_edited,
    SMessage::EditorEdited { content: "edited".to_string() },
    r#"{"EditorEdited":{"content":"edited"}}"#
);
kat!(kat_editor_save_request, SMessage::EditorSaveRequest, r#""EditorSaveRequest""#);

// --- CONSOLE (The Bottom Pane) ---

kat!(
    kat_console_append,
    SMessage::ConsoleAppend("out".to_string()),
    r#"{"ConsoleAppend":"out"}"#
);
kat!(
    kat_console_input,
    SMessage::ConsoleInput("ls -la".to_string()),
    r#"{"ConsoleInput":"ls -la"}"#
);

// --- MIDDEN (The Terminal) ---

kat!(kat_no_op, SMessage::NoOp, r#""NoOp""#);
kat!(
    kat_terminal_output,
    SMessage::TerminalOutput("line1\nline2".to_string()),
    r#"{"TerminalOutput":"line1\nline2"}"#
);
kat!(
    kat_terminal_error,
    SMessage::TerminalError("E: no vug".to_string()),
    r#"{"TerminalError":"E: no vug"}"#
);
kat!(
    kat_file_system_event,
    SMessage::FileSystemEvent("created /una/tmp".to_string()),
    r#"{"FileSystemEvent":"created /una/tmp"}"#
);
kat!(
    kat_trigger_upload,
    SMessage::TriggerUpload(PathBuf::from("/una/upload.bin")),
    r#"{"TriggerUpload":"/una/upload.bin"}"#
);

// --- PRINCIPIA (The Basal Ganglia) — one KAT per carried sub-variant ---

kat!(
    kat_principia_set_system_root,
    SMessage::Principia(PrincipiaCommand::SetSystemRoot(PathBuf::from("/una"))),
    r#"{"Principia":{"SetSystemRoot":"/una"}}"#
);
kat!(
    kat_principia_system_root_changed,
    SMessage::Principia(PrincipiaCommand::SystemRootChanged(PathBuf::from("/una"))),
    r#"{"Principia":{"SystemRootChanged":"/una"}}"#
);

// --- PRINCIPIA / PREFERENCES — one KAT per verb, plus the value domain ---

kat!(
    kat_principia_pref_get,
    SMessage::Principia(PrincipiaCommand::PrefGet {
        ns: "aether".to_string(),
        key: "homepage".to_string(),
    }),
    r#"{"Principia":{"PrefGet":{"ns":"aether","key":"homepage"}}}"#
);
kat!(
    kat_principia_pref_value_is,
    SMessage::Principia(PrincipiaCommand::PrefValueIs {
        ns: "aether".to_string(),
        key: "homepage".to_string(),
        value: Some(PrefValue::Str("https://una.os/".to_string())),
    }),
    r#"{"Principia":{"PrefValueIs":{"ns":"aether","key":"homepage","value":{"Str":"https://una.os/"}}}}"#
);
kat!(
    kat_principia_pref_value_is_unset,
    SMessage::Principia(PrincipiaCommand::PrefValueIs {
        ns: "aether".to_string(),
        key: "homepage".to_string(),
        value: None,
    }),
    r#"{"Principia":{"PrefValueIs":{"ns":"aether","key":"homepage","value":null}}}"#
);
kat!(
    kat_principia_pref_set,
    SMessage::Principia(PrincipiaCommand::PrefSet {
        ns: "aether".to_string(),
        key: "window.width".to_string(),
        value: PrefValue::Int(1280),
    }),
    r#"{"Principia":{"PrefSet":{"ns":"aether","key":"window.width","value":{"Int":1280}}}}"#
);
kat!(
    kat_principia_pref_list,
    SMessage::Principia(PrincipiaCommand::PrefList {
        ns: "system".to_string(),
    }),
    r#"{"Principia":{"PrefList":{"ns":"system"}}}"#
);
kat!(
    kat_principia_pref_list_is,
    SMessage::Principia(PrincipiaCommand::PrefListIs {
        ns: "system".to_string(),
        entries: vec![
            ("locale".to_string(), PrefValue::Str("en-US".to_string())),
            ("scale".to_string(), PrefValue::Float(1.5)),
        ],
    }),
    r#"{"Principia":{"PrefListIs":{"ns":"system","entries":[["locale",{"Str":"en-US"}],["scale",{"Float":1.5}]]}}}"#
);
kat!(
    kat_principia_pref_changed,
    SMessage::Principia(PrincipiaCommand::PrefChanged {
        ns: "stria".to_string(),
        key: "muted".to_string(),
        value: PrefValue::Bool(true),
        clamped: false,
    }),
    r#"{"Principia":{"PrefChanged":{"ns":"stria","key":"muted","value":{"Bool":true}}}}"#
);
// PRINCIPIA2 (SR32): an out-of-range write to a declared key is clamped; the flag rides the wire
// only when true, so the unclamped golden above is unchanged.
kat!(
    kat_principia_pref_changed_clamped,
    SMessage::Principia(PrincipiaCommand::PrefChanged {
        ns: "system".to_string(),
        key: "display.brightness".to_string(),
        value: PrefValue::Int(16),
        clamped: true,
    }),
    r#"{"Principia":{"PrefChanged":{"ns":"system","key":"display.brightness","value":{"Int":16},"clamped":true}}}"#
);
kat!(
    kat_principia_pref_error,
    SMessage::Principia(PrincipiaCommand::PrefError {
        ns: "aether".to_string(),
        key: "window..width".to_string(),
        message: "empty key segment".to_string(),
    }),
    r#"{"Principia":{"PrefError":{"ns":"aether","key":"window..width","message":"empty key segment"}}}"#
);

/// The typed value domain must not widen or coerce across the wire — a float
/// that reads back as an int would silently retype a stored preference.
#[test]
fn pref_value_types_survive_the_wire_exactly() {
    let cases = [
        (PrefValue::Str("x".to_string()), r#"{"Str":"x"}"#),
        (PrefValue::Int(1), r#"{"Int":1}"#),
        (PrefValue::Float(1.0), r#"{"Float":1.0}"#),
        (PrefValue::Bool(false), r#"{"Bool":false}"#),
    ];
    for (value, golden) in cases {
        let json = serde_json::to_string(&value).expect("serialize");
        assert_eq!(json, golden, "wire shape drift for {value:?}");
        let back: PrefValue = serde_json::from_str(golden).expect("deserialize");
        assert_eq!(back, value, "round-trip retyped {value:?}");
        assert_eq!(pref_value_variant_name(&back), pref_value_variant_name(&value));
    }
}

// --- MATRIX (The Spatial Cortex) — one KAT per carried sub-variant ---

kat!(
    kat_matrix_ingest_topology,
    SMessage::Matrix(MatrixEvent::IngestTopology {
        ui_dag: "dag-ui".to_string(),
        semantic_dag: "dag-sem".to_string(),
    }),
    r#"{"Matrix":{"IngestTopology":{"ui_dag":"dag-ui","semantic_dag":"dag-sem"}}}"#
);
kat!(
    kat_matrix_graft_topology,
    SMessage::Matrix(MatrixEvent::GraftTopology {
        target_id: "n1".to_string(),
        payload: "symbols".to_string(),
    }),
    r#"{"Matrix":{"GraftTopology":{"target_id":"n1","payload":"symbols"}}}"#
);
kat!(
    kat_matrix_focus_sector,
    SMessage::Matrix(MatrixEvent::FocusSector("euclase".to_string())),
    r#"{"Matrix":{"FocusSector":"euclase"}}"#
);
kat!(
    kat_matrix_sector_focused,
    SMessage::Matrix(MatrixEvent::SectorFocused {
        target: "euclase".to_string(),
        context: "raw sector context".to_string(),
    }),
    r#"{"Matrix":{"SectorFocused":{"target":"euclase","context":"raw sector context"}}}"#
);
kat!(
    kat_matrix_node_selected,
    SMessage::Matrix(MatrixEvent::NodeSelected(PathBuf::from("/una/node.rs"))),
    r#"{"Matrix":{"NodeSelected":"/una/node.rs"}}"#
);
kat!(
    kat_matrix_topology_mutated,
    SMessage::Matrix(MatrixEvent::TopologyMutated(vec![
        ("a".to_string(), "crate".to_string(), 0),
        ("b".to_string(), "fn".to_string(), 2),
    ])),
    r#"{"Matrix":{"TopologyMutated":[["a","crate",0],["b","fn",2]]}}"#
);
kat!(
    kat_matrix_browse_to,
    SMessage::Matrix(MatrixEvent::BrowseTo {
        principal: Origin::LocalUser("peter".to_string()),
        path: "handlers/matrix".to_string(),
    }),
    r#"{"Matrix":{"BrowseTo":{"principal":{"LocalUser":"peter"},"path":"handlers/matrix"}}}"#
);
kat!(
    kat_matrix_dir_listed,
    SMessage::Matrix(MatrixEvent::DirListed(BrowseListing {
        path: "src".to_string(),
        parent: Some(String::new()),
        breadcrumbs: vec![(String::new(), String::new()), ("src".to_string(), "src".to_string())],
        entries: vec![
            BrowseEntry {
                path: "src/sub".to_string(),
                name: "sub".to_string(),
                kind: BrowseKind::Dir,
                size: 0,
                is_symlink: false,
            },
            BrowseEntry {
                path: "src/main.rs".to_string(),
                name: "main.rs".to_string(),
                kind: BrowseKind::File,
                size: 42,
                is_symlink: false,
            },
        ],
    })),
    r#"{"Matrix":{"DirListed":{"path":"src","parent":"","breadcrumbs":[["",""],["src","src"]],"entries":[{"path":"src/sub","name":"sub","kind":"Dir","size":0,"is_symlink":false},{"path":"src/main.rs","name":"main.rs","kind":"File","size":42,"is_symlink":false}]}}}"#
);
kat!(
    kat_matrix_file_op,
    SMessage::Matrix(MatrixEvent::FileOp {
        principal: Origin::LocalUser("peter".to_string()),
        verb: FsVerb::Rename,
        path: "notes.md".to_string(),
        arg: Some("renamed.md".to_string()),
        confirmed: false,
    }),
    r#"{"Matrix":{"FileOp":{"principal":{"LocalUser":"peter"},"verb":"Rename","path":"notes.md","arg":"renamed.md","confirmed":false}}}"#
);
kat!(
    kat_matrix_fs_op_result,
    SMessage::Matrix(MatrixEvent::FsOpResult {
        principal: Origin::LocalUser("peter".to_string()),
        verb: FsVerb::Delete,
        path: "notes.md".to_string(),
        outcome: FsOutcome::Denied { reason: "read-only volume".to_string() },
    }),
    r#"{"Matrix":{"FsOpResult":{"principal":{"LocalUser":"peter"},"verb":"Delete","path":"notes.md","outcome":{"Denied":{"reason":"read-only volume"}}}}}"#
);

// --- UI EVENTS ---

kat!(
    kat_input,
    SMessage::Input { target: "chat".to_string(), text: "hi".to_string() },
    r#"{"Input":{"target":"chat","text":"hi"}}"#
);
kat!(kat_template_action, SMessage::TemplateAction(3), r#"{"TemplateAction":3}"#);
kat!(kat_nav_select, SMessage::NavSelect(1), r#"{"NavSelect":1}"#);
kat!(kat_dock_action, SMessage::DockAction(2), r#"{"DockAction":2}"#);
kat!(kat_upload_request, SMessage::UploadRequest, r#""UploadRequest""#);
kat!(
    kat_file_selected,
    SMessage::FileSelected(PathBuf::from("/una/pick.txt")),
    r#"{"FileSelected":"/una/pick.txt"}"#
);
kat!(kat_toggle_sidebar, SMessage::ToggleSidebar, r#""ToggleSidebar""#);
kat!(
    kat_load_history,
    SMessage::LoadHistory { offset: 100 },
    r#"{"LoadHistory":{"offset":100}}"#
);
kat!(
    kat_update_matrix_selection,
    SMessage::UpdateMatrixSelection(vec!["a".to_string(), "b".to_string()]),
    r#"{"UpdateMatrixSelection":["a","b"]}"#
);
kat!(
    kat_matrix_file_click,
    SMessage::MatrixFileClick(PathBuf::from("/una/map.rs")),
    r#"{"MatrixFileClick":"/una/map.rs"}"#
);
kat!(kat_aule_ignite, SMessage::AuleIgnite, r#""AuleIgnite""#);
kat!(kat_timer, SMessage::Timer, r#""Timer""#);
kat!(
    kat_create_node,
    SMessage::CreateNode {
        model: "una-prime".to_string(),
        history: true,
        temperature: 0.5,
        system_prompt: "be kind".to_string(),
    },
    r#"{"CreateNode":{"model":"una-prime","history":true,"temperature":0.5,"system_prompt":"be kind"}}"#
);
kat!(
    kat_node_action,
    SMessage::NodeAction { action: "pause".to_string(), active: false },
    r#"{"NodeAction":{"action":"pause","active":false}}"#
);
kat!(
    kat_complex_input,
    SMessage::ComplexInput {
        target: "vein".to_string(),
        subject: "subject".to_string(),
        body: "body".to_string(),
        point_break: true,
        action: "send".to_string(),
    },
    r#"{"ComplexInput":{"target":"vein","subject":"subject","body":"body","point_break":true,"action":"send"}}"#
);
kat!(
    kat_shard_select,
    SMessage::ShardSelect("s9".to_string()),
    r#"{"ShardSelect":"s9"}"#
);
kat!(
    kat_dispatch_payload,
    SMessage::DispatchPayload("payload".to_string()),
    r#"{"DispatchPayload":"payload"}"#
);
kat!(
    kat_toggle_matrix_node,
    SMessage::ToggleMatrixNode("kernel".to_string()),
    r#"{"ToggleMatrixNode":"kernel"}"#
);
kat!(kat_ui_ready, SMessage::UiReady, r#""UiReady""#);

// --- LOGS (the Console log-viewer sub-enum) ----------------------------------
kat!(
    kat_logs_filter,
    SMessage::Logs(LogEvent::LogFilter("gpu".to_string())),
    r#"{"Logs":{"LogFilter":"gpu"}}"#
);
kat!(
    kat_logs_source_all,
    SMessage::Logs(LogEvent::LogSource(LogSource::All)),
    r#"{"Logs":{"LogSource":"All"}}"#
);
kat!(
    kat_logs_source_subsystem,
    SMessage::Logs(LogEvent::LogSource(LogSource::Subsystem("kernel".to_string()))),
    r#"{"Logs":{"LogSource":{"Subsystem":"kernel"}}}"#
);
kat!(
    kat_logs_pause,
    SMessage::Logs(LogEvent::LogPause(true)),
    r#"{"Logs":{"LogPause":true}}"#
);
kat!(
    kat_logs_tail,
    SMessage::Logs(LogEvent::LogTail {
        lines: vec![LogLine {
            seq: 1,
            level: "info".to_string(),
            source: "net".to_string(),
            content: "link up".to_string(),
        }],
        dropped: 2,
        paused: false,
    }),
    r#"{"Logs":{"LogTail":{"lines":[{"seq":1,"level":"info","source":"net","content":"link up"}],"dropped":2,"paused":false}}}"#
);

// --- FACET (SR29) ------------------------------------------------------------

fn facet_info() -> FacetImageInfo {
    FacetImageInfo {
        path: "/una/a.png".to_string(),
        format: "png".to_string(),
        source_width: 4,
        source_height: 2,
        width: 2,
        height: 4,
        orientation: 6,
        colour: "sRGB chunk".to_string(),
        bit_depth: 8,
        has_alpha: true,
        frames: 1,
        bytes: 99,
        edits: 0,
    }
}

kat!(
    kat_facet_image_open,
    SMessage::Facet(FacetCommand::ImageOpen {
        receipt_id: 7,
        principal: Origin::LocalUser("una".to_string()),
        path: "/una/a.png".to_string(),
    }),
    r#"{"Facet":{"ImageOpen":{"receipt_id":7,"principal":{"LocalUser":"una"},"path":"/una/a.png"}}}"#
);
kat!(
    kat_facet_image_opened,
    SMessage::Facet(FacetCommand::ImageOpened { receipt_id: 7, handle: 1, info: facet_info() }),
    r#"{"Facet":{"ImageOpened":{"receipt_id":7,"handle":1,"info":{"path":"/una/a.png","format":"png","source_width":4,"source_height":2,"width":2,"height":4,"orientation":6,"colour":"sRGB chunk","bit_depth":8,"has_alpha":true,"frames":1,"bytes":99,"edits":0}}}}"#
);
kat!(
    kat_facet_image_info,
    SMessage::Facet(FacetCommand::ImageInfo { receipt_id: 8, handle: 1 }),
    r#"{"Facet":{"ImageInfo":{"receipt_id":8,"handle":1}}}"#
);
kat!(
    kat_facet_image_info_is,
    SMessage::Facet(FacetCommand::ImageInfoIs { receipt_id: 8, handle: 1, info: facet_info() }),
    r#"{"Facet":{"ImageInfoIs":{"receipt_id":8,"handle":1,"info":{"path":"/una/a.png","format":"png","source_width":4,"source_height":2,"width":2,"height":4,"orientation":6,"colour":"sRGB chunk","bit_depth":8,"has_alpha":true,"frames":1,"bytes":99,"edits":0}}}}"#
);
kat!(
    kat_facet_image_edit_crop,
    SMessage::Facet(FacetCommand::ImageEdit {
        receipt_id: 9,
        handle: 1,
        edit: FacetEdit::Crop { x: 1, y: 2, width: 3, height: 4 },
    }),
    r#"{"Facet":{"ImageEdit":{"receipt_id":9,"handle":1,"edit":{"Crop":{"x":1,"y":2,"width":3,"height":4}}}}}"#
);
kat!(
    kat_facet_image_edit_adjust,
    SMessage::Facet(FacetCommand::ImageEdit {
        receipt_id: 10,
        handle: 1,
        edit: FacetEdit::Adjust { brightness: 1.5, contrast: 0.5 },
    }),
    r#"{"Facet":{"ImageEdit":{"receipt_id":10,"handle":1,"edit":{"Adjust":{"brightness":1.5,"contrast":0.5}}}}}"#
);
kat!(
    kat_facet_image_edit_undo,
    SMessage::Facet(FacetCommand::ImageEdit { receipt_id: 11, handle: 1, edit: FacetEdit::Undo }),
    r#"{"Facet":{"ImageEdit":{"receipt_id":11,"handle":1,"edit":"Undo"}}}"#
);
kat!(
    kat_facet_image_render,
    SMessage::Facet(FacetCommand::ImageRender {
        receipt_id: 12,
        handle: 1,
        width: 640,
        height: 480,
        view: FacetView {
            zoom: FacetZoom::Percent(200),
            pan_x: -3,
            pan_y: 4,
            quarter_turns: 1,
            flip_h: true,
            flip_v: false,
        },
    }),
    r#"{"Facet":{"ImageRender":{"receipt_id":12,"handle":1,"width":640,"height":480,"view":{"zoom":{"Percent":200},"pan_x":-3,"pan_y":4,"quarter_turns":1,"flip_h":true,"flip_v":false}}}}"#
);
kat!(
    kat_facet_image_render_fit,
    SMessage::Facet(FacetCommand::ImageRender {
        receipt_id: 13,
        handle: 1,
        width: 2,
        height: 2,
        view: FacetView::default(),
    }),
    r#"{"Facet":{"ImageRender":{"receipt_id":13,"handle":1,"width":2,"height":2,"view":{"zoom":"Fit","pan_x":0,"pan_y":0,"quarter_turns":0,"flip_h":false,"flip_v":false}}}}"#
);
kat!(
    kat_facet_image_rendered,
    SMessage::Facet(FacetCommand::ImageRendered {
        receipt_id: 13,
        handle: 1,
        width: 1,
        height: 1,
        rgba: vec![1, 2, 3, 255],
    }),
    r#"{"Facet":{"ImageRendered":{"receipt_id":13,"handle":1,"width":1,"height":1,"rgba":[1,2,3,255]}}}"#
);
kat!(
    kat_facet_image_export,
    SMessage::Facet(FacetCommand::ImageExport {
        receipt_id: 14,
        handle: 1,
        path: "/una/out.png".to_string(),
        format: FacetFormat::Png,
        overwrite: false,
    }),
    r#"{"Facet":{"ImageExport":{"receipt_id":14,"handle":1,"path":"/una/out.png","format":"Png","overwrite":false}}}"#
);
kat!(
    kat_facet_image_exported,
    SMessage::Facet(FacetCommand::ImageExported {
        receipt_id: 14,
        handle: 1,
        path: "/una/out.png".to_string(),
        bytes: 321,
    }),
    r#"{"Facet":{"ImageExported":{"receipt_id":14,"handle":1,"path":"/una/out.png","bytes":321}}}"#
);
kat!(
    kat_facet_image_close,
    SMessage::Facet(FacetCommand::ImageClose { handle: 1 }),
    r#"{"Facet":{"ImageClose":{"handle":1}}}"#
);
kat!(
    kat_facet_image_error,
    SMessage::Facet(FacetCommand::ImageError {
        receipt_id: 15,
        handle: None,
        message: "no such file".to_string(),
    }),
    r#"{"Facet":{"ImageError":{"receipt_id":15,"handle":null,"message":"no such file"}}}"#
);

fn facet_variant_name(c: &FacetCommand) -> &'static str {
    match c {
        FacetCommand::ImageOpen { .. } => "ImageOpen",
        FacetCommand::ImageOpened { .. } => "ImageOpened",
        FacetCommand::ImageInfo { .. } => "ImageInfo",
        FacetCommand::ImageInfoIs { .. } => "ImageInfoIs",
        FacetCommand::ImageEdit { .. } => "ImageEdit",
        FacetCommand::ImageRender { .. } => "ImageRender",
        FacetCommand::ImageRendered { .. } => "ImageRendered",
        FacetCommand::ImageExport { .. } => "ImageExport",
        FacetCommand::ImageExported { .. } => "ImageExported",
        FacetCommand::ImageClose { .. } => "ImageClose",
        FacetCommand::ImageError { .. } => "ImageError",
    }
}

fn facet_edit_variant_name(e: &FacetEdit) -> &'static str {
    match e {
        FacetEdit::Crop { .. } => "Crop",
        FacetEdit::Rotate { .. } => "Rotate",
        FacetEdit::Flip { .. } => "Flip",
        FacetEdit::Resize { .. } => "Resize",
        FacetEdit::Adjust { .. } => "Adjust",
        FacetEdit::Undo => "Undo",
        FacetEdit::Redo => "Redo",
        FacetEdit::Reset => "Reset",
    }
}

#[test]
fn facet_vocabulary_complete() {
    assert_eq!(facet_variant_name(&FacetCommand::ImageClose { handle: 0 }), "ImageClose");
    assert_eq!(facet_edit_variant_name(&FacetEdit::Reset), "Reset");
    assert_eq!(FacetFormat::Jpeg, FacetFormat::Jpeg);
}

#[test]
fn facet_open_image_association() {
    assert_eq!(FacetCommand::image_mime_for("/a/b/Photo.JPG"), Some("image/jpeg"));
    assert_eq!(FacetCommand::image_mime_for("x.png"), Some("image/png"));
    assert_eq!(FacetCommand::image_mime_for("dir.png/readme"), None);
    assert_eq!(FacetCommand::image_mime_for("anim.webp"), Some("image/webp"));
    assert_eq!(FacetCommand::image_mime_for("notes.txt"), None);
    assert_eq!(FacetCommand::image_mime_for("png"), None);
    // TYPECORE (B450): the one table — the spellings the host twin alone knew, and a type Facet does not claim.
    assert_eq!(FacetCommand::image_mime_for("scan.JFIF"), Some("image/jpeg"));
    assert_eq!(FacetCommand::image_mime_for("C:\\x\\old.dib"), Some("image/bmp"));
    assert_eq!(FacetCommand::image_mime_for("logo.svg"), None);
    assert!(type_core::EXT_TABLE.iter().all(|(e, m)| FacetCommand::image_mime_for(&format!("f.{e}")).map_or(true, |x| x == *m)));
    assert_eq!(FacetCommand::local_image_path("file:///home/una/My%20Photo.jpeg").as_deref(), Some("/home/una/My Photo.jpeg"));
    assert_eq!(FacetCommand::local_image_path("file://localhost/a/b.png?x=1#f").as_deref(), Some("/a/b.png"));
    assert_eq!(FacetCommand::local_image_path("/a/b.webp").as_deref(), Some("/a/b.webp"));
    assert_eq!(FacetCommand::local_image_path("https://una.os/b.png"), None);
    assert_eq!(FacetCommand::local_image_path("file:///a/b.html"), None);
    assert_eq!(FacetCommand::local_image_path("file:///a/b%2.png").as_deref(), Some("/a/b%2.png"));
    assert!(FacetCommand::ImageOpen { receipt_id: 1, principal: Origin::System("t".into()), path: "a".into() }.is_request());
    assert!(!FacetCommand::ImageExported { receipt_id: 1, handle: 1, path: "a".into(), bytes: 0 }.is_request());
}

// --- TABULA (TABULABUS, SR74) ---

kat!(
    kat_tabula_editor_open,
    SMessage::Tabula(TabulaCommand::EditorOpen { principal: Origin::LocalUser("una".into()), path: "/una/x.rs".into() }),
    r#"{"Tabula":{"EditorOpen":{"principal":{"LocalUser":"una"},"path":"/una/x.rs"}}}"#
);
kat!(
    kat_tabula_editor_loaded,
    SMessage::Tabula(TabulaCommand::EditorLoaded {
        path: "/una/x.rs".into(),
        content: "fn main() {}\n".into(),
        lines: 1,
        language: "rust".into(),
        read_only: false,
    }),
    r#"{"Tabula":{"EditorLoaded":{"path":"/una/x.rs","content":"fn main() {}\n","lines":1,"language":"rust","read_only":false}}}"#
);
kat!(
    kat_tabula_editor_loaded_log,
    SMessage::Tabula(TabulaCommand::EditorLoaded {
        path: "/mnt/UNAOS.LOG".into(),
        content: ":: log ::\n".into(),
        lines: 1,
        language: "txt".into(),
        read_only: true,
    }),
    r#"{"Tabula":{"EditorLoaded":{"path":"/mnt/UNAOS.LOG","content":":: log ::\n","lines":1,"language":"txt","read_only":true}}}"#
);
kat!(
    kat_tabula_editor_save,
    SMessage::Tabula(TabulaCommand::EditorSave { principal: Origin::LocalUser("una".into()), path: "/una/x.rs".into() }),
    r#"{"Tabula":{"EditorSave":{"principal":{"LocalUser":"una"},"path":"/una/x.rs"}}}"#
);
kat!(
    kat_tabula_editor_saved,
    SMessage::Tabula(TabulaCommand::EditorSaved { path: "/una/x.rs".into() }),
    r#"{"Tabula":{"EditorSaved":{"path":"/una/x.rs"}}}"#
);
kat!(
    kat_tabula_editor_goto,
    SMessage::Tabula(TabulaCommand::EditorGoto {
        principal: Origin::System("vein".into()),
        path: "/una/x.rs".into(),
        line: 3,
        col: 7,
    }),
    r#"{"Tabula":{"EditorGoto":{"principal":{"System":"vein"},"path":"/una/x.rs","line":3,"col":7}}}"#
);
kat!(
    kat_tabula_editor_insert,
    SMessage::Tabula(TabulaCommand::EditorInsert {
        principal: Origin::LocalUser("una".into()),
        path: "/una/x.rs".into(),
        text: "// hi\n".into(),
    }),
    r#"{"Tabula":{"EditorInsert":{"principal":{"LocalUser":"una"},"path":"/una/x.rs","text":"// hi\n"}}}"#
);
kat!(
    kat_tabula_editor_dirty,
    SMessage::Tabula(TabulaCommand::EditorDirty { path: "/una/x.rs".into(), dirty: true }),
    r#"{"Tabula":{"EditorDirty":{"path":"/una/x.rs","dirty":true}}}"#
);
kat!(
    kat_tabula_editor_closed,
    SMessage::Tabula(TabulaCommand::EditorClosed { path: "/una/x.rs".into() }),
    r#"{"Tabula":{"EditorClosed":{"path":"/una/x.rs"}}}"#
);
kat!(
    kat_tabula_editor_error,
    SMessage::Tabula(TabulaCommand::EditorError { path: "/una/nope.rs".into(), why: "No such file or directory (os error 2)".into() }),
    r#"{"Tabula":{"EditorError":{"path":"/una/nope.rs","why":"No such file or directory (os error 2)"}}}"#
);

fn tabula_variant_name(c: &TabulaCommand) -> &'static str {
    match c {
        TabulaCommand::EditorOpen { .. } => "EditorOpen",
        TabulaCommand::EditorLoaded { .. } => "EditorLoaded",
        TabulaCommand::EditorSave { .. } => "EditorSave",
        TabulaCommand::EditorSaved { .. } => "EditorSaved",
        TabulaCommand::EditorGoto { .. } => "EditorGoto",
        TabulaCommand::EditorInsert { .. } => "EditorInsert",
        TabulaCommand::EditorDirty { .. } => "EditorDirty",
        TabulaCommand::EditorClosed { .. } => "EditorClosed",
        TabulaCommand::EditorError { .. } => "EditorError",
    }
}

#[test]
fn tabula_vocabulary_complete_and_requests_split() {
    let p = || Origin::LocalUser("una".into());
    let reqs = [
        TabulaCommand::EditorOpen { principal: p(), path: "a".into() },
        TabulaCommand::EditorSave { principal: p(), path: "a".into() },
        TabulaCommand::EditorGoto { principal: p(), path: "a".into(), line: 1, col: 1 },
        TabulaCommand::EditorInsert { principal: p(), path: "a".into(), text: "x".into() },
        TabulaCommand::EditorClosed { path: "a".into() },
    ];
    let answers = [
        TabulaCommand::EditorLoaded { path: "a".into(), content: String::new(), lines: 0, language: "txt".into(), read_only: false },
        TabulaCommand::EditorSaved { path: "a".into() },
        TabulaCommand::EditorDirty { path: "a".into(), dirty: false },
        TabulaCommand::EditorError { path: "a".into(), why: "w".into() },
    ];
    for r in &reqs {
        assert!(r.is_request(), "{} must be served", tabula_variant_name(r));
        assert_eq!(r.path(), "a");
    }
    for a in &answers {
        assert!(!a.is_request(), "{} is an answer, inert as input", tabula_variant_name(a));
        assert_eq!(a.path(), "a");
    }
}

// --- COMPLETENESS GUARD ------------------------------------------------------
//
// Exhaustive matches over the message vocabulary, with NO wildcard arm.
// If any of these stop compiling, a variant was added, removed, or renamed:
// that is a wire-vocabulary change. Add (or retire) the corresponding golden
// KAT above AND extend the match — do not add a `_` arm.

fn smessage_variant_name(m: &SMessage) -> &'static str {
    match m {
        SMessage::StateInvalidated => "StateInvalidated",
        SMessage::Ping => "Ping",
        SMessage::Kill(_) => "Kill",
        SMessage::Log { .. } => "Log",
        SMessage::CorePulse { .. } => "CorePulse",
        SMessage::EuclaseResize(_, _) => "EuclaseResize",
        SMessage::VugPulse => "VugPulse",
        SMessage::AudioChunk { .. } => "AudioChunk",
        SMessage::Spectrum { .. } => "Spectrum",
        SMessage::UserPrompt(_) => "UserPrompt",
        SMessage::AiToken(_) => "AiToken",
        SMessage::AnalyzeContext { .. } => "AnalyzeContext",
        SMessage::NetworkLog(_) => "NetworkLog",
        SMessage::NetworkState(_) => "NetworkState",
        SMessage::GetDiff { .. } => "GetDiff",
        SMessage::DiffPayload { .. } => "DiffPayload",
        SMessage::ContextTelemetry { .. } => "ContextTelemetry",
        SMessage::FileEvent { .. } => "FileEvent",
        SMessage::StorageQuery { .. } => "StorageQuery",
        SMessage::StorageQueryResult { .. } => "StorageQueryResult",
        SMessage::StorageSave { .. } => "StorageSave",
        SMessage::StorageSaveResult { .. } => "StorageSaveResult",
        SMessage::StorageLoadPaged { .. } => "StorageLoadPaged",
        SMessage::StorageLoadPagedResult { .. } => "StorageLoadPagedResult",
        SMessage::ReEmbed { .. } => "ReEmbed",
        SMessage::ReEmbedBatch { .. } => "ReEmbedBatch",
        SMessage::ReEmbedWrite { .. } => "ReEmbedWrite",
        SMessage::ReEmbedDone { .. } => "ReEmbedDone",
        SMessage::OpenDocument { .. } => "OpenDocument",
        SMessage::SurfaceBlit { .. } => "SurfaceBlit",
        SMessage::PlayMedia { .. } => "PlayMedia",
        SMessage::BrowserNavBack => "BrowserNavBack",
        SMessage::BrowserNavForward => "BrowserNavForward",
        SMessage::BrowserNavReload => "BrowserNavReload",
        SMessage::BrowserScroll(_, _) => "BrowserScroll",
        SMessage::BrowserClick(_, _) => "BrowserClick",
        SMessage::BrowserResize(_, _) => "BrowserResize",
        SMessage::BrowserKey(_) => "BrowserKey",
        SMessage::BrowserText(_) => "BrowserText",
        SMessage::BrowserPaste(_) => "BrowserPaste",
        SMessage::BrowserUrlChanged(_) => "BrowserUrlChanged",
        SMessage::BrowserTitleChanged(_) => "BrowserTitleChanged",
        SMessage::BrowserFaviconChanged { .. } => "BrowserFaviconChanged",
        SMessage::BrowserCopy => "BrowserCopy",
        SMessage::BrowserSelectAll => "BrowserSelectAll",
        SMessage::BrowserClipboardSet(_) => "BrowserClipboardSet",
        SMessage::MediaPoster { .. } => "MediaPoster",
        SMessage::MediaPause { .. } => "MediaPause",
        SMessage::MediaResume { .. } => "MediaResume",
        SMessage::MediaSeek { .. } => "MediaSeek",
        SMessage::MediaStop { .. } => "MediaStop",
        SMessage::MediaMute { .. } => "MediaMute",
        SMessage::MediaVolume { .. } => "MediaVolume",
        SMessage::MediaRate { .. } => "MediaRate",
        SMessage::MediaOpened { .. } => "MediaOpened",
        SMessage::MediaFrame { .. } => "MediaFrame",
        SMessage::MediaEnded { .. } => "MediaEnded",
        SMessage::MediaError { .. } => "MediaError",
        SMessage::ProbeList { .. } => "ProbeList", // REALSENSE (SR92): the four probe verbs
        SMessage::ProbeOpen { .. } => "ProbeOpen",
        SMessage::ProbeFrame { .. } => "ProbeFrame",
        SMessage::ProbeClose { .. } => "ProbeClose",
        SMessage::EditorLoad { .. } => "EditorLoad",
        SMessage::EditorEdited { .. } => "EditorEdited",
        SMessage::EditorSaveRequest => "EditorSaveRequest",
        SMessage::ConsoleAppend(_) => "ConsoleAppend",
        SMessage::ConsoleInput(_) => "ConsoleInput",
        SMessage::NoOp => "NoOp",
        SMessage::TerminalOutput(_) => "TerminalOutput",
        SMessage::TerminalError(_) => "TerminalError",
        SMessage::FileSystemEvent(_) => "FileSystemEvent",
        SMessage::TriggerUpload(_) => "TriggerUpload",
        SMessage::Principia(_) => "Principia",
        SMessage::Matrix(_) => "Matrix",
        SMessage::Logs(_) => "Logs",
        SMessage::Facet(_) => "Facet",
        SMessage::Input { .. } => "Input",
        SMessage::TemplateAction(_) => "TemplateAction",
        SMessage::NavSelect(_) => "NavSelect",
        SMessage::DockAction(_) => "DockAction",
        SMessage::UploadRequest => "UploadRequest",
        SMessage::FileSelected(_) => "FileSelected",
        SMessage::ToggleSidebar => "ToggleSidebar",
        SMessage::LoadHistory { .. } => "LoadHistory",
        SMessage::UpdateMatrixSelection(_) => "UpdateMatrixSelection",
        SMessage::MatrixFileClick(_) => "MatrixFileClick",
        SMessage::AuleIgnite => "AuleIgnite",
        SMessage::Timer => "Timer",
        SMessage::CreateNode { .. } => "CreateNode",
        SMessage::NodeAction { .. } => "NodeAction",
        SMessage::ComplexInput { .. } => "ComplexInput",
        SMessage::ShardSelect(_) => "ShardSelect",
        SMessage::DispatchPayload(_) => "DispatchPayload",
        SMessage::ToggleMatrixNode(_) => "ToggleMatrixNode",
        SMessage::UiReady => "UiReady",
        SMessage::Tabula(_) => "Tabula",
        SMessage::BrowserMouseMove(_, _) => "BrowserMouseMove",
        SMessage::BrowserCursorChanged(_) => "BrowserCursorChanged", // AETHERHOVER (SR67) fold: the exhaustive match gains the two motion variants
        SMessage::BrowserGo { .. } => "BrowserGo", // AETHERCHROME (SR84): the chrome's eighteen verbs
        SMessage::BrowserNavStop => "BrowserNavStop",
        SMessage::BrowserTabNew => "BrowserTabNew",
        SMessage::BrowserTabClose(_) => "BrowserTabClose",
        SMessage::BrowserTabSelect(_) => "BrowserTabSelect",
        SMessage::BrowserTabStep(_) => "BrowserTabStep",
        SMessage::BrowserZoom(_) => "BrowserZoom",
        SMessage::BrowserFind { .. } => "BrowserFind",
        SMessage::BrowserFindClose => "BrowserFindClose",
        SMessage::BrowserBookmarkToggle => "BrowserBookmarkToggle",
        SMessage::BrowserPageInfoAsk => "BrowserPageInfoAsk",
        SMessage::BrowserTabs { .. } => "BrowserTabs",
        SMessage::BrowserNavState { .. } => "BrowserNavState",
        SMessage::BrowserFindResult { .. } => "BrowserFindResult",
        SMessage::BrowserStatus(_) => "BrowserStatus",
        SMessage::BrowserBookmarks { .. } => "BrowserBookmarks",
        SMessage::BrowserPageInfo { .. } => "BrowserPageInfo",
        SMessage::BrowserDownload { .. } => "BrowserDownload",
        SMessage::BrowserDevtoolsKey { .. } => "BrowserDevtoolsKey", // AETHERDEVTOOLS2 (SR112): the pane's seven verbs
        SMessage::BrowserDevtoolsDock { .. } => "BrowserDevtoolsDock",
        SMessage::BrowserDevtoolsShare(_) => "BrowserDevtoolsShare",
        SMessage::BrowserDevtoolsPanel(_) => "BrowserDevtoolsPanel",
        SMessage::BrowserDevtoolsEval(_) => "BrowserDevtoolsEval",
        SMessage::BrowserDevtoolsGoto { .. } => "BrowserDevtoolsGoto",
        SMessage::BrowserDevtoolsLayout { .. } => "BrowserDevtoolsLayout",
        SMessage::BrowserDevtoolsLines(_) => "BrowserDevtoolsLines",
        SMessage::BrowserA11y(_) => "BrowserA11y", // A11YFACE (SR124): the accessibility verbs and rows
        SMessage::BrowserA11yLines(_) => "BrowserA11yLines",
        SMessage::BrowserA11yPrefs { .. } => "BrowserA11yPrefs",
    }
}

fn principia_variant_name(c: &PrincipiaCommand) -> &'static str {
    match c {
        PrincipiaCommand::SetSystemRoot(_) => "SetSystemRoot",
        PrincipiaCommand::SystemRootChanged(_) => "SystemRootChanged",
        PrincipiaCommand::PrefGet { .. } => "PrefGet",
        PrincipiaCommand::PrefValueIs { .. } => "PrefValueIs",
        PrincipiaCommand::PrefSet { .. } => "PrefSet",
        PrincipiaCommand::PrefList { .. } => "PrefList",
        PrincipiaCommand::PrefListIs { .. } => "PrefListIs",
        PrincipiaCommand::PrefChanged { .. } => "PrefChanged",
        PrincipiaCommand::PrefError { .. } => "PrefError",
    }
}

fn pref_value_variant_name(v: &PrefValue) -> &'static str {
    match v {
        PrefValue::Str(_) => "Str",
        PrefValue::Int(_) => "Int",
        PrefValue::Float(_) => "Float",
        PrefValue::Bool(_) => "Bool",
    }
}

fn matrix_variant_name(e: &MatrixEvent) -> &'static str {
    match e {
        MatrixEvent::IngestTopology { .. } => "IngestTopology",
        MatrixEvent::GraftTopology { .. } => "GraftTopology",
        MatrixEvent::FocusSector(_) => "FocusSector",
        MatrixEvent::SectorFocused { .. } => "SectorFocused",
        MatrixEvent::NodeSelected(_) => "NodeSelected",
        MatrixEvent::TopologyMutated(_) => "TopologyMutated",
        MatrixEvent::BrowseTo { .. } => "BrowseTo",
        MatrixEvent::DirListed(_) => "DirListed",
        MatrixEvent::FileOp { .. } => "FileOp",
        MatrixEvent::FsOpResult { .. } => "FsOpResult",
    }
}

fn logevent_variant_name(e: &LogEvent) -> &'static str {
    match e {
        LogEvent::LogFilter(_) => "LogFilter",
        LogEvent::LogSource(_) => "LogSource",
        LogEvent::LogPause(_) => "LogPause",
        LogEvent::LogTail { .. } => "LogTail",
    }
}

#[test]
fn completeness_guard_matches_are_exhaustive() {
    // The real guard is at compile time: the matches above have no wildcard
    // arm, so any vocabulary change fails this test crate's build until the
    // KAT set is updated. The assertions below just exercise the functions.
    assert_eq!(smessage_variant_name(&SMessage::Ping), "Ping");
    assert_eq!(
        smessage_variant_name(&SMessage::ContextTelemetry { skeletons: vec![] }),
        "ContextTelemetry"
    );
    assert_eq!(
        principia_variant_name(&PrincipiaCommand::SetSystemRoot(PathBuf::from("/una"))),
        "SetSystemRoot"
    );
    assert_eq!(
        matrix_variant_name(&MatrixEvent::FocusSector("euclase".to_string())),
        "FocusSector"
    );
    assert_eq!(
        logevent_variant_name(&LogEvent::LogPause(true)),
        "LogPause"
    );
}

// --- NON-WIRE EXCEPTION: ContextTelemetry / WeightedSkeleton -----------------
//
// `WeightedSkeleton.content` is `#[serde(skip)]` BY DESIGN: the content rides
// an in-process `Arc<String>` for zero-copy thread transfer, and `Arc` pointers
// are meaningless across address spaces (README: inter-process telemetry is
// deferred to `unafs` shared memory). The honest runtime proof is therefore a
// LOSSY-WIRE assertion, not a serialize-error assertion: serialization
// SUCCEEDS, but the content never crosses the wire in either direction.

/// The frozen (lossy) wire shape: `path` and `score` only — no `content` key.
const CONTEXT_TELEMETRY_GOLDEN: &str =
    r#"{"ContextTelemetry":{"skeletons":[{"path":"/una/ctx/vein.rs","score":0.5}]}}"#;

fn sample_context_telemetry() -> SMessage {
    SMessage::ContextTelemetry {
        skeletons: vec![WeightedSkeleton {
            path: PathBuf::from("/una/ctx/vein.rs"),
            score: 0.5,
            content: Arc::new("fn secret_payload() {}".to_string()),
        }],
    }
}

#[test]
fn context_telemetry_serializes_but_content_never_leaves_the_process() {
    let msg = sample_context_telemetry();
    let json = serde_json::to_string(&msg).expect("serialization succeeds (lossy, by design)");
    assert_eq!(json, CONTEXT_TELEMETRY_GOLDEN, "lossy wire shape drifted");
    assert!(
        !json.contains("content") && !json.contains("secret_payload"),
        "skeleton content leaked onto the wire: {json}"
    );
}

#[test]
fn context_telemetry_round_trip_loses_content_by_design() {
    let back: SMessage =
        serde_json::from_str(CONTEXT_TELEMETRY_GOLDEN).expect("golden must deserialize");
    let SMessage::ContextTelemetry { skeletons } = back else {
        panic!("golden deserialized to the wrong variant");
    };
    assert_eq!(skeletons.len(), 1);
    assert_eq!(skeletons[0].path, PathBuf::from("/una/ctx/vein.rs"));
    assert_eq!(skeletons[0].score, 0.5);
    assert!(
        skeletons[0].content.is_empty(),
        "content must come back Default-empty: the wire cannot carry it"
    );
}

#[test]
fn context_telemetry_ignores_injected_content_field() {
    // A peer cannot smuggle skeleton content in over the wire either: an
    // incoming `content` key is treated as an unknown field and dropped.
    let injected =
        r#"{"ContextTelemetry":{"skeletons":[{"path":"/una/ctx/vein.rs","score":0.5,"content":"smuggled"}]}}"#;
    let back: SMessage =
        serde_json::from_str(injected).expect("unknown `content` field is ignored, not an error");
    let SMessage::ContextTelemetry { skeletons } = back else {
        panic!("injected payload deserialized to the wrong variant");
    };
    assert!(
        skeletons[0].content.is_empty(),
        "injected content must NOT populate the in-process Arc"
    );
}
