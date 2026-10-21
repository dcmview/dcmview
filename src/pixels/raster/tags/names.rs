//! The names the metadata tree shows for TIFF and EXIF entries: a keyword
//! for each tag number it knows, per kind of directory, and a name for each
//! value type. Every keyword in the tree comes from here or from a literal
//! in the reader, never from the file.

/// What a directory of entries (an IFD) holds, which decides what a tag
/// number means in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::pixels::raster) enum IfdKind {
    /// A TIFF page, or `IFD0` or `IFD1` of an EXIF block.
    Image,
    /// The EXIF directory (tag 34665 of an image directory).
    Exif,
    /// The GPS directory (tag 34853 of an image directory).
    Gps,
    /// The interoperability directory (tag 40965 of an EXIF directory).
    Interop,
}

/// The keyword of entry `tag` in a directory of `kind`; `Unknown` for a tag
/// this table does not name, as the DICOM tree says for an unknown element.
pub(in crate::pixels::raster) fn ifd_tag_keyword(kind: IfdKind, tag: u16) -> &'static str {
    let table = match kind {
        IfdKind::Image => IMAGE_TAGS,
        IfdKind::Exif => EXIF_TAGS,
        IfdKind::Gps => GPS_TAGS,
        IfdKind::Interop => INTEROP_TAGS,
    };
    table
        .binary_search_by_key(&tag, |(tag, _)| *tag)
        .map_or("Unknown", |index| table[index].1)
}

/// The name of TIFF value type `code`, shown in the tree's type column; `""`
/// for a code that is not a type (the entry's value is then a problem).
pub(in crate::pixels::raster) fn ifd_type_name(code: u16) -> &'static str {
    match code {
        1 => "BYTE",
        2 => "ASCII",
        3 => "SHORT",
        4 => "LONG",
        5 => "RATIONAL",
        6 => "SBYTE",
        7 => "UNDEFINED",
        8 => "SSHORT",
        9 => "SLONG",
        10 => "SRATIONAL",
        11 => "FLOAT",
        12 => "DOUBLE",
        13 => "IFD",
        16 => "LONG8",
        17 => "SLONG8",
        18 => "IFD8",
        _ => "",
    }
}

/// The bytes one value of TIFF value type `code` takes; `None` for a code
/// that is not a type.
pub(in crate::pixels::raster) fn ifd_type_size(code: u16) -> Option<u64> {
    match code {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 | 11 | 13 => Some(4),
        5 | 10 | 12 | 16 | 17 | 18 => Some(8),
        _ => None,
    }
}

// Each table is sorted by tag number (a unit test holds them to it).

const IMAGE_TAGS: &[(u16, &str)] = &[
    (254, "NewSubfileType"),
    (255, "SubfileType"),
    (256, "ImageWidth"),
    (257, "ImageLength"),
    (258, "BitsPerSample"),
    (259, "Compression"),
    (262, "PhotometricInterpretation"),
    (263, "Threshholding"),
    (266, "FillOrder"),
    (269, "DocumentName"),
    (270, "ImageDescription"),
    (271, "Make"),
    (272, "Model"),
    (273, "StripOffsets"),
    (274, "Orientation"),
    (277, "SamplesPerPixel"),
    (278, "RowsPerStrip"),
    (279, "StripByteCounts"),
    (280, "MinSampleValue"),
    (281, "MaxSampleValue"),
    (282, "XResolution"),
    (283, "YResolution"),
    (284, "PlanarConfiguration"),
    (285, "PageName"),
    (286, "XPosition"),
    (287, "YPosition"),
    (296, "ResolutionUnit"),
    (297, "PageNumber"),
    (301, "TransferFunction"),
    (305, "Software"),
    (306, "DateTime"),
    (315, "Artist"),
    (316, "HostComputer"),
    (317, "Predictor"),
    (318, "WhitePoint"),
    (319, "PrimaryChromaticities"),
    (320, "ColorMap"),
    (322, "TileWidth"),
    (323, "TileLength"),
    (324, "TileOffsets"),
    (325, "TileByteCounts"),
    (330, "SubIFDs"),
    (332, "InkSet"),
    (338, "ExtraSamples"),
    (339, "SampleFormat"),
    (340, "SMinSampleValue"),
    (341, "SMaxSampleValue"),
    (347, "JPEGTables"),
    (513, "JPEGInterchangeFormat"),
    (514, "JPEGInterchangeFormatLength"),
    (529, "YCbCrCoefficients"),
    (530, "YCbCrSubSampling"),
    (531, "YCbCrPositioning"),
    (532, "ReferenceBlackWhite"),
    (700, "XMP"),
    (18246, "Rating"),
    (33432, "Copyright"),
    (33550, "ModelPixelScale"),
    (33723, "IPTC"),
    (33922, "ModelTiepoint"),
    (34377, "Photoshop"),
    (34665, "ExifIFD"),
    (34675, "ICCProfile"),
    (34735, "GeoKeyDirectory"),
    (34853, "GPSIFD"),
    (40091, "XPTitle"),
    (40092, "XPComment"),
    (40093, "XPAuthor"),
    (40094, "XPKeywords"),
    (40095, "XPSubject"),
    (42112, "GDALMetadata"),
    (42113, "GDALNoData"),
    (50706, "DNGVersion"),
    (50708, "UniqueCameraModel"),
    (50838, "ImageJMetadataByteCounts"),
    (50839, "ImageJMetadata"),
];

