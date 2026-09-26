use anyhow::{anyhow, Context, Result};
use dicom_object::DefaultDicomObject;

/// Maps 8-bit PALETTE COLOR indices to interleaved RGB through the object's
/// Red, Green, and Blue Palette Color Lookup Tables.
pub(super) fn palette_indices_to_rgb8(
    object: &DefaultDicomObject,
    indices: &[u8],
    bits_allocated: u32,
) -> Result<Vec<u8>> {
    if bits_allocated != 8 {
        return Err(anyhow!(
            "palette display requires 8-bit indices, found {bits_allocated}"
        ));
    }
    let red = read_palette_channel(
        object,
        "RedPaletteColorLookupTableDescriptor",
        "RedPaletteColorLookupTableData",
    )?;
    let green = read_palette_channel(
        object,
        "GreenPaletteColorLookupTableDescriptor",
        "GreenPaletteColorLookupTableData",
    )?;
    let blue = read_palette_channel(
        object,
        "BluePaletteColorLookupTableDescriptor",
        "BluePaletteColorLookupTableData",
    )?;
    apply_palette(indices, &red, &green, &blue)
}

#[derive(Debug, PartialEq, Eq)]
struct PaletteChannel {
    first_mapped: i32,
    values: Vec<u8>,
}

fn read_palette_channel(
    object: &DefaultDicomObject,
    descriptor_name: &str,
    data_name: &str,
) -> Result<PaletteChannel> {
    let descriptor = object
        .element_by_name(descriptor_name)
        .with_context(|| format!("missing {descriptor_name}"))?
        .to_multi_int::<i32>()
        .with_context(|| format!("invalid {descriptor_name}"))?;
    if descriptor.len() != 3 {
        return Err(anyhow!("{descriptor_name} must contain three values"));
    }
    let entry_count = if descriptor[0] == 0 {
        65_536_usize
    } else {
        usize::try_from(descriptor[0]).context("negative palette entry count")?
    };
    if entry_count == 0 {
        return Err(anyhow!("palette lookup table is empty"));
    }
    let bits = descriptor[2];
    if bits != 8 && bits != 16 {
        return Err(anyhow!("unsupported palette bit depth {bits}"));
    }
    let words = object
        .element_by_name(data_name)
        .with_context(|| format!("missing {data_name}"))?
        .to_multi_int::<u16>()
        .with_context(|| format!("invalid {data_name}"))?;
    if words.len() < entry_count {
        return Err(anyhow!(
            "{data_name} contains {} entries, expected {entry_count}",
            words.len()
        ));
    }
    let values = words
        .into_iter()
        .take(entry_count)
        .map(|value| {
            if bits == 16 {
                (value >> 8) as u8
            } else {
                value as u8
            }
        })
        .collect();
    Ok(PaletteChannel {
        first_mapped: descriptor[1],
        values,
    })
}

fn apply_palette(
    indices: &[u8],
    red: &PaletteChannel,
    green: &PaletteChannel,
    blue: &PaletteChannel,
) -> Result<Vec<u8>> {
    if red.first_mapped != green.first_mapped
        || red.first_mapped != blue.first_mapped
        || red.values.len() != green.values.len()
        || red.values.len() != blue.values.len()
        || red.values.is_empty()
    {
        return Err(anyhow!("palette channel descriptors do not match"));
    }

    let last = red.values.len() - 1;
    let mut rgb = Vec::with_capacity(indices.len().saturating_mul(3));
    for index in indices {
        let mapped = i32::from(*index) - red.first_mapped;
        let lut_index = mapped.clamp(0, last as i32) as usize;
        rgb.extend_from_slice(&[
            red.values[lut_index],
            green.values[lut_index],
            blue.values[lut_index],
        ]);
    }
    Ok(rgb)
}

#[cfg(test)]
mod tests {
    use super::{apply_palette, PaletteChannel};

    #[test]
    fn maps_indices_and_clamps_outside_the_descriptor_range() {
        let red = PaletteChannel {
            first_mapped: 1,
            values: vec![10, 20, 30],
        };
        let green = PaletteChannel {
            first_mapped: 1,
            values: vec![40, 50, 60],
        };
        let blue = PaletteChannel {
            first_mapped: 1,
            values: vec![70, 80, 90],
        };
        assert_eq!(
            apply_palette(&[0, 1, 2, 3, 4], &red, &green, &blue).unwrap(),
            [10, 40, 70, 10, 40, 70, 20, 50, 80, 30, 60, 90, 30, 60, 90]
        );
    }

    #[test]
    fn rejects_mismatched_channels() {
        let red = PaletteChannel {
            first_mapped: 0,
            values: vec![0, 1],
        };
        let green = PaletteChannel {
            first_mapped: 1,
            values: vec![0, 1],
        };
        let blue = PaletteChannel {
            first_mapped: 0,
            values: vec![0, 1],
        };
        assert!(apply_palette(&[0], &red, &green, &blue).is_err());
    }
}
