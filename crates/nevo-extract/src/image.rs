//! Image sizes from headers only, so a 100 KB file claiming 100000×100000 pixels is refused
//! before anything tries to decode it.

use crate::Kind;

/// Image size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}

/// Reads the size from the header, or `None` if the header is missing or cut short. For GIFs,
/// the largest of the screen size and the first frame's size (frames may be bigger than the
/// screen, and decoders allocate for them).
pub(crate) fn dimensions(kind: Kind, b: &[u8]) -> Option<Dimensions> {
    let d = |width, height| Some(Dimensions { width, height });
    match kind {
        Kind::Png => {
            if b.get(12..16)? != b"IHDR" {
                return None;
            }
            d(be32(b, 16)?, be32(b, 20)?)
        }
        Kind::Gif => {
            let (w, h) = (le16(b, 6)? as u32, le16(b, 8)? as u32);
            match gif_first_frame(b) {
                Some((fw, fh)) => d(w.max(fw), h.max(fh)),
                None => d(w, h),
            }
        }
        Kind::Jpeg => jpeg(b),
        Kind::Webp => match b.get(12..16)? {
            b"VP8 " => {
                // Lossy: a 3-byte frame tag, the start code 9d 01 2a, then 14-bit sizes.
                if b.get(23..26)? != [0x9d, 0x01, 0x2a] {
                    return None;
                }
                d((le16(b, 26)? & 0x3fff) as u32, (le16(b, 28)? & 0x3fff) as u32)
            }
            b"VP8L" => {
                // Lossless: signature 0x2f, then width-1 and height-1 in 14 bits each.
                if *b.get(20)? != 0x2f {
                    return None;
                }
                let bits = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
                d((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
            }
            b"VP8X" => {
                // Extended: canvas width-1 and height-1 in 24 bits each.
                let w = u32::from_le_bytes([*b.get(24)?, *b.get(25)?, *b.get(26)?, 0]) + 1;
                let h = u32::from_le_bytes([*b.get(27)?, *b.get(28)?, *b.get(29)?, 0]) + 1;
                d(w, h)
            }
            _ => None,
        },
        _ => None,
    }
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

/// Walks JPEG segments to the first frame header (SOF0–SOF15, minus the non-frame markers
/// C4, C8 and CC).
fn jpeg(b: &[u8]) -> Option<Dimensions> {
    let mut i = 2;
    loop {
        // Markers may be padded with any number of 0xff bytes.
        while *b.get(i)? == 0xff && *b.get(i + 1)? == 0xff {
            i += 1;
        }
        if *b.get(i)? != 0xff {
            return None;
        }
        let marker = *b.get(i + 1)?;
        if matches!(marker, 0xd0..=0xd9 | 0x01) {
            // Standalone markers have no length.
            i += 2;
            continue;
        }
        let len = be16(b, i + 2)? as usize;
        if len < 2 {
            return None;
        }
        if matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            let height = be16(b, i + 5)? as u32;
            let width = be16(b, i + 7)? as u32;
            return Some(Dimensions { width, height });
        }
        i += 2 + len;
    }
}

/// Size of the first image frame, skipping the colour table and extension blocks.
fn gif_first_frame(b: &[u8]) -> Option<(u32, u32)> {
    let flags = *b.get(10)?;
    let mut i = 13;
    if flags & 0x80 != 0 {
        i += 3 << ((flags & 7) + 1);
    }
    loop {
        match *b.get(i)? {
            0x2c => return Some((le16(b, i + 5)? as u32, le16(b, i + 7)? as u32)),
            0x21 => {
                // Extension: label, then sub-blocks until a zero length.
                i += 2;
                loop {
                    let n = *b.get(i)? as usize;
                    i += 1 + n;
                    if n == 0 {
                        break;
                    }
                }
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png() {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend(640u32.to_be_bytes());
        b.extend(480u32.to_be_bytes());
        assert_eq!(
            dimensions(Kind::Png, &b),
            Some(Dimensions {
                width: 640,
                height: 480
            })
        );
        assert_eq!(dimensions(Kind::Png, &b[..20]), None);
    }

    #[test]
    fn jpeg_skips_segments() {
        let mut b = vec![
            0xff, 0xd8, 0xff, 0xe1, 0x00, 0x06, 1, 2, 3, 4, 0xff, 0xff, 0xc2, 0x00, 0x11, 8,
        ];
        b.extend(300u16.to_be_bytes());
        b.extend(400u16.to_be_bytes());
        assert_eq!(
            dimensions(Kind::Jpeg, &b),
            Some(Dimensions {
                width: 400,
                height: 300
            })
        );
        assert_eq!(dimensions(Kind::Jpeg, &b[..12]), None);
    }

    #[test]
    fn gif_frame_bigger_than_screen() {
        let mut b = b"GIF89a\x0a\x00\x0a\x00\x00\x00\x00".to_vec();
        b.extend([0x21, 0xf9, 4, 0, 0, 0, 0, 0]);
        b.extend([0x2c, 0, 0, 0, 0, 0x10, 0x27, 0x10, 0x27, 0]);
        assert_eq!(
            dimensions(Kind::Gif, &b),
            Some(Dimensions {
                width: 10000,
                height: 10000
            })
        );
    }

    #[test]
    fn webp() {
        let mut b = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        b.extend([0x1f, 0x03, 0x00, 0xdf, 0x01, 0x00]);
        assert_eq!(
            dimensions(Kind::Webp, &b),
            Some(Dimensions {
                width: 800,
                height: 480
            })
        );
        let mut b = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f".to_vec();
        b.extend((99u32 | (49 << 14)).to_le_bytes());
        assert_eq!(dimensions(Kind::Webp, &b), Some(Dimensions { width: 100, height: 50 }));
    }
}
