//! Float to integer conversions that the geometry, the icons and the benches share.

/// A rounded pixel size as `u32`. The float cast saturates and a negative value gives 0, which
/// the callers clamp to at least one pixel.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a saturating cast of a rounded, non-negative pixel size"
)]
pub(crate) const fn round_to_u32(value: f64) -> u32 {
    value.round() as u32
}

/// A rounded cell count as `u16`. The float cast saturates and a negative value gives 0.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a saturating cast of a rounded, non-negative cell count"
)]
pub(crate) const fn round_to_u16(value: f64) -> u16 {
    value.round() as u16
}
