use core::f32::consts::PI;

/// Converts degrees to radians.
#[inline]
pub fn to_radians(degrees: f32) -> f32 {
    degrees * (PI / 180.0)
}

/// Converts radians to degrees.
#[inline]
pub fn to_degrees(radians: f32) -> f32 {
    radians * (180.0 / PI)
}
