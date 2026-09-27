use crate::api::contracts::{WindowMode, WindowPreset};
use crate::types::{DicomLut, NativePixelDataKind, ResolvedWindow};
use dicom_dictionary_std::tags;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PixelPaddingRange {
    low: f64,
    high: f64,
}

impl PixelPaddingRange {
    pub(crate) fn new(value: f64, range_limit: Option<f64>) -> Self {
        let limit = range_limit.unwrap_or(value);
        Self {
            low: value.min(limit),
            high: value.max(limit),
        }
    }

    pub(crate) fn bounds(self) -> (f64, f64) {
        (self.low, self.high)
    }

    pub(crate) fn contains(self, stored_value: f64) -> bool {
        stored_value >= self.low && stored_value <= self.high
    }

    pub(crate) fn mask(self, stored_samples: &[f64]) -> Vec<bool> {
        stored_samples
            .iter()
            .map(|sample| self.contains(*sample))
            .collect()
    }
}

pub(crate) fn read_pixel_padding_range(
    object: &dicom_object::DefaultDicomObject,
    kind: NativePixelDataKind,
) -> Option<PixelPaddingRange> {
    let read = |tag| object.get(tag)?.to_float64().ok();
    let (value_tag, limit_tag) = match kind {
        NativePixelDataKind::Integer => {
            (tags::PIXEL_PADDING_VALUE, tags::PIXEL_PADDING_RANGE_LIMIT)
        }
        NativePixelDataKind::Float32 => (
            tags::FLOAT_PIXEL_PADDING_VALUE,
            tags::FLOAT_PIXEL_PADDING_RANGE_LIMIT,
        ),
        NativePixelDataKind::Float64 => (
            tags::DOUBLE_FLOAT_PIXEL_PADDING_VALUE,
            tags::DOUBLE_FLOAT_PIXEL_PADDING_RANGE_LIMIT,
        ),
    };
    Some(PixelPaddingRange::new(read(value_tag)?, read(limit_tag)))
}

pub(crate) fn exclude_padding_samples(samples: &[f64], padding_mask: &[bool]) -> Vec<f64> {
    samples
        .iter()
        .zip(padding_mask)
        .filter_map(|(sample, is_padding)| (!is_padding).then_some(*sample))
        .collect()
}

pub(crate) fn apply_padding_background(windowed: &mut [u8], padding_mask: &[bool]) {
    for (sample, is_padding) in windowed.iter_mut().zip(padding_mask) {
        if *is_padding {
            *sample = 0;
        }
    }
}

/// The Modality LUT, or else the rescale, applied to one stored value.
pub(crate) fn modality_value(
    stored: f64,
    modality_lut: Option<&DicomLut>,
    rescale_slope: f64,
    rescale_intercept: f64,
) -> f64 {
    match modality_lut.filter(|lut| !lut.entries.is_empty()) {
        Some(lut) => f64::from(lut_entry(lut, stored)),
        None => stored * rescale_slope + rescale_intercept,
    }
}

fn lut_entry(lut: &DicomLut, value: f64) -> u16 {
    let last = lut.entries.len().saturating_sub(1) as i64;
    let offset = (value as i64) - i64::from(lut.first_mapped_value);
    lut.entries[offset.clamp(0, last) as usize]
}

pub(crate) fn apply_modality_transform(
    samples: &[f64],
    modality_lut: Option<&DicomLut>,
    rescale_slope: f64,
    rescale_intercept: f64,
) -> Vec<f64> {
    samples
        .iter()
        .map(|value| modality_value(*value, modality_lut, rescale_slope, rescale_intercept))
        .collect()
}

/// The VOI LUT that presents the frame instead of a window: only in default
/// mode, with no window requested or stored.
pub(crate) fn selected_voi_lut(
    mode: WindowMode,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    default_window: Option<WindowPreset>,
    voi_lut: Option<&DicomLut>,
) -> Option<&DicomLut> {
    if mode != WindowMode::Default
        || requested_wc.is_some()
        || requested_ww.is_some()
        || default_window.is_some()
    {
        return None;
    }
    voi_lut.filter(|lut| !lut.entries.is_empty() && matches!(lut.bits_per_entry, 8 | 16))
}

