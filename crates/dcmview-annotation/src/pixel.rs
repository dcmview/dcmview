//! Pixel digests: what `px:<64 lowercase hex>` in
//! [`FileRef::pixel_digest`](crate::FileRef) is a digest of (1.6).
//!
//! A pixel digest names the samples of one frame, whatever file they are
//! stored in. It is evidence beside a file's key: two files whose frames
//! hold the same samples in the same layout have the same pixel digests,
//! and a file whose bytes changed and whose samples did not keeps them.
//!
//! # Definition
//!
//! The digest is the BLAKE3 hash, with no key and no context, of these
//! bytes, in this order:
//!
//! 1. **A header of 24 bytes** ([`PixelLayout::header`]): six unsigned
//!    32-bit integers, each little-endian:
//!
//!    | Bytes | Value |
//!    |---|---|
//!    | 0..4 | rows |
//!    | 4..8 | columns |
//!    | 8..12 | samples per pixel |
//!    | 12..16 | bits allocated: the width of one sample as it is hashed, 8, 16, 32 or 64 |
//!    | 16..20 | sample format: 0 unsigned integer, 1 two's complement signed integer, 2 IEEE 754 binary floating point |
//!    | 20..24 | planar configuration: always 0, because the samples that follow are interleaved |
//!
//! 2. **The samples**, `rows * columns * samples_per_pixel` of them
//!    ([`PixelLayout::sample_bytes`] bytes): row by row from the top, within
//!    a row pixel by pixel from the left, within a pixel sample by sample in
//!    the order the file declares its samples (red, green, blue for RGB),
//!    each sample little-endian in `bits_allocated / 8` bytes.
//!
//! It is written `px:` followed by the 32 bytes of the hash as 64 lowercase
//! hexadecimal characters, first byte first ([`pixel_digest_text`]).
//!
//! # Which samples
//!
//! The samples are the ones the file stores, before anything is done for
//! display:
//!
//! - no rescale, modality, VOI or palette lookup is applied to a DICOM
//!   frame: a `PALETTE COLOR` frame is hashed as its indices, one sample a
//!   pixel;
//! - nothing is inverted: a `MONOCHROME1` frame and a `MONOCHROME2` frame
//!   that store the same samples have the same digest. The photometric
//!   interpretation is not part of the digest;
//! - no colour is converted: a frame stored as `YBR_FULL` is hashed as its
//!   Y, Cb and Cr samples and has another digest than the same picture
//!   stored as RGB;
//! - a sample is hashed as its value in the width it is allocated: a
//!   sample of 12 bits allocated 16 is two bytes, an unsigned value with
//!   the bits above it zero and a signed value sign-extended. Bits outside
//!   the stored field are not pixel data and never reach a digest,
//!   whatever a file or its codestream carries there. A frame
//!   of 8-bit samples has another digest than the same values allocated 16
//!   bits. A sample narrower than a byte (a one-bit DICOM frame, a 1-, 2-
//!   or 4-bit PNG) is hashed as one byte holding its value, so
//!   `bits_allocated` is 8;
//! - signed and unsigned samples with the same bytes have different
//!   digests, because the sample format is in the header;
//! - a frame stored colour-by-plane is hashed colour-by-pixel, so the
//!   planar configuration a file uses does not change its digest;
//! - an image file's samples are the ones the format stores: a PNG palette
//!   image is hashed as the colours its indices name, three samples a
//!   pixel, or four when the palette has transparency (`tRNS`), the fourth
//!   being the entry's alpha. The `tRNS` of a gray or RGB PNG names one
//!   colour as transparent and adds no sample: such an image is hashed as
//!   its gray or RGB samples alone. An alpha channel is a sample like the
//!   others, a TIFF `WhiteIsZero` page is hashed as stored, and 16-bit
//!   samples stored big-endian are hashed little-endian.
//!
//! So the same samples have one digest in each of these, which are the
//! layouts the viewer serves raw samples of and decodes exactly:
//!
//! | Stored as | Layouts |
//! |---|---|
//! | DICOM, uncompressed or deflated, little-endian | one sample a pixel of 1, 8, 16, 32 or 64 bits; colour by pixel or by plane |
//! | DICOM, uncompressed, Explicit VR Big Endian | samples of 8, 16, 32 or 64 bits, hashed in sample order, each sample little-endian |
//! | DICOM, RLE or JPEG XL Lossless | one sample a pixel, and colour |
//! | DICOM, JPEG Lossless, JPEG-LS Lossless or JPEG 2000 Lossless | one sample a pixel |
//! | PNG, TIFF | every layout the viewer decodes |
//!
//! # Frames without a digest
//!
//! A digest is a function of the samples a file stores, so a frame has one
//! only when decoding it gives exactly those samples. A frame coded with a
//! lossy process (baseline JPEG in a DICOM file, a JPEG or WebP image) has
//! none: two conforming decoders may differ in the last bit of a sample.
//! Neither has a frame whose chrominance is stored at half resolution, nor
//! a TIFF page with associated (premultiplied) alpha, nor a frame the
//! viewer serves no raw samples of: a colour frame coded with JPEG
//! Lossless or JPEG 2000 has none, although its coding is lossless. Such a
//! frame's entry
//! in [`FileRef::pixel_digest`](crate::FileRef) is `null`, like that of a
//! frame nobody has decoded.

