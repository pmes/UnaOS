// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// End-to-end proof that the Finder state reaches the BUS: drive the real
// `matrix::ignite` event loop over a `Synapse`, fire Finder request events,
// and assert the browse/result events come back on a subscription. This is
// the honest "reaches the bus" witness — not a direct function call.

use std::sync::Arc;
use std::time::Duration;

use bandy::{MatrixEvent, Origin, SMessage, Synapse};
use bandy::state::{FsOutcome, FsVerb};

fn touch(p: &std::path::Path) {
    std::fs::write(p, b"x").unwrap();
}

/// Receive matrix events until one matches `pick`, or time out.
async fn recv_matrix<T>(
    rx: &mut tokio::sync::broadcast::Receiver<SMessage>,
    pick: impl Fn(&MatrixEvent) -> Option<T>,
) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let msg = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("timed out waiting for matrix event")
            .expect("synapse closed");
        if let SMessage::Matrix(ev) = msg {
            if let Some(v) = pick(&ev) {
                return v;
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn browse_and_file_op_reach_the_bus() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    touch(&root.join("seed.txt"));

    let synapse = Synapse::new();
    // Subscribe BEFORE ignite so we catch everything it fires.
    let mut rx = synapse.subscribe();

    let ignite_synapse = synapse.clone();
    let ignite_root = Arc::new(root.clone());
    let handle = tokio::spawn(async move {
        matrix::ignite(ignite_synapse, ignite_root).await;
    });

    // Give ignite a moment to subscribe.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let who = Origin::LocalUser("peter".to_string());

    // 1. Navigate to root → expect a DirListed carrying seed.txt.
    synapse.fire(SMessage::Matrix(MatrixEvent::BrowseTo {
        principal: who.clone(),
        path: String::new(),
    }));
    let listing = recv_matrix(&mut rx, |ev| match ev {
        MatrixEvent::DirListed(l) => Some(l.clone()),
        _ => None,
    })
    .await;
    assert!(listing.entries.iter().any(|e| e.name == "seed.txt"));

    // 2. NewFolder → expect an FsOpResult Ok, principal preserved, folder real.
    synapse.fire(SMessage::Matrix(MatrixEvent::FileOp {
        principal: who.clone(),
        verb: FsVerb::NewFolder,
        path: String::new(),
        arg: Some("viabus".to_string()),
        confirmed: false,
    }));
    let (p, outcome) = recv_matrix(&mut rx, |ev| match ev {
        MatrixEvent::FsOpResult { principal, verb: FsVerb::NewFolder, outcome, .. } => {
            Some((principal.clone(), outcome.clone()))
        }
        _ => None,
    })
    .await;
    assert_eq!(p, who);
    assert_eq!(outcome, FsOutcome::Ok { path: "viabus".to_string() });
    assert!(root.join("viabus").is_dir());

    handle.abort();
}