/// A selected VOI LUT applied to one post-modality value, scaled to 8 bits.
pub(crate) fn voi_lut_value(lut: &DicomLut, value: f64) -> u8 {
    let output_max = (1_u32 << lut.bits_per_entry) - 1;
    let output = u32::from(lut_entry(lut, value));
    ((output * 255 + output_max / 2) / output_max) as u8
}

pub(crate) fn apply_voi_lut_if_selected(
    mode: WindowMode,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    default_window: Option<WindowPreset>,
    voi_lut: Option<&DicomLut>,
    samples: &[f64],
) -> Option<Vec<u8>> {
    let lut = selected_voi_lut(mode, requested_wc, requested_ww, default_window, voi_lut)?;
    Some(
        samples
            .iter()
            .map(|value| voi_lut_value(lut, *value))
            .collect(),
    )
}

pub fn resolve_window(
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    default_window: Option<WindowPreset>,
    samples: &[f64],
) -> Option<ResolvedWindow> {
    requested_or_stored_window(requested_wc, requested_ww, default_window)
        .or_else(|| percentile_window(samples))
}

fn requested_or_stored_window(
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    default_window: Option<WindowPreset>,
) -> Option<ResolvedWindow> {
    if let (Some(center), Some(width)) = (requested_wc, requested_ww) {
        return Some(ResolvedWindow { center, width });
    }
    default_window.map(|window| ResolvedWindow {
        center: window.center,
        width: window.width,
    })
}

/// How often each post-modality value occurs in a frame: the order statistics
/// the automatic windows need, from a histogram of stored values rather than a
/// sorted copy of every sample.
pub(crate) struct ValueDistribution {
    /// Distinct values (a value may repeat when a Modality LUT maps several
    /// stored values to it) and their counts, ascending.
    entries: Vec<(f64, u64)>,
    total: u64,
}

