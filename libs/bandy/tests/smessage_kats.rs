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
use bandy::signals::{MatrixEvent, PrefValue, PrincipiaCommand, SMessage};
use bandy::state::DispatchRecord;
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
    SMessage::StorageQuery { receipt_id: 7, embedding: vec![0.5, 0.25] },
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
kat!(
    kat_storage_save,
    SMessage::StorageSave {
        receipt_id: 8,
        sender: "vein".to_string(),
        content: "engram body".to_string(),
        timestamp: "2026-07-13T00:00:00Z".to_string(),
        embedding: vec![0.0],
        memory_type: "engram".to_string(),
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
    }),
    r#"{"Principia":{"PrefChanged":{"ns":"stria","key":"muted","value":{"Bool":true}}}}"#
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
        SMessage::BrowserUrlChanged(_) => "BrowserUrlChanged",
        SMessage::BrowserTitleChanged(_) => "BrowserTitleChanged",
        SMessage::BrowserFaviconChanged { .. } => "BrowserFaviconChanged",
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
