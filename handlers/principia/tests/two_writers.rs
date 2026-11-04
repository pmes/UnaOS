// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! PRINCIPIA-TWO-WRITERS (SR635): two Principias over ONE home, each on its own Synapse (lumen's and una's
//! processes), hear each other's writes through the serve loop's watch on the settings folder.

use std::time::Duration;

use bandy::{PrefValue, PrincipiaCommand, SMessage, Synapse};
use tokio::sync::broadcast::Receiver;

/// Mount one Principia on a fresh bus the SR628 way (its receiver taken first); answer the bus and an observer.
fn mount(home: &std::path::Path) -> (Synapse, Receiver<SMessage>) {
    let syn = Synapse::new();
    let obs = syn.subscribe();
    let prx = syn.subscribe();
    tokio::spawn(principia::serve(syn.clone(), prx, principia::Principia::with_home(home)));
    (syn, obs)
}

/// The next `PrefChanged` for `ns`/`key` on `obs` within `within`, or `None`.
async fn changed(obs: &mut Receiver<SMessage>, ns: &str, key: &str, within: Duration) -> Option<Option<PrefValue>> {
    tokio::time::timeout(within, async {
        loop {
            if let Ok(SMessage::Principia(PrincipiaCommand::PrefChanged { ns: n, key: k, value, .. })) = obs.recv().await {
                if n == ns && k == key {
                    return value;
                }
            }
        }
    })
    .await
    .ok()
}

/// PRINCIPIA-TWO-WRITERS (SR635): A sets `vug.photo.camera_mm`; B (another bus, the same ~/settings) posts it within
/// the 1 s cadence and its PrefGet answers it. A unsets it; B posts the unset form (`value: None`). B sets
/// `vug.photo.principal_px`; B's bus carries exactly ONE PrefChanged for it (its reply: the watch does not re-post its
/// own write) and A posts it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_principias_over_one_home_hear_each_others_writes() {
    let home = tempfile::tempdir().unwrap();
    let (a, mut obs_a) = mount(home.path());
    let (b, mut obs_b) = mount(home.path());
    let wait = Duration::from_secs(5);

    a.fire(SMessage::Principia(PrincipiaCommand::PrefSet {
        ns: "vug".into(),
        key: "photo.camera_mm".into(),
        value: PrefValue::Float(400.0),
    }));
    assert_eq!(changed(&mut obs_a, "vug", "photo.camera_mm", wait).await, Some(Some(PrefValue::Float(400.0))), "A answers its set");
    assert_eq!(
        changed(&mut obs_b, "vug", "photo.camera_mm", wait).await,
        Some(Some(PrefValue::Float(400.0))),
        "B hears A's write within the cadence"
    );

    b.fire(SMessage::Principia(PrincipiaCommand::PrefGet { ns: "vug".into(), key: "photo.camera_mm".into() }));
    let got = tokio::time::timeout(wait, async {
        loop {
            if let Ok(SMessage::Principia(PrincipiaCommand::PrefValueIs { ns, key, value })) = obs_b.recv().await {
                if ns == "vug" && key == "photo.camera_mm" {
                    return value;
                }
            }
        }
    })
    .await
    .expect("B answers its PrefGet");
    assert_eq!(got, Some(PrefValue::Float(400.0)), "B's store holds A's value");

    a.fire(SMessage::Principia(PrincipiaCommand::PrefUnset { ns: "vug".into(), key: "photo.camera_mm".into() }));
    assert_eq!(changed(&mut obs_a, "vug", "photo.camera_mm", wait).await, Some(None), "A answers its unset");
    assert_eq!(changed(&mut obs_b, "vug", "photo.camera_mm", wait).await, Some(None), "B posts the unset form for a key gone");

    b.fire(SMessage::Principia(PrincipiaCommand::PrefSet {
        ns: "vug".into(),
        key: "photo.principal_px".into(),
        value: PrefValue::Str("250,320".into()),
    }));
    let px = Some(Some(PrefValue::Str("250,320".into())));
    assert_eq!(changed(&mut obs_b, "vug", "photo.principal_px", wait).await, px, "B answers its set");
    assert_eq!(
        changed(&mut obs_b, "vug", "photo.principal_px", Duration::from_secs(3)).await,
        None,
        "B's watch never re-posts its own write"
    );
    assert_eq!(changed(&mut obs_a, "vug", "photo.principal_px", wait).await, px, "A hears B's write");
}
