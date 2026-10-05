use super::{exif_orientation, Header, HeaderReader, EXIF_SCAN_MAX_BYTES};
use crate::api::contracts::RasterColorType;
use anyhow::{ensure, Result};

pub(super) fn png(input: &mut HeaderReader) -> Result<Header> {
    ensure!(
        input.read::<8>(0)? == *b"\x89PNG\r\n\x1a\n",
        "invalid PNG signature"
    );
    let mut offset = 8;
    let mut header: Option<Header> = None;
    let mut palette = false;
    loop {
        input.step()?;
        let chunk = input.read::<8>(offset)?;
        let length = u64::from(u32::from_be_bytes(chunk[..4].try_into()?));
        let kind = &chunk[4..];
        let start = offset + 8;
        match header.take() {
            None => {
                ensure!(kind == b"IHDR" && length == 13, "missing PNG IHDR");
                let data = input.read::<13>(start)?;
                let depth = data[8];
                let color = match (data[9], depth) {
                    (0, 1 | 2 | 4 | 8 | 16) => RasterColorType::Gray,
                    (2, 8 | 16) => RasterColorType::Rgb,
                    (3, 1 | 2 | 4 | 8) => RasterColorType::Palette,
                    (4, 8 | 16) => RasterColorType::GrayAlpha,
                    (6, 8 | 16) => RasterColorType::Rgba,
                    _ => anyhow::bail!("invalid PNG sample layout"),
                };
                ensure!(
                    data[10] == 0 && data[11] == 0 && data[12] <= 1,
                    "invalid PNG encoding"
                );
                header = Some(Header::new(
                    u32::from_be_bytes(data[..4].try_into()?),
                    u32::from_be_bytes(data[4..8].try_into()?),
                    color,
                    depth.into(),
                ));
            }
            Some(mut image) => {
                match kind {
                    b"IDAT" => {
                        ensure!(
                            image.metadata.color_type != RasterColorType::Palette || palette,
                            "missing PNG palette"
                        );
                        return Ok(image);
                    }
                    b"IHDR" | b"IEND" => anyhow::bail!("invalid PNG chunk order"),
                    b"PLTE" => palette = true,
                    b"tRNS" => image.metadata.has_alpha = true,
                    b"iCCP" => image.metadata.has_icc = true,
                    b"acTL" => image.metadata.animated = true,
                    b"sBIT" => {
                        let expected = match image.metadata.color_type {
                            RasterColorType::Gray => 1,
                            RasterColorType::GrayAlpha => 2,
                            RasterColorType::Rgba => 4,
                            _ => 3,
                        };
                        ensure!(length == expected, "invalid PNG significant bits length");
                        image.metadata.significant_bits = Some(input.bytes(start, length)?);
                    }
                    b"eXIf" => {
                        image.metadata.orientation =
                            exif_orientation(&input.bytes(start, length.min(EXIF_SCAN_MAX_BYTES))?);
                    }
                    _ => {}
                }
                header = Some(image);
            }
        }
        input.check_range(start, length + 4)?;
        offset = start + length + 4;
    }
}

pub(super) fn jpeg(input: &mut HeaderReader) -> Result<Header> {
    ensure!(
        input.read::<2>(0)? == [0xff, 0xd8],
        "invalid JPEG signature"
    );
    let mut offset = 2;
    let mut orientation = 1;
    let mut has_icc = false;
    loop {
        input.step()?;
        ensure!(input.read::<1>(offset)?[0] == 0xff, "invalid JPEG marker");
        // Any number of fill bytes may stand between the 0xff and its marker.
        offset = input.skip_run(offset + 1, 0xff)?;
        let marker = input.read::<1>(offset)?[0];
        offset += 1;
        ensure!(
            !matches!(marker, 0 | 0xd8..=0xda),
            "JPEG ends before a frame header"
        );
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let length = u16::from_be_bytes(input.read::<2>(offset)?) as u64;
        ensure!(length >= 2, "invalid JPEG segment length");
        input.check_range(offset, length)?;
        let start = offset + 2;
        let payload = length - 2;
        if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            ensure!(payload >= 6, "short JPEG frame header");
            let data = input.read::<6>(start)?;
            let depth = u32::from(data[0]);
            ensure!((2..=16).contains(&depth), "invalid JPEG precision");
            let color = match data[5] {
                1 => RasterColorType::Gray,
                3 => RasterColorType::Rgb,
                4 => RasterColorType::Cmyk,
                _ => anyhow::bail!("unsupported JPEG component count"),
            };
            ensure!(
                payload == 6 + u64::from(data[5]) * 3,
                "invalid JPEG frame components"
            );
            let mut header = Header::new(
                u16::from_be_bytes([data[3], data[4]]).into(),
                u16::from_be_bytes([data[1], data[2]]).into(),
                color,
                depth,
            );
            header.metadata.orientation = orientation;
            header.metadata.has_icc = has_icc;
            return Ok(header);
        }
        if marker == 0xe1 && payload >= 6 && input.read::<6>(start)? == *b"Exif\0\0" {
            orientation =
                exif_orientation(&input.bytes(start + 6, (payload - 6).min(EXIF_SCAN_MAX_BYTES))?);
        } else if marker == 0xe2 && payload >= 14 {
            has_icc |= input.read::<12>(start)? == *b"ICC_PROFILE\0";
        } else if marker == 0xee && payload >= 12 {
            // The Adobe transform distinguishes CMYK/YCCK; both are described
            // by their four stored components and will serve RGB on decode.
            let _adobe = input.read::<12>(start)?;
        }
        offset += length;
    }
}