const EXIF_TAGS: &[(u16, &str)] = &[
    (33434, "ExposureTime"),
    (33437, "FNumber"),
    (34850, "ExposureProgram"),
    (34852, "SpectralSensitivity"),
    (34855, "ISOSpeedRatings"),
    (34864, "SensitivityType"),
    (36864, "ExifVersion"),
    (36867, "DateTimeOriginal"),
    (36868, "DateTimeDigitized"),
    (36880, "OffsetTime"),
    (36881, "OffsetTimeOriginal"),
    (36882, "OffsetTimeDigitized"),
    (37121, "ComponentsConfiguration"),
    (37122, "CompressedBitsPerPixel"),
    (37377, "ShutterSpeedValue"),
    (37378, "ApertureValue"),
    (37379, "BrightnessValue"),
    (37380, "ExposureBiasValue"),
    (37381, "MaxApertureValue"),
    (37382, "SubjectDistance"),
    (37383, "MeteringMode"),
    (37384, "LightSource"),
    (37385, "Flash"),
    (37386, "FocalLength"),
    (37396, "SubjectArea"),
    (37500, "MakerNote"),
    (37510, "UserComment"),
    (37520, "SubSecTime"),
    (37521, "SubSecTimeOriginal"),
    (37522, "SubSecTimeDigitized"),
    (40960, "FlashpixVersion"),
    (40961, "ColorSpace"),
    (40962, "PixelXDimension"),
    (40963, "PixelYDimension"),
    (40964, "RelatedSoundFile"),
    (40965, "InteropIFD"),
    (41486, "FocalPlaneXResolution"),
    (41487, "FocalPlaneYResolution"),
    (41488, "FocalPlaneResolutionUnit"),
    (41495, "SensingMethod"),
    (41728, "FileSource"),
    (41729, "SceneType"),
    (41985, "CustomRendered"),
    (41986, "ExposureMode"),
    (41987, "WhiteBalance"),
    (41988, "DigitalZoomRatio"),
    (41989, "FocalLengthIn35mmFilm"),
    (41990, "SceneCaptureType"),
    (41991, "GainControl"),
    (41992, "Contrast"),
    (41993, "Saturation"),
    (41994, "Sharpness"),
    (41996, "SubjectDistanceRange"),
    (42016, "ImageUniqueID"),
    (42032, "CameraOwnerName"),
    (42033, "BodySerialNumber"),
    (42034, "LensSpecification"),
    (42035, "LensMake"),
    (42036, "LensModel"),
    (42037, "LensSerialNumber"),
];

const GPS_TAGS: &[(u16, &str)] = &[
    (0, "GPSVersionID"),
    (1, "GPSLatitudeRef"),
    (2, "GPSLatitude"),
    (3, "GPSLongitudeRef"),
    (4, "GPSLongitude"),
    (5, "GPSAltitudeRef"),
    (6, "GPSAltitude"),
    (7, "GPSTimeStamp"),
    (8, "GPSSatellites"),
    (9, "GPSStatus"),
    (10, "GPSMeasureMode"),
    (11, "GPSDOP"),
    (12, "GPSSpeedRef"),
    (13, "GPSSpeed"),
    (14, "GPSTrackRef"),
    (15, "GPSTrack"),
    (16, "GPSImgDirectionRef"),
    (17, "GPSImgDirection"),
    (18, "GPSMapDatum"),
    (19, "GPSDestLatitudeRef"),
    (20, "GPSDestLatitude"),
    (21, "GPSDestLongitudeRef"),
    (22, "GPSDestLongitude"),
    (23, "GPSDestBearingRef"),
    (24, "GPSDestBearing"),
    (25, "GPSDestDistanceRef"),
    (26, "GPSDestDistance"),
    (27, "GPSProcessingMethod"),
    (28, "GPSAreaInformation"),
    (29, "GPSDateStamp"),
    (30, "GPSDifferential"),
    (31, "GPSHPositioningError"),
];

const INTEROP_TAGS: &[(u16, &str)] =
    &[(1, "InteroperabilityIndex"), (2, "InteroperabilityVersion")];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_tables_are_sorted_for_binary_search() {
        for table in [IMAGE_TAGS, EXIF_TAGS, GPS_TAGS, INTEROP_TAGS] {
            assert!(table.windows(2).all(|pair| pair[0].0 < pair[1].0));
        }
    }
}
