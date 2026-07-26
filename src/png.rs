//! A minimal PNG encoder.
//!
//! Only needed so images can be handed to external clipboard helpers, which
//! want `image/png` bytes. Deflate is emitted as *stored* (uncompressed) blocks:
//! that is a legal zlib stream every decoder accepts, costs about 100 lines,
//! and avoids taking on a compression dependency for a clipboard payload that
//! never touches disk.

use crate::image::Rgba8;

/// The largest payload a single stored deflate block can hold.
const MAX_STORED_BLOCK: usize = u16::MAX as usize;

/// Encodes an image as PNG bytes.
pub fn encode(image: &Rgba8) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(image.width() as u32).to_be_bytes());
    ihdr.extend_from_slice(&(image.height() as u32).to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // colour type: truecolour with alpha
    ihdr.push(0); // deflate
    ihdr.push(0); // adaptive filtering
    ihdr.push(0); // no interlacing
    write_chunk(&mut out, b"IHDR", &ihdr);

    write_chunk(&mut out, b"IDAT", &zlib_stored(&raw_scanlines(image)));
    write_chunk(&mut out, b"IEND", &[]);
    out
}

/// Prefixes each row with a filter byte, as the PNG format requires.
fn raw_scanlines(image: &Rgba8) -> Vec<u8> {
    let width = image.width();
    let stride = width * 4;
    let bytes = image.as_bytes();
    let mut out = Vec::with_capacity((stride + 1) * image.height());
    for row in 0..image.height() {
        out.push(0); // filter type 0: none
        out.extend_from_slice(&bytes[row * stride..(row + 1) * stride]);
    }
    out
}

/// Wraps data in a zlib stream built from stored deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / MAX_STORED_BLOCK * 5 + 16);
    // zlib header: deflate, 32K window, no preset dictionary, default level.
    out.push(0x78);
    out.push(0x01);

    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    } else {
        for (index, chunk) in data.chunks(MAX_STORED_BLOCK).enumerate() {
            let is_last = (index + 1) * MAX_STORED_BLOCK >= data.len();
            out.push(u8::from(is_last));
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }

    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);

    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + *byte as u32) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: usize, height: usize) -> Rgba8 {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[x as u8, y as u8, 128, 255]);
            }
        }
        Rgba8::from_raw(width, height, pixels).expect("valid buffer")
    }

    #[test]
    fn output_starts_with_the_png_signature() {
        let bytes = encode(&solid(4, 4));
        assert_eq!(&bytes[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    }

    #[test]
    fn the_header_records_the_image_dimensions() {
        let bytes = encode(&solid(7, 3));
        // IHDR data starts at byte 16: 8 signature + 4 length + 4 type.
        assert_eq!(&bytes[16..20], &7u32.to_be_bytes());
        assert_eq!(&bytes[20..24], &3u32.to_be_bytes());
        assert_eq!(bytes[24], 8, "bit depth");
        assert_eq!(bytes[25], 6, "RGBA colour type");
    }

    #[test]
    fn all_three_required_chunks_are_present_and_ordered() {
        let bytes = encode(&solid(4, 4));
        let ihdr = find(&bytes, b"IHDR").expect("IHDR");
        let idat = find(&bytes, b"IDAT").expect("IDAT");
        let iend = find(&bytes, b"IEND").expect("IEND");
        assert!(ihdr < idat && idat < iend, "chunks out of order");
        assert!(bytes.ends_with(&crc32(b"IEND").to_be_bytes()));
    }

    #[test]
    fn known_checksums_match_the_reference_values() {
        // Standard test vectors for both algorithms.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn a_payload_larger_than_one_block_is_split_and_terminated() {
        // Three rows of 40000 px comfortably exceeds the 65535-byte block cap.
        let wide = solid(40_000, 3);
        let scanlines = raw_scanlines(&wide);
        assert!(scanlines.len() > MAX_STORED_BLOCK * 2);

        let stream = zlib_stored(&scanlines);
        // Walk the block headers and check exactly the final one is flagged.
        let mut offset = 2;
        let mut blocks = 0;
        loop {
            let is_last = stream[offset] == 1;
            let len = u16::from_le_bytes([stream[offset + 1], stream[offset + 2]]) as usize;
            let nlen = u16::from_le_bytes([stream[offset + 3], stream[offset + 4]]);
            assert_eq!(nlen, !(len as u16), "block length complement is wrong");
            offset += 5 + len;
            blocks += 1;
            if is_last {
                break;
            }
            assert!(offset < stream.len(), "ran past the end without a final block");
        }
        assert!(blocks > 1, "expected the payload to be split");
        assert_eq!(offset + 4, stream.len(), "adler32 should follow the last block");
    }

    #[test]
    fn each_scanline_carries_a_filter_byte() {
        let image = solid(3, 2);
        let scanlines = raw_scanlines(&image);
        assert_eq!(scanlines.len(), (3 * 4 + 1) * 2);
        assert_eq!(scanlines[0], 0, "row 0 filter byte");
        assert_eq!(scanlines[13], 0, "row 1 filter byte");
    }

    /// Finds a chunk type marker in the encoded stream.
    fn find(bytes: &[u8], kind: &[u8; 4]) -> Option<usize> {
        bytes.windows(4).position(|w| w == kind)
    }
}