/// The length of the header a pixel digest begins with, in bytes.
pub const PIXEL_DIGEST_HEADER_BYTES: usize = 24;

/// How the samples of a frame are numbers. The value is the one the header
/// of a pixel digest holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum SampleFormat {
    /// Unsigned integers.
    Unsigned = 0,
    /// Two's complement signed integers.
    Signed = 1,
    /// IEEE 754 binary floating point, of `bits_allocated` bits.
    Float = 2,
}

/// What a pixel digest says of a frame besides its samples: the header of
/// the module's definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PixelLayout {
    pub rows: u32,
    pub columns: u32,
    pub samples_per_pixel: u32,
    /// The width of one sample as it is hashed, in bits: 8, 16, 32 or 64.
    pub bits_allocated: u32,
    pub sample_format: SampleFormat,
}

impl PixelLayout {
    /// The 24 bytes a digest of a frame of this layout begins with: rows,
    /// columns, samples per pixel, bits allocated, the sample format's
    /// value and 0, each as a little-endian `u32`.
    pub fn header(&self) -> [u8; PIXEL_DIGEST_HEADER_BYTES] {
        let mut header = [0; PIXEL_DIGEST_HEADER_BYTES];
        let values = [
            self.rows,
            self.columns,
            self.samples_per_pixel,
            self.bits_allocated,
            self.sample_format as u32,
            0,
        ];
        for (bytes, value) in header.chunks_exact_mut(4).zip(values) {
            bytes.copy_from_slice(&value.to_le_bytes());
        }
        header
    }

    /// The length of the samples of one frame of this layout, in bytes:
    /// `rows * columns * samples_per_pixel * (bits_allocated / 8)`.
    ///
    /// `None` when `bits_allocated` is not 8, 16, 32 or 64, when `rows`,
    /// `columns` or `samples_per_pixel` is 0, and when the product does not
    /// fit a `u64`. A layout this is `None` for has no digest.
    pub fn sample_bytes(&self) -> Option<u64> {
        if !matches!(self.bits_allocated, 8 | 16 | 32 | 64) {
            return None;
        }
        let bytes = u64::from(self.rows)
            .checked_mul(u64::from(self.columns))?
            .checked_mul(u64::from(self.samples_per_pixel))?
            .checked_mul(u64::from(self.bits_allocated / 8))?;
        (bytes != 0).then_some(bytes)
    }
}

/// A pixel digest as a document writes it: `px:` and the 32 bytes of the
/// hash as 64 lowercase hexadecimal characters, first byte first.
pub fn pixel_digest_text(digest: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(67);
    text.push_str("px:");
    for byte in digest {
        text.push(HEX[usize::from(byte >> 4)] as char);
        text.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    text
}
