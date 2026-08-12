// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Falsifiable proofs for the Console log viewer (comscan's first capability):
// the bounded scrollback, live tail, pause/resume scroll-lock, the text +
// source filters, and the bus round-trip through `serve`. Each test fails on
// its own if the property it names regresses.

use bandy::state::{LogLine, LogSource};
use bandy::{LogEvent, SMessage, Synapse};
use comscan::LogView;

use std::time::Duration;
use tokio::time::timeout;

// ── Pure-core props (no bus, no runtime) ────────────────────────────────────

/// The ring NEVER grows past its cap; the OLDEST record is the one evicted; and
/// every eviction is counted. Offer cap+16 and assert exactly 16 dropped, the
/// survivors are the newest `cap`, and the head is the record at seq 16.
#[test]
fn scrollback_is_bounded_and_drops_oldest() {
    let cap = 8;
    let mut v = LogView::new(cap);
    let offered = cap + 16;
    for i in 0..offered {
        v.ingest("info", "test", format!("line-{i}"));
    }
    assert_eq!(v.len(), cap, "ring grew past its cap");
    assert_eq!(v.dropped(), 16, "eviction count wrong");

    let snap = v.snapshot();
    assert_eq!(snap.len(), cap);
    // Drop-OLDEST: the survivors are seqs 16..24, in order, present-preserving.
    assert_eq!(snap.first().unwrap().seq, 16, "did not drop the OLDEST");
    assert_eq!(snap.first().unwrap().content, "line-16");
    assert_eq!(snap.last().unwrap().content, format!("line-{}", offered - 1));
}

/// A zero cap is refused (clamped to 1): a scrollback that could not hold the
/// line it just took would loop dropping it.
#[test]
fn zero_cap_is_clamped() {
    let mut v = LogView::new(0);
    assert_eq!(v.cap(), 1);
    v.ingest("info", "test", "a");
    v.ingest("info", "test", "b");
    assert_eq!(v.len(), 1);
    assert_eq!(v.snapshot()[0].content, "b");
    assert_eq!(v.dropped(), 1);
}

/// Live tail: a fresh record appears at the end of the snapshot, and `on_log`
/// hands back a `LogTail` carrying it while not paused.
#[test]
fn live_tail_appends() {
    let mut v = LogView::new(16);
    v.ingest("info", "kernel", "boot");
    let ev = v.on_log("warn", "kernel", "late").expect("unpaused ingest emits a tail");
    match ev {
        LogEvent::LogTail { lines, paused, .. } => {
            assert!(!paused);
            assert_eq!(lines.len(), 2);
            assert_eq!(lines.last().unwrap().content, "late");
        }
        other => panic!("expected LogTail, got {other:?}"),
    }
}

/// Pause is scroll-lock: paused ingest still GROWS the ring but emits NOTHING;
/// resume re-emits and the frozen records are all present.
#[test]
fn pause_freezes_tail_but_ring_keeps_filling() {
    let mut v = LogView::new(16);
    v.set_paused(true);
    assert!(v.on_log("info", "a", "one").is_none(), "paused tail must not emit");
    assert!(v.on_log("info", "a", "two").is_none());
    assert_eq!(v.len(), 2, "paused ingest still fills the ring");

    // Resume via the command path — it must answer immediately with everything.
    let ev = v.apply(&LogEvent::LogPause(false)).expect("resume emits a tail");
    match ev {
        LogEvent::LogTail { lines, paused, .. } => {
            assert!(!paused);
            assert_eq!(lines.len(), 2);
        }
        other => panic!("expected LogTail, got {other:?}"),
    }
    // And the next live record now flows again.
    assert!(v.on_log("info", "a", "three").is_some());
}

/// Text filter: a match shows ONLY matches, a no-match shows EMPTY, clearing
/// restores all. Case-insensitive, and it spans content/source/level.
#[test]
fn text_filter_matches_nomatch_clear() {
    let mut v = LogView::new(32);
    v.ingest("info", "net", "link up");
    v.ingest("error", "net", "link DOWN");
    v.ingest("info", "gpu", "frame drawn");

    v.set_filter("link");
    assert_eq!(v.snapshot().len(), 2, "filter should keep both 'link' lines");

    v.set_filter("DoWn"); // case-insensitive
    let s = v.snapshot();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].content, "link DOWN");

    v.set_filter("nonexistent-zzz");
    assert!(v.snapshot().is_empty(), "a no-match filter must show empty");

    // Filter also spans the source and level fields.
    v.set_filter("gpu");
    assert_eq!(v.snapshot().len(), 1);
    v.set_filter("error");
    assert_eq!(v.snapshot().len(), 1);

    v.set_filter("");
    assert_eq!(v.snapshot().len(), 3, "clearing the filter restores every line");
}

