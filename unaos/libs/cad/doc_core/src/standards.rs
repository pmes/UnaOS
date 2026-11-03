// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, 3D) — shared-core: doc_core::standards, the standard hole sizes a measured hole is read as.
//!
//! VUGINTENTSCAN (LEDGER SR508, VUG-INTENT-STANDARDS): ISO metric screw threads as data — the coarse pitch of ISO 262
//! (ISO general-purpose metric screw threads, selected sizes), the clearance holes of ISO 273 (ISO 20273 since 2017:
//! fine / medium / coarse series), and the tap drill `d - P` (the ISO 2306 rule for coarse threads). The medium series
//! is the general-purpose one, so [`nearest_standard`] prefers it within the tolerance (a measured 6.4 at sigma 0.3 is
//! the M6 medium clearance 6.6; at sigma 0.05 it is the M6 fine clearance 6.4).

/// The hole series.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Fine,
    Medium,
    Coarse,
    TapDrill,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Fine => "fine",
            Class::Medium => "medium",
            Class::Coarse => "coarse",
            Class::TapDrill => "tap-drill",
        }
    }
}

/// One thread: nominal diameter, ISO 262 coarse pitch, ISO 273 clearance holes (mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thread {
    pub name: &'static str,
    pub nominal_mm: f64,
    pub pitch_mm: f64,
    pub fine_mm: f64,
    pub medium_mm: f64,
    pub coarse_mm: f64,
}

impl Thread {
    /// The hole of `class` for this thread (the tap drill is `d - P`).
    pub fn hole(&self, class: Class) -> f64 {
        match class {
            Class::Fine => self.fine_mm,
            Class::Medium => self.medium_mm,
            Class::Coarse => self.coarse_mm,
            Class::TapDrill => self.nominal_mm - self.pitch_mm,
        }
    }
}

/// A standard hole: which thread, its pitch, the series and the hole diameter (mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Standard {
    pub thread: &'static str,
    pub nominal_mm: f64,
    pub pitch_mm: f64,
    pub class: Class,
    pub hole_mm: f64,
}

const fn t(name: &'static str, nominal_mm: f64, pitch_mm: f64, fine_mm: f64, medium_mm: f64, coarse_mm: f64) -> Thread {
    Thread { name, nominal_mm, pitch_mm, fine_mm, medium_mm, coarse_mm }
}

/// ISO 262 coarse pitch and ISO 273 clearance holes, M1.6 to M30 (the preferred and second-choice sizes).
pub const METRIC: [Thread; 15] = [
    t("M1.6", 1.6, 0.35, 1.7, 1.8, 2.0),
    t("M2", 2.0, 0.4, 2.2, 2.4, 2.6),
    t("M2.5", 2.5, 0.45, 2.7, 2.9, 3.1),
    t("M3", 3.0, 0.5, 3.2, 3.4, 3.6),
    t("M4", 4.0, 0.7, 4.3, 4.5, 4.8),
    t("M5", 5.0, 0.8, 5.3, 5.5, 5.8),
    t("M6", 6.0, 1.0, 6.4, 6.6, 7.0),
    t("M8", 8.0, 1.25, 8.4, 9.0, 10.0),
    t("M10", 10.0, 1.5, 10.5, 11.0, 12.0),
    t("M12", 12.0, 1.75, 13.0, 13.5, 14.5),
    t("M14", 14.0, 2.0, 15.0, 15.5, 16.5),
    t("M16", 16.0, 2.0, 17.0, 17.5, 18.5),
    t("M20", 20.0, 2.5, 21.0, 22.0, 24.0),
    t("M24", 24.0, 3.0, 25.0, 26.0, 28.0),
    t("M30", 30.0, 3.5, 31.0, 33.0, 35.0),
];

/// A stated size equals a table value when it is within this (mm): a lookup, not a measurement band.
const EXACT_MM: f64 = 1e-9;

const CLEARANCE: [Class; 3] = [Class::Medium, Class::Fine, Class::Coarse];
const OTHERS: [Class; 3] = [Class::Fine, Class::Coarse, Class::TapDrill];

fn standard(th: &Thread, class: Class) -> Standard {
    Standard { thread: th.name, nominal_mm: th.nominal_mm, pitch_mm: th.pitch_mm, class, hole_mm: th.hole(class) }
}

/// The thread and series a clearance hole of exactly `diameter_mm` is (medium first).
pub fn clearance_for(diameter_mm: f64) -> Option<(&'static str, Class)> {
    for class in CLEARANCE {
        if let Some(th) = METRIC.iter().find(|th| (th.hole(class) - diameter_mm).abs() <= EXACT_MM) {
            return Some((th.name, class));
        }
    }
    None
}

/// The nearest hole of `classes` within `tol` of `d`.
fn nearest_of(d: f64, tol: f64, classes: &[Class]) -> Option<Standard> {
    let mut best: Option<(f64, Standard)> = None;
    for th in &METRIC {
        for &class in classes {
            let e = (th.hole(class) - d).abs();
            if e <= tol && best.as_ref().is_none_or(|(b, _)| e < *b) {
                best = Some((e, standard(th, class)));
            }
        }
    }
    best.map(|(_, s)| s)
}

/// The standard hole a measured diameter `d` is read as: the nearest MEDIUM clearance within `tol`, else the nearest
/// fine / coarse clearance or tap drill within `tol`; None past every one (or a `tol` that is not a finite number >= 0).
pub fn nearest_standard(d: f64, tol: f64) -> Option<Standard> {
    if !(d.is_finite() && tol.is_finite() && tol >= 0.0) {
        return None;
    }
    nearest_of(d, tol, &[Class::Medium]).or_else(|| nearest_of(d, tol, &OTHERS))
}
