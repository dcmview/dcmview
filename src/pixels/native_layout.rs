// This shared extraction seam intentionally lands before its native-path consumer.
#![allow(dead_code)]

use crate::types::NativePixelDataKind;
use dicom_core::Tag;
use dicom_dictionary_std::tags;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeByteOrder {
    LittleEndian,
    BigEndian,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct NativeFrameLayout<'a> {
    pub rows: u32,
    pub columns: u32,
    pub samples_per_pixel: u32,
    pub bits_allocated: u32,
    pub planar_configuration: Option<u32>,
    pub photometric_interpretation: &'a str,
    pub byte_order: NativeByteOrder,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum NativeLayoutError {
    #[error("native pixel data has zero-valued image geometry or SamplesPerPixel")]
    InvalidGeometry,
    #[error("native BitsAllocated {0} is unsupported")]
    UnsupportedBitsAllocated(u32),
    #[error("native pixel layout size overflowed")]
    SizeOverflow,
    #[error("native pixel data frame {frame} extends beyond {available} source bytes")]
    FrameOutOfBounds { frame: u32, available: usize },
    #[error("one-bit native pixels require SamplesPerPixel 1")]
    UnsupportedOneBitSamples,
    #[error("PlanarConfiguration {0} is unsupported")]
    UnsupportedPlanarConfiguration(u32),
    #[error("planar native pixels require at least two samples per pixel")]
    InvalidPlanarSamples,
    #[error("YBR_FULL_422 requires 8-bit, interleaved, three-sample pixels")]
    InvalidYbrFull422Layout,
    #[error("YBR_FULL_422 requires an even Columns value")]
    OddYbrFull422Columns,
    #[error("native pixel buffer length {actual} does not match expected length {expected}")]
    BufferLength { actual: usize, expected: usize },
    #[error("multiple native pixel data elements are present")]
    AmbiguousPixelElements,
    #[error("no native pixel data element is present")]
    MissingPixelElement,
}

impl NativeFrameLayout<'_> {
    pub(crate) fn stored_frame_bits(self) -> Result<usize, NativeLayoutError> {
        let rows = usize::try_from(self.rows).map_err(|_| NativeLayoutError::SizeOverflow)?;
        let columns = usize::try_from(self.columns).map_err(|_| NativeLayoutError::SizeOverflow)?;
        let samples =
            usize::try_from(self.samples_per_pixel).map_err(|_| NativeLayoutError::SizeOverflow)?;
        if rows == 0 || columns == 0 || samples == 0 {
            return Err(NativeLayoutError::InvalidGeometry);
        }

        if self.is_ybr_full_422() {
            self.validate_ybr_full_422()?;
            return rows
                .checked_mul(columns)
                .and_then(|pixels| pixels.checked_mul(16))
                .ok_or(NativeLayoutError::SizeOverflow);
        }

        match self.bits_allocated {
            1 if samples != 1 => Err(NativeLayoutError::UnsupportedOneBitSamples),
            1 => rows
                .checked_mul(columns)
                .ok_or(NativeLayoutError::SizeOverflow),
            bits if bits > 0 && bits.is_multiple_of(8) => rows
                .checked_mul(columns)
                .and_then(|pixels| pixels.checked_mul(samples))
                .and_then(|values| values.checked_mul(bits as usize))
                .ok_or(NativeLayoutError::SizeOverflow),
            bits => Err(NativeLayoutError::UnsupportedBitsAllocated(bits)),
        }
    }

    /// Whole frames that `value_length` bytes of native pixel data hold, or
    /// `None` when the frame size cannot be computed. One-bit frames are
    /// packed back to back across byte boundaries, so this counts bits.
    pub(crate) fn frame_capacity(self, value_length: u64) -> Option<u64> {
        let frame_bits = u64::try_from(self.stored_frame_bits().ok()?).ok()?;
        Some(value_length.checked_mul(8)? / frame_bits)
    }

    pub(crate) fn stored_frame_bytes(self) -> Result<usize, NativeLayoutError> {
        let bits = self.stored_frame_bits()?;
        bits.checked_add(7)
            .map(|rounded| rounded / 8)
            .ok_or(NativeLayoutError::SizeOverflow)
    }

    pub(crate) fn expanded_frame_bytes(self) -> Result<usize, NativeLayoutError> {
        if self.bits_allocated == 1 {
            // The raw/display seam exposes one canonical byte per binary
            // sample even though the DICOM value field is bit-packed.
            return self.stored_frame_bits();
        }
        let rows = usize::try_from(self.rows).map_err(|_| NativeLayoutError::SizeOverflow)?;
        let columns = usize::try_from(self.columns).map_err(|_| NativeLayoutError::SizeOverflow)?;
        let samples =
            usize::try_from(self.samples_per_pixel).map_err(|_| NativeLayoutError::SizeOverflow)?;
        if self.bits_allocated == 0 || !self.bits_allocated.is_multiple_of(8) {
            return Err(NativeLayoutError::UnsupportedBitsAllocated(
                self.bits_allocated,
            ));
        }
        let bytes_per_sample = usize::try_from(self.bits_allocated / 8)
            .map_err(|_| NativeLayoutError::SizeOverflow)?;
        rows.checked_mul(columns)
            .and_then(|pixels| pixels.checked_mul(samples))
            .and_then(|values| values.checked_mul(bytes_per_sample))
            .ok_or(NativeLayoutError::SizeOverflow)
    }

    pub(crate) fn extract_raw_frame(
        self,
        source: &[u8],
        frame: u32,
    ) -> Result<Vec<u8>, NativeLayoutError> {
        if self.bits_allocated == 1 {
            return extract_lsb_first_one_bit_frame(source, frame, self.stored_frame_bits()?);
        }

        let frame_bytes = self.stored_frame_bytes()?;
        let start = usize::try_from(frame)
            .ok()
            .and_then(|frame| frame.checked_mul(frame_bytes))
            .ok_or(NativeLayoutError::SizeOverflow)?;
        let end = start
            .checked_add(frame_bytes)
            .ok_or(NativeLayoutError::SizeOverflow)?;
        let stored = source
            .get(start..end)
            .ok_or(NativeLayoutError::FrameOutOfBounds {
                frame,
                available: source.len(),
            })?;

        if let Some(planar) = self.planar_configuration {
            if planar > 1 {
                return Err(NativeLayoutError::UnsupportedPlanarConfiguration(planar));
            }
        }
        let mut raw = stored.to_vec();
        canonicalize_little_endian(&mut raw, self.bits_allocated, self.byte_order)?;
        Ok(raw)
    }

    pub(crate) fn extract_display_frame(
        self,
        source: &[u8],
        frame: u32,
    ) -> Result<Vec<u8>, NativeLayoutError> {
        let raw = self.extract_raw_frame(source, frame)?;
        if self.is_ybr_full_422() {
            expand_ybr_full_422(&raw, self.rows, self.columns)
        } else if self.planar_configuration == Some(1) {
            interleave_planar_samples(
                &raw,
                self.rows,
                self.columns,
                self.samples_per_pixel,
                self.bits_allocated,
            )
        } else {
            Ok(raw)
        }
    }

    fn is_ybr_full_422(self) -> bool {
        self.photometric_interpretation
            .trim()
            .eq_ignore_ascii_case("YBR_FULL_422")
    }

    fn validate_ybr_full_422(self) -> Result<(), NativeLayoutError> {
        if self.bits_allocated != 8
            || self.samples_per_pixel != 3
            || self.planar_configuration.unwrap_or(0) != 0
        {
            return Err(NativeLayoutError::InvalidYbrFull422Layout);
        }
        if !self.columns.is_multiple_of(2) {
            return Err(NativeLayoutError::OddYbrFull422Columns);
        }
        Ok(())
    }
}

