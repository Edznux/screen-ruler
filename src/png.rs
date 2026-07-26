//! PNG encoding for the clipboard.
//!
//! The external clipboard helpers want `image/png` bytes. Encoding is delegated
//! to the `png` crate, which `arboard` and `xcap` already pull in through
//! `image` — so naming it directly costs nothing and buys real deflate
//! compression instead of the stored (uncompressed) blocks this module used to
//! emit by hand.

use ::png::{BitDepth, ColorType, Encoder};

use crate::image::Rgba8;

/// Encodes an image as PNG bytes.
pub fn encode(image: &Rgba8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();

    let mut encoder = Encoder::new(&mut out, image.width() as u32, image.height() as u32);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);

    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("cannot write the PNG header: {e}"))?;
    writer
        .write_image_data(image.as_bytes())
        .map_err(|e| format!("cannot write the PNG data: {e}"))?;
    writer
        .finish()
        .map_err(|e| format!("cannot finish the PNG stream: {e}"))?;

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: usize, height: usize) -> Rgba8 {
        let mut pixels = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[x as u8, y as u8, 128, 255]);
            }
        }
        Rgba8::from_raw(width, height, pixels).expect("valid buffer")
    }

    /// Decodes `bytes` back into raw RGBA, so the tests assert against what a
    /// clipboard consumer would actually see rather than our own byte layout.
    fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let decoder = ::png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().expect("valid PNG");
        let mut buffer = vec![0; reader.output_buffer_size().expect("known size")];
        let info = reader.next_frame(&mut buffer).expect("decodable frame");
        buffer.truncate(info.buffer_size());
        (info.width, info.height, buffer)
    }

    #[test]
    fn output_starts_with_the_png_signature() {
        let bytes = encode(&gradient(4, 4)).expect("encodes");
        assert_eq!(&bytes[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    }

    #[test]
    fn pixels_survive_a_round_trip_unchanged() {
        let image = gradient(7, 3);
        let (width, height, pixels) = decode(&encode(&image).expect("encodes"));

        assert_eq!((width, height), (7, 3));
        assert_eq!(pixels, image.as_bytes(), "RGBA data changed in transit");
    }

    #[test]
    fn a_payload_larger_than_one_deflate_block_round_trips() {
        // Three rows of 40000 px comfortably exceeds the 65535-byte cap on a
        // single stored block, which is where the previous hand-rolled encoder
        // had to split. The crate must handle the split transparently.
        let image = gradient(40_000, 3);
        let (width, height, pixels) = decode(&encode(&image).expect("encodes"));

        assert_eq!((width, height), (40_000, 3));
        assert_eq!(pixels, image.as_bytes());
    }

    #[test]
    fn compression_beats_the_raw_pixel_size() {
        // A flat image is highly compressible; stored blocks could never shrink
        // it, so this also guards against silently regressing to no deflate.
        let image = gradient(256, 256);
        let encoded = encode(&image).expect("encodes");
        assert!(
            encoded.len() < image.as_bytes().len() / 2,
            "expected real compression, got {} bytes for {} raw",
            encoded.len(),
            image.as_bytes().len()
        );
    }
}
