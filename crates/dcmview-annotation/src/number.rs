//! How the model writes numbers.
//!
//! Every floating-point member of the model is written the same way, so the
//! bytes a Rust process writes match what a JavaScript client writes for the
//! same value: a whole number has no fraction (`340`, `30`, `0`), never
//! `340.0` and never `-0.0`.

use serde::{Serialize, Serializer};

/// Serializes a number: without a fraction when it is a whole number of
/// magnitude below 2^53 (`30`, not `30.0`; `0`, not `-0.0`), otherwise as
/// serde_json writes an `f64` (the shortest decimal that reads back to the
/// same value). A value that is not finite is a serialization error. The
/// value is not rounded; coordinates are quantized by
/// [`serialize_coordinate`] before they reach this rule.
pub(crate) fn serialize_number<S: Serializer>(
    value: &f64,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if !value.is_finite() {
        return Err(serde::ser::Error::custom("A number must be finite."));
    }
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f64(*value)
    }
}

/// [`serialize_number`] for an optional member: `None` is `null`.
pub(crate) fn serialize_optional_number<S: Serializer>(
    value: &Option<f64>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    struct Number(f64);

    impl Serialize for Number {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serialize_number(&self.0, serializer)
        }
    }

    value.map(Number).serialize(serializer)
}

/// Serializes a geometry number: [`crate::geometry::quantize`]d, then written
/// as [`serialize_number`] writes it (`340`, not `340.0`; `12.346` for
/// `12.3456`; `0` for `-0.0004`). A quantized coordinate inside an image has
/// at most three decimals. A value that is not finite is a serialization
/// error.
pub(crate) fn serialize_coordinate<S: Serializer>(
    value: &f64,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serialize_number(&crate::geometry::quantize(*value), serializer)
}