pub(crate) fn select_native_pixel_element(
    has_pixel_data: bool,
    has_float_pixel_data: bool,
    has_double_float_pixel_data: bool,
) -> Result<NativePixelDataKind, NativeLayoutError> {
    match (
        has_pixel_data,
        has_float_pixel_data,
        has_double_float_pixel_data,
    ) {
        (true, false, false) => Ok(NativePixelDataKind::Integer),
        (false, true, false) => Ok(NativePixelDataKind::Float32),
        (false, false, true) => Ok(NativePixelDataKind::Float64),
        (false, false, false) => Err(NativeLayoutError::MissingPixelElement),
        _ => Err(NativeLayoutError::AmbiguousPixelElements),
    }
}

pub(crate) const fn native_pixel_element_tag(kind: NativePixelDataKind) -> Tag {
    match kind {
        NativePixelDataKind::Integer => tags::PIXEL_DATA,
        NativePixelDataKind::Float32 => tags::FLOAT_PIXEL_DATA,
        NativePixelDataKind::Float64 => tags::DOUBLE_FLOAT_PIXEL_DATA,
    }
}

fn extract_lsb_first_one_bit_frame(
    source: &[u8],
    frame: u32,
    frame_bits: usize,
) -> Result<Vec<u8>, NativeLayoutError> {
    let start_bit = usize::try_from(frame)
        .ok()
        .and_then(|frame| frame.checked_mul(frame_bits))
        .ok_or(NativeLayoutError::SizeOverflow)?;
    let end_bit = start_bit
        .checked_add(frame_bits)
        .ok_or(NativeLayoutError::SizeOverflow)?;
    let available_bits = source
        .len()
        .checked_mul(8)
        .ok_or(NativeLayoutError::SizeOverflow)?;
    if end_bit > available_bits {
        return Err(NativeLayoutError::FrameOutOfBounds {
            frame,
            available: source.len(),
        });
    }

    let mut output = vec![0_u8; frame_bits];
    for (output_bit, output_sample) in output.iter_mut().enumerate() {
        let source_bit = start_bit + output_bit;
        *output_sample = (source[source_bit / 8] >> (source_bit % 8)) & 1;
    }
    Ok(output)
}

