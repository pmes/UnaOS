//! The one tolerance policy (BP-vug-2) and the named degenerate-input refusal.

/// A tolerance: `lin` in millimetres, `ang` in radians. Every core takes one as an argument; no core carries an
/// epsilon of its own.
///
/// [`Tol::DEFAULT`] is `lin = 1e-6 mm` (one nanometre) and `ang = 1e-9 rad`. The reasoning:
/// * 1 nm is about a thousand times finer than the finest step of the machines Vug drives (a good CNC or printer
///   resolves ~1 um), so two points the default calls equal are equal on any machine.
/// * f64 holds it with room to spare: at a 1 m part (1e3 mm) one ulp is 1e3 * 2.2e-16 = 2.2e-13 mm, seven decades
///   under `lin`, so the tolerance is a modelling decision, never the arithmetic's noise.
/// * `ang = lin / 1000 mm`: an angular error of `ang` at a 1 m lever arm moves a point by exactly `lin`, so the two
///   tolerances agree across the working envelope.
/// * Against Open CASCADE: `Precision::Confusion()` is 1e-7 and `Precision::Angular()` is 1e-12 rad, in the
///   document's model units (OCCT is unit-less, usually read as mm). Vug fixes the unit to the millimetre so a
///   tolerance never changes meaning with a document's unit, and sits one decade looser than OCCT's confusion,
///   which is still far finer than manufacturing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tol {
    /// Linear tolerance in millimetres.
    pub lin: f64,
    /// Angular tolerance in radians.
    pub ang: f64,
}

impl Tol {
    /// 1e-6 mm and 1e-9 rad (see the type's documentation).
    pub const DEFAULT: Tol = Tol { lin: 1e-6, ang: 1e-9 };

    /// A tolerance; refuses a non-finite or non-positive value.
    pub fn new(lin: f64, ang: f64) -> Result<Tol, Degenerate> {
        if !(lin.is_finite() && ang.is_finite()) {
            return Err(Degenerate::NonFinite);
        }
        if lin <= 0.0 || ang <= 0.0 {
            return Err(Degenerate::BadTolerance);
        }
        Ok(Tol { lin, ang })
    }
}

impl Default for Tol {
    fn default() -> Self {
        Tol::DEFAULT
    }
}

/// Why an input cannot be computed on: the named refusal every constructor and query returns instead of a panic
/// or a silent garbage value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Degenerate {
    /// A direction or axis of zero (or sub-tolerance) length.
    ZeroVector,
    /// A transform scale of zero, negative or non-finite.
    ZeroScale,
    /// A matrix with no inverse.
    Singular,
    /// A circle, arc or sphere of zero or negative radius.
    ZeroRadius,
    /// Points that should span a plane or a triangle are collinear.
    Collinear,
    /// A triangle or polygon of zero area.
    ZeroArea,
    /// Fewer points than the operation needs.
    TooFewPoints,
    /// A NaN or infinite input.
    NonFinite,
    /// A tolerance that is zero or negative.
    BadTolerance,
    /// An interval whose low end is above its high end.
    EmptyInterval,
}