/// Source facet: selecting a subsystem shows only that source; `All` restores.
#[test]
fn source_select_facets_by_subsystem() {
    let mut v = LogView::new(32);
    v.ingest("info", "kernel", "a");
    v.ingest("info", "net", "b");
    v.ingest("info", "kernel", "c");

    v.set_source(LogSource::Subsystem("kernel".into()));
    let s = v.snapshot();
    assert_eq!(s.len(), 2);
    assert!(s.iter().all(|l| l.source == "kernel"));

    // Source AND text filter compose.
    v.set_filter("c");
    assert_eq!(v.snapshot().len(), 1);

    v.set_filter("");
    v.set_source(LogSource::All);
    assert_eq!(v.snapshot().len(), 3);
}

/// `view_state` is the Console-pane snapshot the vessel wraps in
/// `ViewEntity::Console`. It must agree with the live tail: the SAME filtered
/// lines, and the same eviction/pause/filter/source state — so a freshly
/// summoned pane and a live subscriber never disagree about what the log says.
#[test]
fn view_state_snapshot_agrees_with_the_live_tail() {
    let cap = 4;
    let mut v = LogView::new(cap);
    for i in 0..(cap + 3) {
        v.ingest("info", if i % 2 == 0 { "kernel" } else { "net" }, format!("line-{i}"));
    }
    v.set_source(LogSource::Subsystem("kernel".into()));
    v.set_paused(true);

    let vs = v.view_state();
    // The lines match the filtered snapshot exactly.
    assert_eq!(vs.lines, v.snapshot());
    assert!(vs.lines.iter().all(|l| l.source == "kernel"));
    // Bounded-ring honesty and the active query ride along.
    assert_eq!(vs.dropped, v.dropped());
    assert!(vs.paused);
    assert_eq!(vs.source, LogSource::Subsystem("kernel".into()));
    assert_eq!(vs.filter, "");
}

// ── Bus round-trip through `serve` ──────────────────────────────────────────

/// Pull LogTails off `rx` until one arrives whose lines satisfy `want`, or time
/// out. Ignores non-`Logs` traffic and command echoes.
async fn next_tail_where(
    rx: &mut tokio::sync::broadcast::Receiver<SMessage>,
    want: impl Fn(&[LogLine]) -> bool,
) -> Vec<LogLine> {
    let deadline = Duration::from_secs(2);
    timeout(deadline, async {
        loop {
            if let Ok(SMessage::Logs(LogEvent::LogTail { lines, .. })) = rx.recv().await {
                if want(&lines) {
                    return lines;
                }
            }
        }
    })
    .await
    .expect("timed out waiting for the expected LogTail")
}

/// The whole contract over a real Synapse: fire `Log` producer messages, then
/// fire a `LogFilter` command, and assert a filtered `LogTail` comes back —
/// only the matching line, published by the handler onto the same bus.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bus_round_trip_filter_yields_filtered_tail() {
    let synapse = Synapse::new();

    // Subscribe the observer BEFORE anything is fired, and hand `serve` its own
    // receiver so there is no gap where traffic could be lost.
    let mut observer = synapse.subscribe();
    let handler_rx = synapse.subscribe();
    let bus = synapse.clone();
    tokio::spawn(async move {
        comscan::serve(bus, handler_rx, 128).await;
    });

    // Three producer records land in the ring.
    synapse.fire(SMessage::Log {
        level: "info".into(),
        source: "net".into(),
        content: "link up".into(),
    });
    synapse.fire(SMessage::Log {
        level: "error".into(),
        source: "gpu".into(),
        content: "reset needed".into(),
    });
    synapse.fire(SMessage::Log {
        level: "info".into(),
        source: "net".into(),
        content: "link down".into(),
    });

    // The unfiltered tail eventually shows all three.
    let all = next_tail_where(&mut observer, |l| l.len() == 3).await;
    assert_eq!(all.len(), 3);

    // Now a filter COMMAND on the bus; the handler must answer with a tail that
    // holds ONLY the "gpu" record.
    synapse.fire(SMessage::Logs(LogEvent::LogFilter("gpu".into())));
    let filtered = next_tail_where(&mut observer, |l| {
        l.len() == 1 && l[0].source == "gpu"
    })
    .await;
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].content, "reset needed");
}

/// The handler must never react to its OWN `LogTail` output — otherwise the
/// published tail would echo on the broadcast bus and re-trigger the handler
/// forever. `apply` answers `None` for a `LogTail`, which is exactly what the
/// `serve` loop relies on (plus its `!matches!(.., LogTail)` guard). Proven
/// deterministically at the core, not by waiting on the absence of a message.
#[test]
fn handler_does_not_react_to_its_own_tail() {
    let mut v = LogView::new(16);
    v.ingest("info", "a", "one");
    let echo = LogEvent::LogTail {
        lines: vec![LogLine { seq: 99, level: "x".into(), source: "x".into(), content: "echo".into() }],
        dropped: 0,
        paused: false,
    };
    assert!(v.apply(&echo).is_none(), "a LogTail must produce no reaction");
    // And the ring is untouched by the echo.
    assert_eq!(v.len(), 1);
}