fn interleave_planar_samples(
    planar: &[u8],
    rows: u32,
    columns: u32,
    samples_per_pixel: u32,
    bits_allocated: u32,
) -> Result<Vec<u8>, NativeLayoutError> {
    if samples_per_pixel < 2 {
        return Err(NativeLayoutError::InvalidPlanarSamples);
    }
    if bits_allocated == 0 || !bits_allocated.is_multiple_of(8) {
        return Err(NativeLayoutError::UnsupportedBitsAllocated(bits_allocated));
    }
    let pixel_count = usize::try_from(rows)
        .ok()
        .and_then(|rows| {
            usize::try_from(columns)
                .ok()
                .and_then(|columns| rows.checked_mul(columns))
        })
        .ok_or(NativeLayoutError::SizeOverflow)?;
    let samples =
        usize::try_from(samples_per_pixel).map_err(|_| NativeLayoutError::SizeOverflow)?;
    let sample_bytes =
        usize::try_from(bits_allocated / 8).map_err(|_| NativeLayoutError::SizeOverflow)?;
    let plane_bytes = pixel_count
        .checked_mul(sample_bytes)
        .ok_or(NativeLayoutError::SizeOverflow)?;
    let expected = plane_bytes
        .checked_mul(samples)
        .ok_or(NativeLayoutError::SizeOverflow)?;
    if planar.len() != expected {
        return Err(NativeLayoutError::BufferLength {
            actual: planar.len(),
            expected,
        });
    }

    let mut interleaved = vec![0_u8; expected];
    for pixel in 0..pixel_count {
        for sample in 0..samples {
            let source = sample * plane_bytes + pixel * sample_bytes;
            let destination = (pixel * samples + sample) * sample_bytes;
            interleaved[destination..destination + sample_bytes]
                .copy_from_slice(&planar[source..source + sample_bytes]);
        }
    }
    Ok(interleaved)
}