pub(super) fn webp(input: &mut HeaderReader) -> Result<Header> {
    let riff = input.read::<12>(0)?;
    ensure!(
        &riff[..4] == b"RIFF" && &riff[8..] == b"WEBP",
        "invalid WebP signature"
    );
    let end = u64::from(u32::from_le_bytes(riff[4..8].try_into()?)) + 8;
    input.check_range(0, end)?;
    let mut offset = 12;
    let mut header = None;
    let mut has_icc = false;
    let mut orientation = 1;
    let mut animated = false;
    let mut alpha = false;
    while offset < end {
        input.step()?;
        ensure!(end - offset >= 8, "truncated WebP chunk");
        let chunk = input.read::<8>(offset)?;
        let length = u64::from(u32::from_le_bytes(chunk[4..].try_into()?));
        let start = offset + 8;
        let padded = length + (length & 1);
        ensure!(padded <= end - start, "truncated WebP payload");
        match &chunk[..4] {
            b"VP8X" => {
                ensure!(
                    header.is_none() && length == 10 && offset == 12,
                    "invalid VP8X header"
                );
                let bytes = input.read::<10>(start)?;
                ensure!(
                    bytes[0] & 0xc1 == 0 && bytes[1..4] == [0, 0, 0],
                    "invalid VP8X flags"
                );
                let u24 = |b: &[u8]| u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16;
                header = Some(Header::new(
                    1 + u24(&bytes[4..7]),
                    1 + u24(&bytes[7..10]),
                    RasterColorType::Rgb,
                    8,
                ));
                alpha |= bytes[0] & 0x10 != 0;
                animated |= bytes[0] & 2 != 0;
            }
            b"VP8 " => {
                ensure!(length >= 10, "short VP8 header");
                let bytes = input.read::<10>(start)?;
                ensure!(
                    bytes[0] & 1 == 0 && bytes[3..6] == [0x9d, 1, 0x2a],
                    "invalid VP8 header"
                );
                let width = u16::from_le_bytes([bytes[6], bytes[7]]) & 0x3fff;
                let height = u16::from_le_bytes([bytes[8], bytes[9]]) & 0x3fff;
                header.get_or_insert_with(|| {
                    Header::new(width.into(), height.into(), RasterColorType::Rgb, 8)
                });
            }
            b"VP8L" => {
                ensure!(length >= 5, "short VP8L header");
                let bytes = input.read::<5>(start)?;
                let bits = u32::from_le_bytes(bytes[1..].try_into()?);
                ensure!(bytes[0] == 0x2f && bits >> 29 == 0, "invalid VP8L header");
                header.get_or_insert_with(|| {
                    Header::new(
                        1 + (bits & 0x3fff),
                        1 + ((bits >> 14) & 0x3fff),
                        RasterColorType::Rgb,
                        8,
                    )
                });
                alpha |= bits & (1 << 28) != 0;
            }
            b"ICCP" => has_icc = true,
            b"EXIF" => {
                orientation =
                    exif_orientation(&input.bytes(start, length.min(EXIF_SCAN_MAX_BYTES))?);
            }
            b"ANIM" => {
                ensure!(length == 6, "invalid ANIM header");
                animated = true;
            }
            b"ALPH" => alpha = true,
            _ => {}
        }
        offset = start + padded;
    }
    let mut header = header.ok_or_else(|| anyhow::anyhow!("missing WebP image header"))?;
    header.metadata.color_type = if alpha {
        RasterColorType::Rgba
    } else {
        RasterColorType::Rgb
    };
    header.metadata.has_alpha = alpha;
    header.metadata.has_icc = has_icc;
    header.metadata.orientation = orientation;
    header.metadata.animated = animated;
    Ok(header)
}
