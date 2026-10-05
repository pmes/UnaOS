// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Facet on a live Synapse: every request answered by receipt, every failure an ImageError.

#![cfg(feature = "chicken-wire-image")]

use bandy::signals::{FacetCommand, FacetEdit, FacetFormat, FacetView, FacetZoom};
use bandy::{Origin, SMessage, Synapse};
use std::time::Duration;

async fn answer(rx: &mut tokio::sync::broadcast::Receiver<SMessage>, receipt: u64) -> FacetCommand {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(SMessage::Facet(c)) = rx.recv().await {
                if !c.is_request() && c.receipt_id() == Some(receipt) {
                    return c;
                }
            }
        }
    })
    .await
    .expect("Facet answered within 10 s")
}

#[tokio::test(flavor = "multi_thread")]
async fn open_edit_render_export_over_the_bus() {
    let synapse = Synapse::new();
    let served = synapse.subscribe();
    tokio::spawn(facet::serve(synapse.clone(), served, facet::Facet::new()));
    let mut rx = synapse.subscribe();
    let card = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/eyes/suites/facet/fixtures/card.png");
    let me = Origin::LocalUser("test".into());

    synapse.fire(SMessage::Facet(FacetCommand::ImageOpen { receipt_id: 1, principal: me.clone(), path: card.into() }));
    let FacetCommand::ImageOpened { handle, info, .. } = answer(&mut rx, 1).await else { panic!("open failed") };
    assert_eq!((info.format.as_str(), info.width, info.height), ("png", 48, 32));

    synapse.fire(SMessage::Facet(FacetCommand::ImageEdit { receipt_id: 2, handle, edit: FacetEdit::Rotate { quarter_turns: 1 } }));
    let FacetCommand::ImageInfoIs { info, .. } = answer(&mut rx, 2).await else { panic!("edit failed") };
    assert_eq!((info.width, info.height, info.edits), (32, 48, 1));

    let view = FacetView { zoom: FacetZoom::Fit, ..FacetView::default() };
    synapse.fire(SMessage::Facet(FacetCommand::ImageRender { receipt_id: 3, handle, width: 64, height: 64, view }));
    let FacetCommand::ImageRendered { width, height, rgba, .. } = answer(&mut rx, 3).await else { panic!("render failed") };
    assert_eq!((width, height, rgba.len()), (64, 64, 64 * 64 * 4));
    assert_eq!(&rgba[..3], &facet::view::FIELD, "fit keeps 1:1 and centres: the corner is the field");

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("x.png").to_string_lossy().into_owned();
    synapse.fire(SMessage::Facet(FacetCommand::ImageExport { receipt_id: 4, handle, path: out.clone(), format: FacetFormat::Png, overwrite: false }));
    let FacetCommand::ImageExported { bytes, .. } = answer(&mut rx, 4).await else { panic!("export failed") };
    assert!(bytes > 0 && std::path::Path::new(&out).exists());

    // Failures are answered, by receipt.
    synapse.fire(SMessage::Facet(FacetCommand::ImageOpen { receipt_id: 5, principal: me, path: "/nonexistent.png".into() }));
    assert!(matches!(answer(&mut rx, 5).await, FacetCommand::ImageError { handle: None, .. }));
    synapse.fire(SMessage::Facet(FacetCommand::ImageRender { receipt_id: 6, handle, width: 5000, height: 1, view: FacetView::default() }));
    assert!(matches!(answer(&mut rx, 6).await, FacetCommand::ImageError { handle: Some(_), .. }));
    synapse.fire(SMessage::Facet(FacetCommand::ImageClose { handle }));
    synapse.fire(SMessage::Facet(FacetCommand::ImageInfo { receipt_id: 7, handle }));
    assert!(matches!(answer(&mut rx, 7).await, FacetCommand::ImageError { .. }), "a closed handle is gone");
}