fn expand_ybr_full_422(
    stored: &[u8],
    rows: u32,
    columns: u32,
) -> Result<Vec<u8>, NativeLayoutError> {
    if !columns.is_multiple_of(2) {
        return Err(NativeLayoutError::OddYbrFull422Columns);
    }
    let pairs = usize::try_from(rows)
        .ok()
        .and_then(|rows| {
            usize::try_from(columns / 2)
                .ok()
                .and_then(|columns| rows.checked_mul(columns))
        })
        .ok_or(NativeLayoutError::SizeOverflow)?;
    let expected = pairs
        .checked_mul(4)
        .ok_or(NativeLayoutError::SizeOverflow)?;
    if stored.len() != expected {
        return Err(NativeLayoutError::BufferLength {
            actual: stored.len(),
            expected,
        });
    }

    let expanded_len = pairs
        .checked_mul(6)
        .ok_or(NativeLayoutError::SizeOverflow)?;
    let mut expanded = Vec::with_capacity(expanded_len);
    for pair in stored.chunks_exact(4) {
        let [y1, y2, cb, cr] = [pair[0], pair[1], pair[2], pair[3]];
        expanded.extend_from_slice(&[y1, cb, cr, y2, cb, cr]);
    }
    Ok(expanded)
}

fn canonicalize_little_endian(
    bytes: &mut [u8],
    bits_allocated: u32,
    byte_order: NativeByteOrder,
) -> Result<(), NativeLayoutError> {
    if bits_allocated == 0 || !bits_allocated.is_multiple_of(8) {
        return Err(NativeLayoutError::UnsupportedBitsAllocated(bits_allocated));
    }
    let sample_bytes =
        usize::try_from(bits_allocated / 8).map_err(|_| NativeLayoutError::SizeOverflow)?;
    if !bytes.len().is_multiple_of(sample_bytes) {
        return Err(NativeLayoutError::BufferLength {
            actual: bytes.len(),
            expected: bytes.len() / sample_bytes * sample_bytes,
        });
    }
    if byte_order == NativeByteOrder::BigEndian && sample_bytes > 1 {
        for sample in bytes.chunks_exact_mut(sample_bytes) {
            sample.reverse();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout<'a>(photometric_interpretation: &'a str) -> NativeFrameLayout<'a> {
        NativeFrameLayout {
            rows: 1,
            columns: 2,
            samples_per_pixel: 1,
            bits_allocated: 8,
            planar_configuration: None,
            photometric_interpretation,
            byte_order: NativeByteOrder::LittleEndian,
        }
    }

    #[test]
    fn counts_whole_frames_including_bit_packed_ones() {
        // Three 1x2 8-bit frames and a partial fourth.
        assert_eq!(layout("MONOCHROME2").frame_capacity(7), Some(3));
        // 3x3 one-bit frames are 9 bits each and straddle byte boundaries.
        let one_bit = NativeFrameLayout {
            rows: 3,
            columns: 3,
            bits_allocated: 1,
            ..layout("MONOCHROME2")
        };
        assert_eq!(one_bit.frame_capacity(9), Some(8));
        let unsupported = NativeFrameLayout {
            bits_allocated: 12,
            ..layout("MONOCHROME2")
        };
        assert_eq!(unsupported.frame_capacity(1024), None);
    }

    #[test]
    fn computes_checked_stored_and_expanded_sizes() {
        let mut rgb = layout("RGB");
        rgb.rows = 2;
        rgb.samples_per_pixel = 3;
        rgb.planar_configuration = Some(0);
        assert_eq!(rgb.stored_frame_bits().unwrap(), 96);
        assert_eq!(rgb.stored_frame_bytes().unwrap(), 12);
        assert_eq!(rgb.expanded_frame_bytes().unwrap(), 12);

        let overflow = NativeFrameLayout {
            rows: u32::MAX,
            columns: u32::MAX,
            samples_per_pixel: u32::MAX,
            bits_allocated: 64,
            ..layout("MONOCHROME2")
        };
        assert_eq!(
            overflow.stored_frame_bits(),
            Err(NativeLayoutError::SizeOverflow)
        );

        let unsupported = NativeFrameLayout {
            bits_allocated: 0,
            ..layout("MONOCHROME2")
        };
        assert_eq!(
            unsupported.stored_frame_bytes(),
            Err(NativeLayoutError::UnsupportedBitsAllocated(0))
        );
    }

    #[test]
    fn extracts_interleaved_and_planar_rgb_as_interleaved_bytes() {
        let mut rgb = layout("RGB");
        rgb.samples_per_pixel = 3;
        rgb.planar_configuration = Some(0);
        assert_eq!(
            rgb.extract_display_frame(&[1, 2, 3, 4, 5, 6], 0).unwrap(),
            [1, 2, 3, 4, 5, 6]
        );

        rgb.planar_configuration = Some(1);
        assert_eq!(
            rgb.extract_raw_frame(&[1, 4, 2, 5, 3, 6], 0).unwrap(),
            [1, 4, 2, 5, 3, 6]
        );
        assert_eq!(
            rgb.extract_display_frame(&[1, 4, 2, 5, 3, 6], 0).unwrap(),
            [1, 2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn sizes_and_expands_ybr_full_422_without_color_conversion() {
        let mut ybr = layout("YBR_FULL_422");
        ybr.samples_per_pixel = 3;
        ybr.planar_configuration = Some(0);
        assert_eq!(ybr.stored_frame_bytes().unwrap(), 4);
        assert_eq!(ybr.expanded_frame_bytes().unwrap(), 6);
        assert_eq!(
            ybr.extract_raw_frame(&[10, 20, 30, 40], 0).unwrap(),
            [10, 20, 30, 40]
        );
        assert_eq!(
            ybr.extract_display_frame(&[10, 20, 30, 40], 0).unwrap(),
            [10, 30, 40, 20, 30, 40]
        );

        ybr.columns = 3;
        assert_eq!(
            ybr.stored_frame_bytes(),
            Err(NativeLayoutError::OddYbrFull422Columns)
        );
    }

    #[test]
    fn extracts_continuous_lsb_first_one_bit_frames() {
        let mut packed = layout("MONOCHROME2");
        packed.columns = 5;
        packed.bits_allocated = 1;
        let source = [0b1100_1101, 0b0000_0010];
        assert_eq!(packed.expanded_frame_bytes().unwrap(), 5);
        assert_eq!(
            packed.extract_raw_frame(&source, 0).unwrap(),
            [1, 0, 1, 1, 0]
        );
        assert_eq!(
            packed.extract_display_frame(&source, 1).unwrap(),
            [0, 1, 1, 0, 1]
        );
        assert_eq!(
            packed.extract_raw_frame(&source, 3),
            Err(NativeLayoutError::FrameOutOfBounds {
                frame: 3,
                available: 2
            })
        );
    }

    #[test]
    fn canonicalizes_big_endian_samples_to_little_endian() {
        let mut big_endian = layout("MONOCHROME2");
        big_endian.bits_allocated = 16;
        big_endian.byte_order = NativeByteOrder::BigEndian;
        assert_eq!(
            big_endian
                .extract_raw_frame(&[0x12, 0x34, 0xab, 0xcd], 0)
                .unwrap(),
            [0x34, 0x12, 0xcd, 0xab]
        );
    }

    #[test]
    fn selects_exactly_one_native_pixel_element_tag() {
        assert_eq!(
            native_pixel_element_tag(select_native_pixel_element(true, false, false).unwrap()),
            tags::PIXEL_DATA
        );
        assert_eq!(
            native_pixel_element_tag(select_native_pixel_element(false, true, false).unwrap()),
            tags::FLOAT_PIXEL_DATA
        );
        assert_eq!(
            native_pixel_element_tag(select_native_pixel_element(false, false, true).unwrap()),
            tags::DOUBLE_FLOAT_PIXEL_DATA
        );
        assert_eq!(
            select_native_pixel_element(true, true, false),
            Err(NativeLayoutError::AmbiguousPixelElements)
        );
        assert_eq!(
            select_native_pixel_element(false, false, false),
            Err(NativeLayoutError::MissingPixelElement)
        );
    }
}
