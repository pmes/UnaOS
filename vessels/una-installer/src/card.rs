// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Amber Bytes (Disks) — the installer's card face, a STUB held by INSTALLERVESSEL (LEDGER SR413) until
//! INSTALLERCARD (SR414) lands its file in this path. The seam is these four items, exactly: `Card` with
//! `Card::probe()`, `face()`, `on_input(&mut Card, &str) -> bool` and `poll(&mut Card)`.

use bandy::SMessage;
use quartzite::tetra::TetraNode;

/// The card face's state, owned by the vessel's serve task.
pub struct Card;

impl Card {
    /// Read the host's writable cards (the stub reads nothing).
    pub fn probe() -> Card {
        Card
    }
}

/// The Install-UnaOS pane.
pub fn face() -> TetraNode {
    TetraNode::Button {
        id: "card.owed".into(),
        label: "Install UnaOS onto a machine or card — owed (INSTALLER-CARD)".into(),
        action: SMessage::ConsoleInput("card help".into()),
    }
}

/// One typed or button line; true when it was the card face's (`card ...`).
pub fn on_input(_card: &mut Card, line: &str) -> bool {
    if !line.starts_with("card") {
        return false;
    }
    println!("[installer] card verb={line} why=owed");
    true
}

/// Drain the card write's progress into `Card` (the stub has no write).
pub fn poll(_card: &mut Card) {}