impl ValueDistribution {
    pub(crate) fn new(mut entries: Vec<(f64, u64)>) -> Self {
        entries.retain(|(_, count)| *count > 0);
        entries.sort_by(|left, right| left.0.total_cmp(&right.0));
        let total = entries.iter().map(|(_, count)| count).sum();
        Self { entries, total }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// The value at `index` of the frame's samples in ascending order.
    fn value_at(&self, index: u64) -> f64 {
        let mut seen = 0;
        for (value, count) in &self.entries {
            seen += count;
            if index < seen {
                return *value;
            }
        }
        self.entries.last().map_or(0.0, |(value, _)| *value)
    }

    /// The window `percentile_window` computes over the same samples.
    fn percentile_window(&self) -> Option<ResolvedWindow> {
        if self.is_empty() {
            return None;
        }
        let total = self.total as f64;
        let low = self.value_at(((total * 0.01).floor() as u64).min(self.total - 1));
        let high = self.value_at(((total * 0.99).ceil() as u64).min(self.total - 1));
        Some(span_window(low, high))
    }

    fn full_dynamic_window(&self) -> Option<ResolvedWindow> {
        let (low, _) = self.entries.first()?;
        let (high, _) = self.entries.last()?;
        Some(span_window(*low, *high))
    }
}

fn span_window(low: f64, high: f64) -> ResolvedWindow {
    let width = (high - low).max(1.0);
    ResolvedWindow {
        center: low + width / 2.0,
        width,
    }
}

/// `resolve_window_with_mode` over a frame's value distribution.
pub(crate) fn resolve_window_from_distribution(
    mode: WindowMode,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    default_window: Option<WindowPreset>,
    distribution: &ValueDistribution,
) -> Option<ResolvedWindow> {
    match mode {
        WindowMode::Default => {
            requested_or_stored_window(requested_wc, requested_ww, default_window)
                .or_else(|| distribution.percentile_window())
        }
        WindowMode::FullDynamic => distribution.full_dynamic_window(),
    }
}

/// Computes window from the true min/max of frame samples (full dynamic range).
/// Ignores explicit wc/ww params and DICOM default_window tags.
fn full_dynamic_window(samples: &[f64]) -> Option<ResolvedWindow> {
    if samples.is_empty() {
        return None;
    }
    let min = samples.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = samples.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    Some(span_window(min, max))
}

/// Resolves window using the specified mode.
/// Default mode: explicit params -> DICOM default_window -> 1st/99th percentile.
/// FullDynamic mode: true min/max of current frame samples, ignores all other inputs.
pub fn resolve_window_with_mode(
    mode: WindowMode,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    default_window: Option<WindowPreset>,
    samples: &[f64],
) -> Option<ResolvedWindow> {
    match mode {
        WindowMode::Default => resolve_window(requested_wc, requested_ww, default_window, samples),
        WindowMode::FullDynamic => full_dynamic_window(samples),
    }
}

fn percentile_window(samples: &[f64]) -> Option<ResolvedWindow> {
    if samples.is_empty() {
        return None;
    }

    let mut values = samples.to_vec();
    values.sort_by(f64::total_cmp);
    let p1_idx = ((values.len() as f64) * 0.01).floor() as usize;
    let p99_idx =
        (((values.len() as f64) * 0.99).ceil() as usize).min(values.len().saturating_sub(1));
    let low = values[p1_idx.min(values.len().saturating_sub(1))];
    let high = values[p99_idx];
    Some(span_window(low, high))
}

pub fn apply_window(samples: &[f64], center: f64, width: f64) -> Vec<u8> {
    samples
        .iter()
        .map(|sample| window_value(*sample, center, width))
        .collect()
}

/// The LINEAR VOI function for one value. DICOM's function is intentionally
/// asymmetric around center - 0.5 and spans width - 1 input units (PS3.3
/// C.11.2.1.2.1). Width 1 is the threshold special case and must not divide
/// by zero.
pub(crate) fn window_value(sample: f64, center: f64, width: f64) -> u8 {
    let width = width.max(1.0);
    let low = center - 0.5 - (width - 1.0) / 2.0;
    let high = center - 0.5 + (width - 1.0) / 2.0;
    if sample <= low {
        0
    } else if sample > high || width == 1.0 {
        255
    } else {
        (((sample - (center - 0.5)) / (width - 1.0) + 0.5) * 255.0).round() as u8
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_modality_transform, apply_voi_lut_if_selected, apply_window};
    use crate::api::contracts::{WindowMode, WindowPreset};
    use crate::types::DicomLut;

    #[test]
    fn modality_lut_maps_and_clamps_stored_values_before_rescale() {
        let lut = DicomLut {
            first_mapped_value: 0,
            bits_per_entry: 16,
            entries: vec![0, 1024, 2048, 4095],
        };
        assert_eq!(
            apply_modality_transform(&[-1.0, 0.0, 1.0, 2.0, 3.0, 4.0], Some(&lut), 99.0, -20.0,),
            [0.0, 0.0, 1024.0, 2048.0, 4095.0, 4095.0]
        );
    }

    #[test]
    fn rescale_remains_the_fallback_without_a_modality_lut() {
        assert_eq!(
            apply_modality_transform(&[0.0, 1.0, 2.0], None, 2.0, -1.0),
            [-1.0, 1.0, 3.0]
        );
    }

    #[test]
    fn voi_lut_maps_post_modality_values_and_clamps_to_descriptor_range() {
        let lut = DicomLut {
            first_mapped_value: 0,
            bits_per_entry: 16,
            entries: vec![0, 21_845, 43_690, 65_535],
        };
        assert_eq!(
            apply_voi_lut_if_selected(
                WindowMode::Default,
                None,
                None,
                None,
                Some(&lut),
                &[0.0, 1.0, 2.0, 3.0, 1024.0],
            ),
            Some(vec![0, 85, 170, 255, 255])
        );
    }

    #[test]
    fn eight_bit_voi_lut_uses_its_declared_output_range() {
        let lut = DicomLut {
            first_mapped_value: 0,
            bits_per_entry: 8,
            entries: vec![0, 85, 170, 255],
        };
        assert_eq!(
            apply_voi_lut_if_selected(
                WindowMode::Default,
                None,
                None,
                None,
                Some(&lut),
                &[0.0, 1.0, 2.0, 3.0],
            ),
            Some(vec![0, 85, 170, 255])
        );
    }

    #[test]
    fn prepared_cr_modality_and_voi_luts_compose_in_pipeline_order() {
        let modality_lut = DicomLut {
            first_mapped_value: 0,
            bits_per_entry: 16,
            entries: vec![0, 1024, 2048, 4095],
        };
        let voi_lut = DicomLut {
            first_mapped_value: 0,
            bits_per_entry: 16,
            entries: vec![0, 21_845, 43_690, 65_535],
        };
        let modality_values =
            apply_modality_transform(&[0.0, 1.0, 2.0, 3.0], Some(&modality_lut), 1.0, 0.0);

        assert_eq!(modality_values, [0.0, 1024.0, 2048.0, 4095.0]);
        assert_eq!(
            apply_voi_lut_if_selected(
                WindowMode::Default,
                None,
                None,
                None,
                Some(&voi_lut),
                &modality_values,
            ),
            Some(vec![0, 255, 255, 255])
        );
    }

    #[test]
    fn explicit_and_dicom_windows_take_precedence_over_voi_lut() {
        let lut = DicomLut {
            first_mapped_value: 0,
            bits_per_entry: 16,
            entries: vec![0, 65_535],
        };
        assert_eq!(
            apply_voi_lut_if_selected(
                WindowMode::Default,
                Some(1.0),
                Some(2.0),
                None,
                Some(&lut),
                &[0.0, 1.0],
            ),
            None
        );
        assert_eq!(
            apply_voi_lut_if_selected(
                WindowMode::Default,
                None,
                None,
                Some(WindowPreset {
                    center: 1.0,
                    width: 2.0,
                }),
                Some(&lut),
                &[0.0, 1.0],
            ),
            None
        );
        assert_eq!(
            apply_voi_lut_if_selected(
                WindowMode::FullDynamic,
                None,
                None,
                None,
                Some(&lut),
                &[0.0, 1.0],
            ),
            None
        );
    }

    #[test]
    fn linear_window_uses_dicom_half_unit_boundaries() {
        assert_eq!(
            apply_window(&[-3.0, -2.0, -1.0, 0.0, 1.0, 2.0], 0.0, 4.0),
            [0, 0, 85, 170, 255, 255]
        );
        assert_eq!(apply_window(&[-1.0, 0.0, 1.0], 0.0, 2.0), [0, 255, 255]);
    }

    #[test]
    fn linear_width_one_is_a_binary_threshold() {
        assert_eq!(
            apply_window(&[9.0, 9.5, 9.500_001, 10.0], 10.0, 1.0),
            [0, 0, 255, 255]
        );
    }

    #[test]
    fn padding_ranges_are_inclusive_order_independent_and_excludable() {
        let samples = [-2048.0, -1024.0, -1023.0, 0.0, 512.0];
        let mask = super::PixelPaddingRange::new(-2048.0, Some(-1024.0)).mask(&samples);
        assert_eq!(mask, [true, true, false, false, false]);
        assert_eq!(
            super::exclude_padding_samples(&samples, &mask),
            [-1023.0, 0.0, 512.0]
        );

        let reversed = super::PixelPaddingRange::new(-1024.0, Some(-2048.0)).mask(&samples);
        assert_eq!(reversed, mask);

        let mut windowed = [255, 170, 85, 42, 0];
        super::apply_padding_background(&mut windowed, &mask);
        assert_eq!(windowed, [0, 0, 85, 42, 0]);
    }
}
